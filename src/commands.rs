// [("bits", commandBits, Mod, Mod), ("greetings", commandGreetings, Mod, Mod), ("giveaway", commandGiveaway, Mod, Mod), ("poll", commandPoll, Mod, Mod), ("watchtime", commandWatchtime, Mod, Mod), ("clip", commandClip, All, All), , ("genwebauth", commandWebAuth, Mod, Mod), ("listads", commandListCommercials, Mod, Mod), ("listsettings", commandListSettings, Mod, Mod), ("unmod", commandUnmod, Mod, Mod)]

// [("watchtime", watchtimeVar), ("watchrank", watchrankVar), ("watchranks", watchranksVar), ("hotkey", hotkeyVar), ("obs:scene-change", obsSceneChangeVar), ("fortnite:wins", fortWinsVar), ("fortnite:kills", fortKillsVar), ("fortnite:lifewins", fortLifeWinsVar), ("fortnite:lifekills", fortLifeKillsVar), ("fortnite:solowins", fortSoloWinsVar), ("fortnite:solokills", fortSoloKillsVar), ("fortnite:duowins", fortDuoWinsVar), ("fortnite:duokills", fortDuoKillsVar), ("fortnite:squadwins", fortSquadWinsVar), ("fortnite:squadkills", fortSquadKillsVar), ("fortnite:season-solowins", fortSeasonSoloWinsVar), ("fortnite:season-solokills", fortSeasonSoloKillsVar), ("fortnite:season-duowins", fortSeasonDuoWinsVar), ("fortnite:season-duokills", fortSeasonDuoKillsVar), ("fortnite:season-squadwins", fortSeasonSquadWinsVar), ("fortnite:season-squadkills", fortSeasonSquadKillsVar)]

use crate::types::*;
use crate::util::*;

use std::mem;
use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use std::{thread,time};
use either::Either::{Left, Right};
use tokio;
use bcrypt::{DEFAULT_COST, hash};
use regex::Regex;
use reqwest::Method;
use reqwest::r#async::{RequestBuilder,Chunk,Decoder};
use irc::client::prelude::*;
use chrono::{Utc, DateTime, FixedOffset, Duration};
use humantime::format_duration;
use itertools::Itertools;
use redis::{self,Commands,Connection};

pub const native_commands: [(&str, fn(Arc<Connection>, Arc<IrcClient>, String, Vec<String>, Option<Message>), bool, bool); 15] = [("echo", echo_cmd, true, true), ("set", set_cmd, true, true), ("unset", unset_cmd, true, true), ("command", command_cmd, true, true), ("title", title_cmd, false, true), ("game", game_cmd, false, true), ("notices", notices_cmd, true, true), ("moderation", moderation_cmd, true, true), ("permit", permit_cmd, true, true), ("multi", multi_cmd, false, true), ("clip", clip_cmd, true, true), ("counters", counters_cmd, true, true), ("phrases", phrases_cmd, true, true), ("commercials", commercials_cmd, true, true), ("songreq", songreq_cmd, false, false)];

pub const command_vars: [(&str, fn(Arc<Connection>, Option<Arc<IrcClient>>, String, Option<Message>, Vec<String>, Vec<String>) -> String); 21] = [("args", args_var), ("user", user_var), ("channel", channel_var), ("cmd", cmd_var), ("counterinc", counterinc_var), ("counter", counter_var), ("phrase", phrase_var), ("countdown", countdown_var), ("date", date_var), ("dateinc", dateinc_var), ("watchtime", watchtime_var), ("watchrank", watchrank_var), ("fortnite:wins", fortnite_wins_var), ("fortnite:kills", fortnite_kills_var), ("pubg:damage", pubg_damage_var), ("pubg:headshots", pubg_headshots_var), ("pubg:kills", pubg_kills_var), ("pubg:roadkills", pubg_roadkills_var), ("pubg:teamkills", pubg_teamkills_var), ("pubg:vehicles-destroyed", pubg_vehicles_destroyed_var), ("pubg:wins", pubg_wins_var)];

pub const command_vars_async: [(&str, fn(Arc<Connection>, Option<Arc<IrcClient>>, String, Option<Message>, Vec<String>, Vec<String>) -> Option<(RequestBuilder, fn((String, Chunk)) -> String)>); 10] = [("uptime", uptime_var), ("followage", followage_var), ("subcount", subcount_var), ("followcount", followcount_var), ("urlfetch", urlfetch_var), ("spotify:playing-title", spotify_playing_title_var), ("spotify:playing-album", spotify_playing_album_var), ("spotify:playing-artist", spotify_playing_artist_var), ("youtube:latest-url", youtube_latest_url_var), ("youtube:latest-title", youtube_latest_title_var)];

pub const twitch_bots: [&str; 20] = ["electricallongboard","lanfusion","cogwhistle","freddyybot","anotherttvviewer","apricotdrupefruit","skinnyseahorse","p0lizei_","xbit01","n3td3v","cachebear","icon_bot","virgoproz","v_and_k","slocool","host_giveaway","nightbot","commanderroot","p0sitivitybot","streamlabs"];

