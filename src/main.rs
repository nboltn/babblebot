#![feature(proc_macro_hygiene, decl_macro, custom_attribute)]

#[macro_use] extern crate log;
#[macro_use] extern crate rocket;

mod commands;
mod types;
mod util;
mod web;

use crate::types::*;
use crate::util::*;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::{thread,time,mem};

use config;
use clap::load_yaml;
use clap::{App, ArgMatches};
use flexi_logger::{Criterion,Naming,Cleanup,Duplicate,Logger};
use crossbeam_channel::{unbounded,Sender,Receiver,RecvTimeoutError};
use irc::error::IrcError;
use irc::client::prelude::*;
use url::Url;
use regex::RegexBuilder;
use serde_json::value::Value::Number;
use chrono::{Utc, DateTime, Timelike};
use http::header::{self,HeaderValue};
use futures::Async;
use futures::future::lazy;
use reqwest::Method;
use reqwest::r#async::Decoder;
use serenity;
use serenity::framework::standard::StandardFramework;
use rocket::routes;
use rocket_contrib::templates::Template;
use rocket_contrib::serve::StaticFiles;
use redis::{self,Commands};

fn main() {
    let yaml = load_yaml!("../cli.yml");
    let matches = App::from_yaml(yaml).get_matches();
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();

    Logger::with_env_or_str("babblebot")
        .log_to_file()
        .directory("logs")
        .append()
        .rotate(Criterion::Size(1000000), Naming::Numbers, Cleanup::Never)
        .duplicate_to_stderr(Duplicate::Warn)
        .format(flexi_logger::default_format)
        .start()
        .unwrap_or_else(|e| panic!("Logger initialization failed with {}", e));

    if let Some(matches) = matches.subcommand_matches("run_command") { run_command(matches) }
    else {
        thread::spawn(move || { new_channel_listener() });
        thread::spawn(move || {
            log_info(None, "main", "starting rocket");
            rocket::ignite()
              .mount("/assets", StaticFiles::from("assets"))
              .mount("/", routes![web::index, web::dashboard, web::commands, web::patreon_cb, web::patreon_refresh, web::spotify_cb, web::twitch_cb, web::twitch_cb_auth, web::public_data, web::data, web::login, web::logout, web::signup, web::password, web::title, web::game, web::new_command, web::save_command, web::trash_command, web::new_notice, web::trash_notice, web::save_setting, web::trash_setting, web::new_blacklist, web::save_blacklist, web::trash_blacklist, web::trash_song])
              .register(catchers![web::internal_error, web::not_found])
              .attach(Template::fairing())
              .attach(RedisConnection::fairing())
              .launch()
        });
        thread::spawn(move || {
            let con = acquire_con();
            let mut bots: HashMap<String, (HashSet<String>, Config)> = HashMap::new();
            let bs: HashSet<String> = con.smembers("bots").unwrap();
            for bot in bs {
                let channel_hash: HashSet<String> = con.smembers(format!("bot:{}:channels", bot)).unwrap();
                if channel_hash.len() > 0 {
                    let passphrase: String = con.get(format!("bot:{}:token", bot)).expect("get:token");
                    let mut channels: Vec<String> = Vec::new();
                    channels.extend(channel_hash.iter().cloned().map(|chan| { format!("#{}", chan) }));
                    let config = Config {
                        server: Some("irc.chat.twitch.tv".to_owned()),
                        use_ssl: Some(true),
                        nickname: Some(bot.to_owned()),
                        password: Some(format!("oauth:{}", passphrase)),
                        channels: Some(channels),
                        ping_timeout: Some(60),
                        ..Default::default()
                    };
                    bots.insert(bot.to_owned(), (channel_hash.clone(), config));
                    for channel in channel_hash.iter() {
                        discord_handler(channel.to_owned());
                    }
                }
            }
            update_live();
            update_stats();
            update_watchtime();
            update_spotify();
            update_patreon();
            refresh_patreon();
            run_notices();
            run_commercials();
            run_reactor(bots);
        });

        loop { thread::sleep(time::Duration::from_secs(60)) }
    }
}

fn run_reactor(bots: HashMap<String, (HashSet<String>, Config)>) {
    bots.iter().for_each(|(bot, channels)| {
        let bot = bot.clone();
        let channels = channels.clone();
        thread::spawn(move || {
            tokio::run(lazy(move || {
                loop {
                    log_info(Some(&bot), "run_reactor", "connecting to irc");
                    let config = (channels.1).clone();
                    match IrcClient::from_config(config) {
                        Err(e) => { log_error(None, "run_reactor", &e.to_string()); break }
                        Ok(client) => {
                            let client = Arc::new(client);
                            let _ = client.identify();
                            let _ = client.send("CAP REQ :twitch.tv/tags");
                            let _ = client.send("CAP REQ :twitch.tv/commands");
                            let (sender, receiver) = unbounded();
                            for channel in channels.0.iter() {
                                rename_channel_listener(channel.clone(), sender.clone());
                                command_listener(channel.clone());
                            }
                            let res = run_client(client, receiver);
                            match res {
                                None => break,
                                Some(e) => { log_error(None, "run_reactor", &e.to_string()); }
                            }
                        }
                    }
                }
                Ok(())
            }));
        });
    });
}

