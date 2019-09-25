// [("bits", commandBits, Mod, Mod), ("greetings", commandGreetings, Mod, Mod), ("giveaway", commandGiveaway, Mod, Mod), ("poll", commandPoll, Mod, Mod), ("watchtime", commandWatchtime, Mod, Mod), ("clip", commandClip, All, All), , ("genwebauth", commandWebAuth, Mod, Mod), ("listads", commandListCommercials, Mod, Mod), ("listsettings", commandListSettings, Mod, Mod), ("unmod", commandUnmod, Mod, Mod)]

// [("watchtime", watchtimeVar), ("watchrank", watchrankVar), ("watchranks", watchranksVar), ("hotkey", hotkeyVar), ("obs:scene-change", obsSceneChangeVar), ("fortnite:wins", fortWinsVar), ("fortnite:kills", fortKillsVar), ("fortnite:lifewins", fortLifeWinsVar), ("fortnite:lifekills", fortLifeKillsVar), ("fortnite:solowins", fortSoloWinsVar), ("fortnite:solokills", fortSoloKillsVar), ("fortnite:duowins", fortDuoWinsVar), ("fortnite:duokills", fortDuoKillsVar), ("fortnite:squadwins", fortSquadWinsVar), ("fortnite:squadkills", fortSquadKillsVar), ("fortnite:season-solowins", fortSeasonSoloWinsVar), ("fortnite:season-solokills", fortSeasonSoloKillsVar), ("fortnite:season-duowins", fortSeasonDuoWinsVar), ("fortnite:season-duokills", fortSeasonDuoKillsVar), ("fortnite:season-squadwins", fortSeasonSquadWinsVar), ("fortnite:season-squadkills", fortSeasonSquadKillsVar)]

use crate::types::*;
use crate::util::*;

use std::mem;
use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use std::{thread,time};
use std::str::FromStr;
use either::Either::{Left, Right};
use tokio;
use bcrypt::{DEFAULT_COST, hash};
use regex::Regex;
use crossbeam_channel::{Sender,Receiver};
use reqwest::Method;
use reqwest::r#async::{RequestBuilder,Chunk,Decoder};
use irc::client::prelude::*;
use chrono::{Utc, DateTime, FixedOffset, Duration};
use humantime::format_duration;
use itertools::Itertools;
use redis::{self,Value,from_redis_value};

pub const native_commands: [(&str, fn(Arc<IrcClient>, String, Vec<String>, Option<Message>, (Sender<Vec<String>>, Receiver<Result<Value, String>>)), bool, bool); 15] = [("echo", echo_cmd, true, true), ("set", set_cmd, true, true), ("unset", unset_cmd, true, true), ("command", command_cmd, true, true), ("title", title_cmd, false, true), ("game", game_cmd, false, true), ("notices", notices_cmd, true, true), ("moderation", moderation_cmd, true, true), ("permit", permit_cmd, true, true), ("multi", multi_cmd, false, true), ("clip", clip_cmd, true, true), ("counters", counters_cmd, true, true), ("phrases", phrases_cmd, true, true), ("commercials", commercials_cmd, true, true), ("songreq", songreq_cmd, true, false)];

