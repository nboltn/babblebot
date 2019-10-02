use crate::types::*;
use crate::commands::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::{thread,mem,time};
use either::Either::{self, Left, Right};
use base64;
use config;
use chrono::{Utc, DateTime};
use http::header::{self,HeaderValue};
use crossbeam_channel::{Sender,Receiver};
use reqwest::Method;
use reqwest::r#async::{RequestBuilder,Decoder};
use futures::future::Future;
use tokio::runtime::Runtime;
use irc::client::prelude::*;
use regex::{Regex,RegexBuilder,Captures,escape};
use redis::{self,Value,Commands,from_redis_value};

pub fn log_info(id: Option<Either<&str, Vec<&str>>>, descriptor: &str, content: &str, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%z");
    match id {
        None => {
            let str = format!("[{}] [{}] {}", timestamp, descriptor, content);
            info!("{}", str);
            redis_call(db.clone(), vec!["lpush", "logs", &str]);
            redis_call(db.clone(), vec!["ltrim", "logs", "0", "9999"]);
        }
        Some(ab) => {
            match ab {
                Left(bot) => {
                    let bot = bot.to_string();
                    let descriptor = descriptor.to_string();
                    let content = content.to_string().replace("\n", " ");
                    thread::spawn(move || {
                        let mut channels: Vec<String> = Vec::new();
                        let channels_: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                        for channel in channels_ {
                            let cbot: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:bot", channel)]).unwrap_or(Value::Data("".as_bytes().to_owned()))).unwrap();
                            if cbot == bot { channels.push(channel); }
                        }

                        let str = format!("[{}] [{}] [{}] {}", timestamp, bot, descriptor, content);
                        info!("{}", str);

                        for channel in channels {
                            let str = format!("[{}] [{}] {}", timestamp, descriptor, content);
                            redis_call(db.clone(), vec!["lpush", &format!("channel:{}:logs", &channel), &str]);
                            redis_call(db.clone(), vec!["ltrim", &format!("channel:{}:logs", &channel), "0", "9999"]);
                        }
                    });
                }
                Right(channels) => {
                    let channels: Vec<String> = channels.iter().map(|c| c.to_string()).collect();
                    let descriptor = descriptor.to_string();
                    let content = content.to_string().replace("\n", " ");
                    thread::spawn(move || {
                        if channels.len() > 0 {
                            if channels.len() > 1 {
                                let bot: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:bot", &channels[0])]).unwrap_or(Value::Data("".as_bytes().to_owned()))).unwrap();
                                let str = format!("[{}] [{}] [{}] {}", timestamp, bot, descriptor, content);
                                info!("{}", str);
                            } else {
                                let str = format!("[{}] [{}] [{}] {}", timestamp, &channels[0], descriptor, content);
                                info!("{}", str);
                            }

                            for channel in channels {
                                let str = format!("[{}] [{}] {}", timestamp, descriptor, content);
                                redis_call(db.clone(), vec!["lpush", &format!("channel:{}:logs", &channel), &str]);
                                redis_call(db.clone(), vec!["ltrim", &format!("channel:{}:logs", &channel), "0", "9999"]);
                            }
                        }
                    });
                }
            }
        }
    }
}

