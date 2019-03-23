use serde::{Serialize, Deserialize};
use rocket_contrib::database;
use rocket_contrib::databases::redis;

#[database("redis")]
pub struct RedisConnection(redis::Connection);

pub enum Setting {
    String(String),
    Bool(bool)
}

#[derive(Debug, Deserialize)]
pub struct HelixUsers {
    pub data: Vec<HelixUsersData>
}

#[derive(Debug, Deserialize)]
pub struct HelixUsersData {
    pub id: String,
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
