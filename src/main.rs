#![feature(proc_macro_hygiene, decl_macro, custom_attribute)]

#[macro_use] extern crate rocket;

mod commands;
mod types;
mod util;
mod web;

use crate::types::*;
use crate::util::*;

use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::{Arc,Mutex};
use std::{thread,time};

use clap::load_yaml;
use config;
use clap::{App, ArgMatches};
use bcrypt::{DEFAULT_COST, hash};
use crossbeam_channel::{unbounded,Sender,Receiver,RecvTimeoutError,TryRecvError};
use irc::error;
use irc::client::prelude::*;
use url::Url;
use regex::{Regex,RegexBuilder};
use serde_json::value::Value::Number;
use chrono::{Utc, DateTime, FixedOffset, Duration, Timelike};
use reqwest::{self, header};
use serenity;
use serenity::framework::standard::StandardFramework;
use rocket::routes;
use rocket_contrib::templates::Template;
use rocket_contrib::serve::StaticFiles;
use r2d2_redis::{r2d2, redis, RedisConnectionManager};
use r2d2_redis::redis::Commands;

fn main() {
    let yaml = load_yaml!("../cli.yml");
    let matches = App::from_yaml(yaml).get_matches();
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();

    let redis_host = settings.get_str("redis_host").unwrap_or("redis://127.0.0.1".to_owned());

    let manager = RedisConnectionManager::new(&redis_host[..]).unwrap();
    let pool = r2d2::Pool::builder().max_size(200).build(manager).unwrap();
    let pool_c1 = pool.clone();

    if let Some(matches) = matches.subcommand_matches("run_command") { run_command(pool.clone(), &settings, matches) }
    else {
        thread::spawn(move || { new_channel_listener(pool_c1) });
        thread::spawn(move || {
            rocket::ignite()
              .mount("/assets", StaticFiles::from("assets"))
              .mount("/", routes![web::index, web::dashboard, web::commands, web::public_data, web::data, web::login, web::logout, web::signup, web::password, web::title, web::game, web::new_command, web::save_command, web::trash_command, web::new_notice, web::trash_notice, web::save_setting, web::trash_setting, web::new_blacklist, web::save_blacklist, web::trash_blacklist])
              .register(catchers![web::internal_error, web::not_found])
              .attach(Template::fairing())
              .attach(RedisConnection::fairing())
              .launch()
        });
        thread::spawn(move || {
            let con = Arc::new(pool.get().unwrap());
            let mut bots: HashMap<String, (HashSet<String>, Config)> = HashMap::new();
            let bs: HashSet<String> = con.smembers("bots").unwrap();
            for bot in bs {
                let passphrase: String = con.get(format!("bot:{}:token", bot)).unwrap();
                let channel_hash: HashSet<String> = con.smembers(format!("bot:{}:channels", bot)).unwrap();
                let mut channels: Vec<String> = Vec::new();
                channels.extend(channel_hash.iter().cloned().map(|chan| { format!("#{}", chan) }));
                let config = Config {
                    server: Some("irc.chat.twitch.tv".to_owned()),
                    use_ssl: Some(true),
                    nickname: Some(bot.to_owned()),
                    password: Some(format!("oauth:{}", passphrase)),
                    channels: Some(channels),
                    ..Default::default()
                };
                bots.insert(bot.to_owned(), (channel_hash.clone(), config));
                for channel in channel_hash.iter() {
                    discord_handler(pool.clone(), channel.to_owned());
                    update_watchtime(pool.clone(), channel.to_owned());
                    update_live(pool.clone(), channel.to_owned());
                    update_pubg(pool.clone(), channel.to_owned());
                }
            }
            run_reactor(pool.clone(), bots);
        });

        loop { thread::sleep(time::Duration::from_secs(60)) }
    }
}

fn run_reactor(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, bots: HashMap<String, (HashSet<String>, Config)>) {
    let con = Arc::new(pool.get().unwrap());
    let mut reactor = IrcReactor::new().unwrap();
    let mut senders: HashMap<String, Vec<Sender<ThreadAction>>> = HashMap::new();
    loop {
        bots.iter().for_each(|(_bot, channels)| {
            let client = Arc::new(reactor.prepare_client_and_connect(&channels.1).unwrap());
            client.identify().unwrap();
            let _ = client.send("CAP REQ :twitch.tv/tags");
            let _ = client.send("CAP REQ :twitch.tv/commands");
            register_handler((*client).clone(), &mut reactor, con.clone());
            for channel in channels.0.iter() {
                let (sender1, receiver1) = unbounded();
                let (sender2, receiver2) = unbounded();
                let (sender3, receiver3) = unbounded();
                let (sender4, receiver4) = unbounded();
                senders.insert(channel.to_owned(), [sender1,sender2,sender3,sender4].to_vec());
                spawn_timers(client.clone(), pool.clone(), channel.to_owned(), [receiver1,receiver2,receiver3].to_vec());
                rename_channel_listener(pool.clone(), client.clone(), channel.to_owned(), senders.clone());
                command_listener(pool.clone(), client.clone(), channel.to_owned(), receiver4);
            }
        });
        let res = reactor.run();
        match res {
            Ok(_) => break,
            Err(e) => {
                eprintln!("[run_reactor] {}", e);
                bots.iter().for_each(|(_bot, channels)| {
                    for channel in channels.0.iter() {
                        if let Some(senders) = senders.get(channel) {
                            for sender in senders {
                                let _ = sender.send(ThreadAction::Kill);
                            }
                        }
                    }
                });
            }
        }
    }
}