fn run_client(client: Arc<IrcClient>, receiver: Receiver<ThreadAction>) -> Option<IrcError> {
    let mut stream = client.stream();
    loop {
        let rsp = receiver.recv_timeout(time::Duration::from_millis(10));
        match rsp {
            Ok(action) => {
                match action {
                    ThreadAction::Kill => break,
                    ThreadAction::Part(chan) => { let _ = client.send_part(format!("#{}", chan)); }
                }
            }
            Err(err) => {
                match err {
                    RecvTimeoutError::Disconnected => break,
                    RecvTimeoutError::Timeout => {}
                }
            }
        }

        match stream.poll() {
            Err(e) => { return Some(e) }
            Ok(msg) => {
                match msg {
                    Async::NotReady => {}
                    Async::Ready(None) => {}
                    Async::Ready(Some(irc_message)) => {
                        let con = Arc::new(acquire_con());
                        match &irc_message.command {
                            Command::Raw(cmd, chans, _) => {
                                if cmd == "USERSTATE" {
                                    let badges = get_badges(&irc_message);
                                    match badges.get("moderator") {
                                        Some(_) => {
                                            for chan in chans {
                                                let channel = &chan[1..];
                                                let _: () = con.set(format!("channel:{}:auth", channel), true).unwrap();
                                            }
                                        }
                                        None => {
                                            for chan in chans {
                                                let channel = &chan[1..];
                                                let _: () = con.set(format!("channel:{}:auth", channel), false).unwrap();
                                            }
                                        }
                                    }
                                }
                            }
                            Command::PRIVMSG(chan, msg) => {
                                let channel = &chan[1..];
                                let nick = get_nick(&irc_message);
                                let prefix: String = con.hget(format!("channel:{}:settings", channel), "command:prefix").unwrap_or("!".to_owned());
                                let mut words = msg.split_whitespace();

                                // parse ircV3 badges
                                if let Some(word) = words.next() {
                                    let mut word = word.to_lowercase();
                                    let mut args: Vec<String> = words.map(|w| w.to_owned()).collect();
                                    let badges = get_badges(&irc_message);

                                    let mut subscriber = false;
                                    if let Some(_value) = badges.get("subscriber") { subscriber = true }

                                    let mut auth = false;
                                    if let Some(_value) = badges.get("broadcaster") { auth = true }
                                    if let Some(_value) = badges.get("moderator") { auth = true }

                                    // moderate incoming messages
                                    // TODO: symbols, length
                                    if !auth {
                                        let display: String = con.get(format!("channel:{}:moderation:display", channel)).unwrap_or("false".to_owned());
                                        let caps: String = con.get(format!("channel:{}:moderation:caps", channel)).unwrap_or("false".to_owned());
                                        let colors: String = con.get(format!("channel:{}:moderation:colors", channel)).unwrap_or("false".to_owned());
                                        let links: Vec<String> = con.smembers(format!("channel:{}:moderation:links", channel)).unwrap_or(Vec::new());
                                        let bkeys: Vec<String> = con.keys(format!("channel:{}:moderation:blacklist:*", channel)).unwrap();
                                        let age: Result<String,_> = con.get(format!("channel:{}:moderation:age", channel));
                                        if colors == "true" && msg.len() > 6 && msg.as_bytes()[0] == 1 && &msg[1..7] == "ACTION" {
                                            let _ = client.send_privmsg(chan, format!("/timeout {} 1", nick));
                                            if display == "true" { send_message(con.clone(), client.clone(), channel.to_owned(), format!("@{} you've been timed out for posting colors", nick)); }
                                        }
                                        if let Ok(age) = age {
                                            let res: Result<i64,_> = age.parse();
                                            if let Ok(age) = res { spawn_age_check(con.clone(), client.clone(), channel.to_string(), nick.clone(), age, display.to_string()); }
                                        }
                                        if caps == "true" {
                                            let limit: String = con.get(format!("channel:{}:moderation:caps:limit", channel)).expect("get:limit");
                                            let trigger: String = con.get(format!("channel:{}:moderation:caps:trigger", channel)).expect("get:trigger");
                                            let subs: String = con.get(format!("channel:{}:moderation:caps:subs", channel)).unwrap_or("false".to_owned());
                                            let limit: Result<f32,_> = limit.parse();
                                            let trigger: Result<f32,_> = trigger.parse();
                                            if let (Ok(limit), Ok(trigger)) = (limit, trigger) {
                                                let len = msg.len() as f32;
                                                if len >= trigger {
                                                    let num = msg.chars().fold(0.0, |acc, c| if c.is_uppercase() { acc + 1.0 } else { acc });
                                                    let ratio = num / len;
                                                    if ratio >= (limit / 100.0) {
                                                        if !subscriber || subscriber && subs != "true" {
                                                            let _ = client.send_privmsg(chan, format!("/timeout {} 1", nick));
                                                            if display == "true" { send_message(con.clone(), client.clone(), channel.to_owned(), format!("@{} you've been timed out for posting too many caps", nick)); }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        if links.len() > 0 && url_regex().is_match(&msg) {
                                            let sublinks: String = con.get(format!("channel:{}:moderation:links:subs", channel)).unwrap_or("false".to_owned());
                                            let permitted: Vec<String> = con.keys(format!("channel:{}:moderation:permitted:*", channel)).unwrap();
                                            let permitted: Vec<String> = permitted.iter().map(|key| { let key: Vec<&str> = key.split(":").collect(); key[4].to_owned() }).collect();
                                            if !(permitted.contains(&nick) || (sublinks == "true" && subscriber)) {
                                                for word in msg.split_whitespace() {
                                                    if url_regex().is_match(word) {
                                                        let mut url: String = word.to_owned();
                                                        if url.len() > 7 && url.is_char_boundary(7) && &url[..7] != "http://" && &url[..8] != "https://" { url = format!("http://{}", url) }
                                                        match Url::parse(&url) {
                                                            Err(_) => {}
                                                            Ok(url) => {
                                                                let mut whitelisted = false;
                                                                for link in &links {
                                                                    let link: Vec<&str> = link.split("/").collect();
                                                                    let mut domain = url.domain().unwrap();
                                                                    if domain.len() > 0 && &domain[..4] == "www." { domain = &domain[4..] }
                                                                    if domain == link[0] {
                                                                        if link.len() > 1 {
                                                                            if url.path().len() > 1 && url.path()[1..] == link[1..].join("/") {
                                                                                whitelisted = true;
                                                                                break;
                                                                            }
                                                                        } else {
                                                                            whitelisted = true;
                                                                            break;
                                                                        }
                                                                    }
                                                                }
                                                                if !whitelisted {
                                                                    let _ = client.send_privmsg(chan, format!("/timeout {} 1", nick));
                                                                    if display == "true" { send_message(con.clone(), client.clone(), channel.to_owned(), format!("@{} you've been timed out for posting links", nick)); }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        for key in bkeys {
                                            let key: Vec<&str> = key.split(":").collect();
                                            let rgx: String = con.hget(format!("channel:{}:moderation:blacklist:{}", channel, key[4]), "regex").expect("hget:regex");
                                            let length: String = con.hget(format!("channel:{}:moderation:blacklist:{}", channel, key[4]), "length").expect("hget:length");
                                            match RegexBuilder::new(&rgx).case_insensitive(true).build() {
                                                Err(e) => { log_error(Some(&channel), "regex_error", &e.to_string()) }
                                                Ok(rgx) => {
                                                    if rgx.is_match(&msg) {
                                                        let _ = client.send_privmsg(chan, format!("/timeout {} {}", nick, length));
                                                        if display == "true" { send_message(con.clone(), client.clone(), channel.to_owned(), format!("@{} you've been timed out for posting a blacklisted phrase", nick)); }
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // expand aliases
                                    let res: Result<String,_> = con.hget(format!("channel:{}:aliases", channel), &word);
                                    if let Ok(alias) = res {
                                        let mut awords = alias.split_whitespace();
                                        if let Some(aword) = awords.next() {
                                            let mut cargs = args.clone();
                                            let mut awords: Vec<String> = awords.map(|w| w.to_owned()).collect();
                                            awords.append(&mut cargs);
                                            word = aword.to_owned();
                                            args = awords.to_owned();
                                        }
                                    }

                                    // parse native commands
                                    for cmd in commands::native_commands.iter() {
                                        if format!("{}{}", prefix, cmd.0) == word {
                                            if args.len() == 0 {
                                                if !cmd.2 || auth { (cmd.1)(con.clone(), client.clone(), channel.to_owned(), args.clone(), Some(irc_message.clone())) }
                                            } else {
                                                if !cmd.3 || auth { (cmd.1)(con.clone(), client.clone(), channel.to_owned(), args.clone(), Some(irc_message.clone())) }
                                            }
                                            break;
                                        }
                                    }

                                    // parse custom commands
                                    let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel.to_owned(), word), "message");
                                    if let Ok(message) = res {
                                        let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel.to_owned(), word), "lastrun");
                                        let mut within5 = false;
                                        if let Ok(lastrun) = res {
                                            let timestamp = DateTime::parse_from_rfc3339(&lastrun).unwrap();
                                            let diff = Utc::now().signed_duration_since(timestamp);
                                            if diff.num_seconds() < 5 { within5 = true }
                                        }
                                        if !within5 {
                                            let mut protected: &str = "cmd";
                                            if args.len() > 0 { protected = "arg" }
                                            let protected: String = con.hget(format!("channel:{}:commands:{}", channel, word), format!("{}_protected", protected)).expect("hget:protected");
                                            if protected == "false" || auth {
                                                let _: () = con.hset(format!("channel:{}:commands:{}", channel, word), "lastrun", Utc::now().to_rfc3339()).unwrap();
                                                send_parsed_message(con.clone(), client.clone(), channel.to_owned(), message.to_owned(), args.clone(), Some(irc_message.clone()));
                                            }
                                        }
                                    }

                                    // parse greetings
                                    let keys: Vec<String> = con.keys(format!("channel:{}:greetings:*", channel)).unwrap();
                                    for key in keys.iter() {
                                        let key: Vec<&str> = key.split(":").collect();
                                        if key[3] == nick {
                                            let msg: String = con.hget(format!("channel:{}:greetings:{}", channel, key[3]), "message").expect("hget:message");
                                            let hours: i64 = con.hget(format!("channel:{}:greetings:{}", channel, key[3]), "hours").expect("hget:hours");
                                            let res: Result<String,_> = con.hget(format!("channel:{}:lastseen", channel), key[3]);
                                            if let Ok(lastseen) = res {
                                                let timestamp = DateTime::parse_from_rfc3339(&lastseen).unwrap();
                                                let diff = Utc::now().signed_duration_since(timestamp);
                                                if diff.num_hours() < hours { break }
                                            }
                                            send_parsed_message(con.clone(), client.clone(), channel.to_owned(), msg, Vec::new(), None);
                                            break;
                                        }
                                    }

                                    let _: () = con.hset(format!("channel:{}:lastseen", channel), nick, Utc::now().to_rfc3339()).unwrap();
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    None
}


fn new_channel_listener() {
    let con = Arc::new(acquire_con());
    let mut conn = acquire_con();
    let mut ps = conn.as_pubsub();
    ps.subscribe("new_channels").unwrap();

    loop {
        let res = ps.get_message();
        if let Ok(msg) = res {
            let channel: String = msg.get_payload().expect("redis:get_payload");
            let mut bots: HashMap<String, (HashSet<String>, Config)> = HashMap::new();
            let bot: String = con.get(format!("channel:{}:bot", channel)).expect("get:bot");
            let passphrase: String = con.get(format!("bot:{}:token", bot)).expect("get:token");
            let mut channel_hash: HashSet<String> = HashSet::new();
            let mut channels: Vec<String> = Vec::new();
            channel_hash.insert(channel.to_owned());
            channels.extend(channel_hash.iter().cloned().map(|chan| { format!("#{}", chan) }));
            let config = Config {
                server: Some("irc.chat.twitch.tv".to_owned()),
                use_ssl: Some(true),
                nickname: Some(bot.to_owned()),
                password: Some(format!("oauth:{}", passphrase)),
                channels: Some(channels),
                ping_timeout: Some(60),
                ..Default::default()
            };
            bots.insert(bot.to_owned(), (channel_hash.clone(), config));
            for channel in channel_hash.iter() {
                discord_handler(channel.to_owned());
            }
            thread::spawn(move || { run_reactor(bots); });
        }
    }
}

fn rename_channel_listener(channel: String, sender: Sender<ThreadAction>) {
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        let mut conn = acquire_con();
        let mut ps = conn.as_pubsub();
        ps.subscribe(format!("channel:{}:signals:rename", channel)).unwrap();

        loop {
            let res = ps.get_message();
            match res {
                Err(e) => { log_error(Some(&channel), "rename_channel_listener", &e.to_string()) }
                Ok(msg) => {
                    let token: String = msg.get_payload().expect("redis:get_payload");

                    let mut settings = config::Config::default();
                    settings.merge(config::File::with_name("Settings")).unwrap();
                    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();

                    let mut headers = header::HeaderMap::new();
                    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
                    headers.insert("Authorization", HeaderValue::from_str(&format!("OAuth {}", &token)).unwrap());
                    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());

                    let req = reqwest::Client::builder().default_headers(headers).build().unwrap();
                    let rsp = req.get("https://api.twitch.tv/kraken/user").send();
                    match rsp {
                        Err(e) => { log_error(Some(&channel), "rename_channel_listener", &e.to_string()) }
                        Ok(mut rsp) => {
                            let text = rsp.text().unwrap();
                            let json: Result<KrakenUser,_> = serde_json::from_str(&text);
                            match json {
                                Err(e) => {
                                    log_error(Some(&channel), "rename_channel_listener", &e.to_string());
                                    log_error(Some(&channel), "request_body", &text);
                                }
                                Ok(json) => {
                                    let _ = sender.send(ThreadAction::Part(channel.clone()));

                                    let bot: String = con.get(format!("channel:{}:bot", &channel)).expect("get:bot");
                                    let _: () = con.srem(format!("bot:{}:channels", &bot), &channel).unwrap();
                                    let _: () = con.sadd("bots", &json.name).unwrap();
                                    let _: () = con.sadd(format!("bot:{}:channels", &json.name), &channel).unwrap();
                                    let _: () = con.set(format!("bot:{}:token", &json.name), &token).unwrap();
                                    let _: () = con.set(format!("channel:{}:bot", &channel), &json.name).unwrap();

                                    let mut bots: HashMap<String, (HashSet<String>, Config)> = HashMap::new();
                                    let mut channel_hash: HashSet<String> = HashSet::new();
                                    let mut channels: Vec<String> = Vec::new();
                                    channel_hash.insert(channel.to_owned());
                                    channels.extend(channel_hash.iter().cloned().map(|chan| { format!("#{}", chan) }));
                                    let config = Config {
                                        server: Some("irc.chat.twitch.tv".to_owned()),
                                        use_ssl: Some(true),
                                        nickname: Some(bot.to_owned()),
                                        password: Some(format!("oauth:{}", token)),
                                        channels: Some(channels),
                                        ..Default::default()
                                    };
                                    bots.insert(bot.to_owned(), (channel_hash.clone(), config));
                                    thread::spawn(move || { run_reactor(bots); });
                                }
                            }
                        }
                    }
                }
            }
        }
    });
}

fn command_listener(channel: String) {
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        let mut conn = acquire_con();
        let mut ps = conn.as_pubsub();
        ps.subscribe(format!("channel:{}:signals:command", channel)).unwrap();

        loop {
            let res = ps.get_message();
            match res {
                Err(e) => { log_error(Some(&channel), "command_listener", &e.to_string()) }
                Ok(msg) => {
                    let payload: String = msg.get_payload().expect("redis:get_payload");
                    let mut words = payload.split_whitespace();
                    let prefix: String = con.hget(format!("channel:{}:settings", channel), "command:prefix").unwrap_or("!".to_owned());
                    if let Some(word) = words.next() {
                        let mut word = word.to_lowercase();
                        let mut args: Vec<String> = words.map(|w| w.to_owned()).collect();

                        // expand aliases
                        let res: Result<String,_> = con.hget(format!("channel:{}:aliases", channel), &word);
                        if let Ok(alias) = res {
                            let mut awords = alias.split_whitespace();
                            if let Some(aword) = awords.next() {
                                let mut cargs = args.clone();
                                let mut awords: Vec<String> = awords.map(|w| w.to_owned()).collect();
                                awords.append(&mut cargs);
                                word = aword.to_owned();
                                args = awords.to_owned();
                            }
                        }

                        // parse native commands
                        for cmd in commands::native_commands.iter() {
                            if format!("{}{}", prefix, cmd.0) == word {
                                connect_and_run_command(cmd.1, con.clone(), channel.clone(), args.clone());
                                break;
                            }
                        }

                        // parse custom commands
                        let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, word), "message");
                        if let Ok(message) = res {
                            connect_and_send_message(con.clone(), channel.clone(), message);
                        }
                    }
                }
            }
        }
    });
}

fn discord_handler(channel: String) {
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        loop {
            let res: Result<String,_> = con.hget(format!("channel:{}:settings", channel), "discord:token");
            if let Ok(token) = res {
                let mut client = serenity::client::Client::new(&token, DiscordHandler { channel: channel.to_owned() }).unwrap();
                client.with_framework(StandardFramework::new());
                log_info(Some(&channel), "discord_handler", "connecting to discord");
                if let Err(e) = client.start() {
                    log_error(Some(&channel), "discord_handler", &e.to_string())
                }
            }
            thread::sleep(time::Duration::from_secs(10));
        }
    });
}

fn run_notices() {
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        loop {
            let channels: Vec<String> = con.smembers("channels").unwrap_or(Vec::new());
            for channel in channels {
                let live: String = con.get(format!("channel:{}:live", channel)).expect("get:live");
                if live == "true" {
                    let keys: Vec<String> = con.keys(format!("channel:{}:notices:*:commands", channel.clone())).unwrap();
                    let ints: Vec<&str> = keys.iter().map(|str| {
                        let int: Vec<&str> = str.split(":").collect();
                        return int[3];
                    }).collect();

                    for int in ints.iter() {
                        let num: u16 = con.get(format!("channel:{}:notices:{}:countdown", channel, int)).unwrap();
                        if num > 0 { let _: () = con.incr(format!("channel:{}:notices:{}:countdown", channel, int), -60).unwrap(); }
                    };

                    let int = ints.iter().filter(|int| {
                        let num: u16 = con.get(format!("channel:{}:notices:{}:countdown", channel, int)).unwrap();
                        return num <= 0;
                    }).fold(0, |acc, int| {
                        let int = int.parse::<u16>().unwrap();
                        if acc > int { return acc } else { return int }
                    });

                    if int != 0 {
                        let _: () = con.set(format!("channel:{}:notices:{}:countdown", channel, int), int.clone()).unwrap();
                        let cmd: String = con.lpop(format!("channel:{}:notices:{}:commands", channel, int)).expect("lpop:commands");
                        let _: () = con.rpush(format!("channel:{}:notices:{}:commands", channel, int), cmd.clone()).unwrap();
                        let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, cmd), "message");
                        if let Ok(message) = res {
                            connect_and_send_message(con.clone(), channel.clone(), message);
                        }
                    }
                }
            }
            thread::sleep(time::Duration::from_secs(60));
        }
    });
}

fn run_commercials() {
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        loop {
            let channels: Vec<String> = con.smembers("channels").unwrap_or(Vec::new());
            for channel in channels {
                let live: String = con.get(format!("channel:{}:live", channel)).expect("get:live");
                if live == "true" {
                    let hourly: String = con.get(format!("channel:{}:commercials:hourly", channel)).unwrap_or("0".to_owned());
                    let hourly: u64 = hourly.parse().unwrap();
                    let recents: Vec<String> = con.lrange(format!("channel:{}:commercials:recent", channel), 0, -1).unwrap();
                    let num = recents.iter().fold(hourly, |acc, lastrun| {
                        let lastrun: Vec<&str> = lastrun.split_whitespace().collect();
                        let timestamp = DateTime::parse_from_rfc3339(&lastrun[0]).unwrap();
                        let diff = Utc::now().signed_duration_since(timestamp);
                        if diff.num_minutes() < 60 {
                            let res: Result<u64,_> = lastrun[1].parse();
                            if let Ok(num) = res {
                                if acc >= num {
                                    return acc - num;
                                } else {
                                    return acc;
                                }
                            } else {
                                return acc;
                            }
                        } else {
                            return acc;
                        }
                    });

                    if num > 0 {
                        let mut within8 = false;
                        let res: Result<String,_> = con.lindex(format!("channel:{}:commercials:recent", channel), 0);
                        if let Ok(lastrun) = res {
                            let lastrun: Vec<&str> = lastrun.split_whitespace().collect();
                            let timestamp = DateTime::parse_from_rfc3339(&lastrun[0]).unwrap();
                            let diff = Utc::now().signed_duration_since(timestamp);
                            if diff.num_minutes() <= 9 {
                                within8 = true;
                            }
                        }
                        if !within8 {
                            let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
                            let submode: String = con.get(format!("channel:{}:commercials:submode", channel)).unwrap_or("false".to_owned());
                            let nres: Result<String,_> = con.get(format!("channel:{}:commercials:notice", channel));
                            let length: u16 = con.llen(format!("channel:{}:commercials:recent", channel)).unwrap();
                            let _: () = con.lpush(format!("channel:{}:commercials:recent", channel), format!("{} {}", Utc::now().to_rfc3339(), num)).unwrap();
                            if length > 7 {
                                let _: () = con.rpop(format!("channel:{}:commercials:recent", channel)).unwrap();
                            }
                            if submode == "true" {
                                let channelC = String::from(channel.clone());
                                connect_and_send_message(con.clone(), channel.clone(), "/subscribers".to_owned());
                                thread::spawn(move || {
                                    let con = Arc::new(acquire_con());
                                    thread::sleep(time::Duration::from_secs(num * 30));
                                    connect_and_send_message(con, channelC, "/subscribersoff".to_owned());
                                });
                            }
                            if let Ok(notice) = nres {
                                let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, notice), "message");
                                if let Ok(message) = res {
                                    connect_and_send_message(con.clone(), channel.clone(), message);
                                }
                            }
                            log_info(Some(&channel), "run_commercials", &format!("{} commercials have been run", num));
                            connect_and_send_message(con.clone(), channel.clone(), format!("{} commercials have been run", num));
                            let future = twitch_kraken_request(con.clone(), &channel, Some("application/json"), Some(format!("{{\"length\": {}}}", num * 30).as_bytes().to_owned()), Method::POST, &format!("https://api.twitch.tv/kraken/channels/{}/commercial", &id)).send().and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() }).map_err(|e| println!("request error: {}", e)).map(move |_body| {});
                            thread::spawn(move || { tokio::run(future) });
                        }
                    }
                }
            }
            thread::sleep(time::Duration::from_secs(600));
        }
    });
}

fn update_patreon() {
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        loop {
            let channels: Vec<String> = con.smembers("channels").unwrap_or(Vec::new());
            for channel in channels {
                let res: Result<String,_> = con.get(format!("channel:{}:patreon:token", &channel));
                if let Ok(_token) = res {
                    let future = patreon_request(con.clone(), &channel, Method::GET, "https://www.patreon.com/api/oauth2/v2/identity?include=memberships").send()
                        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                        .map_err(|e| println!("request error: {}", e))
                        .map(move |body| {
                            let con = Arc::new(acquire_con());
                            let body = std::str::from_utf8(&body).unwrap();
                            let json: Result<PatreonIdentity,_> = serde_json::from_str(&body);
                            match json {
                                Err(e) => {
                                    log_error(Some(&channel), "update_patreon", &e.to_string());
                                    log_error(Some(&channel), "request_body", &body);
                                }
                                Ok(json) => {
                                    let mut settings = config::Config::default();
                                    settings.merge(config::File::with_name("Settings")).unwrap();
                                    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
                                    let patreon_id = settings.get_str("patreon_id").unwrap_or("".to_owned());
                                    let patreon_sub: String = con.get(format!("channel:{}:patreon:subscribed", &channel)).unwrap();

                                    let mut subscribed = false;
                                    for membership in &json.data.relationships.memberships.data {
                                        if membership.id == patreon_id { subscribed = true }
                                    }

                                    if subscribed {
                                        let _: () = con.set(format!("channel:{}:patreon:subscribed", &channel), true).unwrap();
                                    } else {
                                        let _: () = con.set(format!("channel:{}:patreon:subscribed", &channel), false).unwrap();
                                        let token = settings.get_str("bot_token").unwrap();
                                        if patreon_sub == "true" {
                                            let _: () = con.publish(format!("channel:{}:signals:rename", &channel), token).unwrap();
                                        }
                                    }
                                }
                            }
                        });
                    thread::spawn(move || { tokio::run(future) });
                }
            }
            thread::sleep(time::Duration::from_secs(3600));
        }
    });
}

