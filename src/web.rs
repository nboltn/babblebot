extern crate jsonwebtoken as jwt;

use crate::types::*;
use crate::util::*;
use std::collections::HashMap;
use std::time::{SystemTime};
use bcrypt::{DEFAULT_COST, hash, verify};
use config;
use reqwest;
use reqwest::header;
use reqwest::header::HeaderValue;
use rocket::{self, Outcome, get, post};
use rocket::http::{Status,Cookie,Cookies};
use rocket::request::{self, Request, FromRequest, Form};
use rocket_contrib::json::Json;
use rocket_contrib::templates::Template;
use jwt::{encode, decode, Header, Validation};

impl<'a, 'r> FromRequest<'a, 'r> for Auth {
    type Error = AuthError;

    fn from_request(request: &'a Request<'r>) -> request::Outcome<Self, Self::Error> {
        let mut settings = config::Config::default();
        settings.merge(config::File::with_name("Settings")).unwrap();
        settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();

        let mut forward = true;
        if let Some(_) = request.headers().get_one("NoForward") { forward = false }

        let auth_cookie = request.cookies().get_private("auth");
        match auth_cookie {
            None => {
                if forward {
                    return Outcome::Forward(());
                } else {
                    return Outcome::Failure((Status::BadRequest, AuthError::Missing));
                }
            }
            Some(cookie) => {
                let secret = settings.get_str("secret_key").unwrap();
                let token = decode(&cookie.value(), secret.as_bytes(), &Validation::default());
                match token {
                    Err(e) => {
                        eprintln!("[from_request] {}",e);
                        if forward {
                            return Outcome::Forward(());
                        } else {
                            return Outcome::Failure((Status::BadRequest, AuthError::Missing));
                        }
                    }
                    Ok(token) => {
                        let auth: Auth = token.claims;
                        Outcome::Success(auth)
                    }
                }
            }
        }
    }
}

#[catch(500)]
pub fn internal_error() -> &'static str { "500" }

#[catch(404)]
pub fn not_found() -> &'static str { "404" }

#[get("/")]
pub fn dashboard(_con: RedisConnection, _auth: Auth) -> Template {
    let context: HashMap<&str, String> = HashMap::new();
    return Template::render("dashboard", &context);
}

#[get("/", rank=2)]
pub fn index(_con: RedisConnection) -> Template {
    let context: HashMap<&str, String> = HashMap::new();
    return Template::render("index", &context);
}

#[get("/<channel>/commands")]
pub fn commands(_con: RedisConnection, channel: String) -> Template {
    let mut context: HashMap<&str, String> = HashMap::new();
    let channel = channel;
    context.insert("channel", channel);
    return Template::render("commands", &context);
}

