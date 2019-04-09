// [("bits", commandBits, Mod, Mod), ("greetings", commandGreetings, Mod, Mod), ("giveaway", commandGiveaway, Mod, Mod), ("poll", commandPoll, Mod, Mod), ("permit", commandPermit, Mod, Mod), ("watchtime", commandWatchtime, Mod, Mod), ("clip", commandClip, All, All), , ("genwebauth", commandWebAuth, Mod, Mod), ("listads", commandListCommercials, Mod, Mod), ("listsettings", commandListSettings, Mod, Mod), ("unmod", commandUnmod, Mod, Mod)]

// [("watchtime", watchtimeVar), ("watchrank", watchrankVar), ("watchranks", watchranksVar), ("hotkey", hotkeyVar), ("obs:scene-change", obsSceneChangeVar), ("fortnite:wins", fortWinsVar), ("fortnite:kills", fortKillsVar), ("fortnite:lifewins", fortLifeWinsVar), ("fortnite:lifekills", fortLifeKillsVar), ("fortnite:solowins", fortSoloWinsVar), ("fortnite:solokills", fortSoloKillsVar), ("fortnite:duowins", fortDuoWinsVar), ("fortnite:duokills", fortDuoKillsVar), ("fortnite:squadwins", fortSquadWinsVar), ("fortnite:squadkills", fortSquadKillsVar), ("fortnite:season-solowins", fortSeasonSoloWinsVar), ("fortnite:season-solokills", fortSeasonSoloKillsVar), ("fortnite:season-duowins", fortSeasonDuoWinsVar), ("fortnite:season-duokills", fortSeasonDuoKillsVar), ("fortnite:season-squadwins", fortSeasonSquadWinsVar), ("fortnite:season-squadkills", fortSeasonSquadKillsVar)]

use crate::types::*;
use crate::util::*;

use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use std::{thread,time};
use irc::client::prelude::*;
use chrono::{Utc, DateTime, FixedOffset, Duration};
use humantime::format_duration;
use itertools::Itertools;
use r2d2_redis::r2d2;
use r2d2_redis::redis::Commands;

pub const native_commands: [(&str, fn(Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, &IrcClient, &str, &Vec<&str>), bool, bool); 12] = [("echo", echo_cmd, true, true), ("set", set_cmd, true, true), ("unset", unset_cmd, true, true), ("command", command_cmd, true, true), ("title", title_cmd, false, true), ("game", game_cmd, false, true), ("notices", notices_cmd, true, true), ("moderation", moderation_cmd, true, true), ("multi", multi_cmd, false, true), ("counters", counters_cmd, true, true), ("phrases", phrases_cmd, true, true), ("commercials", commercials_cmd, true, true)];

pub const command_vars: [(&str, fn(Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, &IrcClient, &str, Option<&Message>, Vec<&str>, &Vec<&str>) -> String); 25] = [("args", args_var), ("uptime", uptime_var), ("user", user_var), ("channel", channel_var), ("cmd", cmd_var), ("counterinc", counterinc_var), ("followage", followage_var), ("subcount", subcount_var), ("followcount", followcount_var), ("counter", counter_var), ("phrase", phrase_var), ("countdown", countdown_var), ("date", date_var), ("dateinc", dateinc_var), ("watchtime", watchtime_var), ("watchrank", watchrank_var), ("youtube:latest-url", youtube_latest_url_var), ("youtube:latest-title", youtube_latest_title_var), ("pubg:damage", pubg_damage_var), ("pubg:headshots", pubg_headshots_var), ("pubg:kills", pubg_kills_var), ("pubg:roadkills", pubg_roadkills_var), ("pubg:teamkills", pubg_teamkills_var), ("pubg:vehiclesDestroyed", pubg_vehicles_destroyed_var), ("pubg:wins", pubg_wins_var)];