fn refresh_patreon() {
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        loop {
            let channels: Vec<String> = con.smembers("channels").unwrap_or(Vec::new());
            for channel in channels {
                let channelC = channel.clone();
                let res: Result<String,_> = con.get(format!("channel:{}:patreon:refresh", &channel));
                if let Ok(token) = res {
                    let future = patreon_refresh(con.clone(), &channel, Method::POST, "https://www.patreon.com/api/oauth2/token", Some(format!("grant_type=refresh_token&refresh_token={}", token).as_bytes().to_owned())).send()
                        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                        .map_err(move |e| log_error(Some(&channelC), "refresh_patreon", &e.to_string()))
                        .map(move |body| {
                            let con = Arc::new(acquire_con());
                            let body = std::str::from_utf8(&body).unwrap();
                            let json: Result<PatreonRsp,_> = serde_json::from_str(&body);
                            match json {
                                Err(e) => {
                                    log_error(Some(&channel), "refresh_patreon", &e.to_string());
                                    log_error(Some(&channel), "request_body", &body);
                                }
                                Ok(json) => {
                                    let _: () = con.set(format!("channel:{}:patreon:token", &channel), &json.access_token).unwrap();
                                    let _: () = con.set(format!("channel:{}:patreon:refresh", &channel), &json.refresh_token).unwrap();
                                }
                            }
                        });
                    thread::spawn(move || { tokio::run(future) });
                }
            }
            thread::sleep(time::Duration::from_secs(2592000));
        }
    });
}

