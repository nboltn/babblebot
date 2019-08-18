use crate::types::*;
use crate::commands::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::{thread,mem};
use base64;
use config;
use chrono::{Utc, DateTime};
use http::header::{self,HeaderValue};
use reqwest::Method;
use reqwest::r#async::{Client,RequestBuilder,Decoder};
use futures::future::{Future,join_all};
use irc::client::prelude::*;
use regex::{Regex,RegexBuilder,Captures,escape};
use redis::{self,Commands,Connection};

pub fn log_info(channel: Option<&str>, descriptor: &str, content: &str) {
    let timestamp = Utc::now().to_rfc3339();
    match channel {
        None => info!("[{}] [{}] {}", timestamp, descriptor, content),
        Some(channel) => info!("[{}] [{}] [{}] {}", timestamp, channel, descriptor, content)
    }
}

pub fn log_error(channel: Option<&str>, descriptor: &str, content: &str) {
    let timestamp = Utc::now().to_rfc3339();
    match channel {
        None => error!("[{}] [{}] {}", timestamp, descriptor, content),
        Some(channel) => error!("[{}] [{}] [{}] {}", timestamp, channel, descriptor, content)
    }
}

pub fn acquire_con() -> redis::Connection {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let redis_host = settings.get_str("redis_host").unwrap_or("redis://127.0.0.1".to_owned());
    let client = redis::Client::open(&redis_host[..]).unwrap();
    loop {
        match client.get_connection() {
            Err(e) => log_error(None, "acquire_con", &e.to_string()),
            Ok(con) => return con
        }
    }
}

pub fn connect_and_send_privmsg(con: Arc<Connection>, channel: String, message: String) {
    let bot: String = con.get(format!("channel:{}:bot", channel)).expect("get:bot");
    let passphrase: String = con.get(format!("bot:{}:token", bot)).expect("get:token");
    let config = Config {
        server: Some("irc.chat.twitch.tv".to_owned()),
        use_ssl: Some(true),
        nickname: Some(bot.to_owned()),
        password: Some(format!("oauth:{}", passphrase)),
        channels: Some(vec![format!("#{}", channel)]),
        ping_timeout: Some(9999),
        ..Default::default()
    };

    match IrcClient::from_config(config) {
        Err(e) => { log_error(None, "connect_and_send_message", &e.to_string()) }
        Ok(client) => {
            let auth: String = con.get(format!("channel:{}:auth", channel)).unwrap_or("false".to_owned());
            if auth == "true" {
                let _ = client.identify();
                let _ = client.send_privmsg(format!("#{}", channel), message);
            }
            let _ = client.send_quit("");
        }
    }
}

pub fn connect_and_send_message(con: Arc<Connection>, channel: String, message: String) {
    let bot: String = con.get(format!("channel:{}:bot", channel)).expect("get:bot");
    let passphrase: String = con.get(format!("bot:{}:token", bot)).expect("get:token");
    let config = Config {
        server: Some("irc.chat.twitch.tv".to_owned()),
        use_ssl: Some(true),
        nickname: Some(bot.to_owned()),
        password: Some(format!("oauth:{}", passphrase)),
        channels: Some(vec![format!("#{}", channel)]),
        ping_timeout: Some(9999),
        ..Default::default()
    };

    match IrcClient::from_config(config) {
        Err(e) => { log_error(None, "connect_and_send_message", &e.to_string()) }
        Ok(client) => {
            let client = Arc::new(client);
            let _ = client.identify();
            send_parsed_message(con, client.clone(), channel, message, Vec::new(), None);
            let _ = client.send_quit("");
        }
    }
}

pub fn connect_and_run_command(cmd: fn(Arc<Connection>, Arc<IrcClient>, String, Vec<String>, Option<Message>), con: Arc<Connection>, channel: String, args: Vec<String>) {
    let bot: String = con.get(format!("channel:{}:bot", channel)).expect("get:bot");
    let passphrase: String = con.get(format!("bot:{}:token", bot)).expect("get:token");
    let config = Config {
        server: Some("irc.chat.twitch.tv".to_owned()),
        use_ssl: Some(true),
        nickname: Some(bot.to_owned()),
        password: Some(format!("oauth:{}", passphrase)),
        channels: Some(vec![format!("#{}", channel)]),
        ping_timeout: Some(9999),
        ..Default::default()
    };

    match IrcClient::from_config(config) {
        Err(e) => { log_error(None, "connect_and_send_message", &e.to_string()) }
        Ok(client) => {
            let client = Arc::new(client);
            let _ = client.identify();
            (cmd)(con, client.clone(), channel, args, None);
            let _ = client.send_quit("");
        }
    }
}