#[get("/api/data")]
pub fn data(con: RedisConnection, auth: Auth) -> Json<ApiData> {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let id: String = redis::cmd("GET").arg(format!("channel:{}:id", auth.channel)).query(&*con).unwrap();
    let token: String = redis::cmd("GET").arg(format!("channel:{}:token", auth.channel)).query(&*con).unwrap();

    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("OAuth {}", token)).unwrap());
    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());

    let req = reqwest::Client::builder().http1_title_case_headers().default_headers(headers).build().unwrap();
    let rsp = req.get(&format!("https://api.twitch.tv/kraken/channels/{}", id)).send();

    match rsp {
        Err(err) => {
            let fields: HashMap<String, String> = HashMap::new();
            let commands: HashMap<String, String> = HashMap::new();
            let notices: HashMap<String, Vec<String>> = HashMap::new();
            let settings: HashMap<String, String> = HashMap::new();
            let blacklist: HashMap<String, HashMap<String,String>> = HashMap::new();
            let songreqs: Vec<(String,String,String)> = Vec::new();
            let json = ApiData { fields: fields, commands: commands, notices: notices, settings: settings, blacklist: blacklist, songreqs: songreqs };
            return Json(json);
        }
        Ok(mut rsp) => {
            let json: Result<KrakenChannel,_> = rsp.json();
            match json {
                Err(err) => {
                    let fields: HashMap<String, String> = HashMap::new();
                    let commands: HashMap<String, String> = HashMap::new();
                    let notices: HashMap<String, Vec<String>> = HashMap::new();
                    let settings: HashMap<String, String> = HashMap::new();
                    let blacklist: HashMap<String, HashMap<String,String>> = HashMap::new();
                    let songreqs: Vec<(String,String,String)> = Vec::new();
                    let json = ApiData { fields: fields, commands: commands, notices: notices, settings: settings, blacklist: blacklist, songreqs: songreqs };
                    return Json(json);
                }
                Ok(json) => {
                    let mut fields: HashMap<String, String> = HashMap::new();
                    let mut commands: HashMap<String, String> = HashMap::new();
                    let mut notices: HashMap<String, Vec<String>> = HashMap::new();
                    let mut blacklist: HashMap<String, HashMap<String,String>> = HashMap::new();
                    let mut songreqs: Vec<(String,String,String)> = Vec::new();

                    fields.insert("status".to_owned(), json.status.to_owned());
                    fields.insert("game".to_owned(), json.game.to_owned());

                    let settings: HashMap<String,String> = redis::cmd("HGETALL").arg(format!("channel:{}:settings", &auth.channel)).query(&*con).unwrap();

                    let keys: Vec<String> = redis::cmd("KEYS").arg(format!("channel:{}:commands:*", &auth.channel)).query(&*con).unwrap();
                    for key in keys.iter() {
                        let cmd: Vec<&str> = key.split(":").collect();
                        let res: Result<String,_> = redis::cmd("HGET").arg(format!("channel:{}:commands:{}", &auth.channel, cmd[3])).arg("message").query(&*con);
                        if let Ok(message) = res {
                            commands.insert(cmd[3].to_owned(), message);
                        }
                    }

                    let keys: Vec<String> = redis::cmd("KEYS").arg(format!("channel:{}:notices:*:commands", &auth.channel)).query(&*con).unwrap();
                    for key in keys.iter() {
                        let int: Vec<&str> = key.split(":").collect();
                        let res: Result<Vec<String>,_> = redis::cmd("LRANGE").arg(format!("channel:{}:notices:{}:commands", &auth.channel, int[3])).arg(0).arg(-1).query(&*con);
                        if let Ok(commands) = res {
                            notices.insert(int[3].to_owned(), commands);
                        }
                    }

                    let keys: Vec<String> = redis::cmd("KEYS").arg(format!("channel:{}:moderation:blacklist:*", &auth.channel)).query(&*con).unwrap();
                    for key in keys {
                        let key: Vec<&str> = key.split(":").collect();
                        let data: HashMap<String,String> = redis::cmd("HGETALL").arg(format!("channel:{}:moderation:blacklist:{}", &auth.channel, key[4])).query(&*con).unwrap();
                        blacklist.insert(key[4].to_owned(), data);
                    }

                    let keys: Vec<String> = redis::cmd("LRANGE").arg(format!("channel:{}:songreqs", &auth.channel)).arg(0).arg(-1).query(&*con).unwrap();
                    for key in keys.iter() {
                        let data: HashMap<String,String> = redis::cmd("HGETALL").arg(format!("channel:{}:songreqs:{}", &auth.channel, key)).query(&*con).unwrap();
                        let src = data["src"].to_owned();
                        let title = data["title"].to_owned();
                        let nick = data["nick"].to_owned();
                        songreqs.push((src,title,nick));
                    }

                    let json = ApiData { fields: fields, commands: commands, notices: notices, settings: settings, blacklist: blacklist, songreqs: songreqs };
                    return Json(json);
                }
            }
        }
    }
}

#[get("/api/<channel>/public_data")]
pub fn public_data(con: RedisConnection, channel: String) -> Json<ApiData> {
    let id: Result<String,_> = redis::cmd("GET").arg(format!("channel:{}:id", channel)).query(&*con);
    if let Ok(id) = id {
        let token: String = redis::cmd("GET").arg(format!("channel:{}:token", channel)).query(&*con).unwrap();
        let mut commands: HashMap<String, String> = HashMap::new();
        let fields: HashMap<String, String> = HashMap::new();
        let settings: HashMap<String, String> = HashMap::new();
        let notices: HashMap<String, Vec<String>> = HashMap::new();
        let blacklist: HashMap<String, HashMap<String,String>> = HashMap::new();
        let songreqs: Vec<(String,String,String)> = Vec::new();

        let keys: Vec<String> = redis::cmd("KEYS").arg(format!("channel:{}:commands:*", channel)).query(&*con).unwrap();
        for key in keys.iter() {
            let cmd: Vec<&str> = key.split(":").collect();
            let res: Result<String,_> = redis::cmd("HGET").arg(format!("channel:{}:commands:{}", channel, cmd[3])).arg("message").query(&*con);
            if let Ok(message) = res {
                commands.insert(cmd[3].to_owned(), message);
            }
        }

        let json = ApiData { fields: fields, commands: commands, notices: notices, settings: settings, blacklist: blacklist, songreqs: songreqs };
        return Json(json);
    } else {
        let fields: HashMap<String, String> = HashMap::new();
        let commands: HashMap<String, String> = HashMap::new();
        let notices: HashMap<String, Vec<String>> = HashMap::new();
        let settings: HashMap<String, String> = HashMap::new();
        let blacklist: HashMap<String, HashMap<String,String>> = HashMap::new();
        let songreqs: Vec<(String,String,String)> = Vec::new();
        let json = ApiData { fields: fields, commands: commands, notices: notices, settings: settings, blacklist: blacklist, songreqs: songreqs };
        return Json(json);
    }
}