pub const twitch_bots: [&str; 20] = ["electricallongboard","lanfusion","cogwhistle","freddyybot","anotherttvviewer","apricotdrupefruit","skinnyseahorse","p0lizei_","xbit01","n3td3v","cachebear","icon_bot","virgoproz","v_and_k","slocool","host_giveaway","nightbot","commanderroot","p0sitivitybot","streamlabs"];

fn args_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 0 {
        let num: Result<usize,_> = vargs[0].parse();
        match num {
            Ok(num) => cargs[num-1].to_owned(),
            Err(_) => "".to_owned()
        }
    } else {
        cargs.join(" ").to_owned()
    }
}

fn cmd_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, irc_message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 0 {
        let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, vargs[0]), "message");
        if let Ok(message) = res {
            let mut message = message;
            for var in command_vars.iter() {
                if var.0 != "cmd" {
                    message = parse_var(var, &message, con.clone(), &client, channel, irc_message, &vargs[1..].to_vec());
                }
            }
            let _ = client.send_privmsg(format!("#{}", channel), message);
        } else {
            for cmd in native_commands.iter() {
                if format!("!{}", cmd.0) == vargs[0] {
                    (cmd.1)(con.clone(), &client, channel, &vargs[1..].to_vec())
                }
            }
        }
    }
    "".to_owned()
}

fn uptime_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
    let rsp = twitch_request_get(con.clone(), channel, &format!("https://api.twitch.tv/kraken/streams?channel={}", id));

    match rsp {
        Err(e) => { "".to_owned() }
        Ok(mut rsp) => {
            let json: Result<KrakenStreams,_> = rsp.json();
            match json {
                Err(e) => { eprintln!("{}",e);"".to_owned() }
                Ok(json) => {
                    if json.total > 0 {
                        let dt = DateTime::parse_from_rfc3339(&json.streams[0].created_at).unwrap();
                        let diff = Utc::now().signed_duration_since(dt);
                        let formatted = format_duration(diff.to_std().unwrap()).to_string();
                        let formatted: Vec<&str> = formatted.split_whitespace().collect();
                        if formatted.len() == 1 {
                            return format!("{}", formatted[0]);
                        } else {
                            return format!("{}{}", formatted[0], formatted[1]);
                        }
                    } else {
                        "".to_owned()
                    }
                }
            }
        }
    }
}

fn user_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if let Some(message) = message {
        let mut display = get_nick(&message);
        if let Some(tags) = &message.tags {
            tags.iter().for_each(|tag| {
                if let Some(value) = &tag.1 {
                    if tag.0 == "display-name" {
                        if let Some(name) = &tag.1 {
                            display = name.to_owned();
                        }
                    }
                }
            });
        }
        return display;
    } else {
        "".to_owned()
    }
}

fn channel_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let display: String = con.get(format!("channel:{}:display-name", channel)).unwrap();
    return display;
}

fn counterinc_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 0 {
        let res: Result<String,_> = con.hget(format!("channel:{}:counters", channel), vargs[0]);
        if let Ok(counter) = res {
            let res: Result<u16,_> = counter.parse();
            if let Ok(num) = res {
                let _: () = con.hset(format!("channel:{}:counters", channel), vargs[0], num + 1).unwrap();
            } else {
                let _: () = con.hset(format!("channel:{}:counters", channel), vargs[0], 1).unwrap();
            }
        } else {
            let _: () = con.hset(format!("channel:{}:counters", channel), vargs[0], 1).unwrap();
        }
    }

    "".to_owned()
}

