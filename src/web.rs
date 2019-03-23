use crate::types::*;
use std::collections::HashMap;
use rocket;
use rocket::{routes,get};
use rocket_contrib::templates::Template;
use rocket_contrib::databases::redis;
use r2d2_redis::redis::Commands;

fn display_name(conn: &redis::Connection, name: String) -> String {
    return conn.get(format!("channel:{}:display_name", name)).unwrap_or("".to_owned());
}

#[get("/")]
pub fn index(conn: RedisConnection) -> Template {
    let mut context: HashMap<&str, String> = HashMap::new();
    Template::render("index", &context)
}

#[get("/<name>")]
pub fn name(conn: RedisConnection, name: String) -> Template {
    let mut context: HashMap<&str, String> = HashMap::new();
    context.insert("name", display_name(&conn, name));
    Template::render("name", &context)
}
