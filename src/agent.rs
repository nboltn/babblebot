#[cfg(windows)] extern crate winapi;
use std::io::Error;
use config;
use redis::{self,Commands};
use http::header::{self,HeaderValue};
use reqwest::Client;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct ApiRsp {
    pub success: bool
}

fn main() {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Config")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let r1 = settings.get_str("channel");
    let r2 = settings.get_str("password");
    let r3 = settings.get_str("domain");
    if let (Ok(channel), Ok(password), Ok(domain)) = (r1, r2, r3) {
        let mut headers = header::HeaderMap::new();
        headers.insert("Content-Type", HeaderValue::from_str("application/x-www-form-urlencoded").unwrap());
        let client = Client::builder().default_headers(headers).build().unwrap();
        let rsp = client.post(&format!("https://{}/api/login", domain)).body(format!("channel={}&password={}", &channel, &password).as_bytes().to_owned()).send();
        match rsp {
            Err(e) => { println!("response error: {}", &e.to_string()); }
            Ok(mut rsp) => {
                let text = rsp.text().unwrap();
                let json: Result<ApiRsp,_> = serde_json::from_str(&text);
                match json {
                    Err(e) => {
                        println!("response error: {}", &e.to_string());
                    }
                    Ok(json) => {
                        if json.success {
                            let mut client: redis::Client;
                            let redis_host = settings.get_str("redis_host").unwrap_or(domain.clone());
                            let res = settings.get_str("redis_pass");
                            if let Ok(redis_pass) = res {
                                client = redis::Client::open(&format!("redis://:{}@{}", &redis_pass, &redis_host)[..]).unwrap();
                            } else {
                                client = redis::Client::open(&format!("redis://{}", &redis_host)[..]).unwrap();
                            }
                            match client.get_connection() {
                                Err(e) => { println!("4"); println!("redis error: {}", &e.to_string()) },
                                Ok(mut con) => {
                                    let mut ps = con.as_pubsub();
                                    ps.subscribe(format!("channel:{}:signals:agent", &channel)).unwrap();
                                    println!("connected");
                                    loop {
                                        let res = ps.get_message();
                                        match res {
                                            Err(e) => println!("redis error: {}", &e.to_string()),
                                            Ok(msg) => {
                                                let res: Result<String,_> = msg.get_payload();
                                                match res {
                                                    Err(e) => println!("redis error: {}", &e.to_string()),
                                                    Ok(payload) => {
                                                        let payload: Vec<&str> = payload.split_whitespace().collect();
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            println!("invalid password");
                        }
                    }
                }
            }
        }
    }
}
