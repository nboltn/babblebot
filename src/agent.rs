#[cfg(windows)] extern crate winapi;
use std::io::Error;
use config;
use redis::{self,Commands};
use http::header::{self,HeaderValue};
use reqwest::Client;
use serde::{Serialize, Deserialize};

#[derive(Deserialize)]
pub struct AgentRsp {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>
}

fn main() {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Config")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let r1 = settings.get_str("channel");
    let r2 = settings.get_str("password");
    let r3 = settings.get_str("base_url");
    if let (Ok(channel), Ok(password), Ok(base_url)) = (r1, r2, r3) {
        let mut headers = header::HeaderMap::new();
        headers.insert("Content-Type", HeaderValue::from_str("application/x-www-form-urlencoded").unwrap());
        let client = Client::builder().default_headers(headers).build().unwrap();
        let rsp = client.post(&format!("{}/api/agent", base_url)).body(format!("channel={}&password={}", &channel, &password).as_bytes().to_owned()).send();
        match rsp {
            Err(e) => { println!("response error: {}", &e.to_string()); }
            Ok(mut rsp) => {
                let text = rsp.text().unwrap();
                let json: Result<AgentRsp,_> = serde_json::from_str(&text);
                match json {
                    Err(e) => {
                        println!("response error: {}", &e.to_string());
                    }
                    Ok(json) => {
                        if json.success {
                            if let (Some(action), Some(args)) = (json.action, json.args) {
                                match action.as_ref() {
                                    "UPDATE" => {
                                    }
                                    "INPUT" => {
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