pub const command_vars: [(&str, fn(Option<Arc<IrcClient>>, String, Option<Message>, Vec<String>, Vec<String>, (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String); 22] = [("args", args_var), ("user", user_var), ("channel", channel_var), ("cmd", cmd_var), ("counterinc", counterinc_var), ("counter", counter_var), ("phrase", phrase_var), ("countdown", countdown_var), ("time", time_var), ("date", date_var), ("dateinc", dateinc_var), ("watchtime", watchtime_var), ("watchrank", watchrank_var), ("fortnite:wins", fortnite_wins_var), ("fortnite:kills", fortnite_kills_var), ("pubg:damage", pubg_damage_var), ("pubg:headshots", pubg_headshots_var), ("pubg:kills", pubg_kills_var), ("pubg:roadkills", pubg_roadkills_var), ("pubg:teamkills", pubg_teamkills_var), ("pubg:vehicles-destroyed", pubg_vehicles_destroyed_var), ("pubg:wins", pubg_wins_var)];

pub const command_vars_async: [(&str, fn(Option<Arc<IrcClient>>, String, Option<Message>, Vec<String>, Vec<String>, (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> Option<(RequestBuilder, fn((String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)) -> String)>); 10] = [("uptime", uptime_var), ("followage", followage_var), ("subcount", subcount_var), ("followcount", followcount_var), ("urlfetch", urlfetch_var), ("spotify:playing-title", spotify_playing_title_var), ("spotify:playing-album", spotify_playing_album_var), ("spotify:playing-artist", spotify_playing_artist_var), ("youtube:latest-url", youtube_latest_url_var), ("youtube:latest-title", youtube_latest_title_var)];

pub const twitch_bots: [&str; 20] = ["electricallongboard","lanfusion","cogwhistle","freddyybot","anotherttvviewer","apricotdrupefruit","skinnyseahorse","p0lizei_","xbit01","n3td3v","cachebear","icon_bot","virgoproz","v_and_k","slocool","host_giveaway","nightbot","commanderroot","p0sitivitybot","streamlabs"];

fn args_var(_client: Option<Arc<IrcClient>>, _channel: String, _message: Option<Message>, vargs: Vec<String>, cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if vargs.len() > 0 {
        let num: Result<usize,_> = vargs[0].parse();
        match num {
            Err(_) => "".to_owned(),
            Ok(num) => {
                if num > 0 && cargs.len() >= num {
                    cargs[num-1].to_owned()
                } else {
                    "".to_owned()
                }
            }
        }
    } else {
        cargs.join(" ").to_owned()
    }
}

fn cmd_var(client: Option<Arc<IrcClient>>, channel: String, irc_message: Option<Message>, vargs: Vec<String>, cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if let Some(client) = client {
        if vargs.len() > 0 {
            let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:commands:{}", channel, vargs[0]), "message"]);
            if let Ok(value) = res {
                let mut message: String = from_redis_value(&value).unwrap();
                for var in command_vars.iter() {
                    if var.0 != "cmd" && var.0 != "var" {
                        let args: Vec<String> = vargs[1..].iter().map(|a| a.to_string()).collect();
                        message = parse_var(var, &message, Some(client.clone()), channel.clone(), irc_message.clone(), args, db.clone());
                    }
                }
                send_parsed_message(client, channel.to_owned(), message, cargs, irc_message, db.clone(), false);
            } else {
                for cmd in native_commands.iter() {
                    if format!("!{}", cmd.0) == vargs[0] {
                        let args: Vec<String> = vargs[1..].iter().map(|a| a.to_string()).collect();
                        match irc_message.clone() {
                            None => { (cmd.1)(client.clone(), channel.to_owned(), args, None, db.clone()) }
                            Some(msg) => { (cmd.1)(client.clone(), channel.to_owned(), args, Some(msg), db.clone()) }
                        }
                    }
                }
            }
        }
    }
    "".to_owned()
}

fn uptime_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> Option<(RequestBuilder, fn((String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)) -> String)> {
    let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", &channel)]).expect(&format!("channel:{}:id", &channel))).unwrap();
    let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
    let builder = twitch_kraken_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/streams?channel={}", &id));
    let func = move |(channel, db, body): (String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)| -> String {
        let body = std::str::from_utf8(&body).unwrap().to_string();
        let json: Result<KrakenStreams,_> = serde_json::from_str(&body);
        match json {
            Err(e) => {
                log_error(None, "uptime_var", &e.to_string(), db.clone());
                log_error(None, "request_body", &body.to_string(), db.clone());
                "".to_owned()
            }
            Ok(json) => {
                if json.total > 0 {
                    let dt = DateTime::parse_from_rfc3339(&json.streams[0].created_at).unwrap();
                    let diff = Utc::now().signed_duration_since(dt);
                    let formatted = format_duration(diff.to_std().unwrap()).to_string();
                    let formatted: Vec<&str> = formatted.split_whitespace().collect();
                    if formatted.len() == 1 {
                        format!("{}", formatted[0])
                    } else {
                        format!("{}{}", formatted[0], formatted[1])
                    }
                } else {
                    "".to_owned()
                }
            }
        }
    };
    return Some((builder, func));
}

fn user_var(_client: Option<Arc<IrcClient>>, _channel: String, message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if let Some(message) = message {
        let mut display = get_nick(&message);
        if let Some(tags) = &message.tags {
            tags.iter().for_each(|tag| {
                if let Some(_value) = &tag.1 {
                    if tag.0 == "display-name" {
                        if let Some(name) = &tag.1 {
                            display = name.to_owned();
                        }
                    }
                }
            });
        }
        display
    } else {
        "".to_owned()
    }
}

fn channel_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    let display: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:display-name", channel)]).expect(&format!("channel:{}:display-name", channel))).unwrap();
    display
}

fn counterinc_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if vargs.len() > 0 {
        let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:counters", channel), &vargs[0]]);
        if let Ok(value) = res {
            let counter: String = from_redis_value(&value).unwrap();
            let res: Result<u16,_> = counter.parse();
            if let Ok(num) = res {
                redis_call(db.clone(), vec!["hset", &format!("channel:{}:counters", channel), &vargs[0], (num + 1).to_string().as_ref()]);
            } else {
                redis_call(db.clone(), vec!["hset", &format!("channel:{}:counters", channel), &vargs[0], "1"]);
            }
        } else {
            redis_call(db.clone(), vec!["hset", &format!("channel:{}:counters", channel), &vargs[0], "1"]);
        }
    }

    "".to_owned()
}

fn followage_var(_client: Option<Arc<IrcClient>>, channel: String, message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> Option<(RequestBuilder, fn((String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)) -> String)> {
    if let Some(message) = message {
        let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
        let user_id = get_id(&message).unwrap();
        let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
        let builder = twitch_kraken_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/users/{}/follows/channels/{}", &user_id, &id));
        let func = move |(channel, db, body): (String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)| -> String {
            let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
            let body = std::str::from_utf8(&body).unwrap().to_string();
            let json: Result<KrakenFollow,_> = serde_json::from_str(&body);
            match json {
                Err(e) => { log_error(Some(Right(vec![&channel])), "followage_var", &e.to_string(), db.clone()); "0m".to_owned() }
                Ok(json) => {
                    let timestamp = DateTime::parse_from_rfc3339(&json.created_at).unwrap();
                    let diff = Utc::now().signed_duration_since(timestamp);
                    let formatted = format_duration(diff.to_std().unwrap()).to_string();
                    let formatted: Vec<&str> = formatted.split_whitespace().collect();
                    if formatted.len() == 1 {
                        format!("{}", formatted[0])
                    } else {
                        format!("{} and {}", formatted[0], formatted[1])
                    }
                }
            }
        };
        return Some((builder, func));
    } else { None }
}

fn subcount_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> Option<(RequestBuilder, fn((String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)) -> String)> {
    let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
    let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
    let builder = twitch_kraken_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}/subscriptions", &id));
    let func = move |(channel, db, body): (String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)| -> String {
        let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
        let body = std::str::from_utf8(&body).unwrap().to_string();
        let json: Result<KrakenSubs,_> = serde_json::from_str(&body);
        match json {
            Err(e) => { log_error(Some(Right(vec![&channel])), "subcount_var", &e.to_string(), db.clone()); log_error(Some(Right(vec![&channel])), "subcount_var", &body, db.clone()); "0".to_owned() }
            Ok(json) => { json.total.to_string() }
        }
    };
    return Some((builder, func));
}

fn followcount_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> Option<(RequestBuilder, fn((String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)) -> String)> {
    let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
    let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
    let builder = twitch_kraken_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}/follows", &id));
    let func = move |(channel, db, body): (String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)| -> String {
        let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
        let body = std::str::from_utf8(&body).unwrap().to_string();
        let json: Result<KrakenFollows,_> = serde_json::from_str(&body);
        match json {
            Err(e) => { log_error(Some(Right(vec![&channel])), "followcount_var", &e.to_string(), db.clone()); "0".to_owned() }
            Ok(json) => { json.total.to_string() }
        }
    };
    return Some((builder, func));
}

fn counter_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if vargs.len() > 0 {
        let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:counters", channel), &vargs[0]]);
        if let Ok(value) = res {
            let counter: String = from_redis_value(&value).unwrap();
            let num: Result<u16,_> = counter.parse();
            match num {
                Ok(num) => num.to_string(),
                Err(_) => "".to_owned()
            }
        } else {
            "0".to_owned()
        }
    } else {
        "".to_owned()
    }
}