fn followage_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if let Some(message) = message {
        let rsp = twitch_request_get(con.clone(), channel, &format!("https://api.twitch.tv/kraken/users?login={}", get_nick(message)));
        match rsp {
            Err(e) => { "".to_owned() }
            Ok(mut rsp) => {
                let json: Result<KrakenUsers,_> = rsp.json();
                match json {
                    Err(e) => { "".to_owned() }
                    Ok(json) => {
                        if json.total > 0 {
                            let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
                            let rsp = twitch_request_get(con.clone(), channel, &format!("https://api.twitch.tv/kraken/users/{}/follows/channels/{}", &json.users[0].id, id));
                            match rsp {
                                Err(e) => { "".to_owned() }
                                Ok(mut rsp) => {
                                    let json: Result<KrakenFollow,_> = rsp.json();
                                    match json {
                                        Err(e) => { "0m".to_owned() }
                                        Ok(json) => {
                                            let timestamp = DateTime::parse_from_rfc3339(&json.created_at).unwrap();
                                            let diff = Utc::now().signed_duration_since(timestamp);
                                            let formatted = format_duration(diff.to_std().unwrap()).to_string();
                                            let formatted: Vec<&str> = formatted.split_whitespace().collect();
                                            if formatted.len() == 1 {
                                                return format!("{}", formatted[0]);
                                            } else {
                                                return format!("{}{}", formatted[0], formatted[1]);
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            "".to_owned()
                        }
                    }
                }
            }
        }
    } else {
        "".to_owned()
    }
}

fn subcount_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
    let rsp = twitch_request_get(con.clone(), channel, &format!("https://api.twitch.tv/kraken/channels/{}/subscriptions", id));

    match rsp {
        Err(e) => { "0".to_owned() }
        Ok(mut rsp) => {
            let json: Result<KrakenSubs,_> = rsp.json();
            match json {
                Err(e) => { "0".to_owned() }
                Ok(json) => { json.total.to_string() }
            }
        }
    }
}

fn followcount_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
    let rsp = twitch_request_get(con.clone(), channel, &format!("https://api.twitch.tv/kraken/channels/{}/follows", id));

    match rsp {
        Err(e) => { "0".to_owned() }
        Ok(mut rsp) => {
            let json: Result<KrakenFollows,_> = rsp.json();
            match json {
                Err(e) => { "0".to_owned() }
                Ok(json) => { json.total.to_string() }
            }
        }
    }
}

fn counter_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 0 {
        let res: Result<String,_> = con.hget(format!("channel:{}:counters", channel), vargs[0]);
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

fn phrase_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 0 {
        let res: Result<String,_> = con.hget(format!("channel:{}:phrases", channel), vargs[0]);
        if let Ok(phrase) = res {
            phrase
        } else {
            "".to_owned()
        }
    } else {
        "".to_owned()
    }
}

fn date_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 0 {
        let res: Result<String,_> = con.hget(format!("channel:{}:phrases", channel), vargs[0]);
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

fn dateinc_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 1 {
        let res: Result<String,_> = con.hget(format!("channel:{}:phrases", channel), vargs[1]);
        if let Ok(phrase) = res {
            let res: Result<DateTime<FixedOffset>,_> = DateTime::parse_from_rfc3339(&phrase);
            if let Ok(dt) = res {
                let res: Result<i64,_> = vargs[0].parse();
                if let Ok(num) = res {
                    let ndt = dt + Duration::seconds(num);
                    let _: () = con.hset(format!("channel:{}:phrases", channel), vargs[1], ndt.to_rfc3339()).unwrap();
                }
            }
        }
    }
    "".to_string()
}

fn countdown_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    "".to_string()
}

fn watchtime_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if let Some(message) = message {
        let res: Result<String,_> = con.hget(format!("channel:{}:watchtimes", channel), get_nick(&message));
        if let Ok(watchtime) = res {
            let res: Result<i64,_> = watchtime.parse();
            if let Ok(num) = res {
                let diff = Duration::minutes(num);
                let formatted = format_duration(diff.to_std().unwrap()).to_string();
                let formatted: Vec<&str> = formatted.split_whitespace().collect();
                if formatted.len() == 1 {
                    return format!("{}", formatted[0]);
                } else {
                    return format!("{}{}", formatted[0], formatted[1]);
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

fn watchrank_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if let Some(message) = message {
        let hashtimes: HashMap<String,String> = con.hgetall(format!("channel:{}:watchtimes", channel)).unwrap_or(HashMap::new());
        let mut watchtimes: Vec<(String,u64)> = Vec::new();
        for (key, value) in hashtimes.iter() {
            let num: u64 = value.parse().unwrap();
            watchtimes.push((key.to_owned(), num));
        }
        let list: String = con.hget(format!("channel:{}:settings", channel), "watchtime:blacklist").unwrap_or("".to_owned());
        let mut blacklist: Vec<&str> = twitch_bots.clone().to_vec();
        blacklist.push(channel);
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
                    return top.iter().map(|(n,t)| n).join(", ").to_owned();
                } else {
                    "".to_owned()
                }
            } else {
                let i = watchtimes.iter().position(|wt| wt.0 == get_nick(message));
                if let Some(i) = i {
                    return (i + 1).to_string();
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

fn youtube_latest_url_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 0 {
        let rsp = request_get(&format!("https://decapi.me/youtube/latest_video?id={}", vargs[0]));
        match rsp {
            Err(e) => { "".to_owned() }
            Ok(mut rsp) => {
                if let Ok(body) = rsp.text() {
                    let data: Vec<&str> = body.split(" - ").collect();
                    if data.len() > 1 {
                        return data[data.len()-1].to_owned();
                    } else {
                        "".to_owned()
                    }
                } else {
                    "".to_owned()
                }
            }
        }
    } else {
        "".to_owned()
    }
}

fn youtube_latest_title_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 0 {
        let rsp = request_get(&format!("https://decapi.me/youtube/latest_video?id={}", vargs[0]));
        match rsp {
            Err(e) => { "".to_owned() }
            Ok(mut rsp) => {
                if let Ok(body) = rsp.text() {
                    let data: Vec<&str> = body.split(" - ").collect();
                    if data.len() > 1 {
                        return data[0..data.len()-1].join(" - ");
                    } else {
                        "".to_owned()
                    }
                } else {
                    "".to_owned()
                }
            }
        }
    } else {
        "".to_owned()
    }
}

fn pubg_damage_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "damageDealt").unwrap_or("0".to_owned());
    return value;
}

fn pubg_headshots_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "headshotKills").unwrap_or("0".to_owned());
    return value;
}

