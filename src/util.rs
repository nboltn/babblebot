// urlRegex = T.pack "(((ht|f)tp(s?))\\:\\/\\/)?[a-zA-Z0-9\\-\\.]+(?<!\\.)\\.(aero|arpa|biz|cat|com|coop|edu|gov|info|jobs|mil|mobi|museum|name|net|org|pro|travel|ac|ad|ae|af|ag|ai|al|am|an|ao|ap|aq|ar|as|at|au|aw|az|ax|ba|bb|bd|be|bf|bg|bh|bi|bj|bm|bn|bo|br|bs|bt|bv|bw|by|bz|ca|cc|cd|cf|cg|ch|ci|ck|cl|cm|cn|co|cr|cs|cu|cv|cx|cy|cz|de|dj|dk|dm|do|dz|ec|ee|eg|eh|er|es|et|eu|fi|fj|fk|fm|fo|fr|ga|gb|gd|ge|gf|gg|gh|gi|gl|gm|gn|gp|gq|gr|gs|gt|gu|gw|gy|hk|hm|hn|hr|ht|hu|id|ie|il|im|in|io|iq|ir|is|it|je|jm|jo|jp|ke|kg|kh|ki|km|kn|kp|kr|kw|ky|kz|la|lb|lc|li|lk|lr|ls|lt|lu|lv|ly|ma|mc|md|mg|mh|mk|ml|mm|mn|mo|mp|mq|mr|ms|mt|mu|mv|mw|mx|my|mz|na|nc|ne|nf|ng|ni|nl|no|np|nr|nu|nz|om|pa|pe|pf|pg|ph|pk|pl|pm|pn|pr|ps|pt|pw|py|qa|re|ro|ru|rw|sa|sb|sc|sd|se|sg|sh|si|sj|sk|sl|sm|sn|so|sr|st|sv|sy|sz|tc|td|tf|tg|th|tj|tk|tl|tm|tn|to|tp|tr|tt|tv|tw|tz|ua|ug|uk|um|us|uy|uz|va|vc|ve|vg|vi|vn|vu|wf|ws|ye|yt|yu|za|zm|zw)(\\:[0-9]+)*(\\/($|[a-zA-Z0-9\\.\\,\\;\\?\\'\\\\\\+&%\\$#\\=~_\\-]+))*"

// cmdVarRegex = "\\(" ++ var ++ "((?: [\\w\\-:/!]+)*)\\)"

use crate::types::*;
use crate::commands::*;
use std::collections::HashMap;
use std::sync::Arc;
use config;
use reqwest;
use irc::client::prelude::*;
use reqwest::header;
use reqwest::header::HeaderValue;
use regex::{Regex,RegexBuilder,Captures};
use r2d2_redis::r2d2;
use r2d2_redis::redis::Commands;

pub fn send_message(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, mut message: String) {
    let me: String = con.hget(format!("channel:{}:settings", channel), "channel:me").unwrap_or("false".to_owned());
    if me == "true" { message = format!("/me {}", message); }
    let _ = client.send_privmsg(format!("#{}", channel), message);
}

pub fn send_parsed_message(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, mut message: String, args: &Vec<String>, irc_message: Option<&Message>) {
    if args.len() > 0 {
        if let Some(char) = args[args.len()-1].chars().next() {
            if char == '@' { message = format!("{} -> {}", args[args.len()-1], message) }
        }
    }
    let me: String = con.hget(format!("channel:{}:settings", channel), "channel:me").unwrap_or("false".to_owned());
    if me == "true" { message = format!("/me {}", message); }
    message = parse_message(&message, con.clone(), Some(&client), channel, irc_message, &args);
    let _ = client.send_privmsg(format!("#{}", channel), message);
}

pub fn request_get(url: &str) -> reqwest::Result<reqwest::Response> {
    let req = reqwest::Client::builder().http1_title_case_headers().build().unwrap();
    let rsp = req.get(url).send();
    return rsp;
}

pub fn request_post(url: &str, body: String) -> reqwest::Result<reqwest::Response> {
    let req = reqwest::Client::builder().http1_title_case_headers().build().unwrap();
    let rsp = req.post(url).body(body).send();
    return rsp;
}

pub fn spotify_request_get(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, channel: &str, url: &str) -> reqwest::Result<reqwest::Response> {
    let token: String = con.hget(format!("channel:{}:settings", channel), "spotify:token").unwrap_or("".to_owned());
    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.api+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());

    let req = reqwest::Client::builder().http1_title_case_headers().default_headers(headers).build().unwrap();
    let rsp = req.get(url).send();
    return rsp;
}

pub fn fortnite_request_get(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, channel: &str, url: &str) -> reqwest::Result<reqwest::Response> {
    let token: String = con.hget(format!("channel:{}:settings", channel), "fortnite:token").unwrap_or("".to_owned());
    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.api+json").unwrap());
    headers.insert("TRN-Api-Key", HeaderValue::from_str(&token).unwrap());

    let req = reqwest::Client::builder().http1_title_case_headers().default_headers(headers).build().unwrap();
    let rsp = req.get(url).send();
    return rsp;
}

pub fn pubg_request_get(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, channel: &str, url: &str) -> reqwest::Result<reqwest::Response> {
    let token: String = con.hget(format!("channel:{}:settings", channel), "pubg:token").unwrap_or("".to_owned());
    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.api+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());

    let req = reqwest::Client::builder().http1_title_case_headers().default_headers(headers).build().unwrap();
    let rsp = req.get(url).send();
    return rsp;
}