fn args_var(_con: Arc<Connection>, _client: Option<Arc<IrcClient>>, _channel: String, _message: Option<Message>, vargs: Vec<String>, cargs: Vec<String>) -> String {
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

fn cmd_var(con: Arc<Connection>, client: Option<Arc<IrcClient>>, channel: String, irc_message: Option<Message>, vargs: Vec<String>, cargs: Vec<String>) -> String {
    if let Some(client) = client {
        if vargs.len() > 0 {
            let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, vargs[0]), "message");
            if let Ok(message) = res {
                let mut message = message;
                for var in command_vars.iter() {
                    if var.0 != "cmd" && var.0 != "var" {
                        let args: Vec<String> = vargs[1..].iter().map(|a| a.to_string()).collect();
                        message = parse_var(var, &message, con.clone(), Some(client.clone()), channel.clone(), irc_message.clone(), args);
                    }
                }
                send_parsed_message(con.clone(), client, channel.to_owned(), message, cargs, irc_message);
            } else {
                for cmd in native_commands.iter() {
                    if format!("!{}", cmd.0) == vargs[0] {
                        let args: Vec<String> = vargs[1..].iter().map(|a| a.to_string()).collect();
                        match irc_message.clone() {
                            None => { (cmd.1)(con.clone(), client.clone(), channel.to_owned(), args, None) }
                            Some(msg) => { (cmd.1)(con.clone(), client.clone(), channel.to_owned(), args, Some(msg)) }
                        }
                    }
                }
            }
        }
    }
    "".to_owned()
}