fn pubg_kills_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "kills").unwrap_or("0".to_owned());
    return value;
}

fn pubg_roadkills_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "roadKills").unwrap_or("0".to_owned());
    return value;
}

fn pubg_teamkills_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "teamKills").unwrap_or("0".to_owned());
    return value;
}

fn pubg_vehicles_destroyed_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "vehicleDestroys").unwrap_or("0".to_owned());
    return value;
}

fn pubg_wins_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: Option<&Message>, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let value: String = con.hget(format!("channel:{}:stats:pubg", channel), "wins").unwrap_or("0".to_owned());
    return value;
}

fn echo_cmd(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    let _ = client.send_privmsg(format!("#{}", channel), args.join(" "));
}

fn set_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    match args.len() {
        1 => {
            let _: () = con.hset(format!("channel:{}:settings", channel), args[0], true).unwrap();
            let _ = client.send_privmsg(format!("#{}", channel), format!("{} has been set to: true", args[0]));
        }
        2 => {
            let _: () = con.hset(format!("channel:{}:settings", channel), args[0], args[1]).unwrap();
            let _ = client.send_privmsg(format!("#{}", channel), format!("{} has been set to: {}", args[0], args[1]));
        }
        _ => {}
    }
}

fn unset_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() == 1 {
        let _: () = con.hdel(format!("channel:{}:settings", channel), args[0]).unwrap();
        let _ = client.send_privmsg(format!("#{}", channel), format!("{} has been unset", args[0]));
    }
}

