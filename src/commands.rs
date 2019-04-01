// [("bits", commandBits, Mod, Mod), ("phrases", commandPhrases, Mod, Mod), ("greetings", commandGreetings, Mod, Mod), ("giveaway", commandGiveaway, Mod, Mod), ("poll", commandPoll, Mod, Mod), ("permit", commandPermit, Mod, Mod), ("watchtime", commandWatchtime, Mod, Mod), ("clip", commandClip, All, All), , ("genwebauth", commandWebAuth, Mod, Mod), ("listads", commandListCommercials, Mod, Mod), ("listsettings", commandListSettings, Mod, Mod), ("unmod", commandUnmod, Mod, Mod)]

// [("args", argsVar), ("watchtime", watchtimeVar), ("watchrank", watchrankVar), ("watchranks", watchranksVar), ("countdown", countdownVar), ("counterinc", counterincVar), ("counter", counterVar), ("phrase", phraseVar), ("hotkey", hotkeyVar), ("obs:scene-change", obsSceneChangeVar), ("youtube:latest-url", youtubeLatestUrlVar), ("youtube:latest-title", youtubeLatestTitleVar), ("fortnite:wins", fortWinsVar), ("fortnite:kills", fortKillsVar), ("fortnite:lifewins", fortLifeWinsVar), ("fortnite:lifekills", fortLifeKillsVar), ("fortnite:solowins", fortSoloWinsVar), ("fortnite:solokills", fortSoloKillsVar), ("fortnite:duowins", fortDuoWinsVar), ("fortnite:duokills", fortDuoKillsVar), ("fortnite:squadwins", fortSquadWinsVar), ("fortnite:squadkills", fortSquadKillsVar), ("fortnite:season-solowins", fortSeasonSoloWinsVar), ("fortnite:season-solokills", fortSeasonSoloKillsVar), ("fortnite:season-duowins", fortSeasonDuoWinsVar), ("fortnite:season-duokills", fortSeasonDuoKillsVar), ("fortnite:season-squadwins", fortSeasonSquadWinsVar), ("fortnite:season-squadkills", fortSeasonSquadKillsVar), ("pubg:damage", pubgDmgVar), ("pubg:headshots", pubgHeadshotsVar), ("pubg:kills", pubgKillsVar), ("pubg:roadkills", pubgRoadKillsVar), ("pubg:teamkills", pubgTeamKillsVar), ("pubg:vehiclesDestroyed", pubgVehiclesDestroyedVar), ("pubg:wins", pubgWinsVar)]

use crate::types::*;
use crate::util::*;

use std::sync::Arc;
use std::collections::HashSet;
use irc::client::prelude::*;
use itertools::Itertools;
use r2d2_redis::r2d2;
use r2d2_redis::redis::Commands;

pub const native_commands: [(&str, fn(Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, &IrcClient, &str, &Vec<&str>), bool, bool); 12] = [("echo", echo_cmd, true, true), ("set", set_cmd, true, true), ("unset", unset_cmd, true, true), ("command", command_cmd, true, true), ("title", title_cmd, false, true), ("game", game_cmd, false, true), ("notices", notices_cmd, true, true), ("moderation", moderation_cmd, true, true), ("runads", runads_cmd, true, true), ("multi", multi_cmd, false, true), ("counters", counters_cmd, true, true), ("commercials", commercials_cmd, true, true)];

pub const command_vars: [(&str, fn(Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, &IrcClient, &str, &Message, Vec<&str>, &Vec<&str>) -> String); 7] = [("cmd", cmd_var), ("uptime", uptime_var), ("user", user_var), ("channel", channel_var), ("followage", followage_var), ("subcount", subcount_var), ("followcount", followcount_var)];

fn cmd_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    "".to_string()
}

fn uptime_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    "".to_string()
}

fn user_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    let nick = get_nick(message);
    return nick;
}

