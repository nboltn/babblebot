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
use std::{thread,time,mem,panic};

use either::Either::{self, Left, Right};
use config;
use clap::load_yaml;
use clap::{App, ArgMatches};
use flexi_logger::{Criterion,Naming,Cleanup,Duplicate,Logger};
use crossbeam_channel::{bounded,unbounded,Sender,Receiver,TryRecvError,RecvTimeoutError};
use irc::client::prelude::*;
use url::Url;
use regex::{Regex,RegexBuilder};
use serde_json::value::Value::Number;
use chrono::{Utc, DateTime, NaiveTime, Timelike};
use http::header::{self,HeaderValue};
use futures::future::join_all;
use reqwest::Method;
use reqwest::r#async::Decoder;
use serenity;
use serenity::framework::standard::StandardFramework;
use rocket::routes;
use rocket_contrib::templates::Template;
use rocket_contrib::serve::StaticFiles;
use redis::{self,Value,from_redis_value};

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

    let (sender, receiver1) = bounded(0);
    let (sender2, receiver) = bounded(0);
    let db = (sender.clone(), receiver.clone());
    redis_listener(receiver1, sender2);

    if let Some(matches) = matches.subcommand_matches("run_command") { run_command(matches, db.clone()) }
    else {
        log_info(None, "main", "starting up", db.clone());

        start_rocket();
        new_channel_listener(db.clone());
        discord_handlers(db.clone());
        update_live(db.clone());
        update_stats(db.clone());
        update_watchtime(db.clone());
        update_patreon(db.clone());
        refresh_spotify(db.clone());
        refresh_twitch_bots(db.clone());
        refresh_twitch_channels(db.clone());

        log_info(None, "main", "connecting to irc", db.clone());
        thread::spawn(move || {
            let mut bots: HashMap<String, (HashSet<String>, Config)> = HashMap::new();
            let bs: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "bots"]).expect("smembers:bots")).unwrap();
            for bot in bs {
                let db = db.clone();
                let channelH: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", &format!("bot:{}:channels", bot)]).expect(&format!("bot:{}:channels", bot))).unwrap();
                if channelH.len() > 0 {
                    let passphrase: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("bot:{}:token", bot)]).expect(&format!("bot:{}:token", bot))).unwrap();
                    let mut channels: Vec<String> = Vec::new();
                    channels.extend(channelH.iter().cloned().map(|chan| { format!("#{}", chan) }));
                    let config = Config {
                        server: Some("irc.chat.twitch.tv".to_owned()),
                        use_ssl: Some(true),
                        nickname: Some(bot.to_owned()),
                        password: Some(format!("oauth:{}", passphrase)),
                        channels: Some(channels),
                        ping_timeout: Some(60),
                        ..Default::default()
                    };
                    bots.insert(bot.to_owned(), (channelH.clone(), config));
                }
            }
            run_reactor(bots, db.clone());
        });

        loop { thread::sleep(time::Duration::from_secs(60)) }
    }
}

fn run_reactor(bots: HashMap<String, (HashSet<String>, Config)>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    bots.iter().for_each(|(bot, channels)| {
        let db = db.clone();
        let bot = bot.clone();
        let channels = channels.clone();
        thread::spawn(move || {
            loop {
                let db = db.clone();
                let mut chans: Vec<&str> = Vec::new();
                for chan in (channels.0).iter() {
                    chans.push(chan);
                }
                log_info(Some(Right(chans.clone())), "run_reactor", "connecting to irc", db.clone());
                let (senderC, receiverC) = bounded(0);
                let (senderA, receiverA) = unbounded();
                let mut senders: HashMap<String, Vec<Sender<ThreadAction>>> = HashMap::new();
                let mut reactor = IrcReactor::new().unwrap();
                let client = Arc::new(reactor.prepare_client_and_connect(&channels.1).unwrap());
                let _ = client.identify();
                let _ = client.send("CAP REQ :twitch.tv/tags");
                let _ = client.send("CAP REQ :twitch.tv/commands");
                register_handler(bot.clone(), (*client).clone(), &mut reactor, db.clone());
                client_listener(client.clone(), db.clone(), receiverC);
                for channel in channels.0.iter() {
                    let (sender1, receiver1) = unbounded();
                    let (sender2, receiver2) = unbounded();
                    let (sender3, receiver3) = unbounded();
                    let (sender4, receiver4) = unbounded();
                    let (sender5, receiver5) = unbounded();
                    senders.insert(channel.to_owned(), [sender1.clone(), sender2.clone(), sender3.clone(), sender4.clone(), sender5.clone()].to_vec());
                    rename_channel_listener(db.clone(), channel.to_owned(), senders.clone(), senderC.clone(), senderA.clone(), receiver1);
                    command_listener(db.clone(), channel.to_owned(), senderC.clone(), receiver2);
                    run_notices(db.clone(), channel.to_owned(), senderC.clone(), receiver3);
                    run_scheduled_notices(db.clone(), channel.to_owned(), senderC.clone(), receiver4);
                    run_commercials(db.clone(), channel.to_owned(), senderC.clone(), receiver5);
                }
                channel_authcheck(db.clone(), chans.iter().map(|c| c.to_string()).collect(), senders.clone(), senderC.clone(), receiverA);
                let res = reactor.run();
                match res {
                    Err(e) => {
                        log_error(Some(Right(chans)), "run_reactor", &e.to_string(), db.clone());
                        let _ = senderA.send(ThreadAction::Kill);
                        for channel in channels.0.iter() {
                            if let Some(senders) = senders.get(channel) {
                                for sender in senders {
                                    let _ = sender.send(ThreadAction::Kill);
                                }
                            }
                        }

                        thread::sleep(time::Duration::from_secs(60));
                    }
                    Ok(_) => { break }
                }
            }
        });
    });
}