pub fn send_message(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, mut message: String) {
    let auth: String = con.get(format!("channel:{}:auth", channel)).unwrap_or("false".to_owned());
    if auth == "true" {
        let me: String = con.hget(format!("channel:{}:settings", channel), "channel:me").unwrap_or("false".to_owned());
        if me == "true" { message = format!("/me {}", message); }
        let _ = client.send_privmsg(format!("#{}", channel), message);
    }
}

pub fn send_parsed_message(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, mut message: String, args: Vec<String>, irc_message: Option<Message>) {
    let auth: String = con.get(format!("channel:{}:auth", channel)).unwrap_or("false".to_owned());
    if auth == "true" {
        if args.len() > 0 {
            if let Some(char) = args[args.len()-1].chars().next() {
                if char == '@' { message = format!("{} -> {}", args[args.len()-1], message) }
            }
        }
        let me: String = con.hget(format!("channel:{}:settings", channel), "channel:me").unwrap_or("false".to_owned());
        if me == "true" { message = format!("/me {}", message); }

        for var in command_vars.iter() {
            message = parse_var(var, &message, con.clone(), Some(client.clone()), channel.clone(), irc_message.clone(), args.clone());
        }

        let mut futures = Vec::new();
        let mut regexes: Vec<String> = Vec::new();
        for var in command_vars_async.iter() {
            let rgx = Regex::new(&format!("\\({} ?((?:[\\w\\-\\?\\._:/&!= ]+)*)\\)", var.0)).unwrap();
            for captures in rgx.captures_iter(&message) {
                if let (Some(capture), Some(vargs)) = (captures.get(0), captures.get(1)) {
                    let vargs: Vec<String> = vargs.as_str().split_whitespace().map(|str| str.to_owned()).collect();
                    if let Some((builder, func)) = (var.1)(con.clone(), Some(client.clone()), channel.clone(), irc_message.clone(), vargs.clone(), args.clone()) {
                        let future = builder.send().and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() }).map(func);
                        futures.push(future);
                        regexes.push(capture.as_str().to_owned());
                    }
                }
            }
        }

        thread::spawn(move || {
            let mut core = tokio_core::reactor::Core::new().unwrap();
            let work = join_all(futures);
            for (i,res) in core.run(work).unwrap().into_iter().enumerate() {
                let rgx = Regex::new(&escape(&regexes[i])).unwrap();
                message = rgx.replace(&message, |_: &Captures| { &res }).to_string();
            }
            let _ = client.send_privmsg(format!("#{}", channel), message);
        });
    }
}

pub fn spawn_age_check(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, nick: String, age: i64, display: String) {
    let res: Result<String,_> = con.hget("account:ages", &nick);
    if let Ok(timestamp) = res {
        let dt = DateTime::parse_from_rfc3339(&timestamp).unwrap();
        let diff = Utc::now().signed_duration_since(dt);
        if diff.num_minutes() < age {
            let length = age - diff.num_minutes();
            let _ = client.send_privmsg(format!("#{}", channel), format!("/timeout {} {}", nick, length * 60));
            if display == "true" { send_message(con.clone(), client.clone(), channel.to_owned(), format!("@{} you've been timed out for not reaching the minimum account age", nick)); }
        }
    } else {
        let future = twitch_kraken_request(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/users?login={}", &nick)).send()
            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
            .map_err(|e| println!("request error: {}", e))
            .map(move |body| {
                let con = Arc::new(acquire_con());
                let body = std::str::from_utf8(&body).unwrap();
                let json: Result<KrakenUsers,_> = serde_json::from_str(&body);
                match json {
                    Err(e) => {
                        log_error(Some(&channel), "spawn_age_check", &e.to_string());
                        log_error(Some(&channel), "request_body", &body);
                    }
                    Ok(json) => {
                        if json.total > 0 {
                            let _: () = con.hset("account:ages", &nick, &json.users[0].created_at).unwrap();
                            let dt = DateTime::parse_from_rfc3339(&json.users[0].created_at).unwrap();
                            let diff = Utc::now().signed_duration_since(dt);
                            if diff.num_minutes() < age {
                                let length = age - diff.num_minutes();
                                let _ = client.send_privmsg(format!("#{}", channel), format!("/timeout {} {}", nick, length * 60));
                                if display == "true" { send_message(con.clone(), client.clone(), channel.to_owned(), format!("@{} you've been timed out for not reaching the minimum account age", nick)); }
                            }
                        }
                    }
                }
            });
        thread::spawn(move || { tokio::run(future) });
    }
}

pub fn request(method: Method, body: Option<Vec<u8>>, url: &str) -> RequestBuilder {
    let client = Client::builder().build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn twitch_kraken_request(con: Arc<Connection>, channel: &str, content: Option<&str>, body: Option<Vec<u8>>, method: Method, url: &str) -> RequestBuilder {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let token: String = con.get(format!("channel:{}:token", channel)).expect("get:token");

    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("OAuth {}", token)).unwrap());
    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());
    if let Some(content) = content { headers.insert("Content-Type", HeaderValue::from_str(content).unwrap()); }

    let client = Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn twitch_helix_request(con: Arc<Connection>, channel: &str, content: Option<&str>, body: Option<Vec<u8>>, method: Method, url: &str) -> RequestBuilder {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let token: String = con.get(format!("channel:{}:token", channel)).expect("get:token");

    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());
    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());
    if let Some(content) = content { headers.insert("Content-Type", HeaderValue::from_str(content).unwrap()); }

    let client = Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn patreon_request(con: Arc<Connection>, channel: &str, method: Method, url: &str) -> RequestBuilder {
    let token: String = con.get(format!("channel:{}:patreon:token", channel)).unwrap_or("".to_owned());
    let mut headers = header::HeaderMap::new();
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());

    let client = Client::builder().default_headers(headers).build().unwrap();
    let builder = client.request(method, url);
    return builder;
}