fn uptime_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> Option<(RequestBuilder, fn((String, Chunk)) -> String)> {
    let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
    let builder = twitch_kraken_request(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/streams?channel={}", &id));
    let func = move |(channel, body): (String, Chunk)| -> String {
        let con = Arc::new(acquire_con());
        let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
        let body = std::str::from_utf8(&body).unwrap().to_string();
        let body = validate_twitch(channel.clone(), body.clone(), twitch_kraken_request_sync(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/streams?channel={}", &id)));
        let json: Result<KrakenStreams,_> = serde_json::from_str(&body);
        match json {
            Err(e) => {
                log_error(None, "uptime_var", &e.to_string());
                log_error(None, "request_body", &body.to_string());
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

fn user_var(_con: Arc<Connection>, _client: Option<Arc<IrcClient>>, _channel: String, message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
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

fn channel_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
    let display: String = con.get(format!("channel:{}:display-name", channel)).expect("get:display-name");
    display
}

fn counterinc_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>) -> String {
    if vargs.len() > 0 {
        let res: Result<String,_> = con.hget(format!("channel:{}:counters", channel), &vargs[0]);
        if let Ok(counter) = res {
            let res: Result<u16,_> = counter.parse();
            if let Ok(num) = res {
                let _: () = con.hset(format!("channel:{}:counters", channel), &vargs[0], num + 1).unwrap();
            } else {
                let _: () = con.hset(format!("channel:{}:counters", channel), &vargs[0], 1).unwrap();
            }
        } else {
            let _: () = con.hset(format!("channel:{}:counters", channel), &vargs[0], 1).unwrap();
        }
    }

    "".to_owned()
}

fn followage_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> Option<(RequestBuilder, fn((String, Chunk)) -> String)> {
    if let Some(message) = message {
        let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
        let user_id = get_id(&message).unwrap();
        let builder = twitch_kraken_request(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/users/{}/follows/channels/{}", &user_id, &id));
        let func = move |(channel, body): (String, Chunk)| -> String {
            let con = Arc::new(acquire_con());
            let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
            let body = std::str::from_utf8(&body).unwrap().to_string();
            // TODO
            // let body = validate_twitch(channel.clone(), body.clone(), twitch_kraken_request_sync(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/users/{}/follows/channels/{}", &user_id, &id)));
            let json: Result<KrakenFollow,_> = serde_json::from_str(&body);
            match json {
                Err(e) => { log_error(Some(Right(&channel)), "followage_var", &e.to_string()); "0m".to_owned() }
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

fn subcount_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> Option<(RequestBuilder, fn((String, Chunk)) -> String)> {
    let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
    let builder = twitch_kraken_request(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}/subscriptions", &id));
    let func = move |(channel, body): (String, Chunk)| -> String {
        let con = Arc::new(acquire_con());
        let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
        let body = std::str::from_utf8(&body).unwrap().to_string();
        let body = validate_twitch(channel.clone(), body.clone(), twitch_kraken_request_sync(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}/subscriptions", &id)));
        let json: Result<KrakenSubs,_> = serde_json::from_str(&body);
        match json {
            Err(e) => { log_error(Some(Right(&channel)), "followage_var", &e.to_string()); "0".to_owned() }
            Ok(json) => { json.total.to_string() }
        }
    };
    return Some((builder, func));
}

fn followcount_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> Option<(RequestBuilder, fn((String, Chunk)) -> String)> {
    let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
    let builder = twitch_kraken_request(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}/follows", &id));
    let func = move |(channel, body): (String, Chunk)| -> String {
        let con = Arc::new(acquire_con());
        let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
        let body = std::str::from_utf8(&body).unwrap().to_string();
        let body = validate_twitch(channel.clone(), body.clone(), twitch_kraken_request_sync(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}/follows", &id)));
        let json: Result<KrakenFollows,_> = serde_json::from_str(&body);
        match json {
            Err(e) => { log_error(Some(Right(&channel)), "followage_var", &e.to_string()); "0".to_owned() }
            Ok(json) => { json.total.to_string() }
        }
    };
    return Some((builder, func));
}

fn counter_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>) -> String {
    if vargs.len() > 0 {
        let res: Result<String,_> = con.hget(format!("channel:{}:counters", channel), &vargs[0]);
        if let Ok(counter) = res {
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

fn phrase_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>) -> String {
    if vargs.len() > 0 {
        let res: Result<String,_> = con.hget(format!("channel:{}:phrases", channel), &vargs[0]);
        if let Ok(phrase) = res {
            phrase
        } else {
            "".to_owned()
        }
    } else {
        "".to_owned()
    }
}

fn date_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>) -> String {
    if vargs.len() > 0 {
        let res: Result<String,_> = con.hget(format!("channel:{}:phrases", channel), &vargs[0]);
        if let Ok(phrase) = res {
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

fn dateinc_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>) -> String {
    if vargs.len() > 1 {
        let res: Result<String,_> = con.hget(format!("channel:{}:phrases", channel), &vargs[0]);
        if let Ok(phrase) = res {
            let res: Result<DateTime<FixedOffset>,_> = DateTime::parse_from_rfc3339(&phrase);
            if let Ok(dt) = res {
                let res: Result<i64,_> = vargs[1].parse();
                if let Ok(num) = res {
                    let ndt = dt + Duration::seconds(num);
                    let _: () = con.hset(format!("channel:{}:phrases", channel), &vargs[0], ndt.to_rfc3339()).unwrap();
                }
            }
        }
    }
    "".to_owned()
}

fn countdown_var(_con: Arc<Connection>, _client: Option<Arc<IrcClient>>, _channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>) -> String {
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

fn watchtime_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
    if let Some(message) = message {
        let res: Result<String,_> = con.hget(format!("channel:{}:watchtimes", channel), get_nick(&message));
        if let Ok(watchtime) = res {
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

fn watchrank_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>) -> String {
    if let Some(message) = message {
        let hashtimes: HashMap<String,String> = con.hgetall(format!("channel:{}:watchtimes", channel)).unwrap_or(HashMap::new());
        let mut watchtimes: Vec<(String,u64)> = Vec::new();
        for (key, value) in hashtimes.iter() {
            let num: u64 = value.parse().unwrap();
            watchtimes.push((key.to_owned(), num));
        }
        let list: String = con.hget(format!("channel:{}:settings", channel), "watchtime:blacklist").unwrap_or("".to_owned());
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

fn urlfetch_var(_con: Arc<Connection>, _client: Option<Arc<IrcClient>>, _channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>) -> Option<(RequestBuilder, fn((String, Chunk)) -> String)> {
    if vargs.len() > 0 {
        let builder = request(Method::GET, None, &vargs[0]);
        let func = move |(channel, body): (String, Chunk)| -> String {
            let body = std::str::from_utf8(&body).unwrap();
            return body.to_owned();
        };
        return Some((builder, func));
    } else { None }
}

fn spotify_playing_title_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> Option<(RequestBuilder, fn((String, Chunk)) -> String)> {
    let builder = spotify_request(con.clone(), &channel, Method::GET, "https://api.spotify.com/v1/me/player/currently-playing", None);
    let func = move |(channel, body): (String, Chunk)| -> String {
        let body = std::str::from_utf8(&body).unwrap();
        let json: Result<SpotifyPlaying,_> = serde_json::from_str(&body);
        match json {
            Err(e) => {
                log_error(None, "spotify_playing_title_var", &e.to_string());
                log_error(None, "request_body", &body);
                "".to_owned()
            }
            Ok(json) => { json.item.name.to_owned() }
        }
    };
    return Some((builder, func));
}

fn spotify_playing_album_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> Option<(RequestBuilder, fn((String, Chunk)) -> String)> {
    let builder = spotify_request(con.clone(), &channel, Method::GET, "https://api.spotify.com/v1/me/player/currently-playing", None);
    let func = move |(channel, body): (String, Chunk)| -> String {
        let body = std::str::from_utf8(&body).unwrap();
        let json: Result<SpotifyPlaying,_> = serde_json::from_str(&body);
        match json {
            Err(e) => {
                log_error(None, "spotify_playing_album_var", &e.to_string());
                log_error(None, "request_body", &body);
                "".to_owned()
            }
            Ok(json) => { json.item.album.name.to_owned() }
        }
    };
    return Some((builder, func));
}

fn spotify_playing_artist_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> Option<(RequestBuilder, fn((String, Chunk)) -> String)> {
    let builder = spotify_request(con.clone(), &channel, Method::GET, "https://api.spotify.com/v1/me/player/currently-playing", None);
    let func = move |(channel, body): (String, Chunk)| -> String {
        let body = std::str::from_utf8(&body).unwrap();
        let json: Result<SpotifyPlaying,_> = serde_json::from_str(&body);
        match json {
            Err(e) => {
                log_error(None, "spotify_playing_artist_var", &e.to_string());
                log_error(None, "request_body", &body);
                "".to_owned()
            }
            Ok(json) => { json.item.artists[0].name.to_owned() }
        }
    };
    return Some((builder, func));
}

fn youtube_latest_url_var(_con: Arc<Connection>, _client: Option<Arc<IrcClient>>, _channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>) -> Option<(RequestBuilder, fn((String, Chunk)) -> String)> {
    if vargs.len() > 0 {
        let builder = request(Method::GET, None, &format!("https://decapi.me/youtube/latest_video?id={}", vargs[0]));
        let func = move |(channel, body): (String, Chunk)| -> String {
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

fn youtube_latest_title_var(_con: Arc<Connection>, _client: Option<Arc<IrcClient>>, _channel: String, _message: Option<Message>, vargs: Vec<String>, _cargs: Vec<String>) -> Option<(RequestBuilder, fn((String, Chunk)) -> String)> {
    if vargs.len() > 0 {
        let builder = request(Method::GET, None, &format!("https://decapi.me/youtube/latest_video?id={}", vargs[0]));
        let func = move |(channel, body): (String, Chunk)| -> String {
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

fn fortnite_wins_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:fortnite", channel), "wins").unwrap_or("0".to_owned());
    value
}

fn fortnite_kills_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:fortnite", channel), "kills").unwrap_or("0".to_owned());
    value
}

fn pubg_damage_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "damageDealt").unwrap_or("0".to_owned());
    value
}

fn pubg_headshots_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "headshotKills").unwrap_or("0".to_owned());
    value
}

fn pubg_kills_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "kills").unwrap_or("0".to_owned());
    value
}

fn pubg_roadkills_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "roadKills").unwrap_or("0".to_owned());
    value
}

fn pubg_teamkills_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "teamKills").unwrap_or("0".to_owned());
    value
}

fn pubg_vehicles_destroyed_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "vehicleDestroys").unwrap_or("0".to_owned());
    value
}

fn pubg_wins_var(con: Arc<Connection>, _client: Option<Arc<IrcClient>>, channel: String, _message: Option<Message>, _vargs: Vec<String>, _cargs: Vec<String>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "wins").unwrap_or("0".to_owned());
    value
}

fn echo_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    send_message(con.clone(), client, channel, args.join(" "));
}

fn set_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    match args.len() {
        0 => {}
        1 => {
            let _: () = con.hset(format!("channel:{}:settings", channel), &args[0], true).unwrap();
            send_message(con.clone(), client, channel, format!("{} has been set to: true", args[0]));
        }
        _ => {
            let _: () = con.hset(format!("channel:{}:settings", channel), &args[0], args[1..].join(" ")).unwrap();
            send_message(con.clone(), client, channel, format!("{} has been set to: {}", &args[0], args[1..].join(" ")));
        }
    }
}

fn unset_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    if args.len() == 1 {
        let _: () = con.hdel(format!("channel:{}:settings", channel), &args[0]).unwrap();
        send_message(con.clone(), client, channel, format!("{} has been unset", &args[0]));
    }
}

fn command_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    if args.len() > 1 {
        match args[0].to_lowercase().as_ref() {
            "add" => {
                if args.len() > 2 {
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, &args[1]), "message", args[2..].join(" ")).unwrap();
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, &args[1]), "cmd_protected", false).unwrap();
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, &args[1]), "arg_protected", false).unwrap();
                    send_message(con.clone(), client, channel, format!("{} has been added", &args[1]));
                }
            }
            "modadd" => {
                if args.len() > 2 {
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, &args[1]), "message", args[2..].join(" ")).unwrap();
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, &args[1]), "cmd_protected", true).unwrap();
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, &args[1]), "arg_protected", true).unwrap();
                    send_message(con.clone(), client, channel, format!("{} has been added", &args[1]));
                }
            }
            "remove" => {
                let _: () = con.del(format!("channel:{}:commands:{}", channel, &args[1])).unwrap();
                send_message(con.clone(), client, channel, format!("{} has been removed", &args[1]));
            }
            "alias" => {
                if args.len() > 2 {
                    // TODO: validate command exists
                    let _: () = con.hset(format!("channel:{}:aliases", channel), &args[1], args[2..].join(" ")).unwrap();
                    send_message(con.clone(), client, channel, format!("{} has been added as an alias to {}", &args[1], args[2..].join(" ")));
                }
            }
            "remalias" => {
                let _: () = con.hdel(format!("channel:{}:aliases", channel), &args[1]).unwrap();
                send_message(con.clone(), client, channel, format!("{} has been removed as an alias", &args[1]));
            }
            _ => {}
        }
    }
}