fn register_handler(bot: String, client: IrcClient, reactor: &mut IrcReactor, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    let clientC = Arc::new(client.clone());
    let msg_handler = move |client: &IrcClient, irc_message: Message| -> irc::error::Result<()> {
        match &irc_message.command {
            Command::PING(_,_) => { let _ = client.send_pong(":tmi.twitch.tv"); }
            Command::Raw(cmd, chans, _) => {
                if cmd == "USERSTATE" {
                    let badges = get_badges(&irc_message);
                    match badges.get("moderator") {
                        Some(_) => {
                            for chan in chans {
                                let channel = &chan[1..];
                                redis_call(db.clone(), vec!["set", &format!("channel:{}:auth", channel), "true"]);
                                redis_call(db.clone(), vec!["set", &format!("channel:{}:auth:last", channel), &format!("{}", Utc::now().to_rfc3339())]);
                            }
                        }
                        None => {
                            for chan in chans {
                                let channel = &chan[1..];
                                redis_call(db.clone(), vec!["set", &format!("channel:{}:auth", channel), "false"]);
                            }
                        }
                    }
                }
            }
            Command::PRIVMSG(chan, msg) => {
                let nick = get_nick(&irc_message);
                if nick != bot {
                    let client = clientC.clone();
                    let channel = &chan[1..];
                    let prefix: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", channel), "command:prefix"]).unwrap_or(Value::Data("!".as_bytes().to_owned()))).unwrap();
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

                        if let Some(donated) = get_bits(&irc_message) {
                            // TODO: queue agent actions
                            let mut bits: u16 = 0;
                            let keys: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["keys", &format!("channel:{}:events:bits:*", channel)]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                            for key in keys {
                                let key: Vec<&str> = key.split(":").collect();
                                let r1: Result<u16,_> = donated.parse();
                                let r2: Result<u16,_> = key[4].parse();
                                if let (Ok(donated), Ok(num)) = (r1, r2) {
                                    if donated >= num && num > bits {
                                        bits = num;
                                    }
                                }
                            }
                            if bits > 0 {
                                let etype: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:events:bits:{}", channel, bits), "type"]).expect(&format!("channel:{}:events:bits:{}", channel, bits))).unwrap();
                                match etype.as_ref() {
                                    "local" => {
                                        let ids: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:events:bits:{}", channel, bits), "actions"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                                        redis_call(db.clone(), vec!["lpush", &format!("channel:{}:local:actions", channel), &ids.join(" ")]);
                                    }
                                    _ => {}
                                }
                            }
                        }

                        // moderate incoming messages
                        // TODO: symbols, length
                        if !auth {
                            let display: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:moderation:display", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                            let caps: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:moderation:caps", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                            let colors: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:moderation:colors", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                            let links: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", &format!("channel:{}:moderation:links", channel)]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                            let bkeys: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["keys", &format!("channel:{}:moderation:blacklist:*", channel)]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                            let age: Result<Value,_> = redis_call(db.clone(), vec!["get", &format!("channel:{}:moderation:age", channel)]);
                            if colors == "true" && msg.len() > 6 && msg.as_bytes()[0] == 1 && &msg[1..7] == "ACTION" {
                                let _ = client.send_privmsg(chan, format!("/timeout {} 1", nick));
                                if display == "true" { send_message(client.clone(), channel.to_owned(), format!("@{} you've been timed out for posting colors", nick), db.clone()); }
                            }
                            if let Ok(value) = age {
                                let age: String = from_redis_value(&value).unwrap();
                                let res: Result<i64,_> = age.parse();
                                if let Ok(age) = res { spawn_age_check(client.clone(), db.clone(), channel.to_string(), nick.clone(), age, display.to_string()); }
                            }
                            if caps == "true" {
                                let limit: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:moderation:caps:limit", channel)]).expect(&format!("channel:{}:moderation:caps:limit", channel))).unwrap();
                                let trigger: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:moderation:caps:trigger", channel)]).expect(&format!("channel:{}:moderation:caps:trigger", channel))).unwrap();
                                let subs: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:moderation:caps:subs", channel)]).expect(&format!("channel:{}:moderation:caps:subs", channel))).unwrap();
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
                                                if display == "true" { send_message(client.clone(), channel.to_owned(), format!("@{} you've been timed out for posting too many caps", nick), db.clone()); }
                                            }
                                        }
                                    }
                                }
                            }
                            if links.len() > 0 && url_regex().is_match(&msg) {
                                let sublinks: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:moderation:links:subs", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                                let permitted: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["keys", &format!("channel:{}:moderation:permitted:*", channel)]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
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
                                                        if display == "true" { send_message(client.clone(), channel.to_owned(), format!("@{} you've been timed out for posting links", nick), db.clone()); }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            for key in bkeys {
                                let key: Vec<&str> = key.split(":").collect();
                                let rgx: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:moderation:blacklist:{}", channel, key[4]), "regex"]).expect(&format!("channel:{}:moderation:blacklist:{}", channel, key[4]))).unwrap();
                                let length: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:moderation:blacklist:{}", channel, key[4]), "length"]).expect(&format!("channel:{}:moderation:blacklist:{}", channel, key[4]))).unwrap();
                                match RegexBuilder::new(&rgx).case_insensitive(true).build() {
                                    Err(e) => { log_error(Some(Right(vec![&channel])), "regex_error", &e.to_string(), db.clone()) }
                                    Ok(rgx) => {
                                        if rgx.is_match(&msg) {
                                            let _ = client.send_privmsg(chan, format!("/timeout {} {}", nick, length));
                                            if display == "true" { send_message(client.clone(), channel.to_owned(), format!("@{} you've been timed out for posting a blacklisted phrase", nick), db.clone()); }
                                            break;
                                        }
                                    }
                                }
                            }
                        }

                        // expand aliases
                        let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:aliases", channel), &word]);
                        if let Ok(value) = res {
                            let alias: String = from_redis_value(&value).unwrap();
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
                                    if !cmd.2 || auth { (cmd.1)(client.clone(), channel.to_owned(), args.clone(), Some(irc_message.clone()), db.clone()) }
                                } else {
                                    if !cmd.3 || auth { (cmd.1)(client.clone(), channel.to_owned(), args.clone(), Some(irc_message.clone()), db.clone()) }
                                }
                                break;
                            }
                        }

                        // parse custom commands
                        let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:commands:{}", channel.to_owned(), word), "message"]);
                        if let Ok(value) = res {
                            let message: String = from_redis_value(&value).unwrap();
                            let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:commands:{}", channel.to_owned(), word), "lastrun"]);
                            let mut within5 = false;
                            if let Ok(value) = res {
                                let lastrun: String = from_redis_value(&value).unwrap();
                                let timestamp = DateTime::parse_from_rfc3339(&lastrun).unwrap();
                                let diff = Utc::now().signed_duration_since(timestamp);
                                if diff.num_seconds() < 5 { within5 = true }
                            }
                            if !within5 {
                                let mut protected: &str = "cmd";
                                if args.len() > 0 { protected = "arg" }
                                let protected: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:commands:{}", channel, word), &format!("{}_protected", protected)]).expect(&format!("{}:{}", &format!("channel:{}:commands:{}", channel, word), &format!("{}_protected", protected)))).unwrap();
                                if protected == "false" || auth {
                                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:commands:{}", channel, word), "lastrun", &Utc::now().to_rfc3339()]);
                                    send_parsed_message(client.clone(), channel.to_owned(), message.to_owned(), args.clone(), Some(irc_message.clone()), db.clone(), None);
                                }
                            }
                        }

                        // parse greetings
                        let keys: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["keys", &format!("channel:{}:greetings:*", channel)]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                        for key in keys.iter() {
                            let key: Vec<&str> = key.split(":").collect();
                            if key[3] == nick {
                                let msg: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:greetings:{}", channel, key[3]), "message"]).expect(&format!("channel:{}:greetings:{}", channel, key[3]))).unwrap();
                                let hours: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:greetings:{}", channel, key[3]), "hours"]).expect(&format!("channel:{}:greetings:{}", channel, key[3]))).unwrap();
                                let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:lastseen", channel), key[3]]);
                                if let Ok(value) = res {
                                    let lastseen: String = from_redis_value(&value).unwrap();
                                    let hours: i64 = hours.parse().expect("hours:parse");
                                    let timestamp = DateTime::parse_from_rfc3339(&lastseen).unwrap();
                                    let diff = Utc::now().signed_duration_since(timestamp);
                                    if diff.num_hours() < hours { break }
                                }
                                send_parsed_message(client.clone(), channel.to_owned(), msg, Vec::new(), None, db.clone(), None);
                                break;
                            }
                        }

                        // parse keywords
                        let keys: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["keys", &format!("channel:{}:keywords:*", channel)]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                        for key in keys.iter() {
                            let key: Vec<&str> = key.split(":").collect();
                            let regex: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:keywords:{}", channel, key[3]), "regex"]).expect(&format!("channel:{}:keywords:{}", channel, key[3]))).unwrap();
                            let cmd: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:keywords:{}", channel, key[3]), "cmd"]).expect(&format!("channel:{}:keywords:{}", channel, key[3]))).unwrap();
                            let res = Regex::new(&regex);
                            if let Ok(rgx) = res {
                                if rgx.is_match(&msg) {
                                    let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:commands:{}", channel, cmd), "message"]);
                                    if let Ok(value) = res {
                                        let message: String = from_redis_value(&value).unwrap();
                                        send_parsed_message(client.clone(), channel.to_owned(), message, Vec::new(), None, db.clone(), None);
                                    }
                                    break;
                                }
                            }
                        }

                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:lastseen", channel), &nick, &Utc::now().to_rfc3339()]);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    };

    reactor.register_client_with_handler(client, msg_handler);
}

fn start_rocket() {
    thread::spawn(move || {
        rocket::ignite()
          .mount("/assets", StaticFiles::from("assets"))
          .mount("/", routes![web::index, web::dashboard, web::commands, web::patreon_cb, web::patreon_refresh, web::spotify_cb, web::twitch_cb, web::public_data, web::data, web::logs, web::local, web::login, web::logout, web::signup, web::password, web::title, web::game, web::new_command, web::save_command, web::trash_command, web::new_notice, web::trash_notice, web::save_setting, web::trash_setting, web::new_blacklist, web::save_blacklist, web::trash_blacklist, web::new_keyword, web::save_keyword, web::trash_keyword, web::trash_song])
          .register(catchers![web::internal_error, web::not_found])
          .attach(Template::fairing())
          .attach(RedisConnection::fairing())
          .launch()
    });
}

