use crate::util::*;

use std::collections::HashMap;
use std::sync::Arc;
use serenity::client::{Context, EventHandler};
use serenity::model::channel::Message;
use serde::{Serialize, Deserialize};
use serde_json::Value;
use regex::{Regex,RegexBuilder,Captures};
use rocket_contrib::database;
use rocket_contrib::databases::redis;
use r2d2_redis::{r2d2, RedisConnectionManager};
use r2d2_redis::redis::Commands;

pub struct DiscordHandler {
    pub pool: r2d2::Pool<r2d2_redis::RedisConnectionManager>,
    pub channel: String
}

impl EventHandler for DiscordHandler {
    fn message(&self, ctx: Context, msg: Message) {
        let con = Arc::new(self.pool.get().unwrap());
        let prefix: String = con.hget(format!("channel:{}:settings", self.channel), "command:prefix").unwrap_or("!".to_owned());
        let id: String = con.hget(format!("channel:{}:settings", self.channel), "discord:mod-channel").unwrap_or("".to_owned());
        if msg.channel_id.as_u64().to_string() == id {
            let rgx = Regex::new("<:(\\w+):\\d+>").unwrap();
            let content = rgx.replace_all(&msg.content, |caps: &Captures| { if let Some(emote) = caps.get(1) { emote.as_str() } else { "" } }).to_string();
            let _: () = con.publish(format!("channel:{}:signals:command", self.channel), content).unwrap();
        } else {
            let mut words = msg.content.split_whitespace();
            if let Some(word) = words.next() {
                let mut word = word.to_lowercase();
                let mut args: Vec<String> = words.map(|w| w.to_owned()).collect();
                let res: Result<String,_> = con.hget(format!("channel:{}:commands:{}", self.channel, word), "message");
                if let Ok(message) = res {
                    let mut message = message;
                    message = parse_message(&message, con.clone(), None, &self.channel, None, &args);
                    let _ = msg.channel_id.say(&ctx.http, message);
                }
            }
        }
    }
}

#[database("redis")]
pub struct RedisConnection(redis::Connection);

#[derive(Debug)]
pub enum AuthError {
    Missing,
    Invalid
}