pub fn log_error(id: Option<Either<&str, Vec<&str>>>, descriptor: &str, content: &str, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%z");
    match id {
        None => {
            let str = format!("[{}] [{}] {}", timestamp, descriptor, content);
            error!("{}", str);
            redis_call(db.clone(), vec!["lpush", "logs", &str]);
            redis_call(db.clone(), vec!["ltrim", "logs", "0", "9999"]);
        }
        Some(ab) => {
            match ab {
                Left(bot) => {
                    let bot = bot.to_string();
                    let descriptor = descriptor.to_string();
                    let content = content.to_string().replace("\n", " ");
                    thread::spawn(move || {
                        let mut channels: Vec<String> = Vec::new();
                        let channels_: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", "channels"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                        for channel in channels_ {
                            let cbot: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:bot", channel)]).unwrap_or(Value::Data("".as_bytes().to_owned()))).unwrap();
                            if cbot == bot { channels.push(channel); }
                        }

                        let str = format!("[{}] [{}] [{}] {}", timestamp, bot, descriptor, content);
                        error!("{}", str);

                        for channel in channels {
                            let str = format!("[{}] [{}] {}", timestamp, descriptor, content);
                            redis_call(db.clone(), vec!["lpush", &format!("channel:{}:logs", &channel), &str]);
                            redis_call(db.clone(), vec!["ltrim", &format!("channel:{}:logs", &channel), "0", "9999"]);
                        }
                    });
                }
                Right(channels) => {
                    let channels: Vec<String> = channels.iter().map(|c| c.to_string()).collect();
                    let descriptor = descriptor.to_string();
                    let content = content.to_string().replace("\n", " ");
                    thread::spawn(move || {
                        if channels.len() > 0 {
                            if channels.len() > 1 {
                                let bot: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:bot", &channels[0])]).unwrap_or(Value::Data("".as_bytes().to_owned()))).unwrap();
                                let str = format!("[{}] [{}] [{}] {}", timestamp, bot, descriptor, content);
                                error!("{}", str);
                            } else {
                                let str = format!("[{}] [{}] [{}] {}", timestamp, &channels[0], descriptor, content);
                                error!("{}", str);
                            }

                            for channel in channels {
                                let str = format!("[{}] [{}] {}", timestamp, descriptor, content);
                                redis_call(db.clone(), vec!["lpush", &format!("channel:{}:logs", &channel), &str]);
                                redis_call(db.clone(), vec!["ltrim", &format!("channel:{}:logs", &channel), "0", "9999"]);
                            }
                        }
                    });
                }
            }
        }
    }
}

pub fn acquire_con() -> redis::Connection {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let redis_uri = settings.get_str("redis_uri").unwrap_or("redis://127.0.0.1".to_owned());
    let client = redis::Client::open(&redis_uri[..]).unwrap();
    loop {
        match client.get_connection() {
            Err(e) => println!("[acquire_con] {}", &e.to_string()),
            Ok(con) => return con
        }
        thread::sleep(time::Duration::from_secs(1));
    }
}

pub fn redis_call(db: (Sender<Vec<String>>, Receiver<Result<Value, String>>), args: Vec<&str>) -> Result<Value, String> {
    (db.0).send(args.iter().map(|a| a.to_string()).collect());
    let rsp = (db.1).recv();
    match rsp {
        Ok(res) => {
            match res {
                Ok(Value::Nil) => Err("nil value".to_string()),
                _ => { return res }
            }
        }
        Err(e) => { Err("recv error".to_string()) }
    }
}

pub fn send_message(client: Arc<IrcClient>, channel: String, mut message: String, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    thread::spawn(move || {
        let auth: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:auth", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
        if auth == "true" {
            let me: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", channel), "channel:me"]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
            if me == "true" { message = format!("/me {}", message); }
            let _ = client.send_privmsg(format!("#{}", channel), message);
        }
    });
}

pub fn send_parsed_message(client: Arc<IrcClient>, channel: String, mut message: String, args: Vec<String>, irc_message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>), runtime: bool) {
    let auth: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:auth", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
    if auth == "true" {
        if args.len() > 0 {
            if let Some(char) = args[args.len()-1].chars().next() {
                if char == '@' { message = format!("{} -> {}", args[args.len()-1], message) }
            }
        }
        let me: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", channel), "channel:me"]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
        if me == "true" { message = format!("/me {}", message); }

        for var in command_vars.iter() {
            message = parse_var(var, &message, Some(client.clone()), channel.clone(), irc_message.clone(), args.clone(), db.clone());
        }

        let mut futures = Vec::new();
        let mut regexes: Vec<String> = Vec::new();
        for var in command_vars_async.iter() {
            let rgx = Regex::new(&format!("\\({} ?((?:[\\w\\-\\?\\._:/&!= ]+)*)\\)", var.0)).unwrap();
            for captures in rgx.captures_iter(&message) {
                if let (Some(capture), Some(vargs)) = (captures.get(0), captures.get(1)) {
                    let vargs: Vec<String> = vargs.as_str().split_whitespace().map(|str| str.to_owned()).collect();
                    if let Some((builder, func)) = (var.1)(Some(client.clone()), channel.clone(), irc_message.clone(), vargs.clone(), args.clone(), db.clone()) {
                        let db = db.clone();
                        let chan = channel.clone();
                        let future = builder.send().and_then(|mut res| { (Ok(chan), Ok(db), mem::replace(res.body_mut(), Decoder::empty()).concat2()) }).map_err(|e| println!("request error: {}", e)).map(func);
                        futures.push(future);
                        regexes.push(capture.as_str().to_owned());
                    }
                }
            }
        }

        if runtime {
            let mut rt = Runtime::new().expect("runtime:new");
            for (i,future) in futures.into_iter().enumerate() {
                let res = rt.block_on(future).unwrap();
                let rgx = Regex::new(&escape(&regexes[i])).expect("regex:new");
                message = rgx.replace(&message, |_: &Captures| { &res }).to_string();
            }
            rt.shutdown_now();
        } else {
            for (i,future) in futures.into_iter().enumerate() {
                let res = future.wait().unwrap();
                let rgx = Regex::new(&escape(&regexes[i])).expect("regex:new");
                message = rgx.replace(&message, |_: &Captures| { &res }).to_string();
            }
        }
        let _ = client.send_privmsg(format!("#{}", channel), message);
    }
}