fn update_spotify() {
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        loop {
            let channels: Vec<String> = con.smembers("channels").unwrap_or(Vec::new());
            for channel in channels {
                let channelC = channel.clone();
                let res: Result<String,_> = con.get(format!("channel:{}:spotify:refresh", &channel));
                if let Ok(token) = res {
                    let future = spotify_refresh(con.clone(), &channel, Method::POST, "https://accounts.spotify.com/api/token", Some(format!("grant_type=refresh_token&refresh_token={}", token).as_bytes().to_owned())).send()
                        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                        .map_err(move |e| log_error(Some(&channelC), "update_spotify", &e.to_string()))
                        .map(move |body| {
                            let con = Arc::new(acquire_con());
                            let body = std::str::from_utf8(&body).unwrap();
                            let json: Result<SpotifyRefresh,_> = serde_json::from_str(&body);
                            match json {
                                Err(e) => {
                                    log_error(Some(&channel), "update_spotify", &e.to_string());
                                    log_error(Some(&channel), "request_body", &body);
                                }
                                Ok(json) => {
                                    let _: () = con.set(format!("channel:{}:spotify:token", &channel), &json.access_token).unwrap();
                                }
                            }
                        });
                    thread::spawn(move || { tokio::run(future) });
                }
            }
            thread::sleep(time::Duration::from_secs(3600));
        }
    });
}