fn phrase_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if vargs.len() > 0 {
        let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:phrases", channel), &vargs[0]]);
        if let Ok(value) = res {
            let phrase: String = from_redis_value(&value).unwrap();
            return phrase;
        } else {
            "".to_owned()
        }
    } else {
        "".to_owned()
    }
}

fn time_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if vargs.len() > 0 {
        if let Ok(tz) = chrono_tz::Tz::from_str(&vargs[0]) {
            let date = Utc::now().with_timezone(&tz);
            return date.format("%l:%M%P").to_string();
        } else {
            "".to_string()
        }
    } else {
        "".to_string()
    }
}

fn date_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if vargs.len() > 0 {
        let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:phrases", channel), &vargs[0]]);
        if let Ok(value) = res {
            let phrase: String = from_redis_value(&value).unwrap();
            let res: Result<DateTime<FixedOffset>,_> = DateTime::parse_from_rfc3339(&phrase);
            if let Ok(dt) = res {
                dt.format("%b %e %Y").to_string()
            } else {
                "".to_string()
            }
        } else {
            "".to_string()
        }
    } else {
        "".to_string()
    }
}

fn dateinc_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if vargs.len() > 1 {
        let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:phrases", channel), &vargs[0]]);
        if let Ok(value) = res {
            let phrase: String = from_redis_value(&value).unwrap();
            let res: Result<DateTime<FixedOffset>,_> = DateTime::parse_from_rfc3339(&phrase);
            if let Ok(dt) = res {
                let res: Result<i64,_> = vargs[1].parse();
                if let Ok(num) = res {
                    let ndt = dt + Duration::seconds(num);
                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:phrases", channel), &vargs[0], &ndt.to_rfc3339()]);
                }
            }
        }
    }
    "".to_owned()
}

fn countdown_var(_client: Option<Arc<IrcClient>>, _channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if vargs.len() > 0 {
        let dt = DateTime::parse_from_str(&vargs[0], "%Y-%m-%dT%H:%M%z");
        if let Ok(timestamp) = dt {
            let diff = timestamp.signed_duration_since(Utc::now());
            if let Ok(std) = diff.to_std() {
                let formatted = format_duration(std).to_string();
                let formatted: Vec<&str> = formatted.split_whitespace().collect();
                if formatted.len() == 1 {
                    format!("{}", formatted[0])
                } else {
                    format!("{} and {}", formatted[0], formatted[1])
                }
            } else {
                "".to_owned()
            }
        } else {
            "".to_owned()
        }
    } else {
        "".to_owned()
    }
}

fn watchtime_var(_client: Option<Arc<IrcClient>>, channel: String, message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if let Some(message) = message {
        let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:watchtimes", channel), &get_nick(&message)]);
        if let Ok(value) = res {
            let watchtime: String = from_redis_value(&value).unwrap();
            let res: Result<i64,_> = watchtime.parse();
            if let Ok(num) = res {
                let diff = Duration::minutes(num);
                let formatted = format_duration(diff.to_std().unwrap()).to_string();
                let formatted: Vec<&str> = formatted.split_whitespace().collect();
                if formatted.len() == 1 {
                    format!("{}", formatted[0])
                } else {
                    format!("{} and {}", formatted[0], formatted[1])
                }
            } else {
                "0m".to_owned()
            }
        } else {
            "0m".to_owned()
        }
    } else {
        "0m".to_owned()
    }
}