pub fn spawn_age_check(client: Arc<IrcClient>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>), channel: String, nick: String, age: i64, display: String) {
    let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", "account:ages", &nick]);
    if let Ok(value) = res {
        let timestamp: String = from_redis_value(&value).unwrap();
        let dt = DateTime::parse_from_rfc3339(&timestamp).unwrap();
        let diff = Utc::now().signed_duration_since(dt);
        if diff.num_minutes() < age {
            let length = age - diff.num_minutes();
            let _ = client.send_privmsg(format!("#{}", channel), format!("/timeout {} {}", nick, length * 60));
            if display == "true" { send_message(client.clone(), channel.to_owned(), format!("@{} you've been timed out for not reaching the minimum account age", nick), db.clone()); }
        }
    } else {
        let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
        let future = twitch_kraken_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/users?login={}", &nick)).send()
            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
            .map_err(|e| println!("request error: {}", e))
            .map(move |body| {
                let body = std::str::from_utf8(&body).unwrap().to_string();
                let json: Result<KrakenUsers,_> = serde_json::from_str(&body);
                match json {
                    Err(e) => {
                        log_error(Some(Right(vec![&channel])), "spawn_age_check", &e.to_string(), db.clone());
                        log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                    }
                    Ok(json) => {
                        if json.total > 0 {
                            redis_call(db.clone(), vec!["hset", "account:ages", &nick, &json.users[0].created_at]);
                            let dt = DateTime::parse_from_rfc3339(&json.users[0].created_at).unwrap();
                            let diff = Utc::now().signed_duration_since(dt);
                            if diff.num_minutes() < age {
                                let length = age - diff.num_minutes();
                                let _ = client.send_privmsg(format!("#{}", channel), format!("/timeout {} {}", nick, length * 60));
                                if display == "true" { send_message(client.clone(), channel.to_owned(), format!("@{} you've been timed out for not reaching the minimum account age", nick), db.clone()); }
                            }
                        }
                    }
                }
            });
        thread::spawn(move || { tokio::run(future) });
    }
}

pub fn request(method: Method, body: Option<Vec<u8>>, url: &str) -> RequestBuilder {
    let client = reqwest::r#async::Client::builder().build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn twitch_kraken_request(token: String, content: Option<&str>, body: Option<Vec<u8>>, method: Method, url: &str) -> RequestBuilder {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();

    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("OAuth {}", token)).unwrap());
    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());
    if let Some(content) = content { headers.insert("Content-Type", HeaderValue::from_str(content).unwrap()); }

    let client = reqwest::r#async::Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn twitch_helix_request(token: String, content: Option<&str>, body: Option<Vec<u8>>, method: Method, url: &str) -> RequestBuilder {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();

    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());
    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());
    if let Some(content) = content { headers.insert("Content-Type", HeaderValue::from_str(content).unwrap()); }

    let client = reqwest::r#async::Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn twitch_user_request(token: String, method: Method, url: &str) -> RequestBuilder {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();

    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("OAuth {}", token)).unwrap());
    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());

    let client = reqwest::r#async::Client::builder().default_headers(headers).build().unwrap();
    let builder = client.request(method, url);
    return builder;
}