fn update_watchtime() {
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        loop {
            let channels: Vec<String> = con.smembers("channels").unwrap_or(Vec::new());
            for channel in channels {
                let live: String = con.get(format!("channel:{}:live", &channel)).unwrap_or("false".to_owned());
                let enabled: String = con.hget(format!("channel:{}:settings", &channel), "viewerstats").unwrap_or("false".to_owned());
                if live == "true" && enabled == "true" {
                    let future = request(Method::GET, None, &format!("http://tmi.twitch.tv/group/user/{}/chatters", &channel)).send()
                        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                        .map_err(|e| println!("request error: {}", e))
                        .map(move |body| {
                            let con = Arc::new(acquire_con());
                            let body = std::str::from_utf8(&body).unwrap();
                            let json: Result<TmiChatters,_> = serde_json::from_str(&body);
                            match json {
                                Err(e) => {
                                    log_error(Some(&channel), "update_watchtime", &e.to_string());
                                    log_error(Some(&channel), "request_body", &body);
                                }
                                Ok(json) => {
                                    let mut nicks: Vec<String> = Vec::new();
                                    let mut moderators = json.chatters.moderators.clone();
                                    let mut viewers = json.chatters.viewers.clone();
                                    let mut vips = json.chatters.vips.clone();
                                    nicks.append(&mut moderators);
                                    nicks.append(&mut viewers);
                                    nicks.append(&mut vips);
                                    for nick in nicks.iter() {
                                        let res: Result<String,_> = con.hget(format!("channel:{}:watchtimes", &channel), nick);
                                        if let Ok(wt) = res {
                                            let num: i64 = wt.parse().unwrap();
                                            let _: () = con.hset(format!("channel:{}:watchtimes", &channel), nick, num + 1).unwrap();
                                        } else {
                                            let _: () = con.hset(format!("channel:{}:watchtimes", &channel), nick, 1).unwrap();
                                        }
                                    }
                                }
                            }
                        });
                    thread::spawn(move || { tokio::run(future) });
                }
            }
            thread::sleep(time::Duration::from_secs(60));
        }
    });
}