fn command_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() > 1 {
        match args[0] {
            "add" => {
                if args.len() > 2 {
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, args[1]), "message", args[2..].join(" ")).unwrap();
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, args[1]), "cmd_protected", false).unwrap();
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, args[1]), "arg_protected", false).unwrap();
                    let _ = client.send_privmsg(format!("#{}", channel), format!("{} has been added", args[1]));
                }
            }
            "modadd" => {
                if args.len() > 2 {
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, args[1]), "message", args[2..].join(" ")).unwrap();
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, args[1]), "cmd_protected", true).unwrap();
                    let _: () = con.hset(format!("channel:{}:commands:{}", channel, args[1]), "arg_protected", true).unwrap();
                    let _ = client.send_privmsg(format!("#{}", channel), format!("{} has been added", args[1]));
                }
            }
            "remove" => {
                let _: () = con.del(format!("channel:{}:commands:{}", channel, args[1])).unwrap();
                let _ = client.send_privmsg(format!("#{}", channel), format!("{} has been removed", args[1]));
            }
            _ => {}
        }
    }
}

fn title_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
    if args.len() == 0 {
        let rsp = twitch_request_get(con.clone(), channel, &format!("https://api.twitch.tv/kraken/channels/{}", id));

        match rsp {
            Err(e) => { println!("{}", e) }
            Ok(mut rsp) => {
                let json: Result<KrakenChannel,_> = rsp.json();
                match json {
                    Err(e) => { println!("{}", e) }
                    Ok(json) => { let _ = client.send_privmsg(format!("#{}", channel), json.status); }
                }
            }
        }
    } else {
        let rsp = twitch_request_put(con.clone(), channel, &format!("https://api.twitch.tv/kraken/channels/{}", id), format!("channel[status]={}", args.join(" ")));

        match rsp {
            Err(e) => { println!("{}", e) }
            Ok(mut rsp) => {
                let json: Result<KrakenChannel,_> = rsp.json();
                match json {
                    Err(e) => { println!("{}", e) }
                    Ok(json) => { let _ = client.send_privmsg(format!("#{}", channel), format!("Title is now set to: {}", json.status)); }
                }
            }
        }
    }
}