pub fn twitch_refresh(method: Method, url: &str, body: Option<Vec<u8>>) -> RequestBuilder {
    let mut headers = header::HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_str("application/x-www-form-urlencoded").unwrap());

    let client = reqwest::r#async::Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn patreon_request(token: String, method: Method, url: &str) -> RequestBuilder {
    let mut headers = header::HeaderMap::new();
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());

    let client = reqwest::r#async::Client::builder().default_headers(headers).build().unwrap();
    let builder = client.request(method, url);
    return builder;
}

pub fn patreon_refresh(method: Method, url: &str, body: Option<Vec<u8>>) -> RequestBuilder {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let id = settings.get_str("patreon_client").unwrap_or("".to_owned());
    let secret = settings.get_str("patreon_secret").unwrap_or("".to_owned());

    let mut headers = header::HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_str("application/x-www-form-urlencoded").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Basic {}", base64::encode(&format!("{}:{}", id, secret)))).unwrap());

    let client = reqwest::r#async::Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn spotify_request(token: String, method: Method, url: &str, body: Option<Vec<u8>>) -> RequestBuilder {
    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.api+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());

    let client = reqwest::r#async::Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn spotify_refresh(method: Method, url: &str, body: Option<Vec<u8>>) -> RequestBuilder {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let id = settings.get_str("spotify_id").unwrap_or("".to_owned());
    let secret = settings.get_str("spotify_secret").unwrap_or("".to_owned());

    let mut headers = header::HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_str("application/x-www-form-urlencoded").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Basic {}", base64::encode(&format!("{}:{}", id, secret)))).unwrap());

    let client = reqwest::r#async::Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn discord_request(token: String, body: Option<Vec<u8>>, method: Method, url: &str) -> RequestBuilder {
    let mut headers = header::HeaderMap::new();
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bot {}", token)).unwrap());
    headers.insert("User-Agent", HeaderValue::from_str("Babblebot (https://gitlab.com/toovs/babblebot, 0.1").unwrap());
    headers.insert("Content-Type", HeaderValue::from_str("application/json").unwrap());

    let client = reqwest::r#async::Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn fortnite_request(token: String, url: &str) -> RequestBuilder {
    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.api+json").unwrap());
    headers.insert("TRN-Api-Key", HeaderValue::from_str(&token).unwrap());

    let client = reqwest::r#async::Client::builder().default_headers(headers).build().unwrap();
    let builder = client.get(url);
    return builder;
}

pub fn pubg_request(token: String, url: &str) -> RequestBuilder {
    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.api+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());

    let client = reqwest::r#async::Client::builder().default_headers(headers).build().unwrap();
    let builder = client.get(url);
    return builder;
}