fn update_live() {
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        loop {
            let channels: Vec<String> = con.smembers("channels").unwrap_or(Vec::new());
            if channels.len() > 0 {
                let mut ids = Vec::new();
                for channel in channels.clone() {
                    let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
                    ids.push(id);
                }
                // TODO: should channels[0] be used here?
                let future = twitch_kraken_request(con.clone(), &channels[0], None, None, Method::GET, &format!("https://api.twitch.tv/kraken/streams?channel={}", ids.join(","))).send()
                    .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                    .map_err(|e| println!("request error: {}", e))
                    .map(move |body| {
                        let con = Arc::new(acquire_con());
                        let body = std::str::from_utf8(&body).unwrap();
                        let json: Result<KrakenStreams,_> = serde_json::from_str(&body);
                        match json {
                            Err(e) => {
                                log_error(Some(&channels[0]), "update_live", &e.to_string());
                                log_error(Some(&channels[0]), "request_body", &body);
                            }
                            Ok(json) => {
                                let live_channels: Vec<String> = json.streams.iter().map(|stream| stream.channel.name.to_owned()).collect();
                                for channel in channels {
                                    let live: String = con.get(format!("channel:{}:live", channel)).expect("get:live");
                                    if live_channels.contains(&channel) {
                                        let stream = json.streams.iter().find(|stream| { return stream.channel.name == channel }).unwrap();
                                        if live == "false" {
                                            let _: () = con.set(format!("channel:{}:live", channel), true).unwrap();
                                            let _: () = con.del(format!("channel:{}:hosts:recent", channel)).unwrap();
                                            // reset notice timers
                                            let keys: Vec<String> = con.keys(format!("channel:{}:notices:*:messages", channel)).unwrap();
                                            for key in keys.iter() {
                                                let int: Vec<&str> = key.split(":").collect();
                                                let _: () = con.set(format!("channel:{}:notices:{}:countdown", channel, int[3]), int[3].clone()).unwrap();
                                            }
                                            // send discord announcements
                                            let tres: Result<String,_> = con.hget(format!("channel:{}:settings", channel), "discord:token");
                                            let ires: Result<String,_> = con.hget(format!("channel:{}:settings", channel), "discord:channel-id");
                                            if let (Ok(_token), Ok(id)) = (tres, ires) {
                                                let message: String = con.hget(format!("channel:{}:settings", channel), "discord:live-message").unwrap_or("".to_owned());
                                                let display: String = con.get(format!("channel:{}:display-name", channel)).expect("get:display-name");
                                                let body = format!("{{ \"content\": \"{}\", \"embed\": {{ \"author\": {{ \"name\": \"{}\" }}, \"title\": \"{}\", \"url\": \"http://twitch.tv/{}\", \"thumbnail\": {{ \"url\": \"{}\" }}, \"fields\": [{{ \"name\": \"Now Playing\", \"value\": \"{}\" }}] }} }}", &message, &display, stream.channel.status, channel, stream.channel.logo, stream.channel.game);
                                                let future = discord_request(con.clone(), &channel, Some(body.as_bytes().to_owned()), Method::POST, &format!("https://discordapp.com/api/channels/{}/messages", id)).send().and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() }).map_err(|e| println!("request error: {}", e)).map(move |_body| {});
                                                thread::spawn(move || { tokio::run(future) });
                                            }
                                        }
                                    } else {
                                        if live == "true" {
                                            let _: () = con.set(format!("channel:{}:live", channel), false).unwrap();
                                            // reset stats
                                            let res: Result<String,_> = con.hget(format!("channel:{}:settings", channel), "stats:reset");
                                            if let Err(_e) = res {
                                                let _: () = con.del(format!("channel:{}:stats:pubg", channel)).unwrap();
                                                let _: () = con.del(format!("channel:{}:stats:fortnite", channel)).unwrap();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    });
                thread::spawn(move || { tokio::run(future) });
            }

            thread::sleep(time::Duration::from_secs(60));
        }
    });
}

fn update_stats() {
    // pubg
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        loop {
            let channels: Vec<String> = con.smembers("channels").unwrap_or(Vec::new());
            for channel in channels {
                let reset: String = con.hget(format!("channel:{}:stats:pubg", &channel), "reset").unwrap_or("false".to_owned());
                let res: Result<String,_> = con.hget(format!("channel:{}:settings", &channel), "stats:reset");
                if let Ok(hour) = res {
                    let res: Result<u32,_> = hour.parse();
                    if let Ok(num) = res {
                        if num == Utc::now().time().hour() && reset == "true" {
                            let _: () = con.del(format!("channel:{}:stats:pubg", &channel)).unwrap();
                        } else if num != Utc::now().time().hour() && reset == "false" {
                            let _: () = con.hset(format!("channel:{}:stats:pubg", &channel), "reset", true).unwrap();
                        }
                    }
                }
                let live: String = con.get(format!("channel:{}:live", &channel)).expect("get:live");
                if live == "true" {
                    let res1: Result<String,_> = con.hget(format!("channel:{}:settings", &channel), "pubg:token");
                    let res2: Result<String,_> = con.hget(format!("channel:{}:settings", &channel), "pubg:name");
                    if let (Ok(_token), Ok(name)) = (res1, res2) {
                        let platform: String = con.hget(format!("channel:{}:settings", &channel), "pubg:platform").unwrap_or("steam".to_owned());
                        let res: Result<String,_> = con.hget(format!("channel:{}:settings", &channel), "pubg:id");
                        if let Ok(id) = res {
                            let mut cursor: String = con.hget(format!("channel:{}:stats:pubg", &channel), "cursor").unwrap_or("".to_owned());
                            let future = pubg_request(con.clone(), &channel, &format!("https://api.pubg.com/shards/{}/players/{}", platform, id)).send()
                                .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                .map_err(|e| println!("request error: {}", e))
                                .map(move |body| {
                                    let con = Arc::new(acquire_con());
                                    let body = std::str::from_utf8(&body).unwrap();
                                    let json: Result<PubgPlayer,_> = serde_json::from_str(&body);
                                    match json {
                                        Err(e) => {
                                            log_error(Some(&channel), "update_pubg", &e.to_string());
                                            log_error(Some(&channel), "request_body", &body);
                                        }
                                        Ok(json) => {
                                            if json.data.relationships.matches.data.len() > 0 {
                                                if cursor == "" { cursor = json.data.relationships.matches.data[0].id.to_owned() }
                                                let _: () = con.hset(format!("channel:{}:stats:pubg", &channel), "cursor", &json.data.relationships.matches.data[0].id).unwrap();
                                                for match_ in json.data.relationships.matches.data.iter() {
                                                    let idC = id.clone();
                                                    let channelC = channel.clone();
                                                    if match_.id == cursor { break }
                                                    else {
                                                        let future = pubg_request(con.clone(), &channel, &format!("https://api.pubg.com/shards/pc-na/matches/{}", &match_.id)).send()
                                                            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                                            .map_err(|e| println!("request error: {}", e))
                                                            .map(move |body| {
                                                                let con = Arc::new(acquire_con());
                                                                let body = std::str::from_utf8(&body).unwrap();
                                                                let json: Result<PubgMatch,_> = serde_json::from_str(&body);
                                                                match json {
                                                                    Err(e) => {
                                                                        log_error(Some(&channelC), "update_pubg", &e.to_string());
                                                                        log_error(Some(&channelC), "request_body", &body);
                                                                    }
                                                                    Ok(json) => {
                                                                        for p in json.included.iter().filter(|i| i.type_ == "participant") {
                                                                            if p.attributes["stats"]["playerId"] == idC {
                                                                                for stat in ["winPlace", "kills", "headshotKills", "roadKills", "teamKills", "damageDealt", "vehicleDestroys"].iter() {
                                                                                    if let Number(num) = &p.attributes["stats"][stat] {
                                                                                        if let Some(num) = num.as_f64() {
                                                                                            let mut statname: String = (*stat).to_owned();
                                                                                            if *stat == "winPlace" { statname = "wins".to_owned() }
                                                                                            let res: Result<String,_> = con.hget(format!("channel:{}:stats:pubg", &channelC), &statname);
                                                                                            if let Ok(old) = res {
                                                                                                let n: u64 = old.parse().unwrap();
                                                                                                if *stat == "winPlace" {
                                                                                                    if num as u64 == 1 {
                                                                                                        let _: () = con.hset(format!("channel:{}:stats:pubg", &channelC), &statname, n + 1).unwrap();
                                                                                                    }
                                                                                                } else {
                                                                                                    let _: () = con.hset(format!("channel:{}:stats:pubg", &channelC), &statname, n + (num as u64)).unwrap();
                                                                                                }
                                                                                            } else {
                                                                                                if *stat == "winPlace" {
                                                                                                    if num as u64 == 1 {
                                                                                                        let _: () = con.hset(format!("channel:{}:stats:pubg", &channelC), &statname, 1).unwrap();
                                                                                                    }
                                                                                                } else {
                                                                                                    let _: () = con.hset(format!("channel:{}:stats:pubg", &channelC), &statname, num as u64).unwrap();
                                                                                                }
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            });
                                                        thread::spawn(move || { tokio::run(future) });
                                                    }
                                                }
                                            }
                                        }
                                    }
                                });
                            thread::spawn(move || { tokio::run(future) });
                        } else {
                            let future = pubg_request(con.clone(), &channel, &format!("https://api.pubg.com/shards/{}/players?filter%5BplayerNames%5D={}", platform, name)).send()
                                .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                .map_err(|e| println!("request error: {}", e))
                                .map(move |body| {
                                    let con = Arc::new(acquire_con());
                                    let body = std::str::from_utf8(&body).unwrap();
                                    let json: Result<PubgPlayers,_> = serde_json::from_str(&body);
                                    match json {
                                        Err(e) => {
                                            log_error(Some(&channel), "update_pubg", &e.to_string());
                                            log_error(Some(&channel), "request_body", &body);
                                        }
                                        Ok(json) => {
                                            if json.data.len() > 0 {
                                                let _: () = con.hset(format!("channel:{}:settings", &channel), "pubg:id", &json.data[0].id).unwrap();
                                            }
                                        }
                                    }
                                });
                            thread::spawn(move || { tokio::run(future) });
                        }
                    }
                }
            }
            thread::sleep(time::Duration::from_secs(60));
        }
    });

    // fortnite
    thread::spawn(move || {
        let con = Arc::new(acquire_con());
        loop {
            let channels: Vec<String> = con.smembers("channels").unwrap_or(Vec::new());
            for channel in channels {
                let reset: String = con.hget(format!("channel:{}:stats:fortnite", &channel), "reset").unwrap_or("false".to_owned());
                let res: Result<String,_> = con.hget(format!("channel:{}:settings", &channel), "stats:reset");
                if let Ok(hour) = res {
                    let num: Result<u32,_> = hour.parse();
                    if let Ok(hour) = num {
                        if hour == Utc::now().time().hour() && reset == "true" {
                            let _: () = con.del(format!("channel:{}:stats:fortnite", &channel)).unwrap();
                        } else if hour != Utc::now().time().hour() && reset == "false" {
                            let _: () = con.hset(format!("channel:{}:stats:fortnite", &channel), "reset", true).unwrap();
                        }
                    }
                }
                let live: String = con.get(format!("channel:{}:live", &channel)).expect("get:live");
                if live == "true" {
                    let res1: Result<String,_> = con.hget(format!("channel:{}:settings", &channel), "fortnite:token");
                    let res2: Result<String,_> = con.hget(format!("channel:{}:settings", &channel), "fortnite:name");
                    if let (Ok(_token), Ok(name)) = (res1, res2) {
                        let platform: String = con.hget(format!("channel:{}:settings", &channel), "pubg:platform").unwrap_or("pc".to_owned());
                        let mut cursor: String = con.hget(format!("channel:{}:stats:fortnite", &channel), "cursor").unwrap_or("".to_owned());
                        let future = fortnite_request(con.clone(), &channel, &format!("https://api.fortnitetracker.com/v1/profile/{}/{}", platform, name)).send()
                            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                            .map_err(|e| println!("request error: {}", e))
                            .map(move |body| {
                                let con = Arc::new(acquire_con());
                                let body = std::str::from_utf8(&body).unwrap();
                                let json: Result<FortniteApi,_> = serde_json::from_str(&body);
                                match json {
                                    Err(e) => {
                                        log_error(Some(&channel), "update_fortnite", &e.to_string());
                                        log_error(Some(&channel), "request_body", &body);
                                    }
                                    Ok(json) => {
                                        if json.recentMatches.len() > 0 {
                                            if cursor == "" { cursor = json.recentMatches[0].id.to_string() }
                                            let _: () = con.hset(format!("channel:{}:stats:fortnite", &channel), "cursor", &json.recentMatches[0].id.to_string()).unwrap();
                                            for match_ in json.recentMatches.iter() {
                                                if match_.id.to_string() == cursor { break }
                                                else {
                                                    let res: Result<String,_> = con.hget(format!("channel:{}:stats:fortnite", &channel), "wins");
                                                    if let Ok(old) = res {
                                                        let n: u64 = old.parse().unwrap();
                                                        let _: () = con.hset(format!("channel:{}:stats:fortnite", &channel), "wins", n + (match_.top1 as u64)).unwrap();
                                                    } else {
                                                        let _: () = con.hset(format!("channel:{}:stats:fortnite", &channel), "wins", match_.top1 as u64).unwrap();
                                                    }

                                                    let res: Result<String,_> = con.hget(format!("channel:{}:stats:fortnite", &channel), "kills");
                                                    if let Ok(old) = res {
                                                        let n: u64 = old.parse().unwrap();
                                                        let _: () = con.hset(format!("channel:{}:stats:fortnite", &channel), "kills", n + (match_.kills as u64)).unwrap();
                                                    } else {
                                                        let _: () = con.hset(format!("channel:{}:stats:fortnite", &channel), "kills", match_.kills as u64).unwrap();
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            });
                        thread::spawn(move || { tokio::run(future) });
                    }
                }
            }
            thread::sleep(time::Duration::from_secs(60));
        }
    });
}

fn spawn_timers(client: Arc<IrcClient>, channel: String, receivers: Vec<Receiver<ThreadAction>>) {
    let notice_con = acquire_con();
    let snotice_con = acquire_con();
    let so_con = acquire_con();
    let comm_con = acquire_con();
    let notice_client = client.clone();
    let snotice_client = client.clone();
    let so_client = client.clone();
    let comm_client = client.clone();
    let notice_channel = channel.clone();
    let snotice_channel = channel.clone();
    let so_channel = channel.clone();
    let comm_channel = channel.clone();
    let notice_receiver = receivers[0].clone();
    let snotice_receiver = receivers[1].clone();
    let so_receiver = receivers[2].clone();
    let comm_receiver = receivers[3].clone();

    // TODO: scheduled notices
    thread::spawn(move || {
        let con = Arc::new(snotice_con);
        loop {
            let rsp = snotice_receiver.recv_timeout(time::Duration::from_secs(60));
            match rsp {
                Ok(action) => {
                    match action {
                        ThreadAction::Kill => break,
                        ThreadAction::Part(_) => {}
                    }
                }
                Err(err) => {
                    match err {
                        RecvTimeoutError::Disconnected => break,
                        RecvTimeoutError::Timeout => {}
                    }
                }
            }

            let live: String = con.get(format!("channel:{}:live", snotice_channel)).expect("get:live");
            if live == "true" {
                let keys: Vec<String> = con.keys(format!("channel:{}:snotices:*", snotice_channel)).unwrap();
                keys.iter().for_each(|key| {
                    let time: String = con.hget(key, "time").expect("hget:time");
                    let cmd: String = con.hget(key, "cmd").expect("hget:cmd");
                    let res = DateTime::parse_from_str(&format!("2000-01-01T{}", time), "%Y-%m-%dT%H:%M%z");
                    if let Ok(dt) = res {
                        let hour = dt.naive_utc().hour();
                        let min = dt.naive_utc().minute();
                        if hour == Utc::now().time().hour() && min == Utc::now().time().minute() {
                            let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, cmd), "message");
                            if let Ok(message) = res {
                                send_parsed_message(con.clone(), snotice_client.clone(), snotice_channel.clone(), message.to_owned(), Vec::new(), None);
                            }
                        }
                    }
                });
            }
        }
    });

    // TODO: shoutouts
    thread::spawn(move || {
        let con = Arc::new(so_con);
        loop {
            let clientC = so_client.clone();
            let channelC = so_channel.clone();
            let live: String = con.get(format!("channel:{}:live", &so_channel)).unwrap_or("false".to_owned());
            let hostm: String = con.hget(format!("channel:{}:settings", &so_channel), "channel:host-message").unwrap_or("".to_owned());
            let autom: String = con.hget(format!("channel:{}:settings", &so_channel), "channel:autohost-message").unwrap_or("".to_owned());
            if live == "true" && (!hostm.is_empty() || !autom.is_empty()) {
                let id: String = con.get(format!("channel:{}:id", &so_channel)).unwrap();
                let recent: Vec<String> = con.smembers(format!("channel:{}:hosts:recent", &so_channel)).unwrap_or(Vec::new());
                let future = twitch_kraken_request(con.clone(), &channelC, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}/hosts", &id)).send()
                    .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                    .map_err(|e| println!("request error: {}", e))
                    .map(move |body| {
                        let con = Arc::new(acquire_con());
                        let body = std::str::from_utf8(&body).unwrap();
                        let json: Result<KrakenHosts,_> = serde_json::from_str(&body);
                        match json {
                            Err(e) => {
                                log_error(Some(&channelC), "auto_shoutouts", &e.to_string());
                                log_error(Some(&channelC), "request_body", &body);
                            }
                            Ok(json) => {
                                let list: String = con.hget(format!("channel:{}:settings", &channelC), "autohost:blacklist").unwrap_or("".to_owned()); // UNDOCUMENTED
                                let mut blacklist: Vec<String> = Vec::new();
                                for nick in list.split_whitespace() { blacklist.push(nick.to_string()) }
                                for host in json.hosts {
                                    let client = clientC.clone();
                                    let channel = channelC.clone();
                                    let blacklist = blacklist.clone();
                                    let hostm = hostm.clone();
                                    let autom = autom.clone();
                                    if !recent.contains(&host.host_id) {
                                        let _: () = con.sadd(format!("channel:{}:hosts:recent", &channel), &host.host_id).unwrap();
                                        let future = twitch_kraken_request(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/streams?channel={}", &host.host_id)).send()
                                            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                            .map_err(|e| println!("request error: {}", e))
                                            .map(move |body| {
                                                let con = Arc::new(acquire_con());
                                                let body = std::str::from_utf8(&body).unwrap();
                                                let json: Result<KrakenStreams,_> = serde_json::from_str(&body);
                                                match json {
                                                    Err(e) => {
                                                        log_error(Some(&channel), "auto_shoutouts", &e.to_string());
                                                        log_error(Some(&channel), "request_body", &body);
                                                    }
                                                    Ok(json) => {
                                                        if !blacklist.contains(&host.host_id) {
                                                            if json.total > 0 {
                                                                if !hostm.is_empty() {
                                                                    let mut message: String = hostm.to_owned();
                                                                    message = replace_var("url", &json.streams[0].channel.url, &message);
                                                                    message = replace_var("name", &json.streams[0].channel.display_name, &message);
                                                                    message = replace_var("game", &json.streams[0].channel.game, &message);
                                                                    message = replace_var("viewers", &json.streams[0].viewers.to_string(), &message);
                                                                    send_message(con.clone(), client.clone(), channel.clone(), message);
                                                                }
                                                            } else {
                                                                if !autom.is_empty() {
                                                                    let future = twitch_kraken_request(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}", &host.host_id)).send()
                                                                        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                                                        .map_err(|e| println!("request error: {}", e))
                                                                        .map(move |body| {
                                                                            let con = Arc::new(acquire_con());
                                                                            let body = std::str::from_utf8(&body).unwrap();
                                                                            let json: Result<KrakenChannel,_> = serde_json::from_str(&body);
                                                                            match json {
                                                                                Err(e) => {
                                                                                    log_error(Some(&channel), "auto_shoutouts", &e.to_string());
                                                                                    log_error(Some(&channel), "request_body", &body);
                                                                                }
                                                                                Ok(json) => {
                                                                                    let mut message: String = autom.to_owned();
                                                                                    message = replace_var("url", &json.url, &message);
                                                                                    message = replace_var("name", &json.display_name, &message);
                                                                                    message = replace_var("game", &json.game, &message);
                                                                                    send_message(con.clone(), client.clone(), channel.clone(), message);
                                                                                }
                                                                            }
                                                                        });
                                                                    thread::spawn(move || { tokio::run(future) });
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            });
                                        thread::spawn(move || { tokio::run(future) });
                                    }
                                }
                            }
                        }
                    });
                thread::spawn(move || { tokio::run(future) });
            }
            let rsp = so_receiver.recv_timeout(time::Duration::from_secs(60));
            match rsp {
                Ok(action) => {
                    match action {
                        ThreadAction::Kill => break,
                        ThreadAction::Part(_) => {}
                    }
                }
                Err(err) => {
                    match err {
                        RecvTimeoutError::Disconnected => break,
                        RecvTimeoutError::Timeout => {}
                    }
                }
            }
        }
    });
}

fn run_command(matches: &ArgMatches) {
    let con = Arc::new(acquire_con());
    let channel: String = matches.value_of("channel").unwrap().to_owned();
    let cmd = matches.values_of("command").unwrap();
    let mut command: Vec<String> = Vec::new();
    for c in cmd { command.push(c.to_owned()) }

    if command.len() > 0 {
        let _: () = con.publish(format!("channel:{}:signals:command", &channel), format!("{}", command.join(" "))).unwrap();
    }
}
