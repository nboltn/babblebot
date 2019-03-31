#![feature(proc_macro_hygiene, decl_macro, custom_attribute)]

#[macro_use] extern crate rocket;

mod commands;
mod types;
mod util;
mod web;

use crate::types::*;
use crate::util::*;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc,mpsc};
use std::sync::mpsc::{Sender, Receiver};
use std::ops::Deref;
use std::{thread,time};

use clap::load_yaml;
use config;
use clap::{App, ArgMatches};
use bcrypt::{DEFAULT_COST, hash};
use irc::error;
use irc::client::prelude::*;
use reqwest::{self, header};
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
    else {
        thread::spawn(move || { new_channel_listener(pool_c1) });
        thread::spawn(move || {
            rocket::ignite()
              .mount("/assets", StaticFiles::from("assets"))
              .mount("/", routes![web::index, web::dashboard, web::data, web::login, web::signup, web::password, web::title, web::game, web::new_command, web::save_command, web::trash_command, web::new_notice, web::trash_notice, web::save_setting, web::trash_setting])
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
                for channel in channel_hash.iter() { live_update(pool.clone(), channel.to_owned()) }
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
        bots.iter().for_each(|(bot, channels)| {
            let client = Arc::new(reactor.prepare_client_and_connect(&channels.1).unwrap());
            client.identify().unwrap();
            let _ = client.send("CAP REQ :twitch.tv/tags");
            let _ = client.send("CAP REQ :twitch.tv/commands");
            register_handler((*client).clone(), &mut reactor, con.clone());
            for channel in channels.0.iter() {
                let (sender, receiver) = mpsc::channel();
                senders.insert(channel.to_owned(), sender);
                spawn_timers(client.clone(), pool.clone(), channel.to_owned(), receiver);
                rename_channel_listener(pool.clone(), client.clone(), channel.to_owned(), senders.clone());
            }
        });
        let res = reactor.run();
        match res {
            Ok(_) => break,
            Err(e) => {
                eprintln!("{}", e);
                bots.iter().for_each(|(bot, channels)| {
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
        for channel in channel_hash.iter() { live_update(pool.clone(), channel.to_owned()) }
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
            Err(e) => { eprintln!("{}", e) }
            Ok(mut rsp) => {
                let json: Result<HelixUsers,_> = rsp.json();
                match json {
                    Err(e) => { eprintln!("{}", e) }
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
    let msg_handler = move |client: &IrcClient, message: Message| -> error::Result<()> {
        match message.command {
            Command::PING(_,_) => { let _ = client.send_pong(":tmi.twitch.tv"); }
            Command::PRIVMSG(chan, msg) => {
                let channel = &chan[1..];
                let mut words = msg.split_whitespace();
                if let Some(word) = words.next() {
                    let args: Vec<&str> = words.collect();
                    let mut badges: HashMap<String, String> = HashMap::new();
                    if let Some(tags) = message.tags {
                        tags.iter().for_each(|tag| {
                            if let Some(value) = &tag.1 {
                                if tag.0 == "badges" {
                                    let bs: Vec<&str> = value.split_whitespace().collect();
                                    for bstr in bs.iter() {
                                        let badge: Vec<&str> = bstr.split("/").collect();
                                        badges.insert(badge[0].to_owned(), badge[1].to_owned());
                                    }
                                }
                            }
                        });
                    }
                    for cmd in commands::native_commands.iter() {
                        if format!("!{}", cmd.0) == word {
                            let mut auth = false;
                            if let Some(value) = badges.get("broadcaster") { if value == "1" { auth = true } }
                            if let Some(value) = badges.get("moderator") { if value == "1" { auth = true } }
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
                        // parse cmd_vars
                        client.send_privmsg(chan, message).unwrap();
                    }
                }
            }
            _ => {}
        }
        Ok(())
    };

    reactor.register_client_with_handler(client, msg_handler);
}

fn live_update(pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>, channel: String) {
    thread::spawn(move || {
        let con = Arc::new(pool.get().unwrap());
        let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
        loop {
            let rsp = twitch_request_get(con.clone(), &channel, &format!("https://api.twitch.tv/kraken/streams?channel={}", id));
            match rsp {
                Err(e) => { println!("{}", e) }
                Ok(mut rsp) => {
                    let json: Result<KrakenStreams,_> = rsp.json();
                    match json {
                        Err(e) => { println!("{}", e); }
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
    // notices
    thread::spawn(move || {
        let con = pool.get().unwrap();
        loop {
            let rsp = receiver.recv_timeout(time::Duration::from_secs(30));
            match rsp {
                Ok(action) => {
                    match action {
                        ThreadAction::Kill => break
                    }
                }
                Err(err) => {
                    match err {
                        mpsc::RecvTimeoutError::Disconnected => break,
                        mpsc::RecvTimeoutError::Timeout => {}
                    }
                }
            }

            let live: String = con.get(format!("channel:{}:live", channel)).unwrap();
            if live == "true" {
                let keys: Vec<String> = con.keys(format!("channel:{}:notices:*:commands", channel)).unwrap();
                let ints: Vec<&str> = keys.iter().map(|str| {
                    let int: Vec<&str> = str.split(":").collect();
                    return int[3]
                }).collect();

                for int in ints.iter() {
                    let num: i16 = con.get(format!("channel:{}:notices:{}:countdown", channel, int)).unwrap();
                    if num > 0 { redis::cmd("DECRBY").arg(format!("channel:{}:notices:{}:countdown", channel, int)).arg(30).execute(con.deref()) }
                };

                let int = ints.iter().filter(|int| {
                    let num: i16 = con.get(format!("channel:{}:notices:{}:countdown", channel, int)).unwrap();
                    return num == 0
                }).fold(0, |acc, int| {
                    let int = int.parse::<i16>().unwrap();
                    if acc > int { return acc } else { return int }
                });

                if int != 0 {
                    let _: () = con.set(format!("channel:{}:notices:{}:countdown", channel, int), int.clone()).unwrap();
                    let cmd: String = con.lpop(format!("channel:{}:notices:{}:commands", channel, int)).unwrap();
                    let _: () = con.rpush(format!("channel:{}:notices:{}:commands", channel, int), cmd.clone()).unwrap();
                    let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, cmd), "message");
                    if let Ok(message) = res {
                        // parse cmd_vars
                        client.send_privmsg(format!("#{}", channel), message).unwrap();
                    }
                }
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
        Err(e) => { println!("{}", e) }
        Ok(mut rsp) => {
            let json: Result<types::HelixUsers,_> = rsp.json();
            match json {
                Err(e) => { println!("{}", e) }
                Ok(json) => {
                    let _: () = con.set(format!("channel:{}:id", channel_name), &json.data[0].id).unwrap();
                    let _: () = con.set(format!("channel:{}:display_name", channel_name), &json.data[0].display_name).unwrap();
                    let _: () = con.publish("new_channels", channel_name).unwrap();
                    println!("channel {} has been added", channel_name)
                }
            }
        }
    }
}
