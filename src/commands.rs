// [("bits", commandBits, Mod, Mod), ("greetings", commandGreetings, Mod, Mod), ("giveaway", commandGiveaway, Mod, Mod), ("poll", commandPoll, Mod, Mod), ("permit", commandPermit, Mod, Mod), ("watchtime", commandWatchtime, Mod, Mod), ("clip", commandClip, All, All), , ("genwebauth", commandWebAuth, Mod, Mod), ("listads", commandListCommercials, Mod, Mod), ("listsettings", commandListSettings, Mod, Mod), ("unmod", commandUnmod, Mod, Mod)]

// [("watchtime", watchtimeVar), ("watchrank", watchrankVar), ("watchranks", watchranksVar), ("counterinc", counterincVar), ("hotkey", hotkeyVar), ("obs:scene-change", obsSceneChangeVar), ("youtube:latest-url", youtubeLatestUrlVar), ("youtube:latest-title", youtubeLatestTitleVar), ("fortnite:wins", fortWinsVar), ("fortnite:kills", fortKillsVar), ("fortnite:lifewins", fortLifeWinsVar), ("fortnite:lifekills", fortLifeKillsVar), ("fortnite:solowins", fortSoloWinsVar), ("fortnite:solokills", fortSoloKillsVar), ("fortnite:duowins", fortDuoWinsVar), ("fortnite:duokills", fortDuoKillsVar), ("fortnite:squadwins", fortSquadWinsVar), ("fortnite:squadkills", fortSquadKillsVar), ("fortnite:season-solowins", fortSeasonSoloWinsVar), ("fortnite:season-solokills", fortSeasonSoloKillsVar), ("fortnite:season-duowins", fortSeasonDuoWinsVar), ("fortnite:season-duokills", fortSeasonDuoKillsVar), ("fortnite:season-squadwins", fortSeasonSquadWinsVar), ("fortnite:season-squadkills", fortSeasonSquadKillsVar), ("pubg:damage", pubgDmgVar), ("pubg:headshots", pubgHeadshotsVar), ("pubg:kills", pubgKillsVar), ("pubg:roadkills", pubgRoadKillsVar), ("pubg:teamkills", pubgTeamKillsVar), ("pubg:vehiclesDestroyed", pubgVehiclesDestroyedVar), ("pubg:wins", pubgWinsVar)]

use crate::types::*;
use crate::util::*;

use std::sync::Arc;
use std::collections::HashSet;
use std::{thread,time};
use irc::client::prelude::*;
use chrono::{Utc, DateTime, FixedOffset, Duration};
use humantime::format_duration;
use itertools::Itertools;
use r2d2_redis::r2d2;
use r2d2_redis::redis::Commands;

pub const native_commands: [(&str, fn(Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, &IrcClient, &str, &Vec<&str>), bool, bool); 12] = [("echo", echo_cmd, true, true), ("set", set_cmd, true, true), ("unset", unset_cmd, true, true), ("command", command_cmd, true, true), ("title", title_cmd, false, true), ("game", game_cmd, false, true), ("notices", notices_cmd, true, true), ("moderation", moderation_cmd, true, true), ("multi", multi_cmd, false, true), ("counters", counters_cmd, true, true), ("phrases", phrases_cmd, true, true), ("commercials", commercials_cmd, true, true)];

pub const command_vars: [(&str, fn(Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, &IrcClient, &str, &Message, Vec<&str>, &Vec<&str>) -> String); 13] = [("args", args_var), ("cmd", cmd_var), ("uptime", uptime_var), ("user", user_var), ("channel", channel_var), ("followage", followage_var), ("subcount", subcount_var), ("followcount", followcount_var), ("counter", counter_var), ("phrase", phrase_var), ("countdown", countdown_var), ("date", date_var), ("dateinc", dateinc_var)];

fn args_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 0 {
        let num: Result<usize,_> = vargs[0].parse();
        match num {
            Ok(num) => cargs[num-1].to_owned(),
            Err(_) => "".to_owned()
        }
    } else {
        "".to_owned()
    }
}

fn cmd_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, irc_message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 0 {
        let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", channel, vargs[0]), "message");
        if let Ok(message) = res {
            let mut message = message;
            for var in command_vars.iter() {
                if var.0 != "cmd" {
                    message = parse_var(var, &message, con.clone(), &client, channel, &irc_message, &vargs[1..].to_vec());
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

fn uptime_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
    let rsp = twitch_request_get(con.clone(), channel, &format!("https://api.twitch.tv/kraken/streams?channel={}", "31673862"));

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
                        return format!("{}{}", formatted[0], formatted[1]);
                    } else {
                        "".to_owned()
                    }
                }
            }
        }
    }
}

fn user_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let nick = get_nick(message);
    return nick;
}

fn channel_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let mut display = channel.to_owned();
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
}

fn followage_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    "".to_string()
}

fn subcount_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    "".to_string()
}

fn followcount_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    "".to_string()
}

fn counter_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    if vargs.len() > 0 {
        let res: Result<String,_> = con.hget(format!("channel:{}:counters", channel), vargs[0]);
        if let Ok(counter) = res {
            let num: Result<i16,_> = counter.parse();
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

fn phrase_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
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

fn date_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
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

fn dateinc_var(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
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

fn countdown_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    "".to_string()
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
                let num: Result<i16,_> = args[1].parse();
                match num {
                    Ok(num) => {
                        if num % 30 == 0 {
                            let _: () = con.rpush(format!("channel:{}:notices:{}:commands", channel, args[1]), args[2]).unwrap();
                            let _: () = con.set(format!("channel:{}:notices:{}:countdown", channel, args[1]), args[1]).unwrap();
                            let _ = client.send_privmsg(format!("#{}", channel), "notice has been added");
                        } else {
                            let _ = client.send_privmsg(format!("#{}", channel), "notice interval must be a multiple of 30");
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
                    let res: Result<i16,_> = counter.parse();
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
                let num: Result<i16,_> = args[1].parse();
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
                                let length: i16 = con.llen(format!("channel:{}:commercials:recent", channel)).unwrap();
                                let _: () = con.lpush(format!("channel:{}:commercials:recent", channel), format!("{} {}", Utc::now().to_rfc3339(), num)).unwrap();
                                if length > 7 {
                                    let _: () = con.rpop(format!("channel:{}:commercials:recent", channel)).unwrap();
                                }
                                if let Ok(mut rsp) = rsp {
                                    if let Ok(text) = rsp.text() {
                                        println!("{}",text);
                                    }
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