fn watchrank_var(_client: Option<Arc<IrcClient>>, channel: String, message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    if let Some(message) = message {
        let hashtimes: HashMap<String,String> = from_redis_value(&redis_call(db.clone(), vec!["hgetall", &format!("channel:{}:watchtimes", channel)]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
        let mut watchtimes: Vec<(String,u64)> = Vec::new();
        for (key, value) in hashtimes.iter() {
            let num: u64 = value.parse().unwrap();
            watchtimes.push((key.to_owned(), num));
        }
        let list: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:settings", channel), "watchtime:blacklist"]).unwrap_or(Value::Data("".as_bytes().to_owned()))).unwrap();
        let mut blacklist: Vec<&str> = twitch_bots.clone().to_vec();
        blacklist.push(&channel);
        for nick in list.split_whitespace() { blacklist.push(nick) }
        watchtimes.sort_by(|a,b| (b.1).partial_cmp(&a.1).unwrap());
        watchtimes = watchtimes.iter().filter(|wt| !blacklist.contains(&&(*wt.0))).map(|x| x.to_owned()).collect();
        if watchtimes.len() > 0 {
            if vargs.len() > 0 {
                let res: Result<usize,_> = vargs[0].parse();
                if let Ok(num) = res {
                    let top: Vec<(String,u64)>;
                    if watchtimes.len() < num {
                        top = watchtimes.drain(..).collect();
                    } else {
                        top = watchtimes.drain(..num).collect();
                    }
                    top.iter().map(|(n, _t)| n).join(", ").to_owned()
                } else {
                    "".to_owned()
                }
            } else {
                let i = watchtimes.iter().position(|wt| wt.0 == get_nick(&message));
                if let Some(i) = i {
                    (i + 1).to_string()
                } else {
                    "".to_owned()
                }
            }
        } else {
            "".to_owned()
        }
    } else {
        "".to_owned()
    }
}

fn urlfetch_var(_client: Option<Arc<IrcClient>>, _channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> Option<(RequestBuilder, fn((String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)) -> String)> {
    if vargs.len() > 0 {
        let builder = request(Method::GET, None, &vargs[0]);
        let func = move |(channel, db, body): (String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)| -> String {
            let body = std::str::from_utf8(&body).unwrap();
            return body.to_owned();
        };
        return Some((builder, func));
    } else { None }
}

fn spotify_playing_title_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> Option<(RequestBuilder, fn((String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)) -> String)> {
    let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:spotify:token", &channel)]).expect(&format!("channel:{}:spotify:token", &channel))).unwrap();
    let builder = spotify_request(token, Method::GET, "https://api.spotify.com/v1/me/player/currently-playing", None);
    let func = move |(channel, db, body): (String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)| -> String {
        let body = std::str::from_utf8(&body).unwrap();
        let json: Result<SpotifyPlaying,_> = serde_json::from_str(&body);
        match json {
            Err(e) => {
                if !body.is_empty() {
                    log_error(None, "spotify_playing_title_var", &e.to_string(), db.clone());
                    log_error(None, "request_body", &body, db.clone());
                }
                "".to_owned()
            }
            Ok(json) => { json.item.name.to_owned() }
        }
    };
    return Some((builder, func));
}

fn spotify_playing_album_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> Option<(RequestBuilder, fn((String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)) -> String)> {
    let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:spotify:token", &channel)]).expect(&format!("channel:{}:spotify:token", &channel))).unwrap();
    let builder = spotify_request(token, Method::GET, "https://api.spotify.com/v1/me/player/currently-playing", None);
    let func = move |(channel, db, body): (String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)| -> String {
        let body = std::str::from_utf8(&body).unwrap();
        let json: Result<SpotifyPlaying,_> = serde_json::from_str(&body);
        match json {
            Err(e) => {
                if !body.is_empty() {
                    log_error(None, "spotify_playing_album_var", &e.to_string(), db.clone());
                    log_error(None, "request_body", &body, db.clone());
                }
                "".to_owned()
            }
            Ok(json) => { json.item.album.name.to_owned() }
        }
    };
    return Some((builder, func));
}

fn spotify_playing_artist_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> Option<(RequestBuilder, fn((String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)) -> String)> {
    let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:spotify:token", &channel)]).expect(&format!("channel:{}:spotify:token", &channel))).unwrap();
    let builder = spotify_request(token, Method::GET, "https://api.spotify.com/v1/me/player/currently-playing", None);
    let func = move |(channel, db, body): (String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)| -> String {
        let body = std::str::from_utf8(&body).unwrap();
        let json: Result<SpotifyPlaying,_> = serde_json::from_str(&body);
        match json {
            Err(e) => {
                if !body.is_empty() {
                    log_error(None, "spotify_playing_artist_var", &e.to_string(), db.clone());
                    log_error(None, "request_body", &body, db.clone());
                }
                "".to_owned()
            }
            Ok(json) => { json.item.artists[0].name.to_owned() }
        }
    };
    return Some((builder, func));
}

fn youtube_latest_url_var(_client: Option<Arc<IrcClient>>, _channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> Option<(RequestBuilder, fn((String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)) -> String)> {
    if vargs.len() > 0 {
        let builder = request(Method::GET, None, &format!("https://decapi.me/youtube/latest_video?id={}", vargs[0]));
        let func = move |(channel, db, body): (String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)| -> String {
            let body = std::str::from_utf8(&body).unwrap();
            let data: Vec<&str> = body.split(" - ").collect();
            if data.len() > 1 {
                data[data.len()-1].to_owned()
            } else {
                "".to_owned()
            }
        };
        return Some((builder, func));
    } else { None }
}

fn youtube_latest_title_var(_client: Option<Arc<IrcClient>>, _channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> Option<(RequestBuilder, fn((String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)) -> String)> {
    if vargs.len() > 0 {
        let builder = request(Method::GET, None, &format!("https://decapi.me/youtube/latest_video?id={}", vargs[0]));
        let func = move |(channel, db, body): (String, (Sender<Vec<String>>, Receiver<Result<Value, String>>), Chunk)| -> String {
            let body = std::str::from_utf8(&body).unwrap();
            let data: Vec<&str> = body.split(" - ").collect();
            if data.len() > 1 {
                data[0..data.len()-1].join(" - ")
            } else {
                "".to_owned()
            }
        };
        return Some((builder, func));
    } else { None }
}

fn fortnite_wins_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    let value: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:fortnite", channel), "wins"]).unwrap_or(Value::Data("0".as_bytes().to_owned()))).unwrap();
    value
}

fn fortnite_kills_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    let value: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:fortnite", channel), "kills"]).unwrap_or(Value::Data("0".as_bytes().to_owned()))).unwrap();
    value
}

fn pubg_damage_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    let value: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:pubg", channel), "damageDealt"]).unwrap_or(Value::Data("0".as_bytes().to_owned()))).unwrap();
    value
}

fn pubg_headshots_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    let value: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:pubg", channel), "headshotKills"]).unwrap_or(Value::Data("0".as_bytes().to_owned()))).unwrap();
    value
}

fn pubg_kills_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    let value: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:pubg", channel), "kills"]).unwrap_or(Value::Data("0".as_bytes().to_owned()))).unwrap();
    value
}