fn game_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
    if args.len() == 0 {
        let rsp = twitch_request_get(con.clone(), channel, &format!("https://api.twitch.tv/kraken/channels/{}", id));

        match rsp {
            Err(e) => { println!("{}", e) }
            Ok(mut rsp) => {
                let json: Result<KrakenChannel,_> = rsp.json();
                match json {
                    Err(e) => { println!("{}", e) }
                    Ok(json) => { let _ = client.send_privmsg(format!("#{}", channel), json.game); }
                }
            }
        }
    } else {
        let rsp = twitch_request_get(con.clone(), channel, &format!("https://api.twitch.tv/helix/games?name={}", args.join(" ")));

        match rsp {
            Err(e) => { println!("{}", e) }
            Ok(mut rsp) => {
                let json: Result<HelixGames,_> = rsp.json();
                match json {
                    Err(e) => { println!("{}", e) }
                    Ok(json) => {
                        if json.data.len() == 0 {
                            let _ = client.send_privmsg(format!("#{}", channel), format!("Unable to find a game matching: {}", args.join(" ")));
                        } else {
                            let name = &json.data[0].name;
                            let rsp = twitch_request_put(con.clone(), channel, &format!("https://api.twitch.tv/kraken/channels/{}", id), format!("channel[game]={}", name));

                            match rsp {
                                Err(e) => { println!("{}", e) }
                                Ok(mut rsp) => {
                                    let json: Result<KrakenChannel,_> = rsp.json();
                                    match json {
                                        Err(e) => { println!("{}", e) }
                                        Ok(json) => { let _ = client.send_privmsg(format!("#{}", channel), format!("Game is now set to: {}", name)); }
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

fn notices_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() > 1 {
        match args[0] {
            "add" => {
                let num: Result<u16,_> = args[1].parse();
                match num {
                    Ok(num) => {
                        if num % 60 == 0 {
                            let _: () = con.rpush(format!("channel:{}:notices:{}:commands", channel, args[1]), args[2]).unwrap();
                            let _: () = con.set(format!("channel:{}:notices:{}:countdown", channel, args[1]), args[1]).unwrap();
                            let _ = client.send_privmsg(format!("#{}", channel), "notice has been added");
                        } else {
                            let _ = client.send_privmsg(format!("#{}", channel), "notice interval must be a multiple of 60");
                        }
                    }
                    Err(_) => {}
                }
            }
            _ => {}
        }
    }
}

fn moderation_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() > 1 {
        match args[0] {
            "links" => {
                if args.len() > 2 {
                    match args[1] {
                        "add" => {
                            let _: () = con.sadd(format!("channel:{}:moderation:links", channel), args[2]).unwrap();
                            let _ = client.send_privmsg(format!("#{}", channel), format!("{} has been whitelisted", args[2]));
                        }
                        "remove" => {
                            let _: () = con.srem(format!("channel:{}:moderation:links", channel), args[2]).unwrap();
                            let _ = client.send_privmsg(format!("#{}", channel), format!("{} has been removed from the whitelist", args[2]));
                        }
                        _ => {}
                    }
                }
            }
            "colors" => {
                match args[1] {
                    "on" => {
                        let _: () = con.set(format!("channel:{}:moderation:colors", channel), true).unwrap();
                        let _ = client.send_privmsg(format!("#{}", channel), "Color filter has been turned on");
                    }
                    "off" => {
                        let _: () = con.set(format!("channel:{}:moderation:colors", channel), false).unwrap();
                        let _ = client.send_privmsg(format!("#{}", channel), "Color filter has been turned off");
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn runads_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() == 1 {

    }
}

fn multi_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() == 0 {
        let streams: HashSet<String> = con.smembers(format!("channel:{}:multi", channel)).unwrap();
        if streams.len() > 0 { let _ = client.send_privmsg(format!("#{}", channel), format!("http://multistre.am/{}/{}", channel, streams.iter().join("/"))); }
    } else if args.len() == 1 && args[0] == "clear" {
        let _: () = con.del(format!("channel:{}:multi", channel)).unwrap();
        let _ = client.send_privmsg(format!("#{}", channel), "!multi has been cleared");
    } else if args.len() > 1 && args[0] == "set" {
        let _: () = con.del(format!("channel:{}:multi", channel)).unwrap();
        for arg in args[1..].iter() {
            let _: () = con.sadd(format!("channel:{}:multi", channel), arg.to_owned()).unwrap();
        }
        let _ = client.send_privmsg(format!("#{}", channel), "!multi has been set");
    }
}

fn counters_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() > 1 {
        match args[0] {
            "set" => {
                if args.len() > 2 {
                    let _: () = con.hset(format!("channel:{}:counters", channel), args[1], args[2]).unwrap();
                    let _ = client.send_privmsg(format!("#{}", channel), format!("{} has been set to: {}", args[1], args[2]));
                }
            }
            "inc" => {
                let res: Result<String,_> = con.hget(format!("channel:{}:counters", channel), args[1]);
                if let Ok(counter) = res {
                    let res: Result<u16,_> = counter.parse();
                    if let Ok(num) = res {
                        let _: () = con.hset(format!("channel:{}:counters", channel), args[1], num + 1).unwrap();
                    } else {
                        let _: () = con.hset(format!("channel:{}:counters", channel), args[1], 1).unwrap();
                    }
                } else {
                    let _: () = con.hset(format!("channel:{}:counters", channel), args[1], 1).unwrap();
                }
                let _ = client.send_privmsg(format!("#{}", channel), format!("{} has been increased", args[1]));
            }
            _ => {}
        }
    }
}

fn phrases_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() > 2 {
        match args[0] {
            "set" => {
                let _: () = con.hset(format!("channel:{}:phrases", channel), args[1], args[2..].join(" ")).unwrap();
                let _ = client.send_privmsg(format!("#{}", channel), format!("{} has been set to: {}", args[1], args[2..].join(" ")));
            }
            _ => {}
        }
    }
}

fn commercials_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() > 1 {
        match args[0] {
            "submode" => {
                match args[1] {
                    "on" => {
                        let _: () = con.set(format!("channel:{}:commercials:submode", channel), true).unwrap();
                        let _ = client.send_privmsg(format!("#{}", channel), "Submode during commercials has been turned on");
                    }
                    "off" => {
                        let _: () = con.set(format!("channel:{}:commercials:submode", channel), false).unwrap();
                        let _ = client.send_privmsg(format!("#{}", channel), "Submode during commercials has been turned off");
                    }
                    _ => {}
                }
            }
            "notice" => {
                let exists: bool = con.exists(format!("channel:{}:commands:{}", channel, args[1])).unwrap();
                if exists {
                    let _: () = con.set(format!("channel:{}:commercials:notice", channel), args[1]).unwrap();
                    let _ = client.send_privmsg(format!("#{}", channel), format!("{} will be run at the start of commercials", args[1]));
                } else {
                    let _ = client.send_privmsg(format!("#{}", channel), format!("{} is not an existing command", args[1]));
                }
            }
            "hourly" => {
                let num: Result<u16,_> = args[1].parse();
                match num {
                    Ok(num) => {
                        let _: () = con.set(format!("channel:{}:commercials:hourly", channel), args[1]).unwrap();
                        let _ = client.send_privmsg(format!("#{}", channel), format!("{} commercials will be run each hour", args[1]));
                    }
                    Err(e) => {
                        let _ = client.send_privmsg(format!("#{}", channel), format!("{} could not be parsed as a number", args[1]));
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
                                if diff.num_minutes() < 9 {
                                    within8 = true;
                                }
                            }
                            if within8 {
                                let _ = client.send_privmsg(format!("#{}", channel), format!("Commercials can't be run within eight minutes of each other"));
                            } else {
                                let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
                                let submode: String = con.get(format!("channel:{}:commercials:submode", channel)).unwrap_or("false".to_owned());
                                let nres: Result<String,_> = con.get(format!("channel:{}:commercials:notice", channel));
                                let rsp = twitch_request_post(con.clone(), channel, &format!("https://api.twitch.tv/kraken/channels/{}/commercial", id), format!("{{\"length\": {}}}", num * 30));
                                let length: u16 = con.llen(format!("channel:{}:commercials:recent", channel)).unwrap();
                                let _: () = con.lpush(format!("channel:{}:commercials:recent", channel), format!("{} {}", Utc::now().to_rfc3339(), num)).unwrap();
                                if length > 7 {
                                    let _: () = con.rpop(format!("channel:{}:commercials:recent", channel)).unwrap();
                                }
                                if submode == "true" {
                                    let client_clone = client.clone();
                                    let channel_clone = String::from(channel);
                                    let _ = client.send_privmsg(format!("#{}", channel), "/subscribers");
                                    thread::spawn(move || {
                                        thread::sleep(time::Duration::from_secs(num * 30));
                                        client_clone.send_privmsg(format!("#{}", channel_clone), "/subscribersoff").unwrap();
                                    });
                                }
                                if let Ok(notice) = nres {
                                    let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, notice), "message");
                                    if let Ok(message) = res {
                                        let _ = client.send_privmsg(format!("#{}", channel), message);
                                    }
                                }
                                let _ = client.send_privmsg(format!("#{}", channel), format!("{} commercials have been run", args[1]));
                            }
                        } else {
                            let _ = client.send_privmsg(format!("#{}", channel), format!("{} must be a number between one and six", args[1]));
                        }
                    }
                    Err(e) => {
                        let _ = client.send_privmsg(format!("#{}", channel), format!("{} could not be parsed as a number", args[1]));
                    }
                }
            }
            _ => {}
        }
    }
}
