#![feature(proc_macro_hygiene, decl_macro, custom_attribute)]

#[macro_use] extern crate rocket;

mod commands;
mod types;
mod util;
mod web;

use crate::types::*;
use crate::util::*;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc,Mutex};
use crossbeam_channel::{unbounded,Sender,Receiver,RecvTimeoutError};
use std::ops::Deref;
use std::{thread,time};

use clap::load_yaml;
use config;
use clap::{App, ArgMatches};
use bcrypt::{DEFAULT_COST, hash};
use irc::error;
use irc::client::prelude::*;
use url::Url;
use regex::Regex;
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
    let pool = r2d2::Pool::builder().max_size(100).build(manager).unwrap();
    let pool_c1 = pool.clone();

    if let Some(matches) = matches.subcommand_matches("add_channel") { add_channel(pool.clone(), &settings, matches) }
    else if let Some(matches) = matches.subcommand_matches("backpack_import") { backpack_import(pool.clone(), &settings, matches) }
    else {
        thread::spawn(move || { new_channel_listener(pool_c1) });
        thread::spawn(move || {
            rocket::ignite()
              .mount("/assets", StaticFiles::from("assets"))
              .mount("/", routes![web::index, web::dashboard, web::data, web::login, web::logout, web::signup, web::password, web::title, web::game, web::new_command, web::save_command, web::trash_command, web::new_notice, web::trash_notice, web::save_setting, web::trash_setting, web::new_blacklist, web::save_blacklist, web::trash_blacklist])
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
                    live_update(pool.clone(), channel.to_owned());
                    discord_handler(pool.clone(), channel.to_owned());
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
    let mut senders: HashMap<String, Sender<ThreadAction>> = HashMap::new();
    loop {
        bots.iter().for_each(|(_bot, channels)| {
            let client = Arc::new(reactor.prepare_client_and_connect(&channels.1).unwrap());
            client.identify().unwrap();
            let _ = client.send("CAP REQ :twitch.tv/tags");
            let _ = client.send("CAP REQ :twitch.tv/commands");
            register_handler((*client).clone(), &mut reactor, con.clone());
            for channel in channels.0.iter() {
                let (sender, receiver) = unbounded();
                senders.insert(channel.to_owned(), sender);
                spawn_timers(client.clone(), pool.clone(), channel.to_owned(), receiver);
                rename_channel_listener(pool.clone(), client.clone(), channel.to_owned(), senders.clone());
            }
        });
        let res = reactor.run();
        match res {
            Ok(_) => break,
            Err(e) => {
                eprintln!("[run_reactor] {}", e);
                bots.iter().for_each(|(_bot, channels)| {
                    for channel in channels.0.iter() {
                        if let Some(sender) = senders.get(channel) {
                            let _ = sender.send(ThreadAction::Kill);
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
            live_update(pool.clone(), channel.to_owned());
            discord_handler(pool.clone(), channel.to_owned());
            update_pubg(pool.clone(), channel.to_owned());
        }
        run_reactor(pool.clone(), bots);
    }
}

fn rename_channel_listener(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, client: Arc<IrcClient>, channel: String, senders: HashMap<String, Sender<ThreadAction>>) {
    thread::spawn(move || {
        let con = pool.get().unwrap();
        let mut conn = pool.get().unwrap();
        let mut ps = conn.as_pubsub();
        ps.subscribe(format!("channel:{}:rename", channel)).unwrap();

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
                        if let Some(sender) = senders.get(&channel) {
                            let _ = sender.send(ThreadAction::Kill);
                        }
                        client.send_quit("").unwrap();

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

fn register_handler(client: IrcClient, reactor: &mut IrcReactor, con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>) {
    let msg_handler = move |client: &IrcClient, irc_message: Message| -> error::Result<()> {
        match &irc_message.command {
            Command::PING(_,_) => { let _ = client.send_pong(":tmi.twitch.tv"); }
            Command::PRIVMSG(chan, msg) => {
                let channel = &chan[1..];
                let mut words = msg.split_whitespace();
                if let Some(word) = words.next() {
                    let word = word.to_lowercase();
                    let args: Vec<&str> = words.collect();
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
                    // filters: caps, symbols, length
                    let colors: String = con.get(format!("channel:{}:moderation:colors", channel)).unwrap_or("false".to_owned());
                    let links: Vec<String> = con.smembers(format!("channel:{}:moderation:links", channel)).unwrap_or(Vec::new());
                    let bkeys: Vec<String> = con.keys(format!("channel:{}:moderation:blacklist:*", channel)).unwrap();
                    if colors == "true" && msg.len() > 6 && msg.as_bytes()[0] == 1 && &msg[1..7] == "ACTION" {
                        let _ = client.send_privmsg(chan, format!("/timeout {} 1", get_nick(&irc_message)));
                    }
                    if links.len() > 0 && url_regex().is_match(&msg) {
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
                                            let _ = client.send_privmsg(chan, format!("/timeout {} 1", get_nick(&irc_message)));
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
                        match Regex::new(&rgx) {
                            Err(e) => { eprintln!("{}", e) }
                            Ok(rgx) => {
                                if rgx.is_match(&msg) {
                                    let _ = client.send_privmsg(chan, format!("/timeout {} {}", get_nick(&irc_message), length));
                                    break;
                                }
                            }
                        }
                    }

                    let mut auth = false;
                    if let Some(value) = badges.get("broadcaster") {
                        if let Some(value) = value {
                            if value == "1" { auth = true }
                        }
                    }
                    if let Some(value) = badges.get("moderator") {
                        if let Some(value) = value {
                            if value == "1" { auth = true }
                        }
                    }

                    for cmd in commands::native_commands.iter() {
                        if format!("!{}", cmd.0) == word {
                            if args.len() == 0 {
                                if !cmd.2 || auth { (cmd.1)(con.clone(), &client, channel, &args) }
                            } else {
                                if !cmd.3 || auth { (cmd.1)(con.clone(), &client, channel, &args) }
                            }
                            break;
                        }
                    }
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
                            let _: () = con.hset(format!("channel:{}:commands:{}", channel, word), "lastrun", Utc::now().to_rfc3339()).unwrap();
                            let mut message = message;
                            for var in commands::command_vars.iter() {
                                message = parse_var(var, &message, con.clone(), &client, channel, Some(&irc_message), &args);
                            }
                            if args.len() > 0 {
                                if let Some(char) = args[args.len()-1].chars().next() {
                                    if char == '@' {
                                        message = format!("{} -> {}", args[args.len()-1], message);
                                    }
                                }
                            }
                            if args.len() == 0 {
                                let protected: String = con.hget(format!("channel:{}:commands:{}", channel, word), "cmd_protected").unwrap();
                                if protected == "false" || auth {
                                    let _ = client.send_privmsg(chan, message);
                                }
                            } else {
                                let protected: String = con.hget(format!("channel:{}:commands:{}", channel, word), "arg_protected").unwrap();
                                if protected == "false" || auth {
                                    let _ = client.send_privmsg(chan, message);
                                }
                            }
                        }
                    }
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
                let mut client = serenity::client::Client::new(&token, DiscordHandler).unwrap();
                client.with_framework(StandardFramework::new());

                if let Err(e) = client.start() {
                    eprintln!("[discord_handler] {}", e);
                }
            }
            thread::sleep(time::Duration::from_secs(10));
        }
    });
}

fn live_update(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, channel: String) {
    thread::spawn(move || {
        let con = Arc::new(pool.get().unwrap());
        let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
        loop {
            let rsp = twitch_request_get(con.clone(), &channel, &format!("https://api.twitch.tv/kraken/streams?channel={}", id));
            match rsp {
                Err(e) => { println!("[live_update] {}", e) }
                Ok(mut rsp) => {
                    let json: Result<KrakenStreams,_> = rsp.json();
                    match json {
                        Err(e) => { println!("[live_update] {}", e); }
                        Ok(json) => {
                            let live: String = con.get(format!("channel:{}:live", channel)).unwrap();
                            if json.total == 0 {
                                if live == "true" {
                                    let _: () = con.set(format!("channel:{}:live", channel), false).unwrap();
                                }
                            } else {
                                if live == "false" {
                                    let _: () = con.set(format!("channel:{}:live", channel), true).unwrap();
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

fn spawn_timers(client: Arc<IrcClient>, pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, channel: String, receiver: Receiver<ThreadAction>) {
    let notice_con = pool.get().unwrap();
    let commercial_con = pool.get().unwrap();
    let notice_client = client.clone();
    let commercial_client = client.clone();
    let notice_channel = channel.clone();
    let commercial_channel = channel.clone();

    // notices
    thread::spawn(move || {
        let con = Arc::new(notice_con);
        loop {
            let rsp = receiver.recv_timeout(time::Duration::from_secs(60));
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
                    if let Ok(message) = res {
                        let mut message = message;
                        for var in commands::command_vars.iter() {
                            message = parse_var(var, &message, con.clone(), &client, &notice_channel, None, &Vec::new());
                        }
                        let _ = notice_client.send_privmsg(format!("#{}", notice_channel), message);
                    }
                }
            }
        }
    });

    // commercials
    thread::spawn(move || {
        let con = Arc::new(commercial_con);
        loop {
            let live: String = con.get(format!("channel:{}:live", commercial_channel)).unwrap();
            if live == "true" {
                let hourly: String = con.get(format!("channel:{}:commercials:hourly", commercial_channel)).unwrap_or("0".to_owned());
                let hourly: u64 = hourly.parse().unwrap();
                let recents: Vec<String> = con.lrange(format!("channel:{}:commercials:recent", commercial_channel), 0, -1).unwrap();
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
                    let res: Result<String,_> = con.lindex(format!("channel:{}:commercials:recent", commercial_channel), 0);
                    if let Ok(lastrun) = res {
                        let lastrun: Vec<&str> = lastrun.split_whitespace().collect();
                        let timestamp = DateTime::parse_from_rfc3339(&lastrun[0]).unwrap();
                        let diff = Utc::now().signed_duration_since(timestamp);
                        if diff.num_minutes() < 9 {
                            within8 = true;
                        }
                        if within8 {
                            thread::sleep(time::Duration::from_secs((9 - (diff.num_minutes() as u64)) * 30));
                        }
                    }
                    let id: String = con.get(format!("channel:{}:id", commercial_channel)).unwrap();
                    let submode: String = con.get(format!("channel:{}:commercials:submode", commercial_channel)).unwrap_or("false".to_owned());
                    let nres: Result<String,_> = con.get(format!("channel:{}:commercials:notice", commercial_channel));
                    let rsp = twitch_request_post(con.clone(), &channel, &format!("https://api.twitch.tv/kraken/channels/{}/commercial", id), format!("{{\"length\": {}}}", num * 30));
                    let length: u16 = con.llen(format!("channel:{}:commercials:recent", commercial_channel)).unwrap();
                    let _: () = con.lpush(format!("channel:{}:commercials:recent", commercial_channel), format!("{} {}", Utc::now().to_rfc3339(), num)).unwrap();
                    if length > 7 {
                        let _: () = con.rpop(format!("channel:{}:commercials:recent", commercial_channel)).unwrap();
                    }
                    if submode == "true" {
                        let client_clone = commercial_client.clone();
                        let channel_clone = String::from(commercial_channel.clone());
                        let _ = commercial_client.send_privmsg(format!("#{}", commercial_channel), "/subscribers");
                        thread::spawn(move || {
                            thread::sleep(time::Duration::from_secs(num * 30));
                            client_clone.send_privmsg(format!("#{}", channel_clone), "/subscribersoff").unwrap();
                        });
                    }
                    if let Ok(notice) = nres {
                        let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", commercial_channel, notice), "message");
                        if let Ok(message) = res {
                            let _ = commercial_client.send_privmsg(format!("#{}", commercial_channel), message);
                        }
                    }
                    let _ = commercial_client.send_privmsg(format!("#{}", commercial_channel), format!("{} commercials have been run", num));
                }
                thread::sleep(time::Duration::from_secs(3600));
            } else {
                thread::sleep(time::Duration::from_secs(60));
            }
        }
    });
}

fn add_channel(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, settings: &config::Config, matches: &ArgMatches) {
    let con = Arc::new(pool.get().unwrap());
    let mut bot_name: String;
    let mut bot_token: String;
    match matches.value_of("bot") {
        Some(bot) => { bot_name = bot.to_owned(); }
        None => { bot_name = settings.get_str("bot_name").unwrap(); }
    }
    match matches.value_of("botToken") {
        Some(token) => { bot_token = token.to_owned(); }
        None => { bot_token = settings.get_str("bot_token").unwrap(); }
    }
    let channel_name = matches.value_of("channel").unwrap();
    let channel_token = matches.value_of("channelToken").unwrap();
    let channel_password = hash(matches.value_of("password").unwrap(), DEFAULT_COST).unwrap();

    let _: () = con.sadd("bots", &bot_name).unwrap();
    let _: () = con.sadd("channels", channel_name).unwrap();
    let _: () = con.sadd(format!("bot:{}:channels", &bot_name), channel_name).unwrap();
    let _: () = con.set(format!("bot:{}:token", &bot_name), bot_token).unwrap();
    let _: () = con.set(format!("channel:{}:bot", channel_name), bot_name).unwrap();
    let _: () = con.set(format!("channel:{}:token", channel_name), channel_token).unwrap();
    let _: () = con.set(format!("channel:{}:password", channel_name), channel_password).unwrap();
    let _: () = con.set(format!("channel:{}:live", channel_name), false).unwrap();

    let client = reqwest::Client::new();
    let rsp = client.get("https://api.twitch.tv/helix/users").header(header::AUTHORIZATION, format!("Bearer {}", channel_token)).send();

    match rsp {
        Err(e) => { println!("[add_channel] {}", e) }
        Ok(mut rsp) => {
            let json: Result<types::HelixUsers,_> = rsp.json();
            match json {
                Err(e) => { println!("[add_channel] {}", e) }
                Ok(json) => {
                    let _: () = con.set(format!("channel:{}:id", channel_name), &json.data[0].id).unwrap();
                    let _: () = con.set(format!("channel:{}:display-name", channel_name), &json.data[0].display_name).unwrap();
                    let _: () = con.publish("new_channels", channel_name).unwrap();
                    println!("channel {} has been added", channel_name)
                }
            }
        }
    }
}

fn backpack_import(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, settings: &config::Config, matches: &ArgMatches) {
    let con = Arc::new(pool.get().unwrap());
    for (nick,watchtime) in BACKPACK_IMPORT.iter() {
        let _: () = con.hset("channel:backpackbrady:watchtimes", *nick, *watchtime).unwrap();
    }
}

const BACKPACK_IMPORT: [(&str,u64); 3249] = [("5cheel",992),("69tipsea",4005),("al___",440),("anotherttvviewer",11681),("collateralsandwich",15487),("commanderroot",21773),("deej_tv",42),("devinwatson106",197),("eternaleif",42),("fatalswordsmen",495),("fbihuge",1369),("fischerblue",8014),("forrian",1420),("governort",92),("grouchyoldvet",466),("host_giveaway",13903),("huntersguidelines",1583),("jace760",398),("kobwebb",1725),("littleninja0106",13),("lo4d50",2763),("m0psy",3304),("max_specks",42),("n3td3v",15438),("nightbot",14710),("omegz",62),("p0lizei_",18392),("papastanimus",16962),("randoecomando",701),("realond",42),("silverstrike",805),("skinnyseahorse",5749),("slocool",11495),("spc_sentinel",2323),("streamlabs",22118),("thacoless",1970),("toadhopperbbq",4871),("upstatestream",1471),("v_and_k",20815),("vaulttec21",645),("villocity",14114),("virgoproz",20731),("woerdy",37),("bradys_bot",21890),("campfireslayer",13324),("chester259",8109),("docransom",6863),("backpackbrady",12324),("beardo42",15190),("djsmlth",2151),("e4ether",1744),("sirhapping",7021),("the_sprig",1086),("toovs",9975),("8inchaydan_",75),("salsh16",2),("electricallongboard",6107),("ljy1602",2),("lxmaa",1),("mircenery",1245),("patriott776",108),("walkingstick75",2),("dumb_twitch_name",25),("fataldazed",20),("hellrazer1950",1269),("voodooodave",4642),("maggame",437),("coobiexd",382),("cakesgames1",23),("ignitorpr",2),("knee22",20),("cephalyx",1),("urbantemple",766),("ashsmokem710",2847),("exomumbles",5368),("frownchain",181),("the_real_ray_charles",307),("shhtime",269),("bepo13",4),("dabmaster_4_2_0",137),("mdeyaf",7),("renascencedk",23),("gambleza",2),("nippletwist1616",5),("dphill",35),("badeggz",1),("canadiansafa",5),("hugguwuggu1",1),("xgnpr0gam3ryt",2),("w_jones",1874),("mataratat",19),("ivoren",284),("guirotv",1425),("afflictedego",21147),("eft_surfer",366),("mr_shortmit",867),("mejubaer",5),("damianek3kk",1),("boomblebe",5727),("spaceman66",1066),("snapzfn",36),("f1nn28",518),("p0sitivitybot",18563),("pomfi",280),("silverbullet3123",8),("chrisxballout",2),("obelgray",6),("dsthamonster",6),("knotvise",15),("yougotgame1988",9),("skwotbinchdidlift",652),("darkcloud86x",256),("seasick",4),("viper35678",33),("pullthetricker",277),("noobcorpse",100),("xx_ebye",91),("rattus_the_rat",9),("jsetv",4),("tiz1126",604),("whoagambit",1),("imbunnyhop",308),("crewar23",27),("skumshop",1766),("th3_cronus",2),("ghostdogsxx",243),("hyenarose",1784),("the_moose_rl",1),("tommyy16",203),("mr_q_",159),("marcioricioli36",1),("taaarfcoe",70),("cpt_ham",81),("kaneefc121",281),("phatblakman",1),("spykevibe",290),("budgiesmugglaa",23),("denyshud",120),("anim0l187",116),("spadesco",4461),("antcaz",2),("cg_firstborn",164),("yeah_yeah",91),("zerohots",116),("onetongorilla",2974),("suigengaming",11555),("oh_its_oliver",762),("creepy_uncle22",74),("okentac",7512),("slfhgh5ghst",5),("arokc",7),("valigar2",583),("keegzmc3",2),("jigokuson",2592),("freez3killztv",16),("vittorinni",1),("golldy",616),("zecret_weapon",252),("smooothbrain",2432),("braxxus_",480),("amiauslander",7),("r1ckybalboa",5809),("thronehq",428),("themreuro",1515),("comepsilon",41),("f1re_man1ac",50),("itsmsb",1776),("killforfun23",5),("tegabyte_",320),("razorvision321",15),("woolfyii",4),("dacxd",3),("bladeyoursenpai",5),("00soap00",92),("thiccquid",5),("malzov",19),("xawoken_",107),("mrblitzer72",4),("mickjjj",1667),("adoyt_",11),("ikoala1",4),("snickelfritzel",137),("jfishmedia",18),("n0tahacker_",196),("keeeztv",61),("kingor85",54),("becauseitscurrentyear",5),("kibble13",226),("xepicztv",6),("evisra",7),("tes93",56),("tysonakakobe",9),("inyx0r",58),("thaizhao",1),("xworkxalreadyx",1),("imfinelol125",3),("neverupgraded",59),("ript_k",70),("derkszilla",1),("natiffan",389),("sourpunch00",1),("mainx6661",64),("burnzy_ttv",1384),("tiny_nw",764),("blackstar1986",18),("kalterfuchs",13),("itsbrottski",1083),("gliiiiiiitch",8),("keygo",2465),("dayzz666",4),("drstonies",1397),("misterdaemonic",116),("icon_bot",13109),("ccup31",3),("jmark420",5),("curtmcgurtt",409),("supersmashedbrolive",169),("cachebear",21691),("km00nn",260),("englishfox56",1),("catchtheimage",228),("donmanolo72",5),("beardhurts",11),("northerntechhermit",133),("tacticalluxx",111),("hitmantwoactuai",4),("viralstrayne",2182),("looty_mcfish",770),("mediaknive",1290),("the_drewsifer",9),("calebmasse",4),("olobaidplaysgames",1),("gnarlynar",3),("johnsolo42",1029),("paul_bunyan_jr",1),("smoke_rises",47),("sturod",3),("firuuzetv",1),("gabi_bird",568),("old_emrys",1),("nutscruncher",175),("aridlemon",76),("z00kalicious",340),("nmc_recon",1),("therealjcurry",7),("allaroundgooddad",2),("doldaslow",89),("filteau",1),("fastbadger",5),("goldenicon",1452),("jobrophoto",1),("zordimax",6),("zaifer9",11),("itzmeelo",3),("killonair",1),("truetotheblue",6),("crankee",234),("extraviewgaming",263),("tawaterboy",145),("syd0h",48),("senta95",3),("epicleptiker",4),("bountyhunter_666",165),("haphinka",22),("band0",1),("firkagep",1),("selo55555",80),("kennylarsson85",804),("xvionas",130),("labolab00",65),("elad_o",26),("absoluteshock",10),("milkycharms",5026),("shooter10m",25),("biglouv",1041),("capnkubes",351),("discreditedtv",5),("yoollie",49),("1thaer",1),("savagegentleman",7),("craysin",107),("italian__ice",2),("crennes83",12),("bjo3171",1),("droppingpogs12",7),("dayel89",140),("blaknoble",108),("phegglet",1),("13gotheem",90),("193rdmp",172),("thefodey",3556),("unfluffybunny",114),("monkeytopwn",1),("starnuttz",3),("cesarsurf",2),("nosebleedgg",156),("mikeester",13),("podcastprimate",853),("jonnerd",358),("manzidanzi",76),("xhotchner",270),("baglerfinagler",11),("zanekyber",448),("alkapow",3),("jkitty209",9),("mannyvg",14),("garaya85",34),("gamerz3344",492),("sacampb",11547),("kl3pt0",62),("chuck_keyworth",15),("riballa13",8),("famouspcgamer",15),("parkz0r",179),("skelling",1),("aguythtsuxatcod",292),("tummuhs",2),("kallliii",8),("xkeniepimpyx",41),("trucambodia",51),("dabbinwiwi",37),("exelot",8),("tamatoathecoconutviking",4),("hson4real",199),("nevpocalypse",1),("yyngjesus",203),("bulletproofbritt",21),("thewanderingplaymaker",67),("xxsmartz",50),("ijliwies_v2",50),("kennyclan02",178),("cest7_",493),("animetiddles",820),("raiventhetank",15),("mr_simples",22),("kingkharmah",2),("fnfivesevenfreak1",20),("hydrantfps",6),("wafflekrew",31),("nittoh_",35),("awensome1234",49),("frameds0ul",126),("apricotdrupefruit",10526),("theitguy1",3),("dead_shot_0810",105),("mikexm8",3),("tnr_kronix",18),("pheelsbadmann",216),("brady8446",17),("interhomie",120),("blazinmenace",88),("bstace",12),("kyazu_",1),("cyclops6eight",28),("kumabit",216),("seikendnsu",8),("rlckbamf",52),("chillchelchew",25),("undercover_scav",164),("tripl3point",5),("echo_ex",1),("siirka_",5521),("yak3",3),("cj809",694),("nofear450",183),("drwangdeathpunch",4),("silverman211",4),("theitalianmango",12),("old_guy",186),("superhandspt",5),("kashmwt5",6),("dn1988allstar",21),("bludgeonballs",3),("richard_oellien",1),("crucifix",50),("biraynokanir",2),("whitewolf4756",48),("xxzandrxx",1),("afsoccer",36),("paitox1",37),("n0citizen",4),("rigataint",21),("charlesgoosa",2),("i3dog",3),("the_dirty_commie",3),("frazstreams",1),("therewearethen",5),("dragunok31",2),("ghostthellama",3672),("slobbishburrito",39),("spiralcycle",163),("mtsimpson",14),("b00mhower",6),("blodbank",27),("majorkilo",334),("tr00p3r_",3),("zbobo_",2),("redsnake8911",6),("77_senya_77",2),("blazedguru",49),("linsoumis",162),("lelouch86",8),("moonshinebadger",7),("cpmxox",5),("spookydoo32",28),("dlaw11",56),("bonestreams",1206),("herbologist_",70),("olddirtybert",17),("rollierodz",14),("yalanlar17",83),("wickett82",1),("subconside",126),("fcdoomsday",10),("liveabike",1388),("ezekiel2517",6),("kstvl",342),("skerp24",37),("johnnywho2",40),("footlonghunter",2),("matttangodelta",4),("jjtlmao",5),("mybeer30",5),("artadchy",9),("scarecrowitagun",3),("tattattack13",153),("thatchosenboy",59),("zerojacen13",1),("airstrikeivanov",193),("theclearing",26),("marsbargam3r",21),("mvntistv",2),("johnmcclane88",1),("pathogenic1",63),("trebulance",26),("jp3650",1),("grannnyshifting",4),("liteaxle",7),("joeyabro",2752),("ch3rnob1ll",2),("theshuffles70",38),("ninjashaper6193",7),("brainiac3252",1),("buttchuggahpka",12),("misterflashed",4),("destructivecritic",1),("agonyvonsolum",3),("ohhkie",32),("beteezy",5),("drboogersnot",6),("fishermanscove",80),("venturarbg",666),("asymmetricscar",2),("pleasehelpswagonme",2),("kjfreshly",40),("chiefjames5",42),("big_mech_dick",2),("fuuchan173",2),("mx614",4),("reborn_chaos",6),("th3fxr",3),("drinkmorewat3r",84),("alpha1775",2),("rhaydor",48),("riox18",1),("zero_hit_points",1),("kamakazeee_",5),("hivory__",4),("nerqo",1),("sbuzzagrilly",41),("darksaberjedi",6996),("carringtoneffect",12),("thinblueline6",33),("mumble_xd",1104),("alaw6163",984),("jimmyjoee",96),("gamebang3r_",1),("hedkraker",2),("grim7345",23),("namcek",15),("r_ay",5),("waddeii",5),("pangea",5),("og_warebear",15),("datgamer1008",32),("pissmonkey",1),("helixxir",5),("wizwaynewall",26),("drhow_",27),("lcrunk307",8),("robot4000222",14),("raumdeuter",52),("0ld_toby",5),("zeijzu",115),("randomguy222",98),("draztiikz",6),("tongeezy",6),("mrpandaaaaaaaaaaaa",3),("airtightalibi",295),("xbit01",1222),("david_innes",6),("burrit0head",24),("trueparadox628",2),("neglectorgaming",34),("yaboydrey",841),("echo1132",1),("twinklefacee",17),("moonfungus",3),("jetliilive",13),("detoxfmt",1),("drybonesusa",227),("bluetoothpe",1),("kingalmond__",20),("destttttttt",8),("bamfhealer",7),("waffless18",1),("mitbrown",2),("aningan2617",2),("batman123350",1),("damagefrequency",2),("activeenergy",2293),("soazluke",184),("tyside",1),("hannah_onic",72),("snappy____",21),("brewtuss",4),("kajunhk",56),("loan4cards",49),("shadeyxd2k",18),("angelpavlov",5),("donaguilon",324),("plutotv_",3),("weyy4",2),("jaystreazy",2),("firethepuffin",451),("jounce",480),("flaredude55",47),("kivong",26),("tgunz83123",11),("martinedition",1),("itzcoopergaming",15),("spedy3s",2),("valkillmor88",8),("sandersssssssssss",33),("jimmyb0i",17),("mrmerksir",439),("highlands1gx",2),("rileyeve1",11),("spekopz",8),("sugeron",3364),("lumpystankpickle",2),("borman148812",1),("kingzy04",10),("flaake1",6),("st3altheh",19),("l1qu1d_swordz",3),("powerhouse_og",8),("milehighmystery",486),("andreas_kw",14),("codem4ster",8),("ganordan",311),("moorhuhnjaeger",114),("manobrown07",2),("niitsud",6),("painkiller",24),("aesirbaldur",1),("cryderz",685),("iamfattygamer",4),("setsuna9787",3),("rybreadl1ve",3),("scsupaplex",2),("recap_me",1),("titsologist",34),("njbailz",28),("rivalzor",3),("callsignjimbo",12),("thalmoopin",4),("gota03",3),("motionz_rg",1),("kingrazor_",3),("potatokai",1944),("unrealhero_",6),("au2ladenosreves",92),("maver_pl",137),("antidoteytv",1),("kiings",23),("wudun_ru",1),("lamelogin",598),("tecnonativo",31),("derjaws",2),("easydjim",2),("hupwupdup",1),("catsmash3r",7),("mitchell0370",2),("stereoradar",4),("phoenixlive07",771),("truckersrage",4),("nasty6800",69),("randomcallsign",398),("mcoates414",1),("phungpman",24),("az77156",3),("drfragensteinmd",4),("basmarksmanuk",41),("lemterion",1),("poncho_spectro",70),("unholyraptor523",124),("bennyberserker",5),("alinekko",963),("stridernox",410),("lparoxysml",58),("savior1405",1),("undeaddemon420",177),("dragonguy215",176),("fluffypillows_tv",4),("letsep",4),("norwegiantroll",23),("diamondbackev",9),("hurricane73",146),("luckystrike83",11),("protolesgo",5),("riho001",2),("explicitcontant",64),("xytorjanin",54),("eveortaga",168),("twizer115",5),("zerrbit",2),("pambosch",5),("5trek3",3),("phobiccoma",13),("auronjohnson",697),("chiefdylanplays",1),("ezepol",2),("murderinmusic",1),("gekyume170",66),("pavlik4000",2),("explodedsoda",19),("networ_king",10),("babonchek",2),("itsdelz",4),("saviormanjones",482),("adhit87",22),("vicadams",25),("blackaiming",5),("divyanshjain",1),("engab1989",9),("stiiintv",2),("ivanthedankable",6),("pro100noob4uk",2),("sandyyy",6),("gokupol",76),("hawt_line",1),("tailsxdd",7),("anthonydoesgamess",55),("leviackermanisgod",1),("regulosion",17),("howlinggamin",15),("zenin__",45),("feenjareen",1),("mikez1872",2),("jesterado",2),("raife12",2),("eightdip",57),("ketokill",15),("krellaz",3),("da_sweetz",60),("tw1zz1e",6),("giggity_1337",322),("decimateyou",92),("hardcarrydurka",53),("acetigg",2),("dabomb07jr",94),("powerwindows3813",4),("splatttit",75),("abhorsean",2),("warskyzo",4),("rd_16",2),("anoniimooze",81),("schutze07",11),("ironcutgaming",3),("swekarlsson",18),("alexan92",3),("heatproofarmin",141),("cyberiaexploratorium",1),("pballer547",456),("roundhandcody",53),("irageruxin",168),("bangshotya",11),("benjison",1),("meowzers67",28),("gnice4u",77),("helsreach",567),("irongaminghd",5),("ashton42004",7),("psychoticangel_",42),("super_jackk",607),("chileangodx",2),("r3dshoot",1),("vonknoxberg",210),("g0ralski",5),("yourkingjoffreybaratheon",8),("piratesbloodycove",26),("drewdaddy194",1),("bobovevo",100),("robotdog",597),("autofire00",49),("federaltax420",444),("vertigo1055",1),("tylerptl",1),("lokogallox47",20),("natertattes",6),("heoden",2),("pocrevocrednu",536),("mikemurawski",53),("welldresseddragons",1889),("joepowwa",38),("iamskippy",2),("thereal_neil",2),("xxbigboss079xx",37),("berryogg",3),("theluda",1),("dannerrs",15),("everclearsip",4),("pewa90",75),("slickqik69",1),("us_futuremarine",32),("chriscoped",66),("dochollidayau",4),("yaga31",11),("fluffy99",2),("brass_casings",11),("bt_stoney",58),("trivus_live",8),("accalia2222",1),("horrendous_one",20),("shaocj",4),("nukenoskill",80),("th3rm1t3",608),("sulphrsauce",10),("sammonlord",48),("jwh_tv",8),("obsceneentity",46),("lmaples",1),("choctaw_savage",10),("t00t1red",3),("kr3ch2",1),("atoxicdude",9),("imnoble_",394),("sh1tw0lf",4),("ur_tinydancer",25),("ers1",2),("themoretz",49),("lindadeluz",64),("matic44",3),("akk95",5),("angelus128",2),("teddydtv",695),("xmoneymaker228x",2),("luffyzert",126),("wildwire94",53),("tastethecrainbow",4),("titbeam",45),("fireworkmangamin",4),("pickleraiderttv",329),("rogue__wolf__",329),("tech_zhentan",18),("tidalnz",1),("ygmiproductions",233),("oghemi",1),("lpj_miksdu",38),("magic_wu_98",5),("lynox_cs",6),("pickle7",381),("buffalo50",25),("mediocre_mages",3),("fuzion6614",2),("pamplemousse23",8),("rivarm",272),("steezz909",17),("csjstreams",2),("dnielmk532",12),("dudrik",10),("layan32",120),("nzrando",5),("misclickedfollow",2),("mcneesh",15),("b0babrett",2),("sashapovar1337",2),("gangster_toast",9),("kingkarry",1),("guardednivlem01",21),("greyfox___",68),("arpit13",3),("talb007",47),("the_judge_chris",71),("letmoo",4),("alekssib63",6),("komarev",83),("misssavauge",11),("iamtanator",3),("isaycawfee",4),("boax",7),("durtykansas",1),("daloopdigga",4),("monkeeeeyd",58),("shark_lizard",1),("stmtb275",116),("redwingsdk",185),("pilchtrain",128),("ikaldy",45),("loveedvardsson",27),("frstythesnowman",4),("percy_77",2),("tango1003",21),("itsbamboocha",3),("bail197",161),("smrt_no2",3),("ferda277",4),("zagunny",2),("spazog",6),("lordjoche",2),("laesk1",28),("meukku123",2),("forgivensinner92",1429),("kamelod96",3),("smellypvt",40),("richard95o",3),("ryanboti1000",13),("deamonfly7",1),("grindhardgaming",7),("kater161",1),("raiann",7),("alexbik",80),("themedicgamer1",624),("thenurkering",106),("dimski69",2),("lilxpigg",1),("specfreq",103),("firescapeone",1),("tapslap2142",45),("ever2000",10),("thesann",1),("the__wrench",4),("alexa_official",27),("jimb0",677),("borstv",4),("denizolak3",4),("peppah",13),("coldhartttv",23),("geminigone_",30),("aven1r",6),("tiniestbuckle",266),("abyss733",21),("lion67",26),("vladcst",4),("lesural",1),("mr_nachonachos",2),("lucazadekid94",19),("mcm_grand",4066),("matteasy",6),("takila21",2500),("stapleboom",22),("gambitvector",140),("stoodent_",41),("hands_mckrakken",62),("kiazius",830),("mellis",54),("looter_mcgootertv",3),("callmebootyfull",6),("duhprankster",46),("gnomeacid",113),("mindriotstitch",34),("damienthedecoy",855),("pfcwolverine",3),("scarfinger68",10),("tucinieks",661),("xrpgnick",2),("maximus_primeiv",638),("dmitriy2018wow",1),("overtymeyt",3),("purdue_fan",69),("ukr4in1an",4),("energizedx131",3),("kapylin",2),("dapps18",14),("easyacey1",16),("ruthless_deathbringer",4),("wikitclown207",3),("jazzysax82z",77),("methman",4),("luddema",3),("totallynotjones_",8074),("wtfbbqhaxtv",1),("eastpoppyfarmreaper",230),("comradeturkey",391),("dissatify",2),("mckuller_",1),("poffan",10),("thailieu",14),("paulbaz0",14),("damitch26",2028),("routpritam",4),("f1uxarn",96),("hype_survey",618),("te_one1984",9),("stalememelord",1),("thekotabear",4),("zmarti1995",9),("sokrator",22),("drunkdeadp00l",10),("yarddonky69",668),("slugv1",280),("fixfixfix",33),("fluxvibez",2),("richardlee666",10),("iblanka",147),("m0t0x",3),("rbondswe",28),("rooneywat",2),("a51sarge",3),("rickochet",22),("pyrocleptic",22),("fireontheplanet",142),("monster151082",46),("hawksimz",742),("itenkks",2611),("sspaise",1809),("officialbevvan",450),("cwhite4491",57),("ghostinc78",258),("darthbazi",1),("cocolat1x",189),("poundnground",33),("kyvany",64),("magara993",5),("rudeboy29k",10),("twigfixture",66),("anas949",25),("laf0u1n3",5),("furuyakvlt",63),("xertz",11),("lowies",640),("radioreece",3),("luki4fun_bot_master",86),("rogue_x",3),("masterterrorman",4),("hotsauce4389",15),("r4ndomcitizen",1),("kristuzzz",16),("timmy3418",1),("soulfrost00",1),("seagulltourney",693),("seyu_",36),("withmass",9),("grassroots",163),("lechiffre",2),("magic3434",10),("ari_mario",5),("nathan_the_mad",2),("peremao",11),("rrekts",7),("dahiak7",164),("tangodoc11",45),("exodoplays",331),("armixler",125),("haitanpwnz",1),("evilsk8errr",34),("mulls01",8),("egorbend365",132),("ceetee",21),("striker94",4),("splinterschism",195),("r6blitzmain",3),("allexblake1730",3),("krumble_uk",24),("gagerox117",23),("skwatbenchdeadlift",650),("ohargohawgmhwroghmwr",9),("slowcup",38),("dryskim",7),("boooooogs",7),("malekai4111",4),("theonlydroid",107),("soneekz",5),("vietnow187",18),("elonkerjuu",300),("onibiflame",9),("anixsin",6),("dirtydoorknob",689),("faithyha",2),("earthwave22",35),("golden_pwny",24),("zmk_montirovka",2),("sulli_uk",4),("auxiliarles",5),("showstopa_tv",3),("pacman5213",1),("vanarambaion",692),("drdeadlytv",32),("itskillercobra",11),("jackyboi_xd",1),("fidelissodalis",16),("frontarthur",30),("groovn",74),("prolands",11),("waitinganimal41",2),("fahnian",5),("jkmz_",92),("deadeight13",1),("iamspacetoast",1),("kurt_josef",7),("krazytoby",3),("zoowiezoo",2),("pronk14",77),("nikokacper",1),("kickassgaming2525",19),("deejayquest",583),("raayno6",11),("spono",1),("theoneinskane",198),("luckyshot6",2),("flyingvolvo",39),("coolpease",5),("fldriftshop",88),("sky_tortoise",1),("clewds",2),("kopper31",64),("0hmtastic",5),("greedypoor",5),("offyeti",3),("vranamkay",1),("nuclearfriend",7),("conor_rn",5),("vonhammerlot",1),("naughtycrumbbum",1039),("rflvh",1),("theunrefinedcanadian",3),("itswanteddd",7),("buckzoid",50),("justjordantv",1),("djvision",3),("crunchynapkins1",3),("blackuwhite",2),("hgeezy",5),("g1nst3r1",24),("nienflo",3),("typhus696",97),("langor1",2),("deadeyekiwibruh",2),("flafi1337",1),("snowm5n",1),("dapeche_",14),("studd_05",2),("samvisegg",1),("suasoria",11),("hashimthaqiiiii",6),("schumin_channel",2),("titanking5",1271),("bigbozztg",3),("badhaircut86",86),("michaelgmo",2),("h00tenat0r",31),("jakobi_raider",936),("snipingk1",10),("tekpede_gaming",3),("inoten",2),("erdincuygn",1),("mihina",83),("1911shields",3),("zethseth",84),("dezkann",10),("alexfireson",3),("rhayg",3),("extramediums",39),("deniel_g",3),("kamikaze_monk",97),("moose_juice",2),("chief_justice",3),("andrewthecrip",41),("sachner",1),("g0blin_darts",26),("c0mpan",113),("trxchanger",8),("ttmmttmmtt",111),("slavi_trifonov",19),("fixemmotorz",83),("mackanh77",18),("klutch94",587),("dalley99",1),("beginna_",22),("turtleone",5),("short_fewz",18),("gmn91",3),("stackpush",358),("dqgxtreme",2),("powerful_r6",11),("mistertag",4),("ss_fan_2006",1),("xcl4w",307),("lovekach",1),("battleaxbomber",299),("m1tch",1),("twillart",23),("grovak",2),("johntravolta24",3),("b3njqmin",146),("luzox",66),("admirallbacon",327),("freeshuhvokadew",34),("tylker14",1),("wiigomax",3),("gardner87",1),("xxx_f3lip3_xxx",1),("sir_kexx",245),("zmeinyi",4),("mand0_o",3),("playshockey",8),("fly_droop",13),("convixiontv",1),("k_steezy",6),("butterbacca36",12),("patacrepetv",3),("photojoe",14),("klauslexau",4),("rocket_bear12",1),("radisnoir",22),("badpanda",33),("sinisa2003",13),("falconnnnnnnnnnn",3),("wooly_woolridge",94),("chupacabrabra",3),("ohgeepaydroe",4),("pimpsam",102),("aweberryz",5),("m_ck10",7),("itsbrunstig",31),("amaterassoo",1),("piratejones1998",37),("nordw4cht",2),("fredraskin",1),("shuawick",1),("maximedrouart",2),("pietrosiliuszwackelmann",2),("ronaan_73",2),("skunkyskull",4),("s1mooo",11),("madjackal964",3),("betonmarc",9),("lone_chance1953",3),("greenswordx",4),("kajson",138),("setenic",1),("olihol",2),("skelet0r",7),("mayhem__ow",5085),("tribblegoggles",6),("deadelush",258),("driftingdoge18",3),("scrubstomp",1),("glitzyphoenix",128),("arakiss64",4),("dickieneedels",111),("trilis_arms",3),("alexandretaveira",1),("flornce",11),("illnessv2",3),("jumpinghobo3",1),("kemmerkaze",11),("ol3live",3),("hugh_munngus",7),("kriswahlstrom",1),("jyxr",88),("jonnyhaull",3),("konskizwiss",6),("smithnguns",8),("lnoah1278",5),("snipm90",8),("greasefart",42),("docbanana96",2),("thebanzaishow",6),("spartan8181",9),("maximusbeastmode",2),("kurtcuckbain",2),("catturdsrevenge",24),("exiborlet",1),("jappa03",18),("themacoroini",1),("ottothefat",8),("parunuts",4),("d_udas",3),("monkeydloui",59),("xarc34",340),("honchomcducket",53),("slaylivetv",607),("vvuiph",127),("deepthroattv",4),("traumlos01",1246),("thehammann98",403),("timwros666",60),("acetrip0",5),("oghilleh",2),("elicious_",3),("tartine64",2),("apomon",4),("xhamsterx",3),("efacn0641",1),("jony_zen",18),("roufiak",72),("lambda_luke",2),("boomdabah",4),("mwmwmwmwmwmwmwwmwmwmmwmwm",71),("dirtymax1893",309),("imoriginalgod",17),("boatingbuckle",10),("ba_dick",1),("daneerz",457),("palt1337",58),("celinecookiequeen",3),("dizziestapollo",1),("stianz0",5),("bossygr",3),("l0ugi",120),("geoffwill",5),("aswdo",6),("sa1ste",16),("jaydeeberty",81),("trademarkgaming",5),("ttvnebgoat1",4),("omarthebeanflckr",1),("swoma612",4),("lt_mmc",13),("alcatas13",10),("nordstrike",133),("zeligauskas",21),("j0hnyj",3),("xxmynameisjeffxx",2),("magikfingaz",1),("flippiesfloop",136),("lepizzztv",1),("astrogrump",28),("870tactical",5),("rooster762",77),("ifixueu",26),("nk_greezy",4),("felixnavidad1",575),("dj_stickyboots",14),("squiggs81",5),("ruthless1717",7),("yago_rogger",1),("old_cheeze",16),("totalkilotv",230),("zmrhacker",1),("rafa_1234",2),("tehnak",9),("moderex",8),("nastysteeze",3),("buzzibaer",88),("jburns",6),("flyerthanaladdinswhip",11),("kestrelhb",6),("danyushkahs",1),("jellypinkus",9),("noobelsuppe",1),("banditfoolish",11),("da_turtal",4),("sherman140",1),("kingjakeiv",124),("vitarellabr2",2),("theprophetbob",2),("goodmorrow52",1),("a300600st",1),("mrcudd1es",299),("ipajaro",1),("hxcrob",1),("uk_oxide",370),("sarrge_",1),("nathanname",1),("jimstarr_gangstarr",2),("leif_on_a_tree",5),("shastapeeks",1),("rigormortis_ftw",3),("cptaerotr",2),("zerglinge",107),("kc_x_crane",19),("bignastyuno",1),("kulexus69",33),("tgrif11",1),("the_goblin_gamer",345),("godwinnielourson2",30),("wolok",39),("desertfox_117",35),("bahminn",1),("redsirin",1),("dejnyx",45),("mtnrn",3),("muffindevin",10),("suspicouspineapple",2),("wazzurox",35),("kaynable",5),("snxtox",3),("rmacdakillaninety6",7),("wuigukin",3),("chelox23",1),("icemodai",1),("doomstar98",6),("thebeardedducky",2),("thehitch0",2),("lyim_",6),("ayyyyyjaden",8),("palacios2003",29),("traeshon",20),("xghxzt",2),("imseeingthings",1),("og_rowie",1),("daddiwarbuxx",3),("justincuster42",3),("fiops",30),("wolflegendxp",7),("zackwde",3),("the_legend259",7),("mykey1115",368),("dwbstreet",3),("zoeamazing",1805),("spoderthegod",3),("panorpa",22),("blanquicet13",23),("raul_dittohead",13),("goosewrestler82",182),("frxstyz",4),("dagillmanxl",157),("jennatals342",2),("kitsunecmdr",2),("thefuzzybear",2),("loccdogg258789",2),("whoady",4),("jjmcflypro33",1844),("kieko891",98),("topramdan",11),("alexkmmll",1),("benikeen",2),("lanbojones",5),("brodogoce",7),("saltybakerx",5),("younyc0rn",826),("kingsegull",9),("mrfnfantastic",145),("jsmithers",1),("joshmoo1",1),("beepie45",83),("vip3rlove",6),("concretewarrior43",4),("28down",15),("landtail",158),("fulldive",1),("bilsantu",1),("lerkylerkerton",5441),("rama53",6),("z_rozay",1),("du3lmaster",6),("dirty_aary",121),("davinchee",101),("faintu",73),("onlybige1978",8),("some_asian_teen",7),("mygamelongname",11),("utrmusic14",146),("donkr4w4llo",3),("emmenaren",17),("aquanim",6),("vaashark",33),("mtgameplayslive",6),("esheetsa",1),("sorrora",330),("jjrfin",2),("whyudribblesomuch",1),("no_maddy_snowb1",1),("danziebwoii",303),("tomekkk23223",3),("axeacdc22",4),("h0xt0nn",11),("gunjaazz",9),("frogsporn",4),("droneuk",13),("secluded_memory",1),("peres_420",2),("bren02000",2),("kantushd",1),("crinjnz",2),("carlsimmer",6),("nixhextv",84),("gabrieldav3",2),("sagenhaftein",28),("mastahswizzle",5),("dw1nne",4),("games8gaming",27),("steveo921",3),("jm0251",1),("tu_amigo_lech",224),("f2pspy",20),("kariminal_",7),("thelogiebear91",5),("kamanah",6),("mynewlife61",1),("bazookaboy_",3),("cptlarson",2),("amzpatrick",1),("valken927",7),("indueer",802),("medezinpumper",1),("rotmgal",4),("loki1202",4865),("eviltomi",7),("simvu",90),("j4kup",1),("ausdrunktv",1),("piesank",4),("pchoss67",15),("vasvas",2),("lulult07",5),("prophet_fmj",7),("st_hasbeen",13),("tomcat_hlg",100),("click977",21),("apple_wookiee",31),("ayeyodope",9),("ljustdontcare",1),("friendlysavage",5),("umapessoa222",1),("zim771",4),("hilldog000",1),("wizarsy",4),("csbarton",12),("s1mp1icity",48),("easynickg",1),("drimk3n",4),("hotelbravosix",5),("prophdawg",10),("dam1lkmann",10),("melo600rr",32),("gafavv",2),("fevdex",5),("avaska1",7),("box230",32),("shroomprophet",76),("naruto999",7),("grayw01f",3),("astroxcam",5),("smegmerino",3),("marlonjq0",1),("sondre_kol",9),("tyleur",11),("budgeuk01",209),("starleydarti",3),("georgetogo",8),("kilkil912",189),("mr_dinkel",1),("multibillyonaire",37),("0x4c554b49",297),("dominanttv",43),("veritas",62),("fleipi",4),("manaa_hamza",3),("animalmother67",31),("cmck052",15),("easyse94",1),("nine_tall_in_in_der",16),("604razor",4),("bignose345",7),("ofuryx",4),("enbandit",4),("xyonfps",1),("koukee",1),("dersohnemann13",17),("imacaz",24),("legu4n",26),("triteassassin",50),("ardubelu",47),("creativerealms",2),("chromolyttv",8),("paytnt89",298),("watch_here_fellaas",1),("xyntt",3),("jesusraids",4),("yasir_ayad",5),("realklaytv",9),("ashmanmedia",2),("chasethace",11),("iqngfs",3),("delbaeths",25),("neznamyclovek1",1),("plague_panther",1),("jwc61290",11),("broomy_1988",5),("kiericograndao",96),("hd_jehu",18),("iloveyou1000000time",2),("griffistoocool2013",6),("xassassintv",2),("fistilly",1),("muratemre551",1),("blacknight2u",3),("shaim",261),("zolyhd",5),("lawrence5422",3),("omnixiaa",2),("mohamad0078007",7),("wh1tehorse_uk",80),("yd_miami",8),("vverlingo1",2),("djacks78",24),("blobtheman",4),("iamsaucyv",2),("lostcuaz",83),("n4jdovski",1),("itskatsuragi",15),("cartmanzo",4),("dutchneon",5),("infinityg_tv",1),("dead_man001",10),("mazokksosok",2),("ihopeyourehappynow",2),("mbregy698",41),("paracro",2),("rebleman619",23),("burntoaster001",3231),("taloxxhd",13),("hellzzspawn",5),("i_chemical_e",21),("mumble_murmur",2),("whoisjaneeq",1),("destroyer29",7),("ztoys4k",27),("magrorob1066",5),("rocisodin",5),("austinjones453",2),("toxicpanda201",1),("air2parsely12",44),("hellsxxdemonxx",5),("eiyore",72),("antebb",1),("gepariano",2),("vallin_",6),("waytogo",10),("biyatiful",4),("differencia",8),("hentaifolders",222),("wrthless",5),("adamthc",1),("orangequince",1),("smeltextv",11),("rustyninja84",99),("monsterboy5011",5),("flint_sketch",73),("oxboro",2),("nozomikosan",18),("ser_joga",1),("bussinnutsbaby",1),("holy_miasma",2),("forresthardcastle",25),("dr_strangelove961",7),("werndawg",2),("dronesvii",78),("ghost_90201",4),("hobod0",106),("jackyace",5),("jibskeo",143),("kayyyyy__",93),("pinoypaulo",58),("rfr_marv",28),("swolobro",750),("xliquiddream",7),("squirrelsmtb",1110),("littlesusie01",773),("tel3ma",4),("rogueangel_1",1099),("raptorprophet",113),("x8th",1),("curtis_stryker",6),("jadealex26",125),("kyle_fulton",4),("tehkis",49),("xpinktwink",1),("basic_chick",52),("bhedges419",6),("redn3ckrampage",2673),("realwasabi",68),("blikzxscorozx",9),("10ks",1),("cid380",647),("spqrforever",369),("revoeighty9",1),("mozzbito",17),("dareyck",2),("jambi7",4),("wearenopros",1),("guyliguyli",15),("quickstryk",28),("winchester7337",1582),("macbullet07",4),("ollieee754",225),("iamdrakey",6),("starbound987",6),("gizmogiant",2),("ville_jkjk",37),("chrispurpngold",3),("fame972",2),("ripper203",62),("souptime_live",7),("sirsnipes9507",13),("cptsmite",30),("alonestargazer",11),("th3_j0k3r2",156),("thefaltynapkn",3),("miamionyx",1),("swabo",20),("jacob_spurgeon",31),("mavado_main",51),("savagejam3s",3),("fancydeathtrap",76),("psychofrost99",9),("the_cmak",255),("dimali1",1),("jasper596344",2),("dottyhack",142),("batmansrage",16),("cls_poison",607),("crustyplunger",25),("crypticchrome",75),("fldude10",1),("glhftehshow",27),("rascalsanchor13",26),("technowens",8),("zorinnn",3),("burlykoala__tv",516),("llcoolguy117",5),("sadcatloaf",954),("theoppositiongaming",2),("tropikuhl55",28),("myturbansdirty",10),("soulcaliber9",50),("sonofwrath11",8),("the1smithy",319),("nathanualsatchul",1),("nihamat",186),("aviciouswalrus",1),("generalgrigori",38),("togzilla",31),("archemoss",35),("thelocalman",28),("theshrew",1),("wybo44",11),("cjanes210",7),("mariozb90",2),("demon_k4153r",3),("taw_nyyfan",19),("rook_kail",11),("karperky2k",3),("embrace_the_glow",9),("luckyohducky",4),("codenamenoobreborn",9),("djerniss",13),("somerandomechap",104),("chessmatch",9),("jusstinn_ttv",6),("kattekaren",526),("looperdog45",146),("knowledgex2",7),("letsdunksomekids",8),("hayzi67",4),("bruuhzerker",1772),("deadlochettv",36),("awwezz",3),("vovintv",1),("cylonicglow",1),("biirdyturdii",1),("galanuth",1),("dutch_tuchd",98),("playingboi",24),("pathogensalad",1),("sirwanka1ot",2),("thegurns",98),("wifiv",37),("mixal",1),("do_or_die_tv",2),("rumarks",1),("not_a_gril_",2),("d_apps",10),("toxic260",4),("2far_",2),("morj2k",2),("ptrictv",1),("gamenastix",2),("exheart888",1),("zedsan82",27),("jwinkz_",169),("gr1mshadows",11),("mizsa7",296),("lexren253",319),("tylerried",38),("fyzzlive",2),("aaronitmar",7),("bobby_bigplays",22),("mat_error404",75),("gram667",2),("angrytoast195",41),("sawbon3z",3),("nickypo0",55),("legendary_kp",2),("artsmah",2),("coconutsaleseo",5),("raptorzm",1),("parallelocat",2),("petricottontail",4),("binary_dragon",109),("nightcore_is_bae",22),("la_sale_bete",2),("cogwhistle",1412),("johndang123",2),("ajsp1401",10),("drummerxdan",2),("ghostinthestram",911),("reklaw01",3),("marcusowreckus",5),("squidalius",70),("oldive",2),("brenno_g",2),("cussyou",215),("freemannr1",2),("mordhorst1",289),("sneakst3r",2),("tar_aldaron",166),("strongbox",133),("blackbird_z",10),("tantobigpentv",3),("gebratenerhund",40),("z0mbiest0mp",12),("darkroastlove",3),("foehammer47",2),("rafaamhs",3),("buckshot_92",2),("marvelmanv3",39),("caleeese",430),("r0b1089",157),("lukeo__o",1),("johnstorm1414",308),("wazuka3",1),("neder9",52),("bluntednitely",92),("polkafinkle",7),("rowa",6),("manu_134",61),("mrl0ner",5),("thesalatus",4),("randlous",4),("jimmiegirig",26),("solaraeona",4),("barenski11",250),("nalkara",2),("aeva4eva",5),("simmermay",567),("thatalexandra",2),("bres14",3),("athyter",23),("kappamutton",2),("wincewind",24),("log657",23),("tenegros",1),("pos3idonplays",3),("7102753235924733293627729",53),("flowerboy900",56),("joytries",21),("momosushi_domo",1305),("ravenquill99",1208),("thebakebones",151),("legion_king1",382),("stroppierdrop84",3),("elipod",5),("rigalika",1),("pharho17",8),("lexatris",24),("iwbo",8),("hoosierdaddy0827",3),("octaviusdamn",13),("evancheddars",2),("retrixtv",2),("nickyballs",1),("ori_______",174),("woodooka",5),("milo884",73),("morgieuk",20),("whiskkeyjack",8),("dontbecreative",2),("mitka12",35),("calebp231",63),("thesurvivor65",1464),("grizz",6),("wikid0ne",1),("darksoulsspielen",4),("pootiswiard",141),("kivlor",1),("mlio",5),("curson",1),("random_gaming_uk",1),("fyreeeeeeee",1),("jensenmiss",35),("kajnake",14),("laf21",1769),("limitingwalnut",6),("i_abuse_aftershock_liss",4),("kidvette58",1463),("lestur",638),("p0n10",93),("pwnguin_pma",997),("zeedius",8),("tmctheshow",199),("ecomoly",131),("hyperevo23",1),("ignitegamers22",1092),("moosenz42",895),("ti_taniumranger",27),("dadestroyerrb",2),("bwucewillis",108),("mrmodules",2),("pooyasp2",4),("juster1c",4),("twisted__fister",372),("shutan127",15),("curbuser",14),("globalerman",1),("kinglebronze",11),("sajuuky",2),("r99azm",1421),("melchi0",169),("beeceemato",3),("pregi01",254),("potbellytv",1),("fjz69",1),("botap",10),("tisselito",37),("cmdr_frantic",5),("tytanoox",77),("lisa_fra_norge1998",7),("gamb_pt",16),("thedailyvuel",1470),("hzodi",318),("booyamond",2),("jagholin",1),("eldorne",1),("ignusd",85),("limitbreaker89",2),("gumball12385",104),("munkie_munkie",243),("dhoomsday",1),("i_na_bani_powtorz",4),("sigtill",2),("papa_niv",4),("zzzdrago",2),("beard2g",2),("e_sleek",88),("jacob_slovakia",22),("bonepack1135",1),("sync_nine",11),("tyrany",63),("kaufy79",35),("anarcraft111",735),("julebrus246",4),("brombaer",2),("dexiar",1),("killface02",214),("acidkiss",5),("hell_horse",2),("hristgunnr",1),("yaaboibluee",2),("dread72",11),("omnipotentdivinity",18),("stooshbatis",62),("mslula",1),("sleepiejawa",3),("jaecee",2),("luvemil",1),("forthesakeofwatching",1),("pinkwraith",6),("yasdo",3),("yoyo5142",1),("teleblaster18",141),("brinkwou19",2),("dana_jessica_booth",2),("arnnhtv",1),("smashtro",54),("sudzzzz",607),("masterniles",4),("poopyjohn_",4),("mercdawg",6),("such_roguery",303),("zebra_bravo",33),("wesmoulder",1),("houseob",1),("kamil_kool_tv",2),("migz90",4),("lime_chill",562),("xuxunn",11),("teletrele",5),("hypervideogames",272),("k0ntraband",2),("hyoketsu50",10),("spartanstu23",268),("pudge_",15),("aypahyo",136),("roshambotv",1),("katnipnap",8),("yunusaltar",39),("dinho_lima",1),("misterstell",32),("satalina03",4),("bustincider",6),("iamfivebears",3),("mucky88",18),("geeeeoffff",10),("david101089",93),("mrvanillahorse",11),("suejak",386),("marnic101",1),("lebitumeavecuneplume",10),("ariesflavian",2),("zensther",3),("dst_dark",20),("staticcling813",5),("brombeer21",14),("hellblazer_101",23),("its_trevss",4),("tweak",4),("aloofball",6),("rlpuppet",1),("whitelme",12),("bagoono",1),("eliasscp",1),("thefatalblack",1),("konverts",91),("estizzeled",1),("eljukes",122),("tesarul",13),("ohioalumni",1),("jaiminhoh",224),("ethicmeta",1),("fumil",3),("hexdtq",1),("stoneycash",2),("soiostats",2),("h4xx0rl33t",113),("forgottenone86",1),("brutus0791",2),("ethnianmandarin",1),("almiage",4),("fantasyline",112),("epicelric",37),("livewithguts",5),("arkoudas",5),("starbajt",6),("rinmaroto",98),("captain__420",13),("tbigonnesse",104),("bobdobbs97",5),("swormzy",1),("arakeel19",1),("craftingtwins2",13),("muciboci",6),("technics",3),("rediskapro",1),("sapiensev",2),("selutiel",283),("alig1096",1),("arsinik13",12),("real_kloudeee",15),("sawasar",3),("ellisthesemidecent",14),("vo1lander",3),("mr_fields37",277),("shed6383",3),("smalz95",5),("alpakatalp",8),("fishyy_tv",1),("thebattykoda",1),("kei_shin",2),("miraclegus",238),("thrashandbash",1),("celebeorn",155),("mrmuffin225",86),("anonas__",1),("neodym123",43),("saintlavoi",10),("akghsun82",86),("island167",58),("kophnic",2),("jadedea",26),("admirallowkey",2),("doufmu",132),("sharkfacekiller",10),("stichlor",1),("plakkerigeplaksnor",1),("numberland",4),("paul_johnson_24_7",26),("danielveres",1),("johnblah",4),("uberpoundcake",2),("deanofbabyy",106),("al_bravo",6),("wasserhine",2),("radiochickenshow",2),("thetrigge",5),("tiphias",127),("shintoxtv",1),("boostau",1),("l0cc_com",3),("zar138",33),("oo_thebizz",209),("cursedbeblessed",6),("c1aw",4),("grebio",3),("blazedhobo",59),("macky509",2),("smokie_tv",1),("rickrolled",1),("justcarlsfine",1),("boiplayzhd",82),("kennystarfighterbolliboli",535),("xpertshooter629",2),("papa_bear6",23),("thatguy616",48),("liviunken",4),("xaxeror",1),("sevroaubarca86",14),("beetle1682",3),("jumpsea0505",25),("melydron_",11),("ryplex",1),("seeingeyedog",2),("mirelyght",12),("moreaziel",130),("josiahh",346),("silpumpp",2),("kody_schneider",1),("w081947",1),("egorry",1),("wiskaz5555",31),("frapless",2),("taburet2",2),("xroostersteelx",1),("krimney",1),("kanamono",151),("nicetoeatyou",1),("mrsh1v91",23),("blluga",1),("brakecz100",3),("wormwoodsc2",1),("shalp",5),("trever000",12),("bubbafrostxl",187),("gptochallenger",9),("jayysonastyy11",6),("ldardeen",2),("paulaflight",4),("blacksockz",3),("dave0fd00m",8),("pecoes",4),("unixchat",1381),("amassofsheep",141),("dogouz",1),("kamulis",1),("famineogre",3),("eckssquared",1),("fluxcupasit0r",19),("gobz0rnator",2),("darkmoonhowler12",8),("eventie",9),("effectzviii",10),("communityshowcase",5),("templaroflight",4),("bilgehan06",1),("viscidicyt",3),("diogenes_k9",3),("fightright86",6),("damnfinemule",39),("magiak",13),("nekufer",5),("svinto007",1),("kewingb",59),("baraanton",1),("snowed1",9),("wong13452",66),("antworks75",1),("fancytables",743),("serendipity42",7),("flagshipfail",6),("miiloowww",3),("madhattercross",1),("dastonar",70),("first_lt_audie_murphy",11),("redstrot97",2),("quwawadota",2),("gargahgg",92),("jisary",1069),("chase_norfleet",1),("grandekike",1),("skadithehealer",2),("tomatosoup898",3),("justgetbent",5),("mormorion",1),("sethos1985",1),("adultsonlyhotclip",1),("skr_tv",4),("mimlisse",2),("xxtechpriestofmarsxx",18),("drcoldknees",8),("reisender200",2),("snobjorn_",1),("rainbumbs",25),("jojotopaz",3),("axmantim",6),("thunderclastgaming",37),("xspitfirehdx",301),("jercohn7",175),("lands37",5),("smukat",1),("karvax",1),("amonte_10",36),("oriogs",1),("thestones2312",119),("threeman",142),("burnbro3021",102),("krovic",5),("bubbla",6),("zenvogaming",1),("finsterrode",5),("hellvortv",1),("trent_usg",1),("mrspudatoehead",1),("sqiao",10),("notgreen1",120),("accurator2",17),("mamadoudu678",14),("nubatack",134),("that_geek_kid",59),("noobishthekittten",13),("max_6",1),("judasgoatt",2),("freedom8339",7),("ultimatekctv",5),("heavenlywaytodie",1),("thezxspectrum",120),("iambigboy",90),("barcodexe",1),("toxickiwis3",3),("lajttwot",3),("qqtenko",1),("rendea",44),("mrchozonomad",32),("baccano1991",3),("leffosties",2),("c00ckiez0r",2),("daffit82",2),("rico379",15),("bannanafarm410",1),("mrmiekal",4),("drdoka0",2),("blakvenom",93),("lwm_",135),("bluegoose81",2),("evescout",7),("ocianic8",68),("nathanielplaygame",1),("mognunie",7),("casper60dk",29),("satanssupervisor",31),("sneakyado",859),("alexn006",10),("horriblegameplay",602),("dissolution09",128),("uninvited_dream",43),("byflp",655),("epicgamingguru",1),("yazms",124),("ws_minion",135),("lashawne",1),("rainingmadness",22),("kreten_",558),("allar222",9),("m0xyys_voicecrack",482),("togut80",1),("skippyfire",156),("vivid_kill3r",8),("candid",3),("hakiki",214),("divinekos",5),("crazymonkiegaming",229),("tleverette",203),("warpedwoodworker",20),("blackhoodtv",8),("theoneandonlyking",37),("zeroatlas1",4),("thesilentdeviant",533),("sammichgaming",1),("yakuv561",94),("myst7555",75),("shahr00z",1),("gcbarker91",20),("ducklepoo",3),("spyderbilt856",3),("rolexed1971",357),("colamanh1",4),("swartzyck",1),("waffles__yo",4),("whodarezwinz44",4),("thatgingerkid",206),("alphasabre",1),("saxonsays",4),("lewknation",54),("mattsmith3687",32),("mohammadrezabg",5),("vtoledo",3),("gin_berry",24),("seamustheebear",11),("papa_dude",164),("bananennanen",10420),("frostymcmuggins",185),("ellis1996",1),("badfriendsgaming",23),("socialblade",11),("supmessa",11),("miplydier121",11),("dwardu2255",14),("hardlight1234",2),("reaperhouse",182),("striderian",1),("odie32406",88),("sapiinn",9),("cakesstation",3),("vikingtylor",123),("riddrick",2),("braxman1000",16),("murderalizer231",4),("datasseur",3),("x_alexop_x",7),("havoc_7i9",1),("totoafrikav2",25),("budman782",12),("crazy_vexed",5),("theolejibbs",31),("camokodiak",1),("stutterking420",8),("vikingbear89",18),("akers_",262),("goody_seven",275),("smudgerstv",3),("dahves",13),("kaptainkayy",242),("stalzer",229),("neoardor",3),("xmaxpayne2504x",17),("stealth930",3),("kreten",1045),("tuler1234567891",72),("zulqarnain_ali",3),("biscy_311",5),("andysatar",1),("merrox",2),("nobodytinypaws",5),("frankdroosevelt",3),("roblox_thiccgirl",29),("vortex_solaris",7),("poiseddragoon",5),("mrtwister27",57),("terrorweapon1945",69),("jwatss",1),("tevieskam",3),("mariquezbro",3),("ibrahim_1993",21),("taisnation",240),("pafkin",1),("sinuousprince18",247),("bohnik",112),("dolsolm4",12),("darkintensions",4),("dduke86",1),("mothersupreme",16),("elper4",3),("cucumbahunter",5),("hankie345",1),("jarmonk",17),("pointbreak101",6),("tzhop",1),("kyler2397",3),("pizo",25),("shnickerslive",14),("thereald1lan",2),("zinc_fn",4),("2rrd",1),("kinkstou",3),("streetz_profit",1),("hogs_yt",5),("raxdj33",5),("battlefield1966",1),("jin118",7),("neddeppat",8),("imlosttoo",32),("rickwilson33",48),("timeallotted",18),("diy_nerd",2),("willmack",29),("zamanbekcisi",4),("knockoutdaze",1),("populy08",84),("tbonewazoo",2),("bellcrozz",1),("simon_j_ryan",2),("cookieman69328",1),("yessret",2),("redjay914",6),("lupus_rubrum",1),("richie_b_",23),("aspeennx",59),("jpass277",2),("pitrick1",8),("l3eeb_2",1),("eczko1337",1),("bezzzpaleva0",1),("boredpandaguy",87),("hannibal1948",2),("serefin71",42),("vemi",2),("rosemurgey85",2),("joebye47",1),("bwags_gaming",2),("yepski2x",46),("oovahh",9),("qpinecone",29),("hypothet1c_",28),("weirdpotato789xyz",11),("cdub9653",11),("connoa1228",4),("urbashka",8),("gift_box_w_l_d",123),("kingstuugietv",13),("xmike3",2),("jaywub3",4),("bohani",5),("jawbrey69",1),("duffster8834",68),("jobi_essen",2319),("mia3503809",4),("saaiber100",1),("rickdane1",3),("bs4fun12",10),("spykeb",1),("13912488518",2),("splash1029",2),("xsofloow",2),("thatrljunior",2),("gemini4667",5),("mizley_2x",5),("mmmaaxxx",2),("zshinobii",1),("iuro197910",1),("saulaga",1),("arm1272",1),("arturnikitas1",1),("chaotic_lemonzz",2),("josh18889",1),("sunbrokuu",1),("bu7ch1",2),("neubie9",85),("reinaldi29",1),("mralladynn",2),("reaper1979mo",2),("nicktamerqc",2),("trebak317",16),("ivoryforsaken",112),("pittsburgh2400",4),("xtiger4ever1980x",1),("bondhawk07",3),("misterh8er",2),("bluefox3311",1),("sknowlt",6),("william881207",5),("ikeaboy83",6),("morphineknifer",3),("jansoe",6),("whos_magic_man",9),("prunte",1),("garythfla",3),("vzaxxonw",3),("xxdan23xx",244),("shor7ey",18),("tarcynic",1),("thesurvivingpr0phet",223),("qqqqqqd3qqqqqqq",1),("impact524",11),("prettiigem",10),("imbaemax",5),("hvacgodd",10),("crimsonhorizon89",1),("blockingisoptional",3),("xdynamiics98x",6),("wxldy",1),("xyphyrlive",6),("king_young_2",2),("hadestom63",1),("ticriss_02",1),("gxd_qwan",2),("centeredbirch86",22),("shack7777",2),("mystic_melon",1),("carlos_killz_",2),("gunnerw",1),("dbe451",3),("clb_gamin",1),("digestedcobra12",3),("lfreire80",7),("mackleangelo",1),("nyckid80",1),("ttvchromolyttvbtwttv",2),("omlaash",2),("deathdrawn",1),("noskythroutarget",2),("ummsenna",1),("sslyzzer",1),("drunkbritishgamer",118),("igi_pl",1),("tuogalen7",27),("fan565989",2),("dathuil",72),("mateusgyn360",410),("stay_drew",4),("v1jay",1),("scu4444",1),("sil600",1),("greezyii",35),("lord_of_tempests",1),("watchdoomsack",2),("killick_tv",1),("nadeoy",3),("hiraiki",6),("kapitan2400",1),("chinonegru",7),("memdial",7),("thebiglost1",4),("ko_zenker",2),("jbonkerz",1),("2003pj",1),("bigfoot_456",11),("travis_t2004",395),("x__xspeedyx__x",2),("kumaair",2),("imperator_davius",29),("vic_likadabooty",15),("zedstein",4),("cwtlindroos",95),("malli92",10),("mezzfit",1364),("sadcroft",1),("crewcutchuck",74),("bansheebe",3),("thedewk87",327),("dolcekavanna",9),("xodius",126),("player_steven_",22),("bryan_gold",378),("faeroice",6),("shaken_bot",18),("xlojz",10),("ferris_bueller7",1),("gyorpb",1),("bellsward",1),("mrritzcraka",306),("zztags",1),("phantomhawk88",5),("tswiggs",1),("stevensydan",1),("brycekiller1",1952),("virtenebris",68),("nateg78",2),("krotik_tv",1),("m4inter",1),("xivladon",2),("foxbird",2),("kennystatss",1),("teckley",3),("dapsonem",69),("mic_killz",7),("liit_man",1),("sounlee",301),("kinginamg",3),("chewydogco",997),("ryanissalty",15),("exshow4478",35),("orbital_blitz",3),("danfrommcdonalds",42),("nuttycottentail",5),("piratazephyri",7),("theluluru22",16),("aboutnineninjas",3),("qgenoq",2),("cerven_",12),("1ronman28",1),("supadonut",206),("schafer1",2),("dragondese",90),("zautin",21),("outfits_",1),("wannnoxxx",1),("martiniv",11),("dukeofdreads",145),("silentzim1",81),("kcentric2",2),("darkness_payed",30),("neal_w",43),("bozeratz",24),("bullbaz",374),("damascus_404",1),("x0nerater",6),("eubyt",212),("tschissl",6),("archlord_hellmare",16),("meikaghost",6),("nizaer",1),("sb10",8),("flattailed_fox",142),("rosethorn91",17),("showtimeix",434),("silentpoacher",7),("patronbullets51",3),("havikzzzz",1),("twiggehtv",20),("manualblub",2),("manyoustupid28",1),("ppn_iimerk",1),("rockerfella99",8),("mrmadnessmd",101),("uxm_legacy",50),("reglius0192",37),("aminorerror",77),("theildeath",4),("breezyce",1),("ianvonr",5),("yaboibuns",32),("kotaharrison",48),("meowskis666",2),("smile_aa",14),("gambit688",14),("milkdudtheafrican",2),("superespen99",2),("t_cyrus_t",1),("bbksniper007",1),("mehranfreeman",1),("jackjohson7878",1),("axelrod__09",2),("tonygabagool",2),("i_dmar_",49),("mohsen8hzh",5),("featllama690",496),("prellium",902),("teyyd",6894),("niixten",1),("colonelmeatball87",1381),("murwaz",267),("gamerbro513",1),("nstysnks",3),("joshuam5635",5),("lukask_is_gay",77),("tillbe",78),("fantikgaming",1),("killersheep82",2),("styxwr",1),("mypmypbi4",13),("dgrayman627",4),("dallasone",2),("doublel2005",1),("valenskilol",4),("pr__conquer",1),("mrsandman710",42),("9o29",4),("chernoalpha9",7),("leviosuhduu",1),("nachofc18",1),("firewolf3235",3),("fearmenow123",5),("hubblybubbly",3),("ccripper",21),("deegaming",5),("rufus_stein",2),("goldfalconr6",5),("luxidyy",3),("shotta360",224),("uk_djc",1),("tinytank03",21),("mfbeast64",16),("mohamadmk0",3),("artworks4free",6029),("chevyisbasik",10),("piotrbati",1),("c1sta",1),("appezz_higgi",2),("im_jeffery2018",2),("andreselpro75510xd",49),("chile0714",3),("bakedpotato408",3),("kyojin_hunter",2),("mau1c2rr",3),("terramaniaco",2),("reiis_7",1),("menogamerjamshidimoha",6),("helghansoldier92",54),("drevmhvze",4),("blackinsanity",2),("bloodhound922",801),("reaper_nades",6),("hrodz18",53),("strangish",5),("u_make_me_sad",2),("austinrod12",4),("eatchizen",1),("ultragman",363),("hastygoldfish9",100),("reaper4467",1),("hottoxic97",7),("hana_5",1),("redrumdom",3),("kirkwood_tv",3),("i_que_gaming",105),("heemhonkho",10),("dethfunk",2),("jebus7",4),("zilleplays",6),("gussyoce",2),("burrito_incognito",149),("benzo_chvz",9),("kenjosun",11),("stiffs",6),("kodysoch",15),("frenchie908",1),("singlewhitemale",1),("xsyy",2),("buzzer94",10),("coldpete_",1),("sin_nemiz",1),("tacticalmonkey556",3),("snappyinteltv",2),("mrsg07",2),("clean1p",39),("madderred75",3),("xkeyy10k",1),("513kernal",277),("ametrine91",372),("dabsorslabs",347),("dantanius",629),("goomes",172),("greyscalez",45),("imfamousjay7",182),("joxoner",2),("mikeyroses",300),("mrken_seikimaru",24),("mrsamuel",1),("oddbawz",45),("quincychaney",31),("shtickinabox",1),("sliddjuret",14),("squably_",24),("thomas___o_o",629),("wp_brujah",626),("estock",81),("sketchyninja",3),("bradg1991",8),("queenpimpin",44),("kofetta",21),("zondervaughn",3),("dexterzonehd",10),("zenusas69",83),("imperiumrum",8),("troparium",1),("vicki988",4),("lovelyqueenxo",2),("professorenglish",175),("fury380",1),("doomraider29",3),("nightmarishtrap",2),("flyykiidtay",2),("qkstmdhqk",1),("slint3",3),("mr_cochlear",8),("playdmb",2),("peppe5995",2),("linshaw33",1),("hahaitsvoid",1),("acdcrulz40",9),("oongachaka",4),("simplafyedgames",61),("juanluis310",2),("keratukos",10),("lohyewleong",1),("cepteris",5),("hymexgame",4),("chew_85",4),("enanozlt",2),("turrufu",5),("geekyear",48),("the_unknown_501",6),("dyinglightexclusives",2),("saskema93",106),("urmomsd4d",6),("deathsongx3",1),("savasrpy",19),("koen_69",2),("chimblz",2),("darkkos991",3),("imamcream96",1),("fardeg",128),("zleysix",3),("superzbros",1),("dr_mupi",46),("thedonredphish",2),("mrbotin",1),("nightrider526",2),("koray_23",2),("victormichelin",2),("tucktuckontwitch",84),("lwb2909",6),("zretixz",1),("thatgirlxx",6),("deadguytwitch",12),("xgamerider",3),("haj0",2),("kapo852",4),("gamergalrxg",2),("kingflores2468",4),("under_donats",41),("hopsin_",22),("xmlgxblitzxmlgx",12),("professorpygpyg",6),("theturtleman90",1),("sandrat214",44),("jestemharyy",3),("llturanll",9),("zotethemightyy",13),("bufflay",2),("jarcpty",2),("traphousebrunch",14),("brittany45",129),("arg12kill",7),("marissa_p",48),("kosmickayles",1),("saky53",2),("mujji650",3),("rodge2550",9),("israeln510",3),("mr_floki_",3),("xarll",3),("laptopfreak21",4),("shahin_quick",40),("ganadara000",19),("jazmindavellhawkins",7),("miguelmolina115",2),("silvershroud113",11),("teischente",131),("mizsanator",84),("paer_nuder",194),("sjttv",4),("galladriell",2),("aetifys",27),("clockjpg",30),("hiitsdnle",1),("dedsecgamer",2),("lonewolf489",1),("perplera",3),("lucaval",5),("stopthattime",13),("cardboardworm",1),("toby655",2),("simfeu",27),("dabgren",2),("shummers",14),("traydog87",3),("olivierdevos",3),("joshstol0329",25),("fogtv",3),("kawboi",12),("hor0ss",1),("jkhahaha20",7),("dyinglightgamer1",10),("tkoenneker",17),("opollux",2),("camboy0313",3),("duhhst",81),("skizordrone",10),("dylanzorn",4),("chrispkr",2),("ghostmonkeytoy",1),("lowspeed_us",6),("mellow_guru",32),("tiger41155154545",2),("brewwey37",67),("carlthecrazy",67),("goblin_juggler",66),("iviiras_reaper",232),("da_jackster",65),("gamingalienist",55),("swartz",5),("amberleex",144),("youwatchoscar",6),("steadymceddy",1),("volcomspade",2),("leaymur",2),("taipiesenpai",284),("jakejarkipaa_",1),("dukedigglerr",2),("johndabomb7",2),("captain_ragtag",2),("timmmmmey",2),("nads_mckenzie",26),("anonymouspope",22),("vilkas91",2),("gibbs570",1),("nekhrod",8),("bostongirl22",3),("motakias",3),("emidiolara6",7),("moony12348",1),("asourchickennugget",2),("faotehwolfo",4),("vashnare",122),("melissaara",1),("gonzalitto10",3),("cytue",10),("xmagpie76x",1),("drgnslayer21",108),("kanakorsa",7),("zammmbiie",285),("rossythelegend04",42),("kidkino9",1),("mmoore222",79),("eion_11",39),("bakavaporwaveasuka",10),("that_awesome_monkey",22),("agilao2",8),("thaprofesza101",2),("cynosas_",1),("stolich27",1),("peacetoallpeople",30),("jjy0098",1),("linkbotw22",26),("kittysennpiie",2),("ur_breath_stank",17),("futuretype1",108),("professorprostate",27),("ant2277",22),("xmartezy82x",3),("guichozeroone",10),("eraman",1),("jokeswithdad",1),("noxiousomen",40),("euadley",2),("clutchg0ds",16),("40kragegamer",1),("djyoungboy777",4),("torgunbjer",1),("tristen002",4),("dreams_05",35),("gman21227",3),("errursorry",4),("eeebbsataaaa",3),("hammym",1),("wallietj",1),("dragonflame115",5),("souphead74",16),("gunslinger30",12),("gowtham22",10),("vyceb",7),("0_prosaucex",133),("kobatk",2),("8infinityswitch",3),("kodiakgamez",9),("youngdirtydank",1),("jadspicko",15),("xxdudeson93xx",4),("mmunnell",23),("doggoblex",50),("srg_duncan",37),("exciting1y",21),("danny72119",2),("freezy131",1),("thaznricebowl",1),("koatoa",1),("virsago2099",70),("blazingstar8",3),("ggr3cco",20),("veryrarelmao",6),("gloft",49),("zarzoupower",10),("gummyworm870",26),("the_z_gaming",22),("patriotda",120),("x0mbigrl",54),("gweekgangx",5),("pawciopolskapl",6),("rickdaverdadee",2),("leokiller990",1),("bentley1313",3),("bgabrielr22",9),("gogiba",1),("moor1ch",1),("sloppykoala",3),("aceup_thesleeve",1),("babsytv",1),("urworstnightmare6912",1),("datway_zayy",2),("fluffypenguin808",2),("killer_of_oz",95),("deathwind21",1),("robert_the_great",3),("rosie2au",12),("sam034_",3),("venomousduck7",13),("jgazmom",323),("yaboitofu112",1),("arsena",60),("frankbilders",27),("humblegamer87",4),("virtualdrugaddict",32),("bigbadmariwolf",2),("disciple__og",1),("george_lukass",1),("flux101cdn",18),("chachin503",9),("ohhalphaa",1),("raidenex98",47),("artoriaus",252),("raidenex45",4),("astalkingbot",245),("arthursarmi",1),("hanepotato",164),("plundererhijack",191),("joesdailygames",2),("angerthosenear",1091),("karmielopez",1),("5pec",1),("jesskimosa",3),("spacehaunt3r",133),("x1bray",2),("recapmetv",4),("rektum420",8),("bee_tch",3),("bossome101_official",30),("sludgewolf_",1),("frezan",16),("mrmeiio",720),("er1k_j",139),("spiiiid",2),("tehmorag",2),("mattjams666",2),("goggz",1),("stockholm_sweden",46),("wes305rod",55),("jixjixi",2),("mr_panko_crumb",3),("egraa",2),("fender21",1),("hypeocracy",1),("happydotz",268),("justicej",1),("marksmenship",161),("nadecook",2),("izzysask",17),("duhsty_",1),("marebearthecarebear",33),("lolibandit",29),("uhcanigetuh",7),("horsepower206",3),("boargutstheimp",2),("xsinpaiix",3),("mr_valkyrial",4),("lzdz2",97),("nofufuz",2),("iviiixv",1),("ohdudesoimba",3),("double_division",5),("spazzticturtle",12),("itsmyrevivee",4),("entitygv",3),("kajo82",35),("we_one",3),("atreyu__",2),("nz_bowtie",1),("dinkandginas",2),("thanatosixu",93),("ohminai",3),("jrblazer",47),("zampoukos",1),("sobczyszyn",18),("burritobytes",1),("driftbudyt",9),("smokedef",2),("reqordz",3),("meshangailee",2),("deranged_strannik",8),("afurah4ze",22),("atroc_za",2),("jkashmash",5),("dystoptimist",25),("zerotheadorkable",9),("sixinchribboncurls",2),("nectarwin",6),("stadius3737",383),("cptkrazyone",4),("scavwithanm4",26),("ladyscav_",7),("hoovilation",188),("yun0_0",6),("diickeed",75),("dogsafedoom",3),("ryanstar78",7),("dwhittle85",484),("staterbot",52),("brockaf",8),("wheelsoff",73),("scorpio040389",6),("lunigoonz",2),("thefallen041",9),("kingmate131",4),("vierneunundneunzig",136),("flipjixxer",3),("jbutcher0",44),("samuri9",57),("ethanoy",14),("remzie",3),("hddjjdhdjd",7),("ausman69",17),("lowkeycountry",2),("jaguar36703",13),("jigosdaddio",245),("lakeop",3),("spankendamonkey",181),("owarned",2),("draxeling",1),("juicemoose89",1),("fire_diamond12",9),("gustavo_cadette",1),("ole_leeroy",2),("nathan_heff",1),("nicole_saccone",3),("easytotype",2),("deltad0lphin",23),("bigeeezy",1),("dat_uppercat",1),("voltage416",36),("peppysteppy",11),("sanyawey",3),("sim_sal",40),("merlinkadatv",1),("lallaway",3),("apexshark101",73),("xupingole24",2),("watzegjij",16),("camrynhamm",2),("malachiatwood",2),("laggiz",2),("t_r_i_x",1),("atlantis609",9),("h3ll0fri3nd",1),("wubberduckie",1),("buddgaf",20),("datcollie",6),("sargesgamingtv",13),("draco2891",1),("exia595",16),("stermor",1),("oneshot702",19),("nezeka1",38),("apexgotted",2),("breezyvirginia",1),("dj_jrock13",2),("ttvabove_jay",20),("zerosdeat",5),("hurricanetron",3),("dubbelniels",4),("mutantclown911",1),("okunoie",1),("nutzznl",10),("zerosmokey",1),("dutcheasygamer",231),("traxxus610",2),("gkimdesu",2),("davqvist",2),("boriszsp",3),("pixelshading",37),("knebauz",55),("taintedsoultv",9),("terror_byte_",1),("flextendo",6),("bisshap",5),("the_real_apox",6),("eranok2207",4),("godofrng09",39),("itsmichmaddie",4),("ippis",7),("mrfritz_",8),("crybbytears",2),("isaiahshorter3",13),("steyelo",4),("mupirocin420",4),("therealmoc50",53),("addmenitrousrabbet69",110),("ninjagroud",4),("xgnpr0gam3r",48),("justkvek",7),("hustledhustler",1),("sinner4ever_17",16),("xxxlilnovaxxx",16),("4r1u5",1),("mattytouray",31),("mohammad05412",3),("swagdickens",9),("hiprutheroleplayer",238),("lordraptorjesusloliguess",3),("d4n63rz0n3",3),("decklunnn",1219),("zotiyaccc",106),("twitch_mrfoxgaming",343),("goodvibesjason27",1),("jadstop",11),("gavatron22",1),("hawkkinson",8),("imatteo",2),("uberestnoob",1),("konaowo",2),("overkilleryt",1),("numbnux0",1),("flakesofbacon",2),("dmitri_antonio_raymondo",11),("fearthatrussian",2),("jd1234567890dg",13),("aommies",2),("undertaker7288",3),("pilipino",3),("skyvaz",70),("officialjammy",1),("bricklebob",1),("aliboi12",1),("bloodsmokah",8),("feuerwehr",871),("raiderboy_707",2),("aerisclau",1),("kamil_2005_gti",69),("clanzer0_hex",3),("derrianr1",7),("boredandsdumb",2),("fenites",1),("crimmjjow",2),("eftpanda",8),("daddylilm0nster",3),("netdrifter",1),("noone_44",2),("isayah12345",12),("aggypande",1),("moonxd7",23),("boons9494",7),("mike01350",1),("krallstor",2),("thejudgeofnd",1),("bilalbhau",17),("supremetico",16),("bettingman99",10),("jag_jla",144),("yeelord6969",9),("brandon0112d",20),("lucklessba",2),("cassie_mv",22),("thesavage1232",8),("buscojudios",1),("preyformary",3),("dustydogs",27),("nicovzlz",3),("frankie1793",2),("demnec",63),("nittoh",22),("chrisboom06",2),("saaaaaaaaaaaaaaaaaaaad",12),("oceanfly",1),("gameralbert_hd",2),("jeckelz",17),("idk123445678",3),("bravox11",2),("gideon_vdm",3),("ahnedyy",42),("debo001",33),("icer_xx",41),("masonj064",3),("masterov48",42),("moneyman4472",42),("landon6161",4),("s1faka",219),("xxsopitanoobxx",1),("tuffman911",23),("daniel64621",3),("spcsullivan",2),("gradelegiontv",9),("lfcjames113",4),("ikks",1),("thatguystreamz",7),("atali",143),("berabowmanarmy",122),("bucknaked11",372),("dores44",207),("flymcawesome",16),("hitmanundead58",10),("jazx9000",373),("killemsquadgaming",288),("laffnavy",13),("mamba_mamba",101),("marbledan000",2),("mysticangel0214",74),("omoxion1",41),("odinnthewise",74),("jse_mustang",1),("elevenrl",1),("sy21best",9),("xflashfloodx",11),("cesarodefault",11),("nobueno9",1),("criggs2014",10),("jqnkoo",8),("albertotorres1656",69),("dragonsball12",10),("dragonball1927",201),("rockoregano5546",88),("markstrom",1),("tejas_",135),("kidmistic",98),("thebookworm12",9),("polyoddity",133),("ttv_deathstroke578",5),("kwlaofjqnwdjisk28",1),("theworstping",2),("killerklown315",2),("kayzken",3),("yiglo67",15),("exstatical",1),("jindura",1),("rwieber6",4),("sistercistern",3),("stenz0s",1),("ordanmintz3",2)];