fn pubg_roadkills_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    let value: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:pubg", channel), "roadKills"]).unwrap_or(Value::Data("0".as_bytes().to_owned()))).unwrap();
    value
}

fn pubg_teamkills_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    let value: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:pubg", channel), "teamKills"]).unwrap_or(Value::Data("0".as_bytes().to_owned()))).unwrap();
    value
}

fn pubg_vehicles_destroyed_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    let value: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:pubg", channel), "vehicleDestroys"]).unwrap_or(Value::Data("0".as_bytes().to_owned()))).unwrap();
    value
}

fn pubg_wins_var(_client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) -> String {
    let value: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:stats:pubg", channel), "wins"]).unwrap_or(Value::Data("0".as_bytes().to_owned()))).unwrap();
    value
}

fn echo_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    send_message(client, channel, args.join(" "), db.clone());
}

fn set_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    match args.len() {
        0 => {}
        1 => {
            redis_call(db.clone(), vec!["hset", &format!("channel:{}:settings", channel), &args[0], "true"]);
            send_message(client, channel, format!("{} has been set to: true", args[0]), db.clone());
        }
        _ => {
            redis_call(db.clone(), vec!["hset", &format!("channel:{}:settings", channel), &args[0], &args[1..].join(" ")]);
            send_message(client, channel, format!("{} has been set to: {}", &args[0], args[1..].join(" ")), db.clone());
        }
    }
}

fn unset_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    if args.len() == 1 {
        redis_call(db.clone(), vec!["hdel", &format!("channel:{}:settings", channel), &args[0]]);
        send_message(client, channel, format!("{} has been unset", &args[0]), db.clone());
    }
}

fn command_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    if args.len() > 1 {
        match args[0].to_lowercase().as_ref() {
            "add" => {
                if args.len() > 2 {
                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:commands:{}", channel, &args[1].to_lowercase()), "message", &args[2..].join(" ")]);
                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:commands:{}", channel, &args[1].to_lowercase()), "cmd_protected", "false"]);
                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:commands:{}", channel, &args[1].to_lowercase()), "arg_protected", "false"]);
                    send_message(client, channel, format!("{} has been added", &args[1]), db.clone());
                }
            }
            "modadd" => {
                if args.len() > 2 {
                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:commands:{}", channel, &args[1].to_lowercase()), "message", &args[2..].join(" ")]);
                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:commands:{}", channel, &args[1].to_lowercase()), "cmd_protected", "true"]);
                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:commands:{}", channel, &args[1].to_lowercase()), "arg_protected", "true"]);
                    send_message(client, channel, format!("{} has been added", &args[1]), db.clone());
                }
            }
            "remove" => {
                redis_call(db.clone(), vec!["del", &format!("channel:{}:commands:{}", channel, &args[1].to_lowercase())]);
                send_message(client, channel, format!("{} has been removed", &args[1]), db.clone());
            }
            "alias" => {
                if args.len() > 2 {
                    // TODO: validate command exists
                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:aliases", channel), &args[1].to_lowercase(), &args[2..].join(" ").to_lowercase()]);
                    send_message(client, channel, format!("{} has been added as an alias to {}", &args[1].to_lowercase(), args[2..].join(" ").to_lowercase()), db.clone());
                }
            }
            "remalias" => {
                redis_call(db.clone(), vec!["hdel", &format!("channel:{}:aliases", channel), &args[1].to_lowercase()]);
                send_message(client, channel, format!("{} has been removed as an alias", &args[1].to_lowercase()), db.clone());
            }
            _ => {}
        }
    }
}

fn title_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
    if args.len() == 0 {
        let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
        let future = twitch_kraken_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}", &id)).send()
            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
            .map_err(|e| println!("request error: {}", e))
            .map(move |body| {
                let body = std::str::from_utf8(&body).unwrap().to_string();
                let json: Result<KrakenChannel,_> = serde_json::from_str(&body);
                match json {
                    Err(e) => {
                        log_error(Some(Right(vec![&channel])), "title_cmd", &e.to_string(), db.clone());
                        log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                    }
                    Ok(json) => { let _ = send_message(client, channel, json.status, db.clone()); }
                }
            });
        thread::spawn(move || { tokio::run(future) });
    } else {
        let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
        let future = twitch_kraken_request(token, Some("application/x-www-form-urlencoded"), Some(format!("channel[status]={}", args.join(" ")).as_bytes().to_owned()), Method::PUT, &format!("https://api.twitch.tv/kraken/channels/{}", &id)).send()
            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
            .map_err(|e| println!("request error: {}", e))
            .map(move |body| {
                let body = std::str::from_utf8(&body).unwrap().to_string();
                let json: Result<KrakenChannel,_> = serde_json::from_str(&body);
                match json {
                    Err(e) => {
                        log_error(Some(Right(vec![&channel])), "title_cmd", &e.to_string(), db.clone());
                        log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                    }
                    Ok(json) => { send_message(client, channel, format!("Title is now set to: {}", json.status), db.clone()); }
                }
            });
        thread::spawn(move || { tokio::run(future) });
    }
}