#[post("/api/login", data="<data>")]
pub fn login(con: RedisConnection, data: Form<ApiLoginReq>, mut cookies: Cookies) -> Json<ApiRsp> {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();

    let exists: bool = redis::cmd("SISMEMBER").arg("channels").arg(data.channel.to_lowercase()).query(&*con).unwrap();
    if exists {
        let hashed: String = redis::cmd("GET").arg(format!("channel:{}:password", data.channel.to_lowercase())).query(&*con).unwrap();
        let authed: bool = verify(&data.password, &hashed).unwrap();
        if authed {
            if let Ok(exp) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                let secret = settings.get_str("secret_key").unwrap();
                let auth = Auth { channel: data.channel.to_lowercase(), exp: exp.as_secs() + 2400000 };
                let token = encode(&Header::default(), &auth, secret.as_bytes()).unwrap();
                cookies.add_private(Cookie::new("auth", token));
            }
            let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
            return Json(json);
        } else {
            let json = ApiRsp { success: false, success_value: None, field: Some("password".to_owned()), error_message: Some("invalid password".to_owned()) };
            return Json(json);
        }
    } else {
        let json = ApiRsp { success: false, success_value: None, field: Some("channel".to_owned()), error_message: Some("channel not found".to_owned()) };
        return Json(json);
    }
}

#[get("/api/logout")]
pub fn logout(_con: RedisConnection, mut cookies: Cookies) -> Json<ApiRsp> {
    cookies.remove_private(Cookie::named("auth"));
    let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
    return Json(json);
}

#[post("/api/signup", data="<data>")]
pub fn signup(con: RedisConnection, mut cookies: Cookies, data: Form<ApiSignupReq>) -> Json<ApiRsp> {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();

    let client = reqwest::Client::new();
    let rsp = client.get("https://api.twitch.tv/helix/users").header(header::AUTHORIZATION, format!("Bearer {}", data.token)).send();

    match rsp {
        Err(_) => {
            let json = ApiRsp { success: false, success_value: None, field: Some("token".to_owned()), error_message: Some("invalid access code".to_owned()) };
            return Json(json);
        }
        Ok(mut rsp) => {
            let json: Result<HelixUsers,_> = rsp.json();
            match json {
                Err(_) => {
                    let json = ApiRsp { success: false, success_value: None, field: Some("token".to_owned()), error_message: Some("invalid access code".to_owned()) };
                    return Json(json);
                }
                Ok(json) => {
                    let exists: bool = redis::cmd("SISMEMBER").arg("channels").arg(&json.data[0].login).query(&*con).unwrap();
                    if exists {
                        let json = ApiRsp { success: false, success_value: None, field: Some("token".to_owned()), error_message: Some("invalid access code".to_owned()) };
                        return Json(json);
                    } else {
                        let removed: u16 = redis::cmd("LREM").arg("invites").arg(1).arg(&data.invite).query(&*con).unwrap();
                        if removed == 0 {
                            let json = ApiRsp { success: false, success_value: None, field: Some("invite".to_owned()), error_message: Some("invalid invite code".to_owned()) };
                            return Json(json);
                        } else {
                            let bot_name  = settings.get_str("bot_name").unwrap();
                            let bot_token = settings.get_str("bot_token").unwrap();

                            redis::cmd("SADD").arg("bots").arg(&bot_name).execute(&*con);
                            redis::cmd("SADD").arg("channels").arg(&json.data[0].login).execute(&*con);
                            redis::cmd("SET").arg(format!("bot:{}:token", &bot_name)).arg(bot_token).execute(&*con);
                            redis::cmd("SADD").arg(format!("bot:{}:channels", &bot_name)).arg(&json.data[0].login).execute(&*con);
                            redis::cmd("SET").arg(format!("channel:{}:bot", &json.data[0].login)).arg(&bot_name).execute(&*con);
                            redis::cmd("SET").arg(format!("channel:{}:token", &json.data[0].login)).arg(&data.token).execute(&*con);
                            redis::cmd("SET").arg(format!("channel:{}:password", &json.data[0].login)).arg(hash(&data.password, DEFAULT_COST).unwrap()).execute(&*con);
                            redis::cmd("SET").arg(format!("channel:{}:live", &json.data[0].login)).arg(false).execute(&*con);
                            redis::cmd("SET").arg(format!("channel:{}:id", &json.data[0].login)).arg(&json.data[0].id).execute(&*con);
                            redis::cmd("SET").arg(format!("channel:{}:display-name", &json.data[0].login)).arg(&json.data[0].display_name).execute(&*con);

                            if let Ok(exp) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                                let secret = settings.get_str("secret_key").unwrap();
                                let auth = Auth { channel: json.data[0].login.to_owned(), exp: exp.as_secs() + 2400000 };
                                let token = encode(&Header::default(), &auth, secret.as_bytes()).unwrap();
                                cookies.add_private(Cookie::new("auth", token));
                            }

                            let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
                            return Json(json);
                        }
                    }
                }
            }
        }
    }
}