fn title_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
    if args.len() == 0 {
        let future = twitch_kraken_request(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}", &id)).send()
            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
            .map_err(|e| println!("request error: {}", e))
            .map(move |body| {
                let con = Arc::new(acquire_con());
                let body = std::str::from_utf8(&body).unwrap().to_string();
                let body = validate_twitch(channel.clone(), body.clone(), twitch_kraken_request_sync(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}", &id)));
                let json: Result<KrakenChannel,_> = serde_json::from_str(&body);
                match json {
                    Err(e) => {
                        log_error(Some(Right(vec![&channel])), "title_cmd", &e.to_string());
                        log_error(Some(Right(vec![&channel])), "request_body", &body);
                    }
                    Ok(json) => { let _ = send_message(con.clone(), client, channel, json.status); }
                }
            });
        thread::spawn(move || { tokio::run(future) });
    } else {
        let future = twitch_kraken_request(con.clone(), &channel, Some("application/x-www-form-urlencoded"), Some(format!("channel[status]={}", args.join(" ")).as_bytes().to_owned()), Method::PUT, &format!("https://api.twitch.tv/kraken/channels/{}", &id)).send()
            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
            .map_err(|e| println!("request error: {}", e))
            .map(move |body| {
                let con = Arc::new(acquire_con());
                let body = std::str::from_utf8(&body).unwrap().to_string();
                let body = validate_twitch(channel.clone(), body.clone(), twitch_kraken_request_sync(con.clone(), &channel, Some("application/x-www-form-urlencoded"), Some(format!("channel[status]={}", args.join(" ")).as_bytes().to_owned()), Method::PUT, &format!("https://api.twitch.tv/kraken/channels/{}", &id)));
                let json: Result<KrakenChannel,_> = serde_json::from_str(&body);
                match json {
                    Err(e) => {
                        log_error(Some(Right(vec![&channel])), "title_cmd", &e.to_string());
                        log_error(Some(Right(vec![&channel])), "request_body", &body);
                    }
                    Ok(json) => { send_message(con.clone(), client, channel, format!("Title is now set to: {}", json.status)); }
                }
            });
        thread::spawn(move || { tokio::run(future) });
    }
}

