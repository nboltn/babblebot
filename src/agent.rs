#[cfg(windows)] use winapi::um::winuser::{INPUT_u, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYBDINPUT, SendInput};
use std::{thread,time};
use config;
use semver::Version;
use redis::{self,Commands};
use http::header::{self,HeaderValue};
use reqwest::Client;
use serde::Deserialize;

const VERSION: &str = "0.1.0";
const KEYUP: u32 = 0x0002;

#[derive(Deserialize)]
pub struct AgentRsp {
    pub version: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>
}

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Config")).unwrap();
    settings.merge(config::Environment::with_prefix("BABBLEBOT")).unwrap();
    let r1 = settings.get_str("channel");
    let r2 = settings.get_str("secret_key");
    let r3 = settings.get_str("base_url");
    if let (Ok(channel), Ok(key), Ok(base_url)) = (r1, r2, r3) {
        loop {
            let mut headers = header::HeaderMap::new();
            headers.insert("Content-Type", HeaderValue::from_str("application/x-www-form-urlencoded").unwrap());
            let client = Client::builder().default_headers(headers).build().unwrap();
            let rsp = client.post(&format!("{}/api/agent", base_url)).body(format!("channel={}&key={}", &channel, &key).as_bytes().to_owned()).send();
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
                            let r1 = Version::parse(VERSION);
                            let r2 = Version::parse(&json.version);
                            if let (Ok(v1), Ok(v2)) = (r1, r2) {
                                if v2 > v1 {
                                    println!("version error: your client is out of date");
                                    thread::sleep(time::Duration::from_secs(600));
                                } else {
                                    if json.success {
                                        if let (Some(action), Some(args)) = (json.action, json.args) {
                                            println!("received action: {}", action);
                                            match action.as_ref() {
                                                "INPUT" => {
                                                    for arg in args.clone() {
                                                        let res: Result<u16,_> = arg.parse();
                                                        if let Ok(num) = res {
                                                            let mut input_u: INPUT_u = unsafe { std::mem::zeroed() };
                                                            unsafe {
                                                                *input_u.ki_mut() = KEYBDINPUT {
                                                                    wScan: num,
                                                                    dwFlags: KEYEVENTF_SCANCODE,
                                                                    dwExtraInfo: 0,
                                                                    wVk: 0,
                                                                    time: 0
                                                                }
                                                            }

                                                            let mut input = INPUT {
                                                                type_: INPUT_KEYBOARD,
                                                                u: input_u
                                                            };
                                                            let ipsize = std::mem::size_of::<INPUT>() as i32;

                                                            unsafe {
                                                                SendInput(1, &mut input, ipsize);
                                                            };
                                                        }
                                                    }
                                                    thread::sleep(time::Duration::from_millis(100));
                                                    for arg in args.clone() {
                                                        let res: Result<u16,_> = arg.parse();
                                                        if let Ok(num) = res {
                                                            let mut input_u: INPUT_u = unsafe { std::mem::zeroed() };
                                                            unsafe {
                                                                *input_u.ki_mut() = KEYBDINPUT {
                                                                    wScan: num,
                                                                    dwFlags: KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP,
                                                                    dwExtraInfo: 0,
                                                                    wVk: 0,
                                                                    time: 0
                                                                }
                                                            }

                                                            let mut input = INPUT {
                                                                type_: INPUT_KEYBOARD,
                                                                u: input_u
                                                            };
                                                            let ipsize = std::mem::size_of::<INPUT>() as i32;

                                                            unsafe {
                                                                SendInput(1, &mut input, ipsize);
                                                            };
                                                        }
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                    } else {
                                        if let Some(msg) = json.error_message {
                                            println!("response error: {}", msg);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

        thread::sleep(time::Duration::from_secs(10));
        }
    }
}