#[post("/api/password", data="<data>")]
pub fn password(con: RedisConnection, data: Form<ApiPasswordReq>, auth: Auth) -> Json<ApiRsp> {
    if data.password.is_empty() {
        let json = ApiRsp { success: false, success_value: None, field: Some("password".to_owned()), error_message: Some("empty password".to_owned()) };
        return Json(json);
    } else {
        redis::cmd("SET").arg(format!("channel:{}:password", &auth.channel)).arg(hash(&data.password, DEFAULT_COST).unwrap()).execute(&*con);
        let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
        return Json(json);
    }
}

#[post("/api/title", data="<data>")]
pub fn title(con: RedisConnection, data: Form<ApiTitleReq>, auth: Auth) -> Json<ApiRsp> {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let id: String = redis::cmd("GET").arg(format!("channel:{}:id", auth.channel)).query(&*con).unwrap();
    let token: String = redis::cmd("GET").arg(format!("channel:{}:token", auth.channel)).query(&*con).unwrap();

    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("OAuth {}", token)).unwrap());
    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());
    headers.insert("Content-Type", HeaderValue::from_str("application/x-www-form-urlencoded").unwrap());

    let req = reqwest::Client::builder().http1_title_case_headers().default_headers(headers).build().unwrap();
    let rsp = req.put(&format!("https://api.twitch.tv/kraken/channels/{}", id)).body(format!("channel[status]={}", data.title)).send();

    match rsp {
        Err(_) => {
            let json = ApiRsp { success: false, success_value: None, field: Some("title-field".to_owned()), error_message: Some("twitch api error".to_owned()) };
            return Json(json);
        }
        Ok(mut rsp) => {
            let json: Result<KrakenChannel,_> = rsp.json();
            match json {
                Err(_) => {
                    let json = ApiRsp { success: false, success_value: None, field: Some("title-field".to_owned()), error_message: Some("twitch api error".to_owned()) };
                    return Json(json);
                }
                Ok(_) => {
                    let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
                    return Json(json);
                }
            }
        }
    }
}