fn game_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
    if args.len() == 0 {
        let future = twitch_kraken_request(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}", &id)).send()
            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
            .map_err(|e| println!("request error: {}", e))
            .map(move |body| {
                let con = Arc::new(acquire_con());
                let body = std::str::from_utf8(&body).unwrap().to_string();
                let body = validate_twitch(channel.clone(), body.clone(), twitch_kraken_request_sync(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/kraken/channels/{}", &id)));
                let json: Result<KrakenChannel,_> = serde_json::from_str(&body);
                match json {
                    Err(e) => {
                        log_error(Some(Right(vec![&channel])), "game_cmd", &e.to_string());
                        log_error(Some(Right(vec![&channel])), "request_body", &body);
                    }
                    Ok(json) => { let _ = send_message(con.clone(), client, channel, json.game); }
                }
            });
        thread::spawn(move || { tokio::run(future) });
    } else {
        let future = twitch_helix_request(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/helix/games?name={}", args.join(" "))).send()
            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
            .map_err(|e| println!("request error: {}", e))
            .map(move |body| {
                let con = Arc::new(acquire_con());
                let body = std::str::from_utf8(&body).unwrap().to_string();
                let body = validate_twitch(channel.clone(), body.clone(), twitch_helix_request_sync(con.clone(), &channel, None, None, Method::GET, &format!("https://api.twitch.tv/helix/games?name={}", args.join(" "))));
                let json: Result<HelixGames,_> = serde_json::from_str(&body);
                match json {
                    Err(e) => {
                        log_error(Some(Right(vec![&channel])), "game_cmd", &e.to_string());
                        log_error(Some(Right(vec![&channel])), "request_body", &body);
                    }
                    Ok(json) => {
                        if json.data.len() == 0 {
                            send_message(con.clone(), client, channel, format!("Unable to find a game matching: {}", args.join(" ")));
                        } else {
                            let name = json.data[0].name.clone();
                            let future = twitch_kraken_request(con.clone(), &channel, Some("application/x-www-form-urlencoded"), Some(format!("channel[game]={}", name).as_bytes().to_owned()), Method::PUT, &format!("https://api.twitch.tv/kraken/channels/{}", &id)).send()
                                .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                                .map_err(|e| println!("request error: {}", e))
                                .map(move |body| {
                                    let con = Arc::new(acquire_con());
                                    let body = std::str::from_utf8(&body).unwrap().to_string();
                                    let body = validate_twitch(channel.clone(), body.clone(), twitch_kraken_request_sync(con.clone(), &channel, Some("application/x-www-form-urlencoded"), Some(format!("channel[game]={}", name).as_bytes().to_owned()), Method::PUT, &format!("https://api.twitch.tv/kraken/channels/{}", &id)));
                                    let json: Result<KrakenChannel,_> = serde_json::from_str(&body);
                                    match json {
                                        Err(e) => {
                                            log_error(Some(Right(vec![&channel])), "game_cmd", &e.to_string());
                                            log_error(Some(Right(vec![&channel])), "request_body", &body);
                                        }
                                        Ok(_json) => { send_message(con.clone(), client, channel, format!("Game is now set to: {}", &name)); }
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

fn notices_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    if args.len() > 1 {
        match args[0].to_lowercase().as_ref() {
            "add" => {
                let num: Result<u16,_> = args[1].parse();
                match num {
                    Ok(num) => {
                        if num % 60 == 0 {
                            let _: () = con.rpush(format!("channel:{}:notices:{}:commands", channel, args[1]), &args[2]).unwrap();
                            let _: () = con.set(format!("channel:{}:notices:{}:countdown", channel, args[1]), &args[1]).unwrap();
                            send_message(con.clone(), client, channel, "notice has been added".to_owned());
                        } else {
                            send_message(con.clone(), client, channel, "notice interval must be a multiple of 60".to_owned());
                        }
                    }
                    Err(_) => {}
                }
            }
            _ => {}
        }
    }
}

fn moderation_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    if args.len() > 1 {
        match args[0].to_lowercase().as_ref() {
            "links" => {
                match args[1].to_lowercase().as_ref() {
                    "add" => {
                        if args.len() > 2 {
                            let _: () = con.sadd(format!("channel:{}:moderation:links", channel), &args[2]).unwrap();
                            send_message(con.clone(), client, channel, format!("{} has been whitelisted", &args[2]));
                        }
                    }
                    "remove" => {
                        if args.len() > 2 {
                            let _: () = con.srem(format!("channel:{}:moderation:links", channel), &args[2]).unwrap();
                            send_message(con.clone(), client, channel, format!("{} has been removed from the whitelist", &args[2]));
                        }
                    }
                    "allowsubs" => {
                        let _: () = con.set(format!("channel:{}:moderation:links:subs", channel), true).unwrap();
                        send_message(con.clone(), client, channel, "Subs are now allowed to post links".to_owned());
                    }
                    "blocksubs" => {
                        let _: () = con.set(format!("channel:{}:moderation:links:subs", channel), false).unwrap();
                        send_message(con.clone(), client, channel, "Subs are not allowed to post links".to_owned());
                    }
                    _ => {}
                }
            }
            "colors" => {
                match args[1].to_lowercase().as_ref() {
                    "on" => {
                        let _: () = con.set(format!("channel:{}:moderation:colors", channel), true).unwrap();
                        send_message(con.clone(), client, channel, "Color filter has been turned on".to_owned());
                    }
                    "off" => {
                        let _: () = con.set(format!("channel:{}:moderation:colors", channel), false).unwrap();
                        send_message(con.clone(), client, channel, "Color filter has been turned off".to_owned());
                    }
                    _ => {}
                }
            }
            "caps" => {
                match args[1].to_lowercase().as_ref() {
                    "set" => {
                        if args.len() > 3 {
                            let _: () = con.set(format!("channel:{}:moderation:caps", channel), true).unwrap();
                            let _: () = con.set(format!("channel:{}:moderation:caps:limit", channel), &args[2]).unwrap();
                            let _: () = con.set(format!("channel:{}:moderation:caps:trigger", channel), &args[3]).unwrap();
                            if args.len() > 4 { let _: () = con.set(format!("channel:{}:moderation:caps:subs", channel), &args[4]).unwrap(); }
                            send_message(con.clone(), client, channel, "Caps filter has been turned on".to_owned());
                        }
                    }
                    "off" => {
                        let _: () = con.del(format!("channel:{}:moderation:caps", channel)).unwrap();
                        send_message(con.clone(), client, channel, "Caps filter has been turned off".to_owned());
                    }
                    _ => {}
                }
            }
            "age" => {
                match args[1].to_lowercase().as_ref() {
                    "set" => {
                        if args.len() > 2 {
                            let _: () = con.set(format!("channel:{}:moderation:age", channel), &args[2]).unwrap();
                            send_message(con.clone(), client, channel, format!("Minimum account age has been set to: {}", &args[2]).to_owned());
                        }
                    }
                    "off" => {
                        let _: () = con.del(format!("channel:{}:moderation:age", channel)).unwrap();
                        send_message(con.clone(), client, channel, "Minimum account age filter has been turned off".to_owned());
                    }
                    _ => {}
                }
            }
            "display" => {
                match args[1].to_lowercase().as_ref() {
                    "on" => {
                        let _: () = con.set(format!("channel:{}:moderation:display", channel), true).unwrap();
                        send_message(con.clone(), client, channel, "Displaying timeout reasons has been turned on".to_owned());
                    }
                    "off" => {
                        let _: () = con.set(format!("channel:{}:moderation:display", channel), false).unwrap();
                        send_message(con.clone(), client, channel, "Displaying timeout reasons has been turned off".to_owned());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn permit_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    if args.len() > 0 {
        let nick = args[0].to_lowercase();
        let _: () = con.set(format!("channel:{}:moderation:permitted:{}", channel, nick), "").unwrap();
        let _: () = con.expire(format!("channel:{}:moderation:permitted:{}", channel, nick), 30).unwrap();
        send_message(con.clone(), client, channel, format!("{} can post links for the next 30 seconds", nick));
    }
}

fn clip_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, _args: Vec<String>, _message: Option<Message>) {
    let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
    let future = twitch_helix_request(con.clone(), &channel, None, None, Method::POST, &format!("https://api.twitch.tv/helix/clips?broadcaster_id={}", &id)).send()
        .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
        .map_err(|e| println!("request error: {}", e))
        .map(move |body| {
            let con = Arc::new(acquire_con());
            let body = std::str::from_utf8(&body).unwrap().to_string();
            let body = validate_twitch(channel.clone(), body.clone(), twitch_helix_request_sync(con.clone(), &channel, None, None, Method::POST, &format!("https://api.twitch.tv/helix/clips?broadcaster_id={}", &id)));
            let json: Result<HelixClips,_> = serde_json::from_str(&body);
            match json {
                Err(e) => {
                    log_error(Some(Right(vec![&channel])), "clip_cmd", &e.to_string());
                    log_error(Some(Right(vec![&channel])), "request_body", &body);
                }
                Ok(json) => {
                    if json.data.len() > 0 {
                        send_message(con.clone(), client, channel, format!("https://clips.twitch.tv/{}", json.data[0].id));
                    }
                }
            }
        });
    thread::spawn(move || { tokio::run(future) });
}

fn multi_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    if args.len() == 0 {
        let streams: HashSet<String> = con.smembers(format!("channel:{}:multi", channel)).unwrap();
        if streams.len() > 0 { let _ = client.send_privmsg(format!("#{}", channel), format!("http://multistre.am/{}/{}", channel, streams.iter().join("/"))); }
    } else if args.len() == 1 && args[0] == "clear" {
        let _: () = con.del(format!("channel:{}:multi", channel)).unwrap();
        send_message(con.clone(), client, channel, "!multi has been cleared".to_owned());
    } else if args.len() > 1 && args[0] == "set" {
        let _: () = con.del(format!("channel:{}:multi", channel)).unwrap();
        for arg in args[1..].iter() {
            let _: () = con.sadd(format!("channel:{}:multi", channel), arg.to_owned()).unwrap();
        }
        send_message(con.clone(), client, channel, "!multi has been set".to_owned());
    }
}

fn counters_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    if args.len() > 1 {
        match args[0].to_lowercase().as_ref() {
            "set" => {
                if args.len() > 2 {
                    let _: () = con.hset(format!("channel:{}:counters", channel), &args[1], &args[2]).unwrap();
                    send_message(con.clone(), client, channel, format!("{} has been set to: {}", &args[1], &args[2]));
                }
            }
            "inc" => {
                let res: Result<String,_> = con.hget(format!("channel:{}:counters", channel), &args[1]);
                if let Ok(counter) = res {
                    let res: Result<u16,_> = counter.parse();
                    if let Ok(num) = res {
                        let _: () = con.hset(format!("channel:{}:counters", channel), &args[1], num + 1).unwrap();
                    } else {
                        let _: () = con.hset(format!("channel:{}:counters", channel), &args[1], 1).unwrap();
                    }
                } else {
                    let _: () = con.hset(format!("channel:{}:counters", channel), &args[1], 1).unwrap();
                }
                send_message(con.clone(), client, channel, format!("{} has been increased", &args[1]));
            }
            _ => {}
        }
    }
}

fn phrases_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    if args.len() > 2 {
        match args[0].to_lowercase().as_ref() {
            "set" => {
                let _: () = con.hset(format!("channel:{}:phrases", channel), &args[1], args[2..].join(" ")).unwrap();
                send_message(con.clone(), client, channel, format!("{} has been set to: {}", &args[1], args[2..].join(" ")));
            }
            _ => {}
        }
    }
}

fn commercials_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, _message: Option<Message>) {
    if args.len() > 1 {
        match args[0].to_lowercase().as_ref() {
            "submode" => {
                match args[1].to_lowercase().as_ref() {
                    "on" => {
                        let _: () = con.set(format!("channel:{}:commercials:submode", channel), true).unwrap();
                        send_message(con.clone(), client, channel, "Submode during commercials has been turned on".to_owned());
                    }
                    "off" => {
                        let _: () = con.set(format!("channel:{}:commercials:submode", channel), false).unwrap();
                        send_message(con.clone(), client, channel, "Submode during commercials has been turned off".to_owned());
                    }
                    _ => {}
                }
            }
            "notice" => {
                let exists: bool = con.exists(format!("channel:{}:commands:{}", channel, &args[1])).unwrap();
                if exists {
                    let _: () = con.set(format!("channel:{}:commercials:notice", channel), &args[1]).unwrap();
                    send_message(con.clone(), client, channel, format!("{} will be run at the start of commercials", &args[1]));
                } else {
                    send_message(con.clone(), client, channel, format!("{} is not an existing command", &args[1]));
                }
            }
            "hourly" => {
                let num: Result<u16,_> = args[1].parse();
                match num {
                    Ok(_num) => {
                        let _: () = con.set(format!("channel:{}:commercials:hourly", channel), &args[1]).unwrap();
                        send_message(con.clone(), client, channel, format!("{} commercials will be run each hour", &args[1]));
                    }
                    Err(_e) => {
                        send_message(con.clone(), client, channel, format!("{} could not be parsed as a number", &args[1]));
                    }
                }
            }
            "run" => {
                let num: Result<u64,_> = args[1].parse();
                match num {
                    Ok(num) => {
                        if num > 0 && num < 7 {
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
                            if within8 {
                                send_message(con.clone(), client, channel, "Commercials can't be run within eight minutes of each other".to_owned());
                            } else {
                                let id: String = con.get(format!("channel:{}:id", channel)).expect("get:id");
                                let submode: String = con.get(format!("channel:{}:commercials:submode", channel)).unwrap_or("false".to_owned());
                                let nres: Result<String,_> = con.get(format!("channel:{}:commercials:notice", channel));
                                let length: u16 = con.llen(format!("channel:{}:commercials:recent", channel)).unwrap();
                                let _: () = con.lpush(format!("channel:{}:commercials:recent", channel), format!("{} {}", Utc::now().to_rfc3339(), num)).unwrap();
                                if length > 7 {
                                    let _: () = con.rpop(format!("channel:{}:commercials:recent", channel)).unwrap();
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
                                if let Ok(notice) = nres {
                                    let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, notice), "message");
                                    if let Ok(message) = res {
                                        send_message(con.clone(), client.clone(), channel.clone(), message);
                                    }
                                }
                                log_info(Some(Right(vec![&channel])), "run_commercials", &format!("{} commercials have been run", args[1]));
                                send_message(con.clone(), client.clone(), channel.clone(), format!("{} commercials have been run", args[1]));
                                let future = twitch_kraken_request(con.clone(), &channel, Some("application/json"), Some(format!("{{\"length\": {}}}", num * 30).as_bytes().to_owned()), Method::POST, &format!("https://api.twitch.tv/kraken/channels/{}/commercial", &id)).send().and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() }).map_err(|e| println!("request error: {}", e)).map(move |_body| {});
                                thread::spawn(move || { tokio::run(future) });
                            }
                        } else {
                            send_message(con.clone(), client.clone(), channel.clone(), format!("{} must be a number between one and six", args[1]));
                        }
                    }
                    Err(_e) => {
                        send_message(con.clone(), client.clone(), channel.clone(), format!("{} could not be parsed as a number", args[1]));
                    }
                }
            }
            _ => {}
        }
    }
}

fn songreq_cmd(con: Arc<Connection>, client: Arc<IrcClient>, channel: String, args: Vec<String>, message: Option<Message>) {
    if let Some(message) = message {
        let nick = get_nick(&message);
        if args.len() > 0 {
            match args[0].to_lowercase().as_ref() {
                "clear" => {
                    let badges = get_badges(&message);
                    let mut auth = false;
                    if let Some(_value) = badges.get("broadcaster") { auth = true }
                    if let Some(_value) = badges.get("moderator") { auth = true }
                    if auth {
                        let _: () = con.del(format!("channel:{}:songreqs", channel)).unwrap();
                        let keys: Vec<String> = con.keys(format!("channel:{}:songreqs:*", channel)).unwrap();
                        for key in keys.iter() {
                            let _: () = con.del(key).unwrap();
                        }
                        send_message(con.clone(), client, channel, "song requests have been cleared".to_owned());
                    }
                }
                _ => {
                    let rgx = Regex::new(r"^[\-_a-zA-Z0-9]+$").unwrap();
                    if rgx.is_match(&args[0]) {
                        let future = twitch_helix_request(con.clone(), &channel, None, None, Method::GET, &format!("https://www.youtube.com/oembed?format=json&url=https://youtube.com/watch?v={}", args[0])).send()
                            .and_then(|mut res| { mem::replace(res.body_mut(), Decoder::empty()).concat2() })
                            .map_err(|e| println!("request error: {}", e))
                            .map(move |body| {
                                let con = Arc::new(acquire_con());
                                let body = std::str::from_utf8(&body).unwrap();
                                if body != "Not Found" {
                                    let mut exists = false;
                                    let entries: Vec<String> = con.lrange(format!("channel:{}:songreqs", channel), 1, -1).unwrap();
                                    for key in entries.iter() {
                                        let entry: String = con.hget(format!("channel:{}:songreqs:{}", channel, key), "nick").expect("hget:nick");
                                        if entry == nick {
                                            exists = true;
                                            break;
                                        }
                                    }
                                    if exists {
                                        send_message(con.clone(), client, channel, format!("{} already has an entry in the queue", nick));
                                    } else {
                                        let json: YoutubeData = serde_json::from_str(&body).unwrap();
                                        let key = hash(&args[0], DEFAULT_COST).unwrap();
                                        let _: () = con.rpush(format!("channel:{}:songreqs", channel), format!("{}", key)).unwrap();
                                        let _: () = con.hset(format!("channel:{}:songreqs:{}", channel, key), "src", format!("https://youtube.com/watch?v={}", args[0])).unwrap();
                                        let _: () = con.hset(format!("channel:{}:songreqs:{}", channel, key), "title", json.title).unwrap();
                                        let _: () = con.hset(format!("channel:{}:songreqs:{}", channel, key), "nick", nick).unwrap();
                                        send_message(con.clone(), client, channel, format!("{} has been added to the queue", args[0]));
                                    }
                                } else {
                                    send_message(con.clone(), client, channel, format!("{} is not a proper youtube id", args[0]));
                                }
                            });
                        thread::spawn(move || { tokio::run(future) });
                    } else {
                        send_message(con.clone(), client, channel, format!("{} is not a proper youtube id", args[0]));
                    }
                }
            }
        }
    }
}