fn channel_var(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, message: &Message, vargs: Vec<&str>, cargs: &Vec<&str>) -> String {
    return channel.to_owned();
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

fn echo_cmd(_con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    client.send_privmsg(format!("#{}", channel), args.join(" ")).unwrap();
}

fn set_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    match args.len() {
        1 => {
            let _: () = con.hset(format!("channel:{}:settings", channel), args[0], true).unwrap();
            client.send_privmsg(format!("#{}", channel), format!("{} has been set to: true", args[0])).unwrap();
        }
        2 => {
            let _: () = con.hset(format!("channel:{}:settings", channel), args[0], args[1]).unwrap();
            client.send_privmsg(format!("#{}", channel), format!("{} has been set to: {}", args[0], args[1])).unwrap();
        }
        _ => {}
    }
}

fn unset_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() == 1 {
        let _: () = con.hdel(format!("channel:{}:settings", channel), args[0]).unwrap();
        client.send_privmsg(format!("#{}", channel), format!("{} has been unset", args[0])).unwrap();
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
                    client.send_privmsg(format!("#{}", channel), format!("{} has been added", args[1])).unwrap();
                }
            }
            "remove" => {
                let _: () = con.del(format!("channel:{}:commands:{}", channel, args[1])).unwrap();
                client.send_privmsg(format!("#{}", channel), format!("{} has been removed", args[1])).unwrap();
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
                    Ok(json) => { client.send_privmsg(format!("#{}", channel), json.status).unwrap() }
                }
            }
        }
    } else {
        let rsp = twitch_request_put(con.clone(), channel, &format!("https://api.twitch.tv/kraken/channels/{}", id), format!("channel[status]={}", args[0]));

        match rsp {
            Err(e) => { println!("{}", e) }
            Ok(mut rsp) => {
                let json: Result<KrakenChannel,_> = rsp.json();
                match json {
                    Err(e) => { println!("{}", e) }
                    Ok(json) => { client.send_privmsg(format!("#{}", channel), format!("Title is now set to: {}", json.status)).unwrap() }
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
                    Ok(json) => { client.send_privmsg(format!("#{}", channel), json.game).unwrap() }
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
                            client.send_privmsg(format!("#{}", channel), format!("Unable to find a game matching: {}", args.join(" "))).unwrap()
                        } else {
                            let name = &json.data[0].name;
                            let rsp = twitch_request_put(con.clone(), channel, &format!("https://api.twitch.tv/kraken/channels/{}", id), format!("channel[game]={}", name));

                            match rsp {
                                Err(e) => { println!("{}", e) }
                                Ok(mut rsp) => {
                                    let json: Result<KrakenChannel,_> = rsp.json();
                                    match json {
                                        Err(e) => { println!("{}", e) }
                                        Ok(json) => { client.send_privmsg(format!("#{}", channel), format!("Game is now set to: {}", name)).unwrap() }
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
                            client.send_privmsg(format!("#{}", channel), "notice has been added").unwrap();
                        } else {
                            client.send_privmsg(format!("#{}", channel), "notice interval must be a multiple of 30").unwrap();
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
                            client.send_privmsg(format!("#{}", channel), format!("{} has been whitelisted", args[2])).unwrap();
                        }
                        "remove" => {
                            let _: () = con.srem(format!("channel:{}:moderation:links", channel), args[2]).unwrap();
                            client.send_privmsg(format!("#{}", channel), format!("{} has been removed from the whitelist", args[2])).unwrap();
                        }
                        _ => {}
                    }
                }
            }
            "colors" => {
                match args[1] {
                    "on" => {
                        let _: () = con.set(format!("channel:{}:moderation:colors", channel), true).unwrap();
                        client.send_privmsg(format!("#{}", channel), "Color filter has been turned on").unwrap();
                    }
                    "off" => {
                        let _: () = con.set(format!("channel:{}:moderation:colors", channel), false).unwrap();
                        client.send_privmsg(format!("#{}", channel), "Color filter has been turned off").unwrap();
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
        if streams.len() > 0 { client.send_privmsg(format!("#{}", channel), format!("http://multistre.am/{}/{}", channel, streams.iter().join("/"))).unwrap() }
    } else if args.len() == 1 && args[0] == "clear" {
        let _: () = con.del(format!("channel:{}:multi", channel)).unwrap();
        client.send_privmsg(format!("#{}", channel), "!multi has been cleared").unwrap();
    } else if args.len() > 1 && args[0] == "set" {
        let _: () = con.del(format!("channel:{}:multi", channel)).unwrap();
        for arg in args[1..].iter() {
            let _: () = con.sadd(format!("channel:{}:multi", channel), arg.to_owned()).unwrap();
        }
        client.send_privmsg(format!("#{}", channel), "!multi has been set").unwrap();
    }
}

fn counters_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() == 1 {

    }
}

fn commercials_cmd(con: Arc<r2d2::PooledConnection<r2d2_redis::RedisConnectionManager>>, client: &IrcClient, channel: &str, args: &Vec<&str>) {
    if args.len() > 1 {
        match args[0] {
            "submode" => {
                match args[1] {
                    "on" => {
                        let _: () = con.set(format!("channel:{}:commercials:submode", channel), true).unwrap();
                        client.send_privmsg(format!("#{}", channel), "Submode during commercials has been turned on").unwrap();
                    }
                    "off" => {
                        let _: () = con.set(format!("channel:{}:commercials:submode", channel), false).unwrap();
                        client.send_privmsg(format!("#{}", channel), "Submode during commercials has been turned off").unwrap();
                    }
                    _ => {}
                }
            }
            "notice" => {
                let exists: bool = con.exists(format!("channel:{}:commands:{}", channel, args[1])).unwrap();
                if exists {
                    let _: () = con.set(format!("channel:{}:commercials:notice", channel), args[1]).unwrap();
                    client.send_privmsg(format!("#{}", channel), format!("{} will be run at the start of commercials", args[1])).unwrap();
                } else {
                    client.send_privmsg(format!("#{}", channel), format!("{} is not an existing command", args[1])).unwrap();
                }
            }
            "hourly" => {
                let num: Result<i16,_> = args[1].parse();
                match num {
                    Ok(num) => {
                        let _: () = con.set(format!("channel:{}:commercials:hourly", channel), args[1]).unwrap();
                        client.send_privmsg(format!("#{}", channel), format!("{} commercials will be run each hour", args[1])).unwrap();
                    }
                    Err(e) => {
                        client.send_privmsg(format!("#{}", channel), format!("{} could not be parsed as a number", args[1])).unwrap();
                    }
                }
            }
            "run" => {
                let num: Result<i16,_> = args[1].parse();
                match num {
                    Ok(num) => {
                        if num > 0 && num < 7 {
                            let id: String = con.get(format!("channel:{}:id", channel)).unwrap();
                            let _ = twitch_request_post(con.clone(), channel, &format!("https://api.twitch.tv/kraken/channels/{}/commercial", id), format!("{{\"length\": {}}}", num * 30));
                            client.send_privmsg(format!("#{}", channel), format!("{} commercials have been run", args[1])).unwrap();
                        } else {
                            client.send_privmsg(format!("#{}", channel), format!("{} must be a number between one and six", args[1])).unwrap();
                        }
                        // ("https://api.twitch.tv/kraken/channels/" ++ cid ++ "/commercial") $ object [("length", Number $ scientific (num * 30) 0)]
                        client.send_privmsg(format!("#{}", channel), format!("{} commercials have been run", args[1])).unwrap();
                    }
                    Err(e) => {
                        client.send_privmsg(format!("#{}", channel), format!("{} could not be parsed as a number", args[1])).unwrap();
                    }
                }
            }
            _ => {}
        }
    }
}