pub fn patreon_refresh(con: Arc<Connection>, channel: &str, method: Method, url: &str, body: Option<Vec<u8>>) -> RequestBuilder {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let id = settings.get_str("patreon_client").unwrap_or("".to_owned());
    let secret = settings.get_str("patreon_secret").unwrap_or("".to_owned());

    let mut headers = header::HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_str("application/x-www-form-urlencoded").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Basic {}", base64::encode(&format!("{}:{}",id,secret)))).unwrap());


    let client = Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn spotify_request(con: Arc<Connection>, channel: &str, method: Method, url: &str, body: Option<Vec<u8>>) -> RequestBuilder {
    let token: String = con.get(format!("channel:{}:spotify:token", channel)).unwrap_or("".to_owned());
    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.api+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());

    let client = Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn spotify_refresh(con: Arc<Connection>, channel: &str, method: Method, url: &str, body: Option<Vec<u8>>) -> RequestBuilder {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let id = settings.get_str("spotify_id").unwrap_or("".to_owned());
    let secret = settings.get_str("spotify_secret").unwrap_or("".to_owned());

    let mut headers = header::HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_str("application/x-www-form-urlencoded").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Basic {}", base64::encode(&format!("{}:{}",id,secret)))).unwrap());


    let client = Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn discord_request(con: Arc<Connection>, channel: &str, body: Option<Vec<u8>>, method: Method, url: &str) -> RequestBuilder {
    let token: String = con.hget(format!("channel:{}:settings", channel), "discord:token").unwrap_or("".to_owned());
    let mut headers = header::HeaderMap::new();
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bot {}", token)).unwrap());
    headers.insert("User-Agent", HeaderValue::from_str("Babblebot (https://gitlab.com/toovs/babblebot, 0.1").unwrap());
    headers.insert("Content-Type", HeaderValue::from_str("application/json").unwrap());

    let client = Client::builder().default_headers(headers).build().unwrap();
    let mut builder = client.request(method, url);
    if let Some(body) = body { builder = builder.body(body); }
    return builder;
}

pub fn fortnite_request(con: Arc<Connection>, channel: &str, url: &str) -> RequestBuilder {
    let token: String = con.hget(format!("channel:{}:settings", channel), "fortnite:token").unwrap_or("".to_owned());
    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.api+json").unwrap());
    headers.insert("TRN-Api-Key", HeaderValue::from_str(&token).unwrap());

    let client = Client::builder().default_headers(headers).build().unwrap();
    let builder = client.get(url);
    return builder;
}

pub fn pubg_request(con: Arc<Connection>, channel: &str, url: &str) -> RequestBuilder {
    let token: String = con.hget(format!("channel:{}:settings", channel), "pubg:token").unwrap_or("".to_owned());
    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.api+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());

    let client = Client::builder().default_headers(headers).build().unwrap();
    let builder = client.get(url);
    return builder;
}

pub fn parse_var(var: &(&str, fn(Arc<Connection>, Option<Arc<IrcClient>>, String, Option<Message>, Vec<String>, Vec<String>) -> String), message: &str, con: Arc<Connection>, client: Option<Arc<IrcClient>>, channel: String, irc_message: Option<Message>, cargs: Vec<String>) -> String {
    let rgx = Regex::new(&format!("\\({} ?((?:[\\w\\-\\?\\._:/&!= ]+)*)\\)", var.0)).unwrap();
    let mut msg: String = message.to_owned();
    for captures in rgx.captures_iter(message) {
        if let Some(capture) = captures.get(1) {
            let vargs: Vec<String> = capture.as_str().split_whitespace().map(|str| str.to_owned()).collect();
            let res = (var.1)(con.clone(), client.clone(), channel.clone(), irc_message.clone(), vargs, cargs.clone());
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