fn game_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
    if args.len() == 0 {
        let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
        let future = twitch_kraken_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}", &id)).send()
            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
            .map_err(|e| println!("request error: {}", e))
            .map(move |body| {
                let body = std::str::from_utf8(&body).unwrap().to_string();
                let json: Result<KrakenChannel,_> = serde_json::from_str(&body);
                match json {
                    Err(e) => {
                        log_error(Some(Right(vec![&channel])), "game_cmd", &e.to_string(), db.clone());
                        log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                    }
                    Ok(json) => { let _ = send_message(client, channel, json.game, db.clone()); }
                }
            });
        thread::spawn(move || { tokio::run(future) });
    } else {
        let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
        let future = twitch_helix_request(token, None, None, Method::GET, &format!("https://api.twitch.tv/helix/games?name={}", args.join(" "))).send()
            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
            .map_err(|e| println!("request error: {}", e))
            .map(move |body| {
                let body = std::str::from_utf8(&body).unwrap().to_string();
                let json: Result<HelixGames,_> = serde_json::from_str(&body);
                match json {
                    Err(e) => {
                        log_error(Some(Right(vec![&channel])), "game_cmd", &e.to_string(), db.clone());
                        log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                    }
                    Ok(json) => {
                        if json.data.len() == 0 {
                            send_message(client, channel, format!("Unable to find a game matching: {}", args.join(" ")), db.clone());
                        } else {
                            let name = json.data[0].name.clone();
                            let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
                            let future = twitch_kraken_request(token, Some("application/x-www-form-urlencoded"), Some(format!("channel[game]={}", name).as_bytes().to_owned()), Method::PUT, &format!("https://api.twitch.tv/kraken/channels/{}", &id)).send()
                                .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                .map_err(|e| println!("request error: {}", e))
                                .map(move |body| {
                                    let body = std::str::from_utf8(&body).unwrap().to_string();
                                    let json: Result<KrakenChannel,_> = serde_json::from_str(&body);
                                    match json {
                                        Err(e) => {
                                            log_error(Some(Right(vec![&channel])), "game_cmd", &e.to_string(), db.clone());
                                            log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                                        }
                                        Ok(_json) => { send_message(client, channel, format!("Game is now set to: {}", &name), db.clone()); }
                                    }
                                });
                            thread::spawn(move || { tokio::run(future) });
                        }
                    }
                }
            });
        thread::spawn(move || { tokio::run(future) });
    }
}

fn notices_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    if args.len() > 1 {
        match args[0].to_lowercase().as_ref() {
            "add" => {
                let num: Result<u16,_> = args[1].parse();
                match num {
                    Ok(num) => {
                        if num % 60 == 0 {
                            redis_call(db.clone(), vec!["rpush", &format!("channel:{}:notices:{}:commands", channel, args[1]), &args[2]]);
                            redis_call(db.clone(), vec!["set", &format!("channel:{}:notices:{}:countdown", channel, args[1]), &args[1]]);
                            send_message(client, channel, "notice has been added".to_owned(), db.clone());
                        } else {
                            send_message(client, channel, "notice interval must be a multiple of 60".to_owned(), db.clone());
                        }
                    }
                    Err(_) => {}
                }
            }
            _ => {}
        }
    }
}