fn redis_listener(receiver: Receiver<Vec<String>>, sender: Sender<Result<Value, String>>) {
    thread::spawn(move || {
        let con = acquire_con();
        loop {
            let rsp = receiver.recv();
            match rsp {
                Err(err) => { break }
                Ok(args) => {
                    if args.len() > 1 {
                        let mut cmd = &mut redis::cmd(&args[0]);
                        for arg in args[1..].iter() {
                            cmd = cmd.arg(arg.clone());
                        }
                        let res = cmd.query(&con);
                        match res {
                            Err(e) => {
                                println!("[redis_listener] {}", &e.to_string());
                                sender.send(Err(e.to_string()));
                            }
                            Ok(res) => {
                                sender.send(Ok(res));
                            }
                        }
                    } else {
                        println!("[redis_listener] not enough args");
                        sender.send(Err("not enough args".to_string()));
                    }
                }
            }
        }
    });
}

fn client_listener(client: Arc<IrcClient>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>), receiver: Receiver<ClientAction>) {
    thread::spawn(move || {
        let con = acquire_con();
        loop {
            let rsp = receiver.recv();
            match rsp {
                Err(err) => { break }
                Ok(action) => {
                    match action {
                        ClientAction::Privmsg(channel, message) => {
                            let _ = client.send_privmsg(format!("#{}", &channel), &message);
                        }
                        ClientAction::Part(channel) => {
                            let _ = client.send_part(format!("#{}", &channel));
                        }
                        ClientAction::Command(channel, mut words) => {
                            let prefix: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", channel), "command:prefix"]).unwrap_or(Value::Data("!".as_bytes().to_owned()))).unwrap();
                            if words.len() > 0 {
                                let mut word = words[0].to_lowercase();
                                let mut args: Vec<String> = words[1..].to_vec();

                                // expand aliases
                                let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:aliases", channel), &word]);
                                if let Ok(value) = res {
                                    let alias: String = from_redis_value(&value).unwrap();
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
                                        (cmd.1)(client.clone(), channel.clone(), args.clone(), None, db.clone());
                                    }
                                }

                                // parse custom commands
                                let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:commands:{}", channel, word), "message"]);
                                if let Ok(value) = res {
                                    let message: String = from_redis_value(&value).unwrap();
                                    let _ = client.send_privmsg(format!("#{}", channel), message);
                                }
                            }
                        }
                    }
                }
            }
        }
    });
}


fn new_channel_listener(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    thread::spawn(move || {
        let mut con = acquire_con();
        let mut ps = con.as_pubsub();
        ps.subscribe("new_channels").unwrap();

        loop {
            let db = db.clone();
            let res = ps.get_message();
            if let Ok(msg) = res {
                let channel: String = msg.get_payload().expect("redis:get_payload");
                let mut bots: HashMap<String, (HashSet<String>, Config)> = HashMap::new();
                let bot: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:bot", channel)]).expect(&format!("channel:{}:bot", channel))).unwrap();
                let passphrase: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("bot:{}:token", bot)]).expect(&format!("bot:{}:token", bot))).unwrap();
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
                thread::spawn(move || { run_reactor(bots, db); });
            }
        }
    });
}

fn channel_authcheck(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>), channels: Vec<String>, senders: HashMap<String, Vec<Sender<ThreadAction>>>, client: Sender<ClientAction>, receiver: Receiver<ThreadAction>) {
    thread::spawn(move || {
        let mut restart = false;
        let mut channels = channels.clone();
        loop {
            for channel in channels.clone() {
                let auth: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:auth", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                match auth.as_ref() {
                    "true" => { redis_call(db.clone(), vec!["set", &format!("channel:{}:auth:last", channel), &format!("{}", Utc::now().to_rfc3339())]); }
                    "false" => { client.send(ClientAction::Privmsg(channel, "/me has entered".to_owned())); }
                    _ => {}
                }
            }

            let rsp = receiver.recv_timeout(time::Duration::from_secs(60));
            match rsp {
                Ok(action) => {
                    match action {
                        ThreadAction::Kill => break,
                        ThreadAction::Part(channel) => {
                            channels.iter().position(|c| c == &channel).map(|i| channels.remove(i));
                            restart = true;
                            break;
                        }
                    }
                }
                Err(err) => {
                    match err {
                        RecvTimeoutError::Disconnected => break,
                        RecvTimeoutError::Timeout => {}
                    }
                }
            }

            for channel in channels.clone() {
                let lastauth: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:auth:last", channel)]).unwrap_or(Value::Data("".as_bytes().to_owned()))).unwrap();
                let res = DateTime::parse_from_rfc3339(&lastauth);
                if let Ok(timestamp) = res {
                    let diff = Utc::now().signed_duration_since(timestamp);
                    if diff.num_hours() > 24 {
                        client.send(ClientAction::Part(channel.clone()));
                        if let Some(senders) = senders.get(&channel) {
                            for sender in senders {
                                let _ = sender.send(ThreadAction::Kill);
                            }
                        }

                        let bot: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:bot", &channel)]).expect(&format!("channel:{}:bot", &channel))).unwrap();
                        redis_call(db.clone(), vec!["srem", &format!("bot:{}:channels", &bot), &channel]);
                        redis_call(db.clone(), vec!["srem", "channels", &channel]);

                        channels.iter().position(|c| c == &channel).map(|i| channels.remove(i));
                        restart = true;
                    }
                } else {
                    redis_call(db.clone(), vec!["set", &format!("channel:{}:auth:last", channel), &format!("{}", Utc::now().to_rfc3339())]);
                }
            }

            if restart { break }
        }

        if restart { channel_authcheck(db.clone(), channels, senders, client, receiver); }
    });
}