pub enum ThreadAction {
    Kill
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscordOpCode {
    pub op: u16,
    pub d: Value
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscordPayload {
    pub heartbeat_inverval: i32
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Auth {
    pub channel: String,
    pub exp: u64
}

#[derive(Debug, Deserialize)]
pub struct TmiChatters {
    pub chatters: Chatters
}

#[derive(Debug, Deserialize)]
pub struct Chatters {
    pub moderators: Vec<String>,
    pub viewers: Vec<String>,
    pub vips: Vec<String>
}

#[derive(Debug, Deserialize)]
pub struct HelixUsers {
    pub data: Vec<HelixUsersData>
}

#[derive(Debug, Deserialize)]
pub struct HelixUsersData {
    pub id: String,
    pub login: String,
    pub display_name: String
}

#[derive(Debug, Deserialize)]
pub struct HelixGames {
    pub data: Vec<HelixGamesData>
}

#[derive(Debug, Deserialize)]
pub struct HelixGamesData {
    pub name: String
}

#[derive(Debug, Deserialize)]
pub struct KrakenHosts {
    pub hosts: Vec<KrakenHost>
}

#[derive(Debug, Deserialize)]
pub struct KrakenHost {
    pub host_id: String,
    pub target_id: String
}

#[derive(Debug, Deserialize)]
pub struct KrakenFollow {
    pub created_at: String
}

#[derive(Debug, Deserialize)]
pub struct KrakenFollows {
    #[serde(rename = "_total")]
    pub total: i32
}

#[derive(Debug, Deserialize)]
pub struct KrakenSubs {
    #[serde(rename = "_total")]
    pub total: i32
}

#[derive(Debug, Deserialize)]
pub struct KrakenUsers {
    #[serde(rename = "_total")]
    pub total: i32,
    pub users: Vec<KrakenUser>
}

#[derive(Debug, Deserialize)]
pub struct KrakenUser {
    #[serde(rename = "_id")]
    pub id: String
}

#[derive(Debug, Deserialize)]
pub struct KrakenChannel {
    pub status: String,
    pub game: String,
    pub name: String,
    pub logo: String,
    pub url: String,
    pub display_name: String
}

#[derive(Debug, Deserialize)]
pub struct KrakenStreams {
    #[serde(rename = "_total")]
    pub total: u16,
    pub streams: Vec<KrakenStream>
}

#[derive(Debug, Deserialize)]
pub struct KrakenStream {
    pub created_at: String,
    pub viewers: i32,
    pub channel: KrakenChannel
}

#[derive(Debug, Deserialize)]
pub struct FortniteApi {
    pub stats: FortniteStats,
    pub lifeTimeStats: Vec<FortniteLifeStat>,
    pub recent_matches: Vec<FortniteMatch>
}

#[derive(Debug, Deserialize)]
pub struct FortniteLifeStat {
    pub key: String,
    pub value: String
}

#[derive(Debug, Deserialize)]
pub struct FortniteStats {
    #[serde(rename = "p2")]
    pub solo: HashMap<String,FortniteStat>,
    #[serde(rename = "p10")]
    pub duo: HashMap<String,FortniteStat>,
    #[serde(rename = "p9")]
    pub squad: HashMap<String,FortniteStat>
}

#[derive(Debug, Deserialize)]
pub struct FortniteStat {
    pub value: String
}

#[derive(Debug, Deserialize)]
pub struct FortniteMatch {
    pub id: i64,
    pub playlist: String,
    pub kills: i16,
    pub top1: i16
}

#[derive(Debug, Deserialize)]
pub struct PubgMatch {
    pub data: PubgMatchData,
    pub included: Vec<PubgMatchIncluded>
}

#[derive(Debug, Deserialize)]
pub struct PubgMatchData {
    pub attributes: PubgMatchAttrs
}

#[derive(Debug, Deserialize)]
pub struct PubgMatchAttrs {
    #[serde(rename = "createdAt")]
    pub created_at: String
}

#[derive(Debug, Deserialize)]
pub struct PubgMatchIncluded {
    #[serde(rename = "type")]
    pub type_: String,
    pub attributes: Value
}

#[derive(Debug, Deserialize)]
pub struct PubgPlayers {
    pub data: Vec<Player>
}

#[derive(Debug, Deserialize)]
pub struct Player {
    pub id: String
}

#[derive(Debug, Deserialize)]
pub struct PubgPlayer {
    pub data: PlayerData
}

#[derive(Debug, Deserialize)]
pub struct PlayerData {
    pub relationships: PlayerRelationships
}

#[derive(Debug, Deserialize)]
pub struct PlayerRelationships {
    pub matches: PlayerMatches
}

#[derive(Debug, Deserialize)]
pub struct PlayerMatches {
    pub data: Vec<PlayerMatch>
}

#[derive(Debug, Deserialize)]
pub struct PlayerMatch {
    pub id: String,
    #[serde(rename = "type")]
    pub mtype: String
}

#[derive(Debug, Deserialize)]
pub struct YoutubeData {
    pub title: String
}

#[derive(Debug, Serialize)]
pub struct ApiData {
    pub fields: HashMap<String, String>,
    pub commands: HashMap<String, String>,
    pub notices: HashMap<String, Vec<String>>,
    pub settings: HashMap<String, String>,
    pub blacklist: HashMap<String, HashMap<String,String>>,
    pub songreqs: Vec<(String,String,String)>
}

#[derive(Serialize)]
pub struct ApiRsp {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>
}

#[derive(FromForm)]
pub struct ApiLoginReq {
    pub channel: String,
    pub password: String
}

#[derive(FromForm)]
pub struct ApiSignupReq {
    pub token: String,
    pub password: String,
    pub invite: String
}

#[derive(FromForm)]
pub struct ApiPasswordReq {
    pub password: String
}

#[derive(FromForm)]
pub struct ApiTitleReq {
    pub title: String
}

#[derive(FromForm)]
pub struct ApiGameReq {
    pub game: String
}

#[derive(FromForm)]
pub struct ApiSaveCommandReq {
    pub command: String,
    pub message: String
}

#[derive(FromForm)]
pub struct ApiTrashCommandReq {
    pub command: String
}

#[derive(FromForm)]
pub struct ApiNoticeReq {
    pub interval: String,
    pub command: String
}

#[derive(FromForm)]
pub struct ApiSaveSettingReq {
    pub name: String,
    pub value: String
}

#[derive(FromForm)]
pub struct ApiTrashSettingReq {
    pub name: String
}

#[derive(FromForm)]
pub struct ApiNewBlacklistReq {
    pub regex: String,
    pub length: String
}

#[derive(FromForm)]
pub struct ApiSaveBlacklistReq {
    pub regex: String,
    pub length: String,
    pub key: String
}

#[derive(FromForm)]
pub struct ApiTrashBlacklistReq {
    pub key: String
}

#[derive(FromForm)]
pub struct ApiTrashSongReq {
    pub index: usize
}