#[post("/api/game", data="<data>")]
pub fn game(con: RedisConnection, data: Form<ApiGameReq>, auth: Auth) -> Json<ApiRsp> {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let id: String = redis::cmd("GET").arg(format!("channel:{}:id", auth.channel)).query(&*con).unwrap();
    let token: String = redis::cmd("GET").arg(format!("channel:{}:token", auth.channel)).query(&*con).unwrap();

    let mut headers = header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_str("application/vnd.twitchtv.v5+json").unwrap());
    headers.insert("Authorization", HeaderValue::from_str(&format!("OAuth {}", token)).unwrap());
    headers.insert("Client-ID", HeaderValue::from_str(&settings.get_str("client_id").unwrap()).unwrap());
    headers.insert("Content-Type", HeaderValue::from_str("application/x-www-form-urlencoded").unwrap());

    let req = reqwest::Client::builder().http1_title_case_headers().default_headers(headers.clone()).build().unwrap();
    let rsp = req.get(&format!("https://api.twitch.tv/helix/games?name={}", data.game)).send();

    match rsp {
        Err(_) => {
            let json = ApiRsp { success: false, success_value: None, field: Some("game".to_owned()), error_message: Some("game not found".to_owned()) };
            return Json(json);
        }
        Ok(mut rsp) => {
            let json: Result<HelixGames,_> = rsp.json();
            match json {
                Err(_) => {
                    let json = ApiRsp { success: false, success_value: None, field: Some("game".to_owned()), error_message: Some("game not found".to_owned()) };
                    return Json(json);
                }
                Ok(json) => {
                    if json.data.len() == 0 {
                        let json = ApiRsp { success: false, success_value: None, field: Some("game".to_owned()), error_message: Some("game not found".to_owned()) };
                        return Json(json);
                    } else {
                        let name = &json.data[0].name;
                        let req = reqwest::Client::builder().http1_title_case_headers().default_headers(headers).build().unwrap();
                        let rsp = req.put(&format!("https://api.twitch.tv/kraken/channels/{}", id)).body(format!("channel[game]={}", name)).send();

                        match rsp {
                            Err(_) => {
                                let json = ApiRsp { success: false, success_value: None, field: Some("game".to_owned()), error_message: Some("twitch api error".to_owned()) };
                                return Json(json);
                            }
                            Ok(mut rsp) => {
                                let json: Result<KrakenChannel,_> = rsp.json();
                                match json {
                                    Err(_) => {
                                        let json = ApiRsp { success: false, success_value: None, field: Some("game".to_owned()), error_message: Some("twitch api error".to_owned()) };
                                        return Json(json);
                                    }
                                    Ok(_) => {
                                        let json = ApiRsp { success: true, success_value: Some(name.to_owned()), field: Some("game".to_owned()), error_message: None };
                                        return Json(json);
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

#[post("/api/new_command", data="<data>")]
pub fn new_command(con: RedisConnection, data: Form<ApiSaveCommandReq>, auth: Auth) -> Json<ApiRsp> {
    if !data.command.is_empty() && !data.message.is_empty() && !data.command.is_empty() {
        redis::cmd("HSET").arg(format!("channel:{}:commands:{}", &auth.channel, &data.command.to_lowercase())).arg("message").arg(&data.message).execute(&*con);
        redis::cmd("HSET").arg(format!("channel:{}:commands:{}", &auth.channel, &data.command.to_lowercase())).arg("cmd_protected").arg(false).execute(&*con);
        redis::cmd("HSET").arg(format!("channel:{}:commands:{}", &auth.channel, &data.command.to_lowercase())).arg("arg_protected").arg(false).execute(&*con);
        let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
        return Json(json);
    } else {
        let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
        return Json(json);
    }
}

#[post("/api/save_command", data="<data>")]
pub fn save_command(con: RedisConnection, data: Form<ApiSaveCommandReq>, auth: Auth) -> Json<ApiRsp> {
    if !data.command.is_empty() && !data.message.is_empty() {
        redis::cmd("HSET").arg(format!("channel:{}:commands:{}", &auth.channel, &data.command)).arg("message").arg(&data.message).execute(&*con);
        let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
        return Json(json);
    } else {
        let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
        return Json(json);
    }
}

#[post("/api/trash_command", data="<data>")]
pub fn trash_command(con: RedisConnection, data: Form<ApiTrashCommandReq>, auth: Auth) -> Json<ApiRsp> {
    if !data.command.is_empty() {
        redis::cmd("DEL").arg(format!("channel:{}:commands:{}", &auth.channel, &data.command)).execute(&*con);
        let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
        return Json(json);
    } else {
        let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
        return Json(json);
    }
}

#[post("/api/new_notice", data="<data>")]
pub fn new_notice(con: RedisConnection, data: Form<ApiNoticeReq>, auth: Auth) -> Json<ApiRsp> {
    if !data.interval.is_empty() && !data.command.is_empty() {
        let n: Result<u16,_> = data.interval.parse();
        match n {
            Ok(num) => {
                if num % 60 == 0 {
                    let exists: bool = redis::cmd("EXISTS").arg(format!("channel:{}:commands:{}", &auth.channel, &data.command)).query(&*con).unwrap();
                    if exists {
                        redis::cmd("RPUSH").arg(format!("channel:{}:notices:{}:commands", &auth.channel, &data.interval)).arg(&data.command).execute(&*con);
                        redis::cmd("SET").arg(format!("channel:{}:notices:{}:countdown", &auth.channel, &data.interval)).arg(&data.interval).execute(&*con);
                        let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
                        return Json(json);
                    } else {
                        let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
                        return Json(json);
                    }
                } else {
                    let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
                    return Json(json);
                }
            }
            Err(_) => {
                let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
                return Json(json);
            }
        }
    } else {
        let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
        return Json(json);
    }
}

#[post("/api/trash_notice", data="<data>")]
pub fn trash_notice(con: RedisConnection, data: Form<ApiNoticeReq>, auth: Auth) -> Json<ApiRsp> {
    if !data.interval.is_empty() && !data.command.is_empty() {
        redis::cmd("LREM").arg(format!("channel:{}:notices:{}:commands", &auth.channel, &data.interval)).arg(0).arg(&data.command).execute(&*con);
        let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
        return Json(json);
    } else {
        let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
        return Json(json);
    }
}

#[post("/api/save_setting", data="<data>")]
pub fn save_setting(con: RedisConnection, data: Form<ApiSaveSettingReq>, auth: Auth) -> Json<ApiRsp> {
    if !data.name.is_empty() && !data.value.is_empty() {
        redis::cmd("HSET").arg(format!("channel:{}:settings", &auth.channel)).arg(&data.name.to_lowercase()).arg(&data.value).execute(&*con);
        let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
        return Json(json);
    } else {
        let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
        return Json(json);
    }
}

#[post("/api/trash_setting", data="<data>")]
pub fn trash_setting(con: RedisConnection, data: Form<ApiTrashSettingReq>, auth: Auth) -> Json<ApiRsp> {
    if !data.name.is_empty() {
        redis::cmd("HDEL").arg(format!("channel:{}:settings", &auth.channel)).arg(&data.name).execute(&*con);
        let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
        return Json(json);
    } else {
        let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
        return Json(json);
    }
}

#[post("/api/new_blacklist", data="<data>")]
pub fn new_blacklist(con: RedisConnection, data: Form<ApiNewBlacklistReq>, auth: Auth) -> Json<ApiRsp> {
    if !data.regex.is_empty() && !data.length.is_empty() {
        let key = hash(&data.regex, 6).unwrap();
        redis::cmd("HSET").arg(format!("channel:{}:moderation:blacklist:{}", &auth.channel, &key)).arg("regex").arg(&data.regex).execute(&*con);
        redis::cmd("HSET").arg(format!("channel:{}:moderation:blacklist:{}", &auth.channel, &key)).arg("length").arg(&data.length).execute(&*con);
        let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
        return Json(json);
    } else {
        let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
        return Json(json);
    }
}

#[post("/api/save_blacklist", data="<data>")]
pub fn save_blacklist(con: RedisConnection, data: Form<ApiSaveBlacklistReq>, auth: Auth) -> Json<ApiRsp> {
    if !data.key.is_empty() && !data.regex.is_empty() && !data.length.is_empty() {
        redis::cmd("HSET").arg(format!("channel:{}:moderation:blacklist:{}", &auth.channel, &data.key)).arg("regex").arg(&data.regex).execute(&*con);
        redis::cmd("HSET").arg(format!("channel:{}:moderation:blacklist:{}", &auth.channel, &data.key)).arg("length").arg(&data.length).execute(&*con);
        let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
        return Json(json);
    } else {
        let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
        return Json(json);
    }
}

#[post("/api/trash_blacklist", data="<data>")]
pub fn trash_blacklist(con: RedisConnection, data: Form<ApiTrashBlacklistReq>, auth: Auth) -> Json<ApiRsp> {
    if !data.key.is_empty() {
        redis::cmd("DEL").arg(format!("channel:{}:moderation:blacklist:{}", &auth.channel, &data.key)).execute(&*con);
        let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
        return Json(json);
    } else {
        let json = ApiRsp { success: false, success_value: None, field: None, error_message: None };
        return Json(json);
    }
}

#[post("/api/trash_song", data="<data>")]
pub fn trash_song(con: RedisConnection, data: Form<ApiTrashSongReq>, auth: Auth) -> Json<ApiRsp> {
    let entries: Vec<String> = redis::cmd("LRANGE").arg(format!("channel:{}:songreqs", &auth.channel)).arg(0).arg(-1).query(&*con).unwrap();
    redis::cmd("LREM").arg(format!("channel:{}:songreqs", &auth.channel)).arg(0).arg(&entries[data.index]).execute(&*con);
    redis::cmd("DEL").arg(format!("channel:{}:songreqs:{}", &auth.channel, &entries[data.index])).execute(&*con);
    let json = ApiRsp { success: true, success_value: None, field: None, error_message: None };
    return Json(json);
}