pub fn parse_var(var: &(&str, fn(Option<Arc<IrcClient>>, String, Option<Message>, Vec<String>, Vec<String>, (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String), message: &str, client: Option<Arc<IrcClient>>, channel: String, irc_message: Option<Message>, cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    let rgx = Regex::new(&format!("\\({} ?((?:[\\w\\-\\?\\._:/&!= ]+)*)\\)", var.0)).unwrap();
    let mut msg: String = message.to_owned();
    for captures in rgx.captures_iter(message) {
        if let Some(capture) = captures.get(1) {
            let vargs: Vec<String> = capture.as_str().split_whitespace().map(|str| str.to_owned()).collect();
            let res = (var.1)(client.clone(), channel.clone(), irc_message.clone(), vargs, cargs.clone(), db.clone());
            msg = rgx.replace(&msg, |_: &Captures| { &res }).to_string();
        }
    }

    return msg.to_owned();
}

/*pub fn parse_code(message: &str) -> String {
    let mut msg: String = message.to_owned();
    let rgx = Regex::new("\\{-(.+?)\\-}").unwrap();
    for captures in rgx.captures_iter(&msg.clone()) {
        if let Some(capture) = captures.get(1) {
            let res = request("POST", format!("function() {{ {} }}", capture.as_str()).as_bytes().to_owned(), "http://localhost:9412/execute", 0);
            match res {
                Err(e) => error!("[parse_code] {}", e),
                Ok((meta,body)) => { msg = rgx.replace(&msg, |_: &Captures| { strip_chars(&body, "\"") }).to_string(); }
            }
        }
    }

    return msg.to_owned();
}*/

pub fn replace_var(var: &str, val: &str, msg: &str) -> String {
    let rgx = Regex::new(&format!("\\({}\\)", var)).unwrap();
    let mut message: String = msg.to_owned();
    for captures in rgx.captures_iter(&msg) {
        if let Some(_capture) = captures.get(0) {
            message = rgx.replace(&message, |_: &Captures| { &val }).to_string();
        }
    }

    return message.to_owned();
}

pub fn get_nick(msg: &Message) -> String {
    let mut name = "";
    if let Some(prefix) = &msg.prefix {
        let split: Vec<&str> = prefix.split("!").collect();
        name = split[0];
    }
    return name.to_owned();
}

pub fn get_id(msg: &Message) -> Option<String> {
    let mut id: Option<String> = None;
    if let Some(tags) = &msg.tags {
        tags.iter().for_each(|tag| {
            if let Some(_value) = &tag.1 {
                if tag.0 == "user-id" {
                    id = (tag.1).clone();
                }
            }
        });
    }
    return id;
}

pub fn get_bits(msg: &Message) -> Option<String> {
    let mut bits: Option<String> = None;
    if let Some(tags) = &msg.tags {
        tags.iter().for_each(|tag| {
            if let Some(_value) = &tag.1 {
                if tag.0 == "bits" {
                    bits = (tag.1).clone();
                }
            }
        });
    }
    return bits;
}

pub fn get_badges(msg: &Message) -> HashMap<String, String> {
    let mut badges = HashMap::new();
    if let Some(tags) = &msg.tags {
        tags.iter().for_each(|tag| {
            if let Some(value) = &tag.1 {
                if tag.0 == "badges" {
                    let bs: Vec<&str> = value.split(",").collect();
                    for bstr in bs.iter() {
                        let badge: Vec<&str> = bstr.split("/").collect();
                        if badge.len() > 1 {
                            badges.insert(badge[0].to_owned(), badge[1].to_owned());
                        } else {
                            badges.insert(badge[0].to_owned(), "".to_owned());
                        }
                    }
                }
            }
        });
    }
    return badges;
}

fn strip_chars(original : &str, strip : &str) -> String {
    original.chars().filter(|&c| !strip.contains(c)).collect()
}

pub fn url_regex() -> Regex {
    RegexBuilder::new("(((ht|f)tp(s?))://)?(([a-zA-Z0-9\\-]+)\\.)+(aero|arpa|biz|cat|com|coop|edu|gov|info|jobs|mil|mobi|museum|name|net|org|pro|travel|ac|ad|ae|af|ag|ai|al|am|an|ao|ap|aq|ar|as|at|au|aw|az|ax|ba|bb|bd|be|bf|bg|bh|bi|bj|bm|bn|bo|br|bs|bt|bv|bw|by|bz|ca|cc|cd|cf|cg|ch|ci|ck|cl|cm|cn|co|cr|cs|cu|cv|cx|cy|cz|de|dj|dk|dm|do|dz|ec|ee|eg|eh|er|es|et|eu|fi|fj|fk|fm|fo|fr|ga|gb|gd|ge|gf|gg|gh|gi|gl|gm|gn|gp|gq|gr|gs|gt|gu|gw|gy|hk|hm|hn|hr|ht|hu|id|ie|il|im|in|io|iq|ir|is|it|je|jm|jo|jp|ke|kg|kh|ki|km|kn|kp|kr|kw|ky|kz|la|lb|lc|li|lk|lr|ls|lt|lu|lv|ly|ma|mc|md|mg|mh|mk|ml|mm|mn|mo|mp|mq|mr|ms|mt|mu|mv|mw|mx|my|mz|na|nc|ne|nf|ng|ni|nl|no|np|nr|nu|nz|om|pa|pe|pf|pg|ph|pk|pl|pm|pn|pr|ps|pt|pw|py|qa|re|ro|ru|rw|sa|sb|sc|sd|se|sg|sh|si|sj|sk|sl|sm|sn|so|sr|st|sv|sy|sz|tc|td|tf|tg|th|tj|tk|tl|tm|tn|to|tp|tr|tt|tv|tw|tz|ua|ug|uk|um|us|uy|uz|va|vc|ve|vg|vi|vn|vu|wf|ws|ye|yt|yu|za|zm|zw)(:[0-9]+)*(/($|[a-zA-Z0-9\\.,;\\?'\\\\\\+&%\\$#=~_\\-]+))*").case_insensitive(true).build().unwrap()
}