pub fn discord_request_post(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, channel: &str, url: &str, body: String) -> reqwest::Result<reqwest::Response> {
    let token: String = con.hget(format!("channel:{}:settings", channel), "discord:token").unwrap_or("".to_owned());
    let mut headers = header::HeaderMap::new();
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bot {}", token)).unwrap());
    headers.insert("User-Agent", HeaderValue::from_str("Babblebot (https://gitlab.com/toovs/babblebot, 0.1").unwrap());
    headers.insert("Content-Type", HeaderValue::from_str("application/json").unwrap());

    let req = reqwest::Client::builder().http1_title_case_headers().default_headers(headers).build().unwrap();
    let rsp = req.post(url).body(body).send();
    return rsp;
}

pub fn twitch_request_get(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, channel: &str, url: &str) -> reqwest::Result<reqwest::Response> {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let token: String = con.get(format!("channel:{}:token", channel)).expect("get:token");

    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("OAuth {}", token)).unwrap());
    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());

    let req = reqwest::Client::builder().http1_title_case_headers().default_headers(headers).build().unwrap();
    let rsp = req.get(url).send();
    return rsp;
}

pub fn twitch_request_put(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, channel: &str, url: &str, body: String) -> reqwest::Result<reqwest::Response> {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let token: String = con.get(format!("channel:{}:token", channel)).expect("get:token");

    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("OAuth {}", token)).unwrap());
    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());
    headers.insert("Content-Type", HeaderValue::from_str("application/x-www-form-urlencoded").unwrap());

    let req = reqwest::Client::builder().http1_title_case_headers().default_headers(headers).build().unwrap();
    let rsp = req.put(url).body(body).send();
    return rsp;
}

pub fn twitch_request_post(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, channel: &str, url: &str, body: String) -> reqwest::Result<reqwest::Response> {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let token: String = con.get(format!("channel:{}:token", channel)).expect("get:token");

    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("OAuth {}", token)).unwrap());
    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());
    headers.insert("Content-Type", HeaderValue::from_str("application/json").unwrap());

    let req = reqwest::Client::builder().http1_title_case_headers().default_headers(headers).build().unwrap();
    let rsp = req.post(url).body(body).send();
    return rsp;
}

pub fn parse_message(message: &str, con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: Option<&IrcClient>, channel: &str, irc_message: Option<&Message>, cargs: &Vec<String>) -> String {
    let mut msg: String = message.to_owned();
    let mut vars: Vec<(&str,String)> = Vec::new();

    let rgx = Regex::new("\\(var ?((?:[\\w\\-\\?\\._:/&!= ]+)*)\\)").unwrap();
    for captures in rgx.captures_iter(message) {
        if let Some(capture) = captures.get(1) {
            let capture: Vec<&str> = capture.as_str().split_whitespace().collect();
            let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, capture[0]), "message");
            if let Ok(cmd) = res {
                let mut cmd_message = cmd;
                for var in command_vars.iter() {
                    if var.0 != "cmd" {
                        let captures: Vec<String> = capture[1..].iter().map(|a| a.to_string()).collect();
                        cmd_message = parse_var(var, &cmd_message, con.clone(), client, channel, irc_message, &captures);
                    }
                }
                cmd_message = parse_code(&cmd_message);
                vars.push((capture[0], cmd_message));
            }
            msg = rgx.replace(&msg, |_: &Captures| { "" }).to_string();
        }
    }

    for var in vars.iter() {
        let rgx = Regex::new(&format!("\\({}\\)", var.0)).unwrap();
        msg = rgx.replace_all(&msg, |_: &Captures| { (var.1).to_owned() }).to_string();
    }

    for var in command_vars.iter() {
        msg = parse_var(var, &msg, con.clone(), client, channel, irc_message, &cargs);
    }

    msg = parse_code(&msg);

    return msg;
}

pub fn parse_var(var: &(&str, fn(Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, Option<&IrcClient>, &str, Option<&Message>, Vec<&str>, &Vec<String>) -> String), message: &str, con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: Option<&IrcClient>, channel: &str, irc_message: Option<&Message>, cargs: &Vec<String>) -> String {
    let rgx = Regex::new(&format!("\\({} ?((?:[\\w\\-\\?\\._:/&!= ]+)*)\\)", var.0)).unwrap();
    let mut msg: String = message.to_owned();
    let mut vargs: Vec<&str> = Vec::new();
    for captures in rgx.captures_iter(message) {
        if let Some(capture) = captures.get(1) {
            vargs = capture.as_str().split_whitespace().collect();
            let res = (var.1)(con.clone(), client, channel, irc_message, vargs, cargs);
            msg = rgx.replace(&msg, |_: &Captures| { &res }).to_string();
        }
    }

    return msg.to_owned();
}

pub fn parse_code(message: &str) -> String {
    let mut msg: String = message.to_owned();
    let rgx = Regex::new("\\{-(.+?)\\-}").unwrap();
    for captures in rgx.captures_iter(&msg.clone()) {
        if let Some(capture) = captures.get(1) {
            let rsp = request_post("http://localhost:9412/execute", format!("function() {{ {} }}", capture.as_str()));
            match rsp {
                Err(e) => error!("[parse_code] {}", e),
                Ok(mut rsp) => { msg = rgx.replace(&msg, |_: &Captures| { strip_chars(&rsp.text().unwrap(), "\"") }).to_string(); }
            }
        }
    }

    return msg.to_owned();
}

pub fn replace_var(var: &str, val: &str, msg: &str) -> String {
    let rgx = Regex::new(&format!("\\({}\\)", var)).unwrap();
    let mut message: String = msg.to_owned();
    for captures in rgx.captures_iter(&msg) {
        if let Some(capture) = captures.get(0) {
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