fn new_channel_listener(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>) {
    let con = pool.get().unwrap();
    let mut conn = pool.get().unwrap();
    let mut ps = conn.as_pubsub();
    ps.subscribe("new_channels").unwrap();

    loop {
        let msg = ps.get_message().unwrap();
        let channel: String = msg.get_payload().unwrap();
        let mut bots: HashMap<String, (HashSet<String>, Config)> = HashMap::new();
        let bot: String = con.get(format!("channel:{}:bot", channel)).unwrap();
        let passphrase: String = con.get(format!("bot:{}:token", bot)).unwrap();
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
            ..Default::default()
        };
        bots.insert(bot.to_owned(), (channel_hash.clone(), config));
        for channel in channel_hash.iter() {
            discord_handler(pool.clone(), channel.to_owned());
            update_watchtime(pool.clone(), channel.to_owned());
            update_live(pool.clone(), channel.to_owned());
            update_pubg(pool.clone(), channel.to_owned());
        }
        run_reactor(pool.clone(), bots);
    }
}

fn rename_channel_listener(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, client: Arc<IrcClient>, channel: String, senders: HashMap<String, Vec<Sender<ThreadAction>>>) {
    thread::spawn(move || {
        let con = pool.get().unwrap();
        let mut conn = pool.get().unwrap();
        let mut ps = conn.as_pubsub();
        ps.subscribe(format!("channel:{}:signals:rename", channel)).unwrap();

        let msg = ps.get_message().unwrap();
        let token: String = msg.get_payload().unwrap();

        let req = reqwest::Client::new();
        let rsp = req.get("https://api.twitch.tv/helix/users").header(header::AUTHORIZATION, format!("Bearer {}", &token)).send();
        match rsp {
            Err(e) => { eprintln!("[rename_channel_listener] {}", e) }
            Ok(mut rsp) => {
                let json: Result<HelixUsers,_> = rsp.json();
                match json {
                    Err(e) => { eprintln!("[rename_channel_listener] {}", e) }
                    Ok(json) => {
                        if let Some(senders) = senders.get(&channel) {
                            for sender in senders {
                                let _ = sender.send(ThreadAction::Kill);
                            }
                        }
                        let _ = client.send_quit("");

                        let bot: String = con.get(format!("channel:{}:bot", &channel)).unwrap();
                        let _: () = con.srem(format!("bot:{}:channels", &bot), &channel).unwrap();
                        let _: () = con.sadd("bots", &json.data[0].login).unwrap();
                        let _: () = con.sadd(format!("bot:{}:channels", &json.data[0].login), &channel).unwrap();
                        let _: () = con.set(format!("bot:{}:token", &json.data[0].login), &token).unwrap();
                        let _: () = con.set(format!("channel:{}:bot", &channel), &json.data[0].login).unwrap();

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
                        run_reactor(pool.clone(), bots);
                    }
                }
            }
        }
    });
}

