use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use rocket::request::FromForm;
use rocket_contrib::database;
use rocket_contrib::databases::redis;

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

pub enum Setting {
    String(String),
    Bool(bool)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Auth {
    pub channel: String,
    pub exp: u64
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
pub struct KrakenChannel {
    pub status: String,
    pub game: String
}

#[derive(Debug, Deserialize)]
pub struct KrakenStreams {
    #[serde(rename = "_total")]
    pub total: i16
}

#[derive(Debug, Serialize)]
pub struct ApiData {
    pub fields: HashMap<String,String>,
    pub commands: HashMap<String,String>
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