fn moderation_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    if args.len() > 1 {
        match args[0].to_lowercase().as_ref() {
            "links" => {
                match args[1].to_lowercase().as_ref() {
                    "add" => {
                        if args.len() > 2 {
                            redis_call(db.clone(), vec!["srem", &format!("channel:{}:moderation:links", channel), &args[2]]);
                            send_message(client, channel, format!("{} has been whitelisted", &args[2]), db.clone());
                        }
                    }
                    "remove" => {
                        if args.len() > 2 {
                            redis_call(db.clone(), vec!["srem", &format!("channel:{}:moderation:links", channel), &args[2]]);
                            send_message(client, channel, format!("{} has been removed from the whitelist", &args[2]), db.clone());
                        }
                    }
                    "allowsubs" => {
                        redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:links:subs", channel), "true"]);
                        send_message(client, channel, "Subs are now allowed to post links".to_owned(), db.clone());
                    }
                    "blocksubs" => {
                        redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:links:subs", channel), "false"]);
                        send_message(client, channel, "Subs are not allowed to post links".to_owned(), db.clone());
                    }
                    _ => {}
                }
            }
            "colors" => {
                match args[1].to_lowercase().as_ref() {
                    "on" => {
                        redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:colors", channel), "true"]);
                        send_message(client, channel, "Color filter has been turned on".to_owned(), db.clone());
                    }
                    "off" => {
                        redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:colors", channel), "true"]);
                        send_message(client, channel, "Color filter has been turned off".to_owned(), db.clone());
                    }
                    _ => {}
                }
            }
            "caps" => {
                match args[1].to_lowercase().as_ref() {
                    "set" => {
                        if args.len() > 3 {
                            redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:caps", channel), "true"]);
                            redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:caps:limit", channel), &args[2]]);
                            redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:caps:trigger", channel), &args[3]]);
                            if args.len() > 4 { redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:caps:subs", channel), &args[4]]); }
                            send_message(client, channel, "Caps filter has been turned on".to_owned(), db.clone());
                        }
                    }
                    "off" => {
                        redis_call(db.clone(), vec!["del", &format!("channel:{}:moderation:caps", channel)]);
                        send_message(client, channel, "Caps filter has been turned off".to_owned(), db.clone());
                    }
                    _ => {}
                }
            }
            "age" => {
                match args[1].to_lowercase().as_ref() {
                    "set" => {
                        if args.len() > 2 {
                            redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:age", channel), &args[2]]);
                            send_message(client, channel, format!("Minimum account age has been set to: {}", &args[2]).to_owned(), db.clone());
                        }
                    }
                    "off" => {
                        redis_call(db.clone(), vec!["del", &format!("channel:{}:moderation:age", channel)]);
                        send_message(client, channel, "Minimum account age filter has been turned off".to_owned(), db.clone());
                    }
                    _ => {}
                }
            }
            "display" => {
                match args[1].to_lowercase().as_ref() {
                    "on" => {
                        redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:display", channel), "true"]);
                        send_message(client, channel, "Displaying timeout reasons has been turned on".to_owned(), db.clone());
                    }
                    "off" => {
                        redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:display", channel), "false"]);
                        send_message(client, channel, "Displaying timeout reasons has been turned off".to_owned(), db.clone());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn permit_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    if args.len() > 0 {
        let nick = args[0].to_lowercase();
        redis_call(db.clone(), vec!["set", &format!("channel:{}:moderation:permitted:{}", channel, nick), ""]);
        redis_call(db.clone(), vec!["expire", &format!("channel:{}:moderation:permitted:{}", channel, nick), "30"]);
        send_message(client, channel, format!("{} can post links for the next 30 seconds", nick), db.clone());
    }
}

fn clip_cmd(client: Arc<IrcClient>, channel: String, _args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
    let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
    let future = twitch_helix_request(token, None, None, Method::POST, &format!("https://api.twitch.tv/helix/clips?broadcaster_id={}", &id)).send()
        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
        .map_err(|e| println!("request error: {}", e))
        .map(move |body| {
            let body = std::str::from_utf8(&body).unwrap().to_string();
            let json: Result<HelixClips,_> = serde_json::from_str(&body);
            match json {
                Err(e) => {
                    log_error(Some(Right(vec![&channel])), "clip_cmd", &e.to_string(), db.clone());
                    log_error(Some(Right(vec![&channel])), "request_body", &body, db.clone());
                }
                Ok(json) => {
                    if json.data.len() > 0 {
                        send_message(client, channel, format!("https://clips.twitch.tv/{}", json.data[0].id), db.clone());
                    }
                }
            }
        });
    thread::spawn(move || { tokio::run(future) });
}

fn multi_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    if args.len() == 0 {
        let streams: HashSet<String> = from_redis_value(&redis_call(db.clone(), vec!["smembers", &format!("channel:{}:multi", channel)]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
        if streams.len() > 0 { let _ = client.send_privmsg(format!("#{}", channel), format!("http://multistre.am/{}/{}", channel, streams.iter().join("/"))); }
    } else if args.len() == 1 && args[0] == "clear" {
        redis_call(db.clone(), vec!["del", &format!("channel:{}:multi", channel)]);
        send_message(client, channel, "!multi has been cleared".to_owned(), db.clone());
    } else if args.len() > 1 && args[0] == "set" {
        redis_call(db.clone(), vec!["del", &format!("channel:{}:multi", channel)]);
        for arg in args[1..].iter() {
            redis_call(db.clone(), vec!["sadd", &format!("channel:{}:multi", channel), arg]);
        }
        send_message(client, channel, "!multi has been set".to_owned(), db.clone());
    }
}

fn counters_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    if args.len() > 1 {
        match args[0].to_lowercase().as_ref() {
            "set" => {
                if args.len() > 2 {
                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:counters", channel), &args[1], &args[2]]);
                    send_message(client, channel, format!("{} has been set to: {}", &args[1], &args[2]), db.clone());
                }
            }
            "inc" => {
                let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:counters", channel), &args[1]]);
                if let Ok(value) = res {
                    let counter: String = from_redis_value(&value).unwrap();
                    let res: Result<u16,_> = counter.parse();
                    if let Ok(num) = res {
                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:counters", channel), &args[1], (num + 1).to_string().as_ref()]);
                    } else {
                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:counters", channel), &args[1], "1"]);
                    }
                } else {
                    redis_call(db.clone(), vec!["hset", &format!("channel:{}:counters", channel), &args[1], "1"]);
                }
                send_message(client, channel, format!("{} has been increased", &args[1]), db.clone());
            }
            _ => {}
        }
    }
}

fn phrases_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    if args.len() > 2 {
        match args[0].to_lowercase().as_ref() {
            "set" => {
                redis_call(db.clone(), vec!["hset", &format!("channel:{}:phrases", channel), &args[1], &args[2..].join(" ")]);
                send_message(client, channel, format!("{} has been set to: {}", &args[1], args[2..].join(" ")), db.clone());
            }
            _ => {}
        }
    }
}

fn commercials_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    if args.len() > 1 {
        match args[0].to_lowercase().as_ref() {
            "submode" => {
                match args[1].to_lowercase().as_ref() {
                    "on" => {
                        redis_call(db.clone(), vec!["set", &format!("channel:{}:commercials:submode", channel), "true"]);
                        send_message(client, channel, "Submode during commercials has been turned on".to_owned(), db.clone());
                    }
                    "off" => {
                        redis_call(db.clone(), vec!["set", &format!("channel:{}:commercials:submode", channel), "false"]);
                        send_message(client, channel, "Submode during commercials has been turned off".to_owned(), db.clone());
                    }
                    _ => {}
                }
            }
            "notice" => {
                let exists: bool = from_redis_value(&redis_call(db.clone(), vec!["exists", &format!("channel:{}:commands:{}", channel, &args[1])]).expect(&format!("channel:{}:commands:{}", channel, &args[1]))).unwrap();
                if exists {
                    redis_call(db.clone(), vec!["set", &format!("channel:{}:commercials:notice", channel), &args[1]]);
                    send_message(client, channel, format!("{} will be run at the start of commercials", &args[1]), db.clone());
                } else {
                    send_message(client, channel, format!("{} is not an existing command", &args[1]), db.clone());
                }
            }
            "hourly" => {
                let num: Result<u16,_> = args[1].parse();
                match num {
                    Ok(_num) => {
                        redis_call(db.clone(), vec!["set", &format!("channel:{}:commercials:hourly", channel), &args[1]]);
                        send_message(client, channel, format!("{} commercials will be run each hour", &args[1]), db.clone());
                    }
                    Err(_e) => {
                        send_message(client, channel, format!("{} could not be parsed as a number", &args[1]), db.clone());
                    }
                }
            }
            "run" => {
                let num: Result<u64,_> = args[1].parse();
                match num {
                    Ok(num) => {
                        if num > 0 && num < 7 {
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
                            if within8 {
                                send_message(client, channel, "Commercials can't be run within eight minutes of each other".to_owned(), db.clone());
                            } else {
                                let id: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:id", channel)]).expect(&format!("channel:{}:id", channel))).unwrap();
                                let submode: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:commercials:submode", channel)]).unwrap_or(Value::Data("false".as_bytes().to_owned()))).unwrap();
                                let nres: Result<Value,_> = redis_call(db.clone(), vec!["get", &format!("channel:{}:commercials:notice", channel)]);
                                let length: u16 = from_redis_value(&redis_call(db.clone(), vec!["llen", &format!("channel:{}:commercials:recent", channel)]).expect(&format!("channel:{}:commercials:recent", channel))).unwrap();
                                redis_call(db.clone(), vec!["lpush", &format!("channel:{}:commercials:recent", channel), &format!("{} {}", Utc::now().to_rfc3339(), num)]);
                                if length > 7 {
                                    redis_call(db.clone(), vec!["rpop", &format!("channel:{}:commercials:recent", channel)]);
                                }
                                if submode == "true" {
                                    let client_clone = client.clone();
                                    let channel_clone = String::from(channel.clone());
                                    let _ = client.send_privmsg(format!("#{}", channel), "/subscribers");
                                    thread::spawn(move || {
                                        thread::sleep(time::Duration::from_secs(num * 30));
                                        client_clone.send_privmsg(format!("#{}", channel_clone), "/subscribersoff").unwrap();
                                    });
                                }
                                if let Ok(value) = nres {
                                    let notice: String = from_redis_value(&value).unwrap();
                                    let res: Result<Value,_> = redis_call(db.clone(), vec!["hget", &format!("channel:{}:commands:{}", channel, notice), "message"]);
                                    if let Ok(value) = res {
                                        let message: String = from_redis_value(&value).unwrap();
                                        send_message(client.clone(), channel.clone(), message, db.clone());
                                    }
                                }
                                log_info(Some(Right(vec![&channel])), "run_commercials", &format!("{} commercials have been run", args[1]), db.clone());
                                send_message(client.clone(), channel.clone(), format!("{} commercials have been run", args[1]), db.clone());
                                let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
                                let future = twitch_kraken_request(token, Some("application/json"), Some(format!("{{\"length\": {}}}", num * 30).as_bytes().to_owned()), Method::POST, &format!("https://api.twitch.tv/kraken/channels/{}/commercial", &id)).send().and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() }).map_err(|e| println!("request error: {}", e)).map(move |_body| {});
                                thread::spawn(move || { tokio::run(future) });
                            }
                        } else {
                            send_message(client.clone(), channel.clone(), format!("{} must be a number between one and six", args[1]), db.clone());
                        }
                    }
                    Err(_e) => {
                        send_message(client.clone(), channel.clone(), format!("{} could not be parsed as a number", args[1]), db.clone());
                    }
                }
            }
            _ => {}
        }
    }
}

fn songreq_cmd(client: Arc<IrcClient>, channel: String, args: Vec<String>, message: Option<Message>, db: (Sender<Vec<String>>, Receiver<Result<Value, String>>)) {
    if let Some(message) = message {
        let nick = get_nick(&message);
        if args.len() > 0 {
            match args[0].to_lowercase().as_ref() {
                "clear" => {
                    redis_call(db.clone(), vec!["del", &format!("channel:{}:songreqs", channel)]);
                    let keys: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["keys", &format!("channel:{}:songreqs:*", channel)]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                    for key in keys.iter() {
                        redis_call(db.clone(), vec!["del", key]);
                    }
                    send_message(client, channel, "song requests have been cleared".to_owned(), db.clone());
                }
                _ => {
                    let rgx = Regex::new(r"^[\-_a-zA-Z0-9]+$").unwrap();
                    if rgx.is_match(&args[0]) {
                        let token: String = from_redis_value(&redis_call(db.clone(), vec!["get", &format!("channel:{}:token", &channel)]).expect(&format!("channel:{}:token", &channel))).unwrap();
                        let future = twitch_helix_request(token, None, None, Method::GET, &format!("https://www.youtube.com/oembed?format=json&url=https://youtube.com/watch?v={}", args[0])).send()
                            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                            .map_err(|e| println!("request error: {}", e))
                            .map(move |body| {
                                let body = std::str::from_utf8(&body).unwrap();
                                if body != "Not Found" {
                                    let mut exists = false;
                                    let entries: Vec<String> = from_redis_value(&redis_call(db.clone(), vec!["lrange", &format!("channel:{}:songreqs", channel), "1", "-1"]).unwrap_or(Value::Bulk(Vec::new()))).unwrap();
                                    for key in entries.iter() {
                                        let entry: String = from_redis_value(&redis_call(db.clone(), vec!["hget", &format!("channel:{}:songreqs:{}", channel, key), "nick"]).expect(&format!("channel:{}:songreqs:{}", channel, key))).unwrap();
                                        if entry == nick {
                                            exists = true;
                                            break;
                                        }
                                    }
                                    if exists {
                                        send_message(client, channel, format!("{} already has an entry in the queue", nick), db.clone());
                                    } else {
                                        let json: YoutubeData = serde_json::from_str(&body).unwrap();
                                        let key = hash(&args[0], DEFAULT_COST).unwrap();
                                        redis_call(db.clone(), vec!["rpush", &format!("channel:{}:songreqs", channel), &format!("{}", key)]);
                                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:songreqs:{}", channel, key), "src", &format!("https://youtube.com/watch?v={}", args[0])]);
                                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:songreqs:{}", channel, key), "title", &json.title]);
                                        redis_call(db.clone(), vec!["hset", &format!("channel:{}:songreqs:{}", channel, key), "nick", &nick]);
                                        send_message(client, channel, format!("{} has been added to the queue", args[0]), db.clone());
                                    }
                                } else {
                                    send_message(client, channel, format!("{} is not a proper youtube id", args[0]), db.clone());
                                }
                            });
                        thread::spawn(move || { tokio::run(future) });
                    } else {
                        send_message(client, channel, format!("{} is not a proper youtube id", args[0]), db.clone());
                    }
                }
            }
        }
    }
}
