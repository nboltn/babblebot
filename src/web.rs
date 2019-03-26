use crate::types::*;
use std::collections::HashMap;
use bcrypt::{DEFAULT_COST, hash, verify};
use config;
use reqwest;
use reqwest::header;
use rocket;
use rocket::{routes,get,post};
use rocket::request::Form;
use rocket_contrib::json::Json;
use rocket_contrib::templates::Template;
use rocket_contrib::databases::redis;
use r2d2_redis::redis::Commands;

fn display_name(con: &redis::Connection, name: String) -> String {
    return con.get(format!("channel:{}:display_name", name)).unwrap_or("".to_owned());
}

#[get("/")]
pub fn index(con: RedisConnection) -> Template {
    let mut context: HashMap<&str, String> = HashMap::new();
    return Template::render("index", &context);
}

#[post("/api/login", data="<data>")]
pub fn login(con: RedisConnection, data: Form<ApiLoginReq>) -> Json<ApiLoginRsp> {
    let json: ApiLoginRsp = ApiLoginRsp { success: false, error: Some("invalid invite code".to_owned()) };
    return Json(json);
}

#[post("/api/signup", data="<data>")]
pub fn signup(con: RedisConnection, data: Form<ApiSignupReq>) -> Json<ApiSignupRsp> {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();

    let client = reqwest::Client::new();
    let rsp = client.get("https://api.twitch.tv/helix/users").header(header::AUTHORIZATION, format!("Bearer {}", data.token)).send();

    match rsp {
        Err(err) => {
            let json: ApiSignupRsp = ApiSignupRsp { success: false, error: Some("token".to_owned()) };
            return Json(json);
        },
        Ok(mut rsp) => {
            let json: Result<HelixUsers,_> = rsp.json();
            match json {
                Err(err) => {
                    let json: ApiSignupRsp = ApiSignupRsp { success: false, error: Some("token".to_owned()) };
                    return Json(json);
                }
                Ok(json) => {
                    let exists: bool = con.sismember("channels", &json.data[0].login).unwrap();
                    if exists {
                        let json: ApiSignupRsp = ApiSignupRsp { success: false, error: Some("token".to_owned()) };
                        return Json(json);
                    } else {
                        let removed: i16 = con.lrem("invites", 1, &data.invite).unwrap();
                        if removed == 0 {
                            let json: ApiSignupRsp = ApiSignupRsp { success: false, error: Some("invite".to_owned()) };
                            return Json(json);
                        } else {
                            let bot_name  = settings.get_str("bot_name").unwrap();
                            let bot_token = settings.get_str("bot_token").unwrap();

                            let _: () = con.sadd("bots", &bot_name).unwrap();
                            let _: () = con.sadd("channels", &json.data[0].login).unwrap();
                            let _: () = con.set(format!("bot:{}:token", &bot_name), bot_token).unwrap();
                            let _: () = con.sadd(format!("bot:{}:channels", &bot_name), &json.data[0].login).unwrap();
                            let _: () = con.set(format!("channel:{}:bot", &json.data[0].login), "babblerbot").unwrap();
                            let _: () = con.set(format!("channel:{}:token", &json.data[0].login), &data.token).unwrap();
                            let _: () = con.set(format!("channel:{}:password", &json.data[0].login), hash(&data.password, DEFAULT_COST).unwrap()).unwrap();
                            let _: () = con.set(format!("channel:{}:live", &json.data[0].login), false).unwrap();
                            let _: () = con.set(format!("channel:{}:id", &json.data[0].login), &json.data[0].id).unwrap();
                            let _: () = con.set(format!("channel:{}:display_name", &json.data[0].login), &json.data[0].display_name).unwrap();
                            let _: () = con.publish("new_channels", &json.data[0].login).unwrap();
                            let json: ApiSignupRsp = ApiSignupRsp { success: true, error: None };
                            return Json(json);
                        }
                    }
                }
            }
        }
    }
}