fn rename_channel_listener(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>), channel: String, senders: HashMap<String, Vec<Sender<ThreadAction>>>, client: Sender<ClientAction>, sender: Sender<ThreadAction>, receiver: Receiver<ThreadAction>) {
    thread::spawn(move || {
        let db = db.clone();
        let mut con = acquire_con();
        let mut ps = con.as_pubsub();
        ps.subscribe(format!("channel:{}:signals:rename", channel)).unwrap();
        ps.set_read_timeout(Some(time::Duration::from_secs(60)));
        loop {
            let rsp = receiver.try_recv();
            match rsp {
                Ok(action) => {
                    match action {
                        ThreadAction::Kill => break,
                        ThreadAction::Part(_) => {}
                    }
                }
                Err(err) => {
                    match err {
                        TryRecvError::Disconnected => break,
                        TryRecvError::Empty => {}
                    }
                }
            }

            let res = ps.get_message();
            match res {
                Err(e) => {
                    if !e.is_timeout() {
                        log_error(Some(Right(vec![&channel])), "rename_channel_listener", &e.to_string(), db.clone());
                    }
                }
                Ok(msg) => {
                    let payload: String = msg.get_payload().expect("redis:get_payload");
                    let tokens: Vec<&str> = payload.split_whitespace().collect();
                    let token: String = tokens[0].to_string();
                    let refresh: String = tokens[1].to_string();

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
                        Err(e) => { log_error(Some(Right(vec![&channel])), "rename_channel_listener", &e.to_string(), db.clone()) }
                        Ok(mut rsp) => {
                            let body = rsp.text().unwrap();
                            let json: Result<KrakenUser,_> = serde_json::from_str(&body);
                            match json {
                                Err(e) => {
                                    log_error(Some(Right(vec![&channel])), "rename_channel_listener", &e.to_string(), db.clone());
                                    log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                                }
                                Ok(json) => {
                                    if json.name != channel {
                                        client.send_timeout(ClientAction::Part(channel.clone()), time::Duration::from_secs(10));
                                        sender.send_timeout(ThreadAction::Part(channel.clone()), time::Duration::from_secs(10));
                                        if let Some(senders) = senders.get(&channel) {
                                            for sender in senders {
                                                let _ = sender.send(ThreadAction::Kill);
                                            }
                                        }

                                        let bot: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:bot", &channel)]).expect(&format!("channel:{}:bot", &channel))).unwrap();
                                        redis_call(db.clone(), vec!["srem", &format!("bot:{}:channels", &bot), &channel]);
                                        redis_call(db.clone(), vec!["sadd", "bots", &json.name]);
                                        redis_call(db.clone(), vec!["sadd", &format!("bot:{}:channels", &json.name), &channel]);
                                        redis_call(db.clone(), vec!["set", &format!("bot:{}:token", &json.name), &token]);
                                        redis_call(db.clone(), vec!["set", &format!("bot:{}:refresh", &json.name), &refresh]);
                                        redis_call(db.clone(), vec!["set", &format!("channel:{}:bot", &channel), &json.name]);

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
                                        thread::spawn(move || { run_reactor(bots, db); });
                                        break;
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

fn command_listener(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>), channel: String, client: Sender<ClientAction>, receiver: Receiver<ThreadAction>) {
    thread::spawn(move || {
        let db = db.clone();
        let mut con = acquire_con();
        let mut ps = con.as_pubsub();
        ps.subscribe(format!("channel:{}:signals:command", channel)).unwrap();
        ps.set_read_timeout(Some(time::Duration::from_secs(60)));
        loop {
            let rsp = receiver.try_recv();
            match rsp {
                Ok(action) => {
                    match action {
                        ThreadAction::Kill => break,
                        ThreadAction::Part(_) => {}
                    }
                }
                Err(err) => {
                    match err {
                        TryRecvError::Disconnected => break,
                        TryRecvError::Empty => {}
                    }
                }
            }

            let res = ps.get_message();
            match res {
                Err(e) => {
                    if !e.is_timeout() { log_error(Some(Right(vec![&channel])), "command_listener", &e.to_string(), db.clone()) }
                }
                Ok(msg) => {
                    let payload: String = msg.get_payload().expect("redis:get_payload");
                    let words: Vec<String> = payload.split_whitespace().map(|w| w.to_string()).collect();
                    client.send_timeout(ClientAction::Command(channel.clone(), words), time::Duration::from_secs(10));
                }
            }
        }
    });
}

fn discord_handlers(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    let channels: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
    for channel in channels {
        let db = db.clone();
        thread::spawn(move || {
            loop {
                let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", channel), "discord:token"]);
                if let Ok(value) = res {
                    let token: String = from_redis_value(&value).unwrap();
                    let mut client = serenity::client::Client::new(&token, DiscordHandler { channel: channel.to_owned(), db: db.clone() }).unwrap();
                    client.with_framework(StandardFramework::new());
                    log_info(Some(Right(vec![&channel])), "discord_handler", "connecting to discord", db.clone());
                    if let Err(e) = client.start() {
                        log_error(Some(Right(vec![&channel])), "discord_handler", &e.to_string(), db.clone())
                    }
                }
                thread::sleep(time::Duration::from_secs(60));
            }
        });
    }
}

fn run_notices(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>), channel: String, client: Sender<ClientAction>, receiver: Receiver<ThreadAction>) {
    thread::spawn(move || {
        let db = db.clone();
        loop {
            let rsp = receiver.recv_timeout(time::Duration::from_secs(60));
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

            let live: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:live", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
            if live == "true" {
                let keys: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["keys", &format!("channel:{}:notices:*:commands", channel.clone())]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                let ints: Vec<&str> = keys.iter().map(|str| {
                    let int: Vec<&str> = str.split(":").collect();
                    return int[3];
                }).collect();

                for int in ints.iter() {
                    let num: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:notices:{}:countdown", channel, int)]).expect(&format!("channel:{}:notices:{}:countdown", channel, int))).unwrap();
                    let num: u16 = num.parse().unwrap();
                    if num > 0 { redis_call(db.clone(), vec!["decrby", &format!("channel:{}:notices:{}:countdown", channel, int), "60"]); }
                };

                let int = ints.iter().filter(|int| {
                    let num: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:notices:{}:countdown", channel, int)]).expect(&format!("channel:{}:notices:{}:countdown", channel, int))).unwrap();
                    let num: u16 = num.parse().unwrap();
                    return num <= 0;
                }).fold(0, |acc, int| {
                    let int = int.parse::<u16>().unwrap();
                    if acc > int { return acc } else { return int }
                });

                if int != 0 {
                    redis_call(db.clone(), vec!["set", &format!("channel:{}:notices:{}:countdown", channel, int), int.to_string().as_ref()]);
                    let cmd: String = from_redis_value(&redis_call(db.clone(), vec!["lpop", &format!("channel:{}:notices:{}:commands", channel, int)]).expect(&format!("channel:{}:notices:{}:commands", channel, int))).unwrap();
                    redis_call(db.clone(), vec!["rpush", &format!("channel:{}:notices:{}:commands", channel, int), &cmd]);
                    let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:commands:{}", channel, cmd), "message"]);
                    if let Ok(value) = res {
                        let message: String = from_redis_value(&value).unwrap();
                        client.send(ClientAction::Privmsg(channel.clone(), message));
                    }
                }
            }
        }
    });
}

fn run_scheduled_notices(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>), channel: String, client: Sender<ClientAction>, receiver: Receiver<ThreadAction>) {
    thread::spawn(move || {
        let db = db.clone();
        loop {
            let rsp = receiver.recv_timeout(time::Duration::from_secs(60));
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

            let live: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:live", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
            if live == "true" {
                let keys: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["keys", &format!("channel:{}:snotices:*", channel)]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                keys.iter().for_each(|key| {
                    let time: String = from_redis_value(&redis_call(db.clone(), vec!["hget", key, "time"]).expect(&format!("{}:{}", key, "time"))).unwrap();
                    let cmd: String = from_redis_value(&redis_call(db.clone(), vec!["hget", key, "cmd"]).expect(&format!("{}:{}", key, "cmd"))).unwrap();
                    let res = NaiveTime::parse_from_str(&time, "%H:%M%z");
                    if let Ok(time) = res {
                        let hour = time.hour();
                        let min = time.minute();
                        if hour == Utc::now().time().hour() && min == Utc::now().time().minute() {
                            let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:commands:{}", channel, cmd), "message"]);
                            if let Ok(value) = res {
                                let message: String = from_redis_value(&value).unwrap();
                                client.send(ClientAction::Privmsg(channel.clone(), message));
                            }
                        }
                    }
                });
            }
        }
    });
}

// TODO: reimplement shoutouts
/*
fn run_shoutouts(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    thread::spawn(move || {
        loop {
            let channels: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
            for channel in channels {
                let channelC = channel.clone();
                let live: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:live", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                let hostm: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:live", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                let autom: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:live", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                let hostm: String = con.hget(format!("channel:{}:settings", &so_channel), "channel:host-message").unwrap_or("".to_owned());
                let autom: String = con.hget(format!("channel:{}:settings", &so_channel), "channel:autohost-message").unwrap_or("".to_owned());
                if live == "true" && (!hostm.is_empty() || !autom.is_empty()) {
                    let id: String = con.get(format!("channel:{}:id", &so_channel)).unwrap();
                    let recent: Vec<String> = con.smembers(format!("channel:{}:hosts:recent", &so_channel)).unwrap_or(Vec::new());
                    let token: String = redis_string_async(vec!["get", &format!("channel:{}:token", &so_channel)]).wait().unwrap().unwrap();
                    let future = twitch_kraken_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}/hosts", &id)).send()
                        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                        .map_err(|e| println!("request error: {}", e))
                        .map(move |body| {
                            let con = Arc::new(acquire_con());
                            let body = std::str::from_utf8(&body).unwrap();
                            let json: Result<KrakenHosts,_> = serde_json::from_str(&body);
                            match json {
                                Err(e) => {
                                    log_error(Some(Right(vec![&channelC])), "auto_shoutouts", &e.to_string(), db.clone());
                                    log_error(Some(Right(vec![&channelC])), "request_body", &body, db.clone());
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
                                            let token: String = redis_string_async(vec!["get", &format!("channel:{}:token", &channel)]).wait().unwrap().unwrap();
                                            let future = twitch_kraken_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/streams?channel={}", &host.host_id)).send()
                                                .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                                .map_err(|e| println!("request error: {}", e))
                                                .map(move |body| {
                                                    let con = Arc::new(acquire_con());
                                                    let body = std::str::from_utf8(&body).unwrap();
                                                    let json: Result<KrakenStreams,_> = serde_json::from_str(&body);
                                                    match json {
                                                        Err(e) => {
                                                            log_error(Some(Right(vec![&channel])), "auto_shoutouts", &e.to_string(), db.clone());
                                                            log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
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
                                                                        send_message(client.clone(), channel.clone(), message, db.clone());
                                                                    }
                                                                } else {
                                                                    if !autom.is_empty() {
                                                                        let token: String = redis_string_async(vec!["get", &format!("channel:{}:token", &channel)]).wait().unwrap().unwrap();
                                                                        let future = twitch_kraken_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}", &host.host_id)).send()
                                                                            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                                                            .map_err(|e| println!("request error: {}", e))
                                                                            .map(move |body| {
                                                                                let con = Arc::new(acquire_con());
                                                                                let body = std::str::from_utf8(&body).unwrap();
                                                                                let json: Result<KrakenChannel,_> = serde_json::from_str(&body);
                                                                                match json {
                                                                                    Err(e) => {
                                                                                        log_error(Some(Right(vec![&channel])), "auto_shoutouts", &e.to_string(), db.clone());
                                                                                        log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                                                                                    }
                                                                                    Ok(json) => {
                                                                                        let mut message: String = autom.to_owned();
                                                                                        message = replace_var("url", &json.url, &message);
                                                                                        message = replace_var("name", &json.display_name, &message);
                                                                                        message = replace_var("game", &json.game, &message);
                                                                                        send_message(client.clone(), channel.clone(), message, db.clone());
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
        }
    });
}*/

fn run_commercials(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>), channel: String, client: Sender<ClientAction>, receiver: Receiver<ThreadAction>) {
    thread::spawn(move || {
        let db = db.clone();
        loop {
            let rsp = receiver.recv_timeout(time::Duration::from_secs(600));
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

            let live: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:live", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
            if live == "true" {
                let hourly: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:commercials:hourly", channel)]).unwrap_or(Value::Data("0".as_bytes().to_owned()))).unwrap();
                let hourly: u64 = hourly.parse().unwrap();
                let recents: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["lrange", &format!("channel:{}:commercials:recent", channel), "0", "-1"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
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
                    let live: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:live", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                    let mut within8 = false;
                    let res: Result<Value,_> = redis_call(db.clone(), vec!["lindex", &format!("channel:{}:commercials:recent", channel), "0"]);
                    if let Ok(value) = res {
                        let lastrun: String = from_redis_value(&value).unwrap();
                        let lastrun: Vec<&str> = lastrun.split_whitespace().collect();
                        let timestamp = DateTime::parse_from_rfc3339(&lastrun[0]).unwrap();
                        let diff = Utc::now().signed_duration_since(timestamp);
                        if diff.num_minutes() <= 9 {
                            within8 = true;
                        }
                    }
                    if !within8 {
                        let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
                        let submode: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:commercials:submode", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                        let nres: Result<Value,_> = redis_call(db.clone(), vec!["get", &format!("channel:{}:commercials:notice", channel)]);
                        let length: u16 = from_redis_value(&redis_call(db.clone(), vec!["llen", &format!("channel:{}:commercials:recent", channel)]).expect(&format!("channel:{}:commercials:recent", channel))).unwrap();
                        redis_call(db.clone(), vec!["lpush", &format!("channel:{}:commercials:recent", channel), &format!("{} {}", Utc::now().to_rfc3339(), num)]);
                        if length > 7 {
                            redis_call(db.clone(), vec!["rpop", &format!("channel:{}:commercials:recent", channel)]);
                        }
                        if submode == "true" {
                            let dbC = db.clone();
                            let channelC = String::from(channel.clone());
                            let clientC = client.clone();
                            client.send(ClientAction::Privmsg(channel.clone(), "/subscribers".to_owned()));
                            thread::spawn(move || {
                                thread::sleep(time::Duration::from_secs(num * 30));
                                clientC.send(ClientAction::Privmsg(channelC, "/subscribersoff".to_owned()));
                            });
                        }
                        if let Ok(value) = nres {
                            let notice: String = from_redis_value(&value).unwrap();
                            let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:commands:{}", channel, notice), "message"]);
                            if let Ok(value) = res {
                                let message: String = from_redis_value(&value).unwrap();
                                client.send(ClientAction::Privmsg(channel.clone(), message));
                            }
                        }
                        log_info(Some(Right(vec![&channel])), "run_commercials", &format!("{} commercials have been run", num), db.clone());
                        client.send(ClientAction::Privmsg(channel.clone(), format!("{} commercials have been run", num)));
                        let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
                        let future = twitch_kraken_request(token, Some("application/json"), Some(format!("{{\"length\": {}}}", num * 30).as_bytes().to_owned()), Method::POST, &format!("https://api.twitch.tv/kraken/channels/{}/commercial", &id)).send().and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() }).map_err(|e| println!("request error: {}", e)).map(move |_body| {});
                        thread::spawn(move || { tokio::run(future) });
                    }
                }
            }
        }
    });
}

fn update_patreon(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    thread::spawn(move || {
        loop {
            let channels: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
            for channel in channels {
                let db = db.clone();
                let dbC = db.clone();
                let res: Result<Value,_> = redis_call(db.clone(), vec!["get", &format!("channel:{}:patreon:token", &channel)]);
                if let Ok(_value) = res {
                    let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:patreon:token", &channel)]).expect(&format!("channel:{}:patreon:token", &channel))).unwrap();
                    let future = patreon_request(token, Method::GET, "https://www.patreon.com/api/oauth2/v2/identity?include=memberships.campaign").send()
                        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                        .map_err(|e| println!("request error: {}", e))
                        .map(move |body| {
                            let body = std::str::from_utf8(&body).unwrap();
                            let json: Result<PatreonIdentity,_> = serde_json::from_str(&body);
                            match json {
                                Err(e) => {
                                    let res: Result<Value,_> = redis_call(db.clone(), vec!["get", &format!("channel:{}:patreon:refresh", &channel)]);
                                    if let Ok(value) = res {
                                        let token: String = from_redis_value(&value).unwrap();
                                        let channelC = channel.clone();
                                        let future = patreon_refresh(Method::POST, "https://www.patreon.com/api/oauth2/token", Some(format!("grant_type=refresh_token&refresh_token={}", token).as_bytes().to_owned())).send()
                                            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                            .map_err(move |e| log_error(Some(Right(vec![&channelC])), "update_patreon", &e.to_string(), db.clone()))
                                            .map(move |body| {
                                                let db = dbC.clone();
                                                let body = std::str::from_utf8(&body).unwrap();
                                                let json: Result<PatreonRsp,_> = serde_json::from_str(&body);
                                                match json {
                                                    Err(e) => {
                                                        log_error(Some(Right(vec![&channel])), "update_patreon", &e.to_string(), db.clone());
                                                        log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                                                    }
                                                    Ok(json) => {
                                                        redis_call(db.clone(), vec!["set", &format!("channel:{}:patreon:token", &channel), &json.access_token]);
                                                        redis_call(db.clone(), vec!["set", &format!("channel:{}:patreon:refresh", &channel), &json.refresh_token]);

                                                        let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:patreon:token", &channel)]).expect(&format!("channel:{}:patreon:token", &channel))).unwrap();
                                                        let future = patreon_request(token, Method::GET, "https://www.patreon.com/api/oauth2/v2/identity?include=memberships.campaign").send()
                                                            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                                            .map_err(|e| println!("request error: {}", e))
                                                            .map(move |body| {
                                                                let body = std::str::from_utf8(&body).unwrap();
                                                                let json: Result<PatreonIdentity,_> = serde_json::from_str(&body);
                                                                match json {
                                                                    Err(e) => {
                                                                        log_error(Some(Right(vec![&channel])), "update_patreon", &e.to_string(), db.clone());
                                                                        log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                                                                    }
                                                                    Ok(json) => {
                                                                        let mut settings = config::Config::default();
                                                                        settings.merge(config::File::with_name("Settings")).unwrap();
                                                                        settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
                                                                        let patreon_id = settings.get_str("patreon_id").unwrap_or("".to_owned());
                                                                        let patreon_sub: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:patreon:subscribed", &channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();

                                                                        let mut subscribed = false;
                                                                        let mut memberships = Vec::new();

                                                                        for membership in &json.data.relationships.memberships.data {
                                                                            memberships.push(membership.id.to_string());
                                                                        }

                                                                        for id in memberships {
                                                                            for inc in &json.included {
                                                                                if let Some(relationships) = &inc.relationships {
                                                                                    if id == inc.id && patreon_id == relationships.campaign.data.id {
                                                                                        subscribed = true;
                                                                                    }
                                                                                }
                                                                            }
                                                                        }

                                                                        if subscribed {
                                                                            redis_call(db.clone(), vec!["set", &format!("channel:{}:patreon:subscribed", &channel), "true"]);
                                                                        } else {
                                                                            redis_call(db.clone(), vec!["set", &format!("channel:{}:patreon:subscribed", &channel), "false"]);
                                                                            let token = settings.get_str("bot_token").unwrap();
                                                                            if patreon_sub == "true" {
                                                                                redis_call(db.clone(), vec!["publish", &format!("channel:{}:signals:rename", &channel), &token]);
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            });
                                                        thread::spawn(move || { tokio::run(future) });
                                                    }
                                                }
                                            });
                                        thread::spawn(move || { tokio::run(future) });
                                    }
                                }
                                Ok(json) => {
                                    let mut settings = config::Config::default();
                                    settings.merge(config::File::with_name("Settings")).unwrap();
                                    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
                                    let patreon_id = settings.get_str("patreon_id").unwrap_or("".to_owned());
                                    let patreon_sub: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:patreon:subscribed", &channel)]).expect(&format!("channel:{}:patreon:subscribed", &channel))).unwrap();

                                    let mut subscribed = false;
                                    let mut memberships = Vec::new();

                                    for membership in &json.data.relationships.memberships.data {
                                        memberships.push(membership.id.to_string());
                                    }

                                    for id in memberships {
                                        for inc in &json.included {
                                            if let Some(relationships) = &inc.relationships {
                                                if id == inc.id && patreon_id == relationships.campaign.data.id {
                                                    subscribed = true;
                                                }
                                            }
                                        }
                                    }

                                    if subscribed {
                                        redis_call(db.clone(), vec!["set", &format!("channel:{}:patreon:subscribed", &channel), "true"]);
                                    } else {
                                        redis_call(db.clone(), vec!["set", &format!("channel:{}:patreon:subscribed", &channel), "false"]);
                                        let token = settings.get_str("bot_token").unwrap();
                                        if patreon_sub == "true" {
                                            redis_call(db.clone(), vec!["publish", &format!("channel:{}:signals:rename", &channel), &token]);
                                        }
                                    }
                                }
                            }
                        });
                    thread::spawn(move || { tokio::run(future) });
                }
            }
            thread::sleep(time::Duration::from_secs(86400));
        }
    });
}

fn refresh_twitch_bots(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    let bots: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "bots"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
    let mut futures = Vec::new();
    for bot in bots.clone() {
        let db = db.clone();
        let dbC = db.clone();
        let botC = bot.clone();
        let res: Result<Value,_> = redis_call(db.clone(), vec!["get", &format!("bot:{}:refresh", &bot)]);
        if let Ok(value) = res {
            let token: String = from_redis_value(&value).unwrap();
            let mut settings = config::Config::default();
            settings.merge(config::File::with_name("Settings")).unwrap();
            settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
            let id = settings.get_str("client_id").unwrap_or("".to_owned());
            let secret = settings.get_str("client_secret").unwrap_or("".to_owned());

            let future = twitch_refresh(Method::POST, "https://id.twitch.tv/oauth2/token", Some(format!("grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}", token, id, secret).as_bytes().to_owned())).send()
                .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                .map_err(move |e| log_error(Some(Left(&botC)), "refresh_twitch", &e.to_string(), db.clone()))
                .map(move |body| {
                    let db = dbC.clone();
                    let body = std::str::from_utf8(&body).unwrap();
                    let json: Result<TwitchRefresh,_> = serde_json::from_str(&body);
                    match json {
                        Err(e) => {
                            log_error(Some(Left(&bot)), "refresh_twitch", &e.to_string(), db.clone());
                            log_error(Some(Left(&bot)), "request_body", &body, db.clone());
                        }
                        Ok(json) => {
                            redis_call(db.clone(), vec!["set", &format!("bot:{}:token", &bot), &json.access_token]);
                            redis_call(db.clone(), vec!["set", &format!("bot:{}:refresh", &bot), &json.refresh_token]);
                        }
                    }
                });
            futures.push(future);
        }
    }
    let mut core = tokio_core::reactor::Core::new().unwrap();
    let work = join_all(futures);
    core.run(work);

    thread::spawn(move || {
        thread::sleep(time::Duration::from_secs(3600));
        let bots: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "bots"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
        loop {
            for bot in bots.clone() {
                let db = db.clone();
                let dbC = db.clone();
                let botC = bot.clone();
                let res: Result<Value,_> = redis_call(db.clone(), vec!["get", &format!("bot:{}:refresh", &bot)]);
                if let Ok(value) = res {
                    let token: String = from_redis_value(&value).unwrap();
                    let mut settings = config::Config::default();
                    settings.merge(config::File::with_name("Settings")).unwrap();
                    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
                    let id = settings.get_str("client_id").unwrap_or("".to_owned());
                    let secret = settings.get_str("client_secret").unwrap_or("".to_owned());

                    let future = twitch_refresh(Method::POST, "https://id.twitch.tv/oauth2/token", Some(format!("grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}", token, id, secret).as_bytes().to_owned())).send()
                        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                        .map_err(move |e| log_error(Some(Left(&botC)), "refresh_twitch", &e.to_string(), db.clone()))
                        .map(move |body| {
                            let db = dbC.clone();
                            let body = std::str::from_utf8(&body).unwrap();
                            let json: Result<TwitchRefresh,_> = serde_json::from_str(&body);
                            match json {
                                Err(e) => {
                                    log_error(Some(Left(&bot)), "refresh_twitch", &e.to_string(), db.clone());
                                    log_error(Some(Left(&bot)), "request_body", &body, db.clone());
                                }
                                Ok(json) => {
                                    redis_call(db.clone(), vec!["set", &format!("bot:{}:token", &bot), &json.access_token]);
                                    redis_call(db.clone(), vec!["set", &format!("bot:{}:refresh", &bot), &json.refresh_token]);
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

fn refresh_twitch_channels(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    let channels: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
    let mut futures = Vec::new();
    for channel in channels.clone() {
        let db = db.clone();
        let dbC = db.clone();
        let channelC = channel.clone();
        let res: Result<Value,_> = redis_call(db.clone(), vec!["get", &format!("channel:{}:refresh", &channel)]);
        if let Ok(value) = res {
            let token: String = from_redis_value(&value).unwrap();
            let mut settings = config::Config::default();
            settings.merge(config::File::with_name("Settings")).unwrap();
            settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
            let id = settings.get_str("client_id").unwrap_or("".to_owned());
            let secret = settings.get_str("client_secret").unwrap_or("".to_owned());

            let future = twitch_refresh(Method::POST, "https://id.twitch.tv/oauth2/token", Some(format!("grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}", token, id, secret).as_bytes().to_owned())).send()
                .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                .map_err(move |e| log_error(Some(Right(vec![&channelC])), "refresh_twitch", &e.to_string(), db.clone()))
                .map(move |body| {
                    let db = dbC.clone();
                    let body = std::str::from_utf8(&body).unwrap();
                    let json: Result<TwitchRefresh,_> = serde_json::from_str(&body);
                    match json {
                        Err(e) => {
                            log_error(Some(Right(vec![&channel])), "refresh_twitch", &e.to_string(), db.clone());
                            log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                        }
                        Ok(json) => {
                            redis_call(db.clone(), vec!["set", &format!("channel:{}:token", &channel), &json.access_token]);
                            redis_call(db.clone(), vec!["set", &format!("channel:{}:refresh", &channel), &json.refresh_token]);
                        }
                    }
                });
            futures.push(future);
        }
    }
    let mut core = tokio_core::reactor::Core::new().unwrap();
    let work = join_all(futures);
    core.run(work);

    thread::spawn(move || {
        thread::sleep(time::Duration::from_secs(3600));
        loop {
            let channels: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
            for channel in channels {
                let db = db.clone();
                let dbC = db.clone();
                let channelC = channel.clone();
                let res: Result<Value,_> = redis_call(db.clone(), vec!["get", &format!("channel:{}:refresh", &channel)]);
                if let Ok(value) = res {
                    let token: String = from_redis_value(&value).unwrap();
                    let mut settings = config::Config::default();
                    settings.merge(config::File::with_name("Settings")).unwrap();
                    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
                    let id = settings.get_str("client_id").unwrap_or("".to_owned());
                    let secret = settings.get_str("client_secret").unwrap_or("".to_owned());

                    let future = twitch_refresh(Method::POST, "https://id.twitch.tv/oauth2/token", Some(format!("grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}", token, id, secret).as_bytes().to_owned())).send()
                        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                        .map_err(move |e| log_error(Some(Right(vec![&channelC])), "refresh_twitch", &e.to_string(), db.clone()))
                        .map(move |body| {
                            let db = dbC.clone();
                            let body = std::str::from_utf8(&body).unwrap();
                            let json: Result<TwitchRefresh,_> = serde_json::from_str(&body);
                            match json {
                                Err(e) => {
                                    log_error(Some(Right(vec![&channel])), "refresh_twitch", &e.to_string(), db.clone());
                                    log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                                }
                                Ok(json) => {
                                    redis_call(db.clone(), vec!["set", &format!("channel:{}:token", &channel), &json.access_token]);
                                    redis_call(db.clone(), vec!["set", &format!("channel:{}:refresh", &channel), &json.refresh_token]);
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

fn refresh_spotify(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    thread::spawn(move || {
        loop {
            let channels: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
            for channel in channels {
                let db = db.clone();
                let dbC = db.clone();
                let channelC = channel.clone();
                let res: Result<Value,_> = redis_call(db.clone(), vec!["get", &format!("channel:{}:spotify:refresh", &channel)]);
                if let Ok(value) = res {
                    let token: String = from_redis_value(&value).unwrap();
                    let future = spotify_refresh(Method::POST, "https://accounts.spotify.com/api/token", Some(format!("grant_type=refresh_token&refresh_token={}", token).as_bytes().to_owned())).send()
                        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                        .map_err(move |e| log_error(Some(Right(vec![&channelC])), "update_spotify", &e.to_string(), db.clone()))
                        .map(move |body| {
                            let db = dbC.clone();
                            let body = std::str::from_utf8(&body).unwrap();
                            let json: Result<SpotifyRefresh,_> = serde_json::from_str(&body);
                            match json {
                                Err(e) => {
                                    log_error(Some(Right(vec![&channel])), "update_spotify", &e.to_string(), db.clone());
                                    log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                                }
                                Ok(json) => {
                                    redis_call(db.clone(), vec!["set", &format!("channel:{}:spotify:token", &channel), &json.access_token]);
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

fn update_watchtime(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    thread::spawn(move || {
        loop {
            let channels: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
            for channel in channels {
                let db = db.clone();
                let live: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:live", &channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                let enabled: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", &channel), "channel:viewerstats"]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                if live == "true" && enabled == "true" {
                    let future = request(Method::GET, None, &format!("http://tmi.twitch.tv/group/user/{}/chatters", &channel)).send()
                        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                        .map_err(|e| println!("request error: {}", e))
                        .map(move |body| {
                            let body = std::str::from_utf8(&body).unwrap();
                            let json: Result<TmiChatters,_> = serde_json::from_str(&body);
                            match json {
                                Err(e) => {
                                    log_error(Some(Right(vec![&channel])), "update_watchtime", &e.to_string(), db.clone());
                                    log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
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
                                        let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:watchtimes", &channel), &nick]);
                                        if let Ok(value) = res {
                                            let wt: String = from_redis_value(&value).unwrap();
                                            let num: i64 = wt.parse().unwrap();
                                            redis_call(db.clone(), vec!["hset", &format!("channel:{}:watchtimes", &channel), &nick, (num + 1).to_string().as_ref()]);
                                        } else {
                                            redis_call(db.clone(), vec!["hset", &format!("channel:{}:watchtimes", &channel), &nick, "1"]);
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

fn update_live(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    thread::spawn(move || {
        loop {
            let db = db.clone();
            let channels: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
            if channels.len() > 0 {
                let mut ids = Vec::new();
                for channel in channels.clone() {
                    let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
                    ids.push(id);
                }
                let channel = channels.iter().next().unwrap();
                let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
                let future = twitch_kraken_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/streams?channel={}", ids.join(","))).send()
                    .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                    .map_err(|e| println!("request error: {}", e))
                    .map(move |body| {
                        let body = std::str::from_utf8(&body).unwrap().to_string();
                        let json: Result<KrakenStreams,_> = serde_json::from_str(&body);
                        match json {
                            Err(e) => {
                                log_error(None, "update_live", &e.to_string(), db.clone());
                                log_error(None, "request_body", &body, db.clone());
                            }
                            Ok(json) => {
                                let live_channels: Vec<String> = json.streams.iter().map(|stream| stream.channel.name.to_owned()).collect();
                                for channel in channels {
                                    let live: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:live", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                                    if live_channels.contains(&channel) {
                                        let stream = json.streams.iter().find(|stream| { return stream.channel.name == channel }).unwrap();
                                        if live == "false" {
                                            redis_call(db.clone(), vec!["set", &format!("channel:{}:live", channel), "true"]);
                                            redis_call(db.clone(), vec!["del", &format!("channel:{}:hosts:recent", channel)]);
                                            // reset notice timers
                                            let keys: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["keys", &format!("channel:{}:notices:*:messages", channel)]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                                            for key in keys.iter() {
                                                let int: Vec<&str> = key.split(":").collect();
                                                redis_call(db.clone(), vec!["set", &format!("channel:{}:notices:{}:countdown", channel, int[3]), int[3].clone()]);
                                            }
                                            // send discord announcements
                                            let tres: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", channel), "discord:token"]);
                                            let ires: Result<Value,_> = redis_call(db.clone(), vec!["hget",&format!("channel:{}:settings", channel), "discord:channel-id"]);
                                            if let (Ok(tvalue), Ok(ivalue)) = (tres, ires) {
                                                let id: String = from_redis_value(&ivalue).unwrap();
                                                let token: String = from_redis_value(&tvalue).unwrap();
                                                let message: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", channel), "discord:live-message"]).unwrap_or(Value::Data("".as_bytes().to_owned()))).unwrap();
                                                let display: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:display-name", channel)]).expect(&format!("channel:{}:display-name", channel))).unwrap();
                                                let body = format!("{{ \"content\": \"{}\", \"embed\": {{ \"author\": {{ \"name\": \"{}\" }}, \"title\": \"{}\", \"url\": \"http://twitch.tv/{}\", \"thumbnail\": {{ \"url\": \"{}\" }}, \"fields\": [{{ \"name\": \"Now Playing\", \"value\": \"{}\" }}] }} }}", &message, &display, stream.channel.status, channel, stream.channel.logo, stream.channel.game);
                                                let future = discord_request(token, Some(body.as_bytes().to_owned()), Method::POST, &format!("https://discordapp.com/api/channels/{}/messages", id)).send().and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() }).map_err(|e| println!("request error: {}", e)).map(move |_body| {});
                                                thread::spawn(move || { tokio::run(future) });
                                            }
                                        }
                                    } else {
                                        if live == "true" {
                                            redis_call(db.clone(), vec!["set", &format!("channel:{}:live", channel), "false"]);
                                            // reset stats
                                            let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", channel), "stats:reset"]);
                                            if let Err(_e) = res {
                                                redis_call(db.clone(), vec!["del", &format!("channel:{}:stats:pubg", channel)]);
                                                redis_call(db.clone(), vec!["del", &format!("channel:{}:stats:fortnite", channel)]);
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

fn update_stats(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    let dbP = db.clone();
    let dbF = db.clone();

    // pubg
    thread::spawn(move || {
        let db = dbP.clone();
        loop {
            let channels: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
            for channel in channels {
                let db = db.clone();
                let reset: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:pubg", &channel), "reset"]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", &channel), "stats:reset"]);
                if let Ok(value) = res {
                    let hour: String = from_redis_value(&value).unwrap();
                    let res: Result<u32,_> = hour.parse();
                    if let Ok(num) = res {
                        if num == Utc::now().time().hour() && reset == "true" {
                            redis_call(db.clone(), vec!["del", &format!("channel:{}:stats:pubg", &channel)]);
                        } else if num != Utc::now().time().hour() && reset == "false" {
                            redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:pubg", &channel), "reset", "true"]);
                        }
                    }
                }
                let live: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:live", &channel)]).expect(&format!("channel:{}:live", &channel))).unwrap();
                if live == "true" {
                    let res1: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", &channel), "pubg:token"]);
                    let res2: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", &channel), "pubg:name"]);
                    if let (Ok(value1), Ok(value2)) = (res1, res2) {
                        let token: String = from_redis_value(&value1).unwrap();
                        let tokenC = token.clone();
                        let name: String = from_redis_value(&value2).unwrap();
                        let platform: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", &channel), "pubg:platform"]).unwrap_or(Value::Data("steam".as_bytes().to_owned()))).unwrap();
                        let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", &channel), "pubg:id"]);
                        if let Ok(value) = res {
                            let id: String = from_redis_value(&value).unwrap();
                            let mut cursor: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:pubg", &channel), "cursor"]).unwrap_or(Value::Data("".as_bytes().to_owned()))).unwrap();
                            let future = pubg_request(token, &format!("https://api.pubg.com/shards/{}/players/{}", platform, id)).send()
                                .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                .map_err(|e| println!("request error: {}", e))
                                .map(move |body| {
                                    let token = tokenC.clone();
                                    let body = std::str::from_utf8(&body).unwrap();
                                    let json: Result<PubgPlayer,_> = serde_json::from_str(&body);
                                    match json {
                                        Err(e) => {
                                            log_error(Some(Right(vec![&channel])), "update_pubg", &e.to_string(), db.clone());
                                            log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                                        }
                                        Ok(json) => {
                                            if json.data.relationships.matches.data.len() > 0 {
                                                if cursor == "" { cursor = json.data.relationships.matches.data[0].id.to_owned() }
                                                redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:pubg", &channel), "cursor", &json.data.relationships.matches.data[0].id]);
                                                for match_ in json.data.relationships.matches.data.iter() {
                                                    let db = db.clone();
                                                    let token = token.clone();
                                                    let idC = id.clone();
                                                    let channelC = channel.clone();
                                                    if match_.id == cursor { break }
                                                    else {
                                                        let future = pubg_request(token, &format!("https://api.pubg.com/shards/pc-na/matches/{}", &match_.id)).send()
                                                            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                                            .map_err(|e| println!("request error: {}", e))
                                                            .map(move |body| {
                                                                let body = std::str::from_utf8(&body).unwrap();
                                                                let json: Result<PubgMatch,_> = serde_json::from_str(&body);
                                                                match json {
                                                                    Err(e) => {
                                                                        log_error(Some(Right(vec![&channelC])), "update_pubg", &e.to_string(), db.clone());
                                                                        log_error(Some(Right(vec![&channelC])), "request_body", &body, db.clone());
                                                                    }
                                                                    Ok(json) => {
                                                                        for p in json.included.iter().filter(|i| i.type_ == "participant") {
                                                                            if p.attributes["stats"]["playerId"] == idC {
                                                                                for stat in ["winPlace", "kills", "headshotKills", "roadKills", "teamKills", "damageDealt", "vehicleDestroys"].iter() {
                                                                                    if let Number(num) = &p.attributes["stats"][stat] {
                                                                                        if let Some(num) = num.as_f64() {
                                                                                            let mut statname: String = (*stat).to_owned();
                                                                                            if *stat == "winPlace" { statname = "wins".to_owned() }
                                                                                            let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:pubg", &channelC), &statname]);
                                                                                            if let Ok(value) = res {
                                                                                                let old: String = from_redis_value(&value).unwrap();
                                                                                                let n: u64 = old.parse().unwrap();
                                                                                                if *stat == "winPlace" {
                                                                                                    if num as u64 == 1 {
                                                                                                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:pubg", &channelC), &statname, (n + 1).to_string().as_ref()]);
                                                                                                    }
                                                                                                } else {
                                                                                                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:pubg", &channelC), &statname, (n + (num as u64)).to_string().as_ref()]);
                                                                                                }
                                                                                            } else {
                                                                                                if *stat == "winPlace" {
                                                                                                    if num as u64 == 1 {
                                                                                                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:pubg", &channelC), &statname, "1"]);
                                                                                                    }
                                                                                                } else {
                                                                                                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:pubg", &channelC), &statname, (num as u64).to_string().as_ref()]);
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
                            let future = pubg_request(token, &format!("https://api.pubg.com/shards/{}/players?filter%5BplayerNames%5D={}", platform, name)).send()
                                .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                .map_err(|e| println!("request error: {}", e))
                                .map(move |body| {
                                    let body = std::str::from_utf8(&body).unwrap();
                                    let json: Result<PubgPlayers,_> = serde_json::from_str(&body);
                                    match json {
                                        Err(e) => {
                                            log_error(Some(Right(vec![&channel])), "update_pubg", &e.to_string(), db.clone());
                                            log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                                        }
                                        Ok(json) => {
                                            if json.data.len() > 0 {
                                                redis_call(db.clone(), vec!["hset", &format!("channel:{}:settings", &channel), "pubg:id", &json.data[0].id]);
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
        let db = dbF.clone();
        loop {
            let channels: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
            for channel in channels {
                let db = db.clone();
                let reset: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:fortnite", &channel), "reset"]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", &channel), "stats:reset"]);
                if let Ok(value) = res {
                    let hour: String = from_redis_value(&value).unwrap();
                    let num: Result<u32,_> = hour.parse();
                    if let Ok(hour) = num {
                        if hour == Utc::now().time().hour() && reset == "true" {
                            redis_call(db.clone(), vec!["del", &format!("channel:{}:stats:fortnite", &channel)]);
                        } else if hour != Utc::now().time().hour() && reset == "false" {
                            redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:fortnite", &channel), "reset", "true"]);
                        }
                    }
                }
                let live: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:live", &channel)]).expect(&format!("channel:{}:live", &channel))).unwrap();
                if live == "true" {
                    let res1: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", &channel), "fortnite:token"]);
                    let res2: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", &channel), "fortnite:name"]);
                    if let (Ok(value1), Ok(value2)) = (res1, res2) {
                        let token: String = from_redis_value(&value1).unwrap();
                        let name: String = from_redis_value(&value2).unwrap();
                        let platform: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", &channel), "fortnite:platform"]).unwrap_or(Value::Data("pc".as_bytes().to_owned()))).unwrap();
                        let mut cursor: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:fortnite", &channel), "cursor"]).unwrap_or(Value::Data("".as_bytes().to_owned()))).unwrap();
                        let future = fortnite_request(token, &format!("https://api.fortnitetracker.com/v1/profile/{}/{}", platform, name)).send()
                            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                            .map_err(|e| println!("request error: {}", e))
                            .map(move |body| {
                                let body = std::str::from_utf8(&body).unwrap();
                                let json: Result<FortniteApi,_> = serde_json::from_str(&body);
                                match json {
                                    Err(e) => {
                                        log_error(Some(Right(vec![&channel])), "update_fortnite", &e.to_string(), db.clone());
                                        log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                                    }
                                    Ok(json) => {
                                        if json.recentMatches.len() > 0 {
                                            if cursor == "" { cursor = json.recentMatches[0].id.to_string() }
                                            redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:fortnite", &channel), "cursor", &json.recentMatches[0].id.to_string()]);
                                            for match_ in json.recentMatches.iter() {
                                                if match_.id.to_string() == cursor { break }
                                                else {
                                                    let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:fortnite", &channel), "wins"]);
                                                    if let Ok(value) = res {
                                                        let old: String = from_redis_value(&value).unwrap();
                                                        let n: u64 = old.parse().unwrap();
                                                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:fortnite", &channel), "wins", (n + (match_.top1 as u64)).to_string().as_ref()]);
                                                    } else {
                                                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:fortnite", &channel), "wins", (match_.top1 as u64).to_string().as_ref()]);
                                                    }

                                                    let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:fortnite", &channel), "kills"]);
                                                    if let Ok(value) = res {
                                                        let old: String = from_redis_value(&value).unwrap();
                                                        let n: u64 = old.parse().unwrap();
                                                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:fortnite", &channel), "kills", (n + (match_.kills as u64)).to_string().as_ref()]);
                                                    } else {
                                                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:stats:fortnite", &channel), "kills", (match_.kills as u64).to_string().as_ref()]);
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

fn run_command(matches: &ArgMatches, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    let channel: String = matches.value_of("channel").unwrap().to_owned();
    let cmd = matches.values_of("command").unwrap();
    let mut command: Vec<String> = Vec::new();
    for c in cmd { command.push(c.to_owned()) }

    if command.len() > 0 {
        redis_call(db.clone(), vec!["publish", &format!("channel:{}:signals:command", &channel), &format!("{}", command.join(" "))]);
    }
}