fn command_listener(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, client: Arc<IrcClient>, channel: String, receiver: Receiver<ThreadAction>) {
    thread::spawn(move || {
        let con = Arc::new(pool.get().unwrap());
        let mut conn = pool.get().unwrap();
        let mut ps = conn.as_pubsub();
        ps.set_read_timeout(Some(time::Duration::from_secs(10)));

        loop {
            let rsp = receiver.try_recv();
            match rsp {
                Ok(action) => {
                    match action {
                        ThreadAction::Kill => break
                    }
                }
                Err(err) => {
                    match err {
                        TryRecvError::Disconnected => break,
                        TryRecvError::Empty => {
                            ps.subscribe(format!("channel:{}:signals:command", channel)).unwrap();

                            let res = ps.get_message();
                            match res {
                                Err(_) => {}
                                Ok(msg) => {
                                    let payload: String = msg.get_payload().unwrap();
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
                                        let args: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();

                                        // parse native commands
                                        for cmd in commands::native_commands.iter() {
                                            if format!("{}{}", prefix, cmd.0) == word {
                                                (cmd.1)(con.clone(), &client, &channel, &args);
                                                break;
                                            }
                                        }

                                        // parse custom commands
                                        let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, word), "message");
                                        if let Ok(message) = res {
                                            let mut message = message;
                                            message = parse_message(&message, con.clone(), &client, &channel, None, &args);
                                            send_message(con.clone(), &client, &channel, message.to_owned(), &args, None);
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
}

fn register_handler(client: IrcClient, reactor: &mut IrcReactor, con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>) {
    let msg_handler = move |client: &IrcClient, irc_message: Message| -> error::Result<()> {
        match &irc_message.command {
            Command::PING(_,_) => { let _ = client.send_pong(":tmi.twitch.tv"); }
            Command::PRIVMSG(chan, msg) => {
                let channel = &chan[1..];
                let nick = get_nick(&irc_message);
                let prefix: String = con.hget(format!("channel:{}:settings", channel), "command:prefix").unwrap_or("!".to_owned());
                let mut words = msg.split_whitespace();

                // parse ircV3 badges
                if let Some(word) = words.next() {
                    let mut word = word.to_lowercase();
                    let mut args: Vec<String> = words.map(|w| w.to_owned()).collect();
                    let mut badges: HashMap<String, Option<String>> = HashMap::new();
                    if let Some(tags) = &irc_message.tags {
                        tags.iter().for_each(|tag| {
                            if let Some(value) = &tag.1 {
                                // println!("{}: {}", tag.0, value);
                                if tag.0 == "badges" {
                                    let bs: Vec<&str> = value.split(",").collect();
                                    for bstr in bs.iter() {
                                        let badge: Vec<&str> = bstr.split("/").collect();
                                        if badge.len() == 2 {
                                            badges.insert(badge[0].to_owned(), Some(badge[1].to_owned()));
                                        } else {
                                            badges.insert(badge[0].to_owned(), None);
                                        }
                                    }
                                }
                            }
                        });
                    }

                    let mut subscriber = false;
                    if let Some(value) = badges.get("subscriber") { subscriber = true }

                    let mut auth = false;
                    if let Some(value) = badges.get("broadcaster") { auth = true }
                    if let Some(value) = badges.get("moderator") { auth = true }

                    // moderate incoming messages
                    // TODO: symbols, length
                    if !auth {
                        let display: String = con.get(format!("channel:{}:moderation:display", channel)).unwrap_or("false".to_owned());
                        let caps: String = con.get(format!("channel:{}:moderation:caps", channel)).unwrap_or("false".to_owned());
                        let colors: String = con.get(format!("channel:{}:moderation:colors", channel)).unwrap_or("false".to_owned());
                        let links: Vec<String> = con.smembers(format!("channel:{}:moderation:links", channel)).unwrap_or(Vec::new());
                        let bkeys: Vec<String> = con.keys(format!("channel:{}:moderation:blacklist:*", channel)).unwrap();
                        if colors == "true" && msg.len() > 6 && msg.as_bytes()[0] == 1 && &msg[1..7] == "ACTION" {
                            let _ = client.send_privmsg(chan, format!("/timeout {} 1", nick));
                            if display == "true" { send_message(con.clone(), &client, channel, format!("@{} you've been timed out for posting colors", nick), &Vec::new(), None); }
                        }
                        if caps == "true" {
                            let limit: String = con.get(format!("channel:{}:moderation:caps:limit", channel)).unwrap();
                            let trigger: String = con.get(format!("channel:{}:moderation:caps:trigger", channel)).unwrap();
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
                                            if display == "true" { send_message(con.clone(), &client, channel, format!("@{} you've been timed out for posting too many caps", nick), &Vec::new(), None); }
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
                                        if url.len() > 7 && &url[..7] != "http://" && &url[..8] != "https://" { url = format!("http://{}", url) }
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
                                                    if display == "true" { send_message(con.clone(), &client, channel, format!("@{} you've been timed out for posting links", nick), &Vec::new(), None); }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        for key in bkeys {
                            let key: Vec<&str> = key.split(":").collect();
                            let rgx: String = con.hget(format!("channel:{}:moderation:blacklist:{}", channel, key[4]), "regex").unwrap();
                            let length: String = con.hget(format!("channel:{}:moderation:blacklist:{}", channel, key[4]), "length").unwrap();
                            match RegexBuilder::new(&rgx).case_insensitive(true).build() {
                                Err(e) => { eprintln!("{}", e) }
                                Ok(rgx) => {
                                    if rgx.is_match(&msg) {
                                        let _ = client.send_privmsg(chan, format!("/timeout {} {}", nick, length));
                                        if display == "true" { send_message(con.clone(), &client, channel, format!("@{} you've been timed out for posting a blacklisted phrase", nick), &Vec::new(), None); }
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
                    let args: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();

                    // parse native commands
                    for cmd in commands::native_commands.iter() {
                        if format!("{}{}", prefix, cmd.0) == word {
                            if args.len() == 0 {
                                if !cmd.2 || auth { (cmd.1)(con.clone(), &client, channel, &args) }
                            } else {
                                if !cmd.3 || auth { (cmd.1)(con.clone(), &client, channel, &args) }
                            }
                            break;
                        }
                    }

                    // parse custom commands
                    let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, word), "message");
                    if let Ok(message) = res {
                        let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, word), "lastrun");
                        let mut within5 = false;
                        if let Ok(lastrun) = res {
                            let timestamp = DateTime::parse_from_rfc3339(&lastrun).unwrap();
                            let diff = Utc::now().signed_duration_since(timestamp);
                            if diff.num_seconds() < 5 { within5 = true }
                        }
                        if !within5 {
                            let mut protected: &str = "cmd";
                            if args.len() > 0 { protected = "arg" }
                            let protected: String = con.hget(format!("channel:{}:commands:{}", channel, word), format!("{}_protected", protected)).unwrap();
                            if protected == "false" || auth {
                                let _: () = con.hset(format!("channel:{}:commands:{}", channel, word), "lastrun", Utc::now().to_rfc3339()).unwrap();
                                send_message(con.clone(), &client, channel, message.to_owned(), &args, Some(&irc_message));
                            }
                        }
                    }

                    // parse greetings
                    let keys: Vec<String> = con.keys(format!("channel:{}:greetings:*", channel)).unwrap();
                    for key in keys.iter() {
                        let key: Vec<&str> = key.split(":").collect();
                        if key[3] == nick {
                            let msg: String = con.hget(format!("channel:{}:greetings:{}", channel, key[3]), "message").unwrap();
                            let hours: i64 = con.hget(format!("channel:{}:greetings:{}", channel, key[3]), "hours").unwrap();
                            let res: Result<String,_> = con.hget(format!("channel:{}:lastseen", channel), key[3]);
                            if let Ok(lastseen) = res {
                                let timestamp = DateTime::parse_from_rfc3339(&lastseen).unwrap();
                                let diff = Utc::now().signed_duration_since(timestamp);
                                if diff.num_hours() < hours { break }
                            }
                            send_message(con.clone(), &client, channel, msg, &Vec::new(), None);
                            break;
                        }
                    }

                    let _: () = con.hset(format!("channel:{}:lastseen", channel), nick, Utc::now().to_rfc3339()).unwrap();
                }
            }
            _ => {}
        }
        Ok(())
    };

    reactor.register_client_with_handler(client, msg_handler);
}

fn discord_handler(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, channel: String) {
    thread::spawn(move || {
        let con = Arc::new(pool.get().unwrap());
        loop {
            let res: Result<String,_> = con.hget(format!("channel:{}:settings", channel), "discord:token");
            if let Ok(token) = res {
                let mut client = serenity::client::Client::new(&token, DiscordHandler { pool: pool.clone(), channel: channel.to_owned() }).unwrap();
                client.with_framework(StandardFramework::new());

                if let Err(e) = client.start() {
                    eprintln!("[discord_handler] {}", e);
                }
            }
            thread::sleep(time::Duration::from_secs(10));
        }
    });
}

fn update_watchtime(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, channel: String) {
    thread::spawn(move || {
        let con = Arc::new(pool.get().unwrap());
        loop {
            let live: String = con.get(format!("channel:{}:live", channel)).unwrap_or("false".to_owned());
            let enabled: String = con.hget(format!("channel:{}:settings", channel), "viewerstats:enabled").unwrap_or("false".to_owned());
            if live == "true" && enabled != "false" {
                let rsp = request_get(&format!("http://tmi.twitch.tv/group/user/{}/chatters", channel));
                match rsp {
                    Err(e) => { println!("[update_watchtime] {}", e) }
                    Ok(mut rsp) => {
                        let json: Result<TmiChatters,_> = rsp.json();
                        match json {
                            Err(e) => { println!("[update_watchtime] {}", e); }
                            Ok(json) => {
                                let mut nicks: Vec<String> = Vec::new();
                                let mut moderators = json.chatters.moderators.clone();
                                let mut viewers = json.chatters.viewers.clone();
                                let mut vips = json.chatters.vips.clone();
                                nicks.append(&mut moderators);
                                nicks.append(&mut viewers);
                                nicks.append(&mut vips);
                                for nick in nicks.iter() {
                                    let res: Result<String,_> = con.hget(format!("channel:{}:watchtimes", channel), nick);
                                    if let Ok(wt) = res {
                                        let num: i64 = wt.parse().unwrap();
                                        let _: () = con.hset(format!("channel:{}:watchtimes", channel), nick, num + 1).unwrap();
                                    } else {
                                        let _: () = con.hset(format!("channel:{}:watchtimes", channel), nick, 1).unwrap();
                                    }
                                }
                            }
                        }
                    }
                }
            }
            thread::sleep(time::Duration::from_secs(60));
        }
    });
}

fn update_live(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, channel: String) {
    thread::spawn(move || {
        let con = Arc::new(pool.get().unwrap());
        let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
        loop {
            let rsp = twitch_request_get(con.clone(), &channel, &format!("https://api.twitch.tv/kraken/streams?channel={}", id));
            match rsp {
                Err(e) => { println!("[update_live] {}", e) }
                Ok(mut rsp) => {
                    let json: Result<KrakenStreams,_> = rsp.json();
                    match json {
                        Err(e) => { println!("[update_live] {}", e); }
                        Ok(json) => {
                            let live: String = con.get(format!("channel:{}:live", channel)).unwrap();
                            if json.total == 0 {
                                if live == "true" {
                                    let _: () = con.set(format!("channel:{}:live", channel), false).unwrap();
                                }
                            } else {
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
                                    if let (Ok(token), Ok(id)) = (tres, ires) {
                                        let message: String = con.hget(format!("channel:{}:settings", channel), "discord:live-message").unwrap_or("".to_owned());
                                        let display: String = con.get(format!("channel:{}:display-name", channel)).unwrap();
                                        let body = format!("{{ \"content\": \"{}\", \"embed\": {{ \"author\": {{ \"name\": \"{}\" }}, \"title\": \"{}\", \"url\": \"http://twitch.tv/{}\", \"thumbnail\": {{ \"url\": \"{}\" }}, \"fields\": [{{ \"name\": \"Now Playing\", \"value\": \"{}\" }}] }} }}", &message, &display, &json.streams[0].channel.status, channel, &json.streams[0].channel.logo, &json.streams[0].channel.game);
                                        let _ = discord_request_post(con.clone(), &channel, &format!("https://discordapp.com/api/channels/{}/messages", id), body);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            thread::sleep(time::Duration::from_secs(60));
        }
    });
}

fn update_pubg(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, channel: String) {
    thread::spawn(move || {
        let con = Arc::new(pool.get().unwrap());
        loop {
            let reset: String = con.hget(format!("channel:{}:stats:pubg", channel), "reset").unwrap_or("false".to_owned());
            let res: Result<String,_> = con.hget(format!("channel:{}:settings", channel), "stats:reset");
            if let Ok(hour) = res {
                let num: Result<u32,_> = hour.parse();
                if let Ok(hour) = num {
                    if hour == Utc::now().time().hour() && reset == "true" {
                        let _: () = con.del(format!("channel:{}:stats:pubg", channel)).unwrap();
                    } else if hour != Utc::now().time().hour() && reset == "false" {
                        let _: () = con.hset(format!("channel:{}:stats:pubg", channel), "reset", true).unwrap();
                    }
                }
            }
            let live: String = con.get(format!("channel:{}:live", channel)).unwrap();
            if live == "true" {
                let res1: Result<String,_> = con.hget(format!("channel:{}:settings", channel), "pubg:token");
                let res2: Result<String,_> = con.hget(format!("channel:{}:settings", channel), "pubg:name");
                if let (Ok(token), Ok(name)) = (res1, res2) {
                    let region: String = con.hget(format!("channel:{}:settings", channel), "pubg:region").unwrap_or("pc-ca".to_owned());
                    let res: Result<String,_> = con.hget(format!("channel:{}:settings", channel), "pubg:id");
                    let mut id: String = "".to_owned();
                    if let Ok(v) = res {
                        id = v;
                    } else {
                        let rsp = pubg_request_get(con.clone(), &channel, &format!("https://api.pubg.com/shards/{}/players?filter%5BplayerNames%5D={}", region, name));
                        match rsp {
                            Err(e) => { println!("[update_pubg] {}", e) }
                            Ok(mut rsp) => {
                                let json: Result<PubgPlayers,_> = rsp.json();
                                match json {
                                    Err(e) => { println!("[update_pubg] {}", e); }
                                    Ok(json) => {
                                        if json.data.len() > 0 {
                                            let _: () = con.hset(format!("channel:{}:settings", channel), "pubg:id", &json.data[0].id).unwrap();
                                            id = json.data[0].id.to_owned();
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !id.is_empty() {
                        let mut cursor: String = con.hget(format!("channel:{}:stats:pubg", channel), "cursor").unwrap_or("".to_owned());
                        let rsp = pubg_request_get(con.clone(), &channel, &format!("https://api.pubg.com/shards/{}/players/{}", region, id));
                        match rsp {
                            Err(e) => { println!("[update_pubg] {}", e) }
                            Ok(mut rsp) => {
                                let json: Result<PubgPlayer,_> = rsp.json();
                                match json {
                                    Err(e) => { println!("[update_pubg] {}", e); }
                                    Ok(json) => {
                                        if cursor == "" { cursor = json.data.relationships.matches.data[0].id.to_owned() }
                                        let _: () = con.hset(format!("channel:{}:stats:pubg", channel), "cursor", &json.data.relationships.matches.data[0].id).unwrap();
                                        for match_ in json.data.relationships.matches.data.iter() {
                                            if match_.id == cursor { break }
                                            else {
                                                let rsp = pubg_request_get(con.clone(), &channel, &format!("https://api.pubg.com/shards/pc-na/matches/{}", &match_.id));
                                                match rsp {
                                                    Err(e) => { println!("[update_pubg] {}", e) }
                                                    Ok(mut rsp) => {
                                                        let json: Result<PubgMatch,_> = rsp.json();
                                                        match json {
                                                            Err(e) => { println!("[update_pubg] {}", e); }
                                                            Ok(json) => {
                                                                for p in json.included.iter().filter(|i| i.type_ == "participant") {
                                                                    if p.attributes["stats"]["playerId"] == id {
                                                                        for stat in ["winPlace", "kills", "headshotKills", "roadKills", "teamKills", "damageDealt", "vehicleDestroys"].iter() {
                                                                            if let Number(num) = &p.attributes["stats"][stat] {
                                                                                if let Some(num) = num.as_f64() {
                                                                                    let mut statname: String = (*stat).to_owned();
                                                                                    if *stat == "winPlace" { statname = "wins".to_owned() }
                                                                                    let res: Result<String,_> = con.hget(format!("channel:{}:stats:pubg", channel), &statname);
                                                                                    if let Ok(old) = res {
                                                                                        let n: u64 = old.parse().unwrap();
                                                                                        if *stat == "winPlace" {
                                                                                            if num as u64 == 1 {
                                                                                                let _: () = con.hset(format!("channel:{}:stats:pubg", channel), &statname, n + 1).unwrap();
                                                                                            }
                                                                                        } else {
                                                                                            let _: () = con.hset(format!("channel:{}:stats:pubg", channel), &statname, n + (num as u64)).unwrap();
                                                                                        }
                                                                                    } else {
                                                                                        if *stat == "winPlace" {
                                                                                            if num as u64 == 1 {
                                                                                                let _: () = con.hset(format!("channel:{}:stats:pubg", channel), &statname, 1).unwrap();
                                                                                            }
                                                                                        } else {
                                                                                            let _: () = con.hset(format!("channel:{}:stats:pubg", channel), &statname, num as u64).unwrap();
                                                                                        }
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            thread::sleep(time::Duration::from_secs(60));
        }
    });
}

fn spawn_timers(client: Arc<IrcClient>, pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, channel: String, receivers: Vec<Receiver<ThreadAction>>) {
    let notice_con = pool.get().unwrap();
    let so_con = pool.get().unwrap();
    let comm_con = pool.get().unwrap();
    let notice_client = client.clone();
    let so_client = client.clone();
    let comm_client = client.clone();
    let notice_channel = channel.clone();
    let so_channel = channel.clone();
    let comm_channel = channel.clone();
    let notice_receiver = receivers[0].clone();
    let so_receiver = receivers[1].clone();
    let comm_receiver = receivers[2].clone();

    // notices
    thread::spawn(move || {
        let con = Arc::new(notice_con);
        loop {
            let rsp = notice_receiver.recv_timeout(time::Duration::from_secs(60));
            match rsp {
                Ok(action) => {
                    match action {
                        ThreadAction::Kill => break
                    }
                }
                Err(err) => {
                    match err {
                        RecvTimeoutError::Disconnected => break,
                        RecvTimeoutError::Timeout => {}
                    }
                }
            }

            let live: String = con.get(format!("channel:{}:live", notice_channel)).unwrap();
            if live == "true" {
                let keys: Vec<String> = con.keys(format!("channel:{}:notices:*:commands", notice_channel)).unwrap();
                let ints: Vec<&str> = keys.iter().map(|str| {
                    let int: Vec<&str> = str.split(":").collect();
                    return int[3];
                }).collect();

                for int in ints.iter() {
                    let num: u16 = con.get(format!("channel:{}:notices:{}:countdown", notice_channel, int)).unwrap();
                    if num > 0 { redis::cmd("DECRBY").arg(format!("channel:{}:notices:{}:countdown", notice_channel, int)).arg(60).execute((*con).deref()) }
                };

                let int = ints.iter().filter(|int| {
                    let num: u16 = con.get(format!("channel:{}:notices:{}:countdown", notice_channel, int)).unwrap();
                    return num <= 0;
                }).fold(0, |acc, int| {
                    let int = int.parse::<u16>().unwrap();
                    if acc > int { return acc } else { return int }
                });

                if int != 0 {
                    let _: () = con.set(format!("channel:{}:notices:{}:countdown", notice_channel, int), int.clone()).unwrap();
                    let cmd: String = con.lpop(format!("channel:{}:notices:{}:commands", notice_channel, int)).unwrap();
                    let _: () = con.rpush(format!("channel:{}:notices:{}:commands", notice_channel, int), cmd.clone()).unwrap();
                    let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", notice_channel, cmd), "message");
                    if let Ok(mut message) = res {
                        let me: String = con.hget(format!("channel:{}:settings", notice_channel), "channel:me").unwrap_or("false".to_owned());
                        if me == "true" { message = format!("/me {}", message); }
                        let message = parse_message(&message, con.clone(), &notice_client, &notice_channel, None, &Vec::new());
                        send_message(con.clone(), &notice_client, &notice_channel, message, &Vec::new(), None);
                    }
                }
            }
        }
    });

    // shoutouts
    thread::spawn(move || {
        let con = Arc::new(so_con);
        loop {
            let live: String = con.get(format!("channel:{}:live", &so_channel)).unwrap_or("false".to_owned());
            let hostm: String = con.hget(format!("channel:{}:settings", &so_channel), "channel:host-message").unwrap_or("".to_owned());
            let autom: String = con.hget(format!("channel:{}:settings", &so_channel), "channel:autohost-message").unwrap_or("".to_owned());
            if live == "true" && (!hostm.is_empty() || !autom.is_empty()) {
                let id: String = con.get(format!("channel:{}:id", &so_channel)).unwrap();
                let recent: Vec<String> = con.smembers(format!("channel:{}:hosts:recent", &so_channel)).unwrap_or(Vec::new());
                let rsp = twitch_request_get(con.clone(), &so_channel, &format!("https://api.twitch.tv/kraken/channels/{}/hosts", &id));
                match rsp {
                    Err(e) => { println!("[auto_shoutouts] {}", e) }
                    Ok(mut rsp) => {
                        let json: Result<KrakenHosts,_> = rsp.json();
                        match json {
                            Err(e) => { println!("[auto_shoutouts] {}", e); }
                            Ok(json) => {
                                let list: String = con.hget(format!("channel:{}:settings", &so_channel), "autohost:blacklist").unwrap_or("".to_owned());
                                let mut blacklist: Vec<&str> = Vec::new();
                                for nick in list.split_whitespace() { blacklist.push(nick) }
                                for host in json.hosts {
                                    if !recent.contains(&host.host_id) {
                                        let _: () = con.sadd(format!("channel:{}:hosts:recent", &so_channel), &host.host_id).unwrap();
                                        let rsp = twitch_request_get(con.clone(), &so_channel, &format!("https://api.twitch.tv/kraken/streams?channel={}", &host.host_id));
                                        match rsp {
                                            Err(e) => { println!("[auto_shoutouts] {}", e) }
                                            Ok(mut rsp) => {
                                                let json: Result<KrakenStreams,_> = rsp.json();
                                                match json {
                                                    Err(e) => { println!("[auto_shoutouts] {}", e); }
                                                    Ok(json) => {
                                                        if !blacklist.contains(&host.host_id.as_ref()) {
                                                            if json.total > 0 {
                                                                if !hostm.is_empty() {
                                                                    let mut message: String = hostm.to_owned();
                                                                    message = replace_var("url", &json.streams[0].channel.url, &message);
                                                                    message = replace_var("name", &json.streams[0].channel.display_name, &message);
                                                                    message = replace_var("game", &json.streams[0].channel.game, &message);
                                                                    message = replace_var("viewers", &json.streams[0].viewers.to_string(), &message);
                                                                    send_message(con.clone(), &so_client, &so_channel, message, &Vec::new(), None);
                                                                }
                                                            } else {
                                                                if !autom.is_empty() {
                                                                    let rsp = twitch_request_get(con.clone(), &so_channel, &format!("https://api.twitch.tv/kraken/channels/{}", &host.host_id));
                                                                    match rsp {
                                                                        Err(e) => { println!("[auto_shoutouts] {}", e) }
                                                                        Ok(mut rsp) => {
                                                                            let json: Result<KrakenChannel,_> = rsp.json();
                                                                            match json {
                                                                                Err(e) => { println!("[auto_shoutouts] {}", e); }
                                                                                Ok(json) => {
                                                                                    let mut message: String = autom.to_owned();
                                                                                    message = replace_var("url", &json.url, &message);
                                                                                    message = replace_var("name", &json.display_name, &message);
                                                                                    message = replace_var("game", &json.game, &message);
                                                                                    send_message(con.clone(), &so_client, &so_channel, message, &Vec::new(), None);
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let rsp = so_receiver.recv_timeout(time::Duration::from_secs(60));
            match rsp {
                Ok(action) => {
                    match action {
                        ThreadAction::Kill => break
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

    // commercials
    thread::spawn(move || {
        let con = Arc::new(comm_con);
        loop {
            let live: String = con.get(format!("channel:{}:live", comm_channel)).unwrap();
            if live == "true" {
                let hourly: String = con.get(format!("channel:{}:commercials:hourly", comm_channel)).unwrap_or("0".to_owned());
                let hourly: u64 = hourly.parse().unwrap();
                let recents: Vec<String> = con.lrange(format!("channel:{}:commercials:recent", comm_channel), 0, -1).unwrap();
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
                    let res: Result<String,_> = con.lindex(format!("channel:{}:commercials:recent", comm_channel), 0);
                    if let Ok(lastrun) = res {
                        let lastrun: Vec<&str> = lastrun.split_whitespace().collect();
                        let timestamp = DateTime::parse_from_rfc3339(&lastrun[0]).unwrap();
                        let diff = Utc::now().signed_duration_since(timestamp);
                        if diff.num_minutes() <= 9 {
                            within8 = true;
                        }
                        if within8 {
                            thread::sleep(time::Duration::from_secs((9 - (diff.num_minutes() as u64)) * 30));
                        }
                    }
                    let id: String = con.get(format!("channel:{}:id", comm_channel)).unwrap();
                    let submode: String = con.get(format!("channel:{}:commercials:submode", comm_channel)).unwrap_or("false".to_owned());
                    let nres: Result<String,_> = con.get(format!("channel:{}:commercials:notice", comm_channel));
                    let rsp = twitch_request_post(con.clone(), &comm_channel, &format!("https://api.twitch.tv/kraken/channels/{}/commercial", id), format!("{{\"length\": {}}}", num * 30));
                    let length: u16 = con.llen(format!("channel:{}:commercials:recent", comm_channel)).unwrap();
                    let _: () = con.lpush(format!("channel:{}:commercials:recent", comm_channel), format!("{} {}", Utc::now().to_rfc3339(), num)).unwrap();
                    if length > 7 {
                        let _: () = con.rpop(format!("channel:{}:commercials:recent", comm_channel)).unwrap();
                    }
                    if submode == "true" {
                        let client_clone = comm_client.clone();
                        let channel_clone = String::from(comm_channel.clone());
                        let _ = comm_client.send_privmsg(format!("#{}", comm_channel), "/subscribers");
                        thread::spawn(move || {
                            thread::sleep(time::Duration::from_secs(num * 30));
                            client_clone.send_privmsg(format!("#{}", channel_clone), "/subscribersoff").unwrap();
                        });
                    }
                    if let Ok(notice) = nres {
                        let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", comm_channel, notice), "message");
                        if let Ok(message) = res {
                            send_message(con.clone(), &client, &comm_channel, message, &Vec::new(), None);
                        }
                    }
                    send_message(con.clone(), &client, &comm_channel, format!("{} commercials have been run", num), &Vec::new(), None);
                }

                let rsp = comm_receiver.recv_timeout(time::Duration::from_secs(3600));
                match rsp {
                    Ok(action) => {
                        match action {
                            ThreadAction::Kill => break
                        }
                    }
                    Err(err) => {
                        match err {
                            RecvTimeoutError::Disconnected => break,
                            RecvTimeoutError::Timeout => {}
                        }
                    }
                }
            } else {
                let rsp = comm_receiver.recv_timeout(time::Duration::from_secs(60));
                match rsp {
                    Ok(action) => {
                        match action {
                            ThreadAction::Kill => break
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
        }
    });
}

fn run_command(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, settings: &config::Config, matches: &ArgMatches) {
    let con = Arc::new(pool.get().unwrap());
    let channel: String = matches.value_of("channel").unwrap().to_owned();
    let cmd = matches.values_of("command").unwrap();
    let mut command: Vec<String> = Vec::new();
    for c in cmd { command.push(c.to_owned()) }

    if command.len() > 0 {
        let args = &command[1..];
        let _: () = con.publish(format!("channel:{}:signals:command", &channel), format!("{}", command.join(" "))).unwrap();
    }
}
