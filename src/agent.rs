#[cfg(windows)] use winapi::um::winuser::{INPUT_u, INPUT, INPUT_KEYBOARD, KEYBDINPUT, SendInput};
use std::{thread,time};
use config;
use redis::{self,Commands};
use http::header::{self,HeaderValue};
use reqwest::Client;
use serde::Deserialize;
use self_update::{self, cargo_crate_version};

const VERSION: u8 = 0;
const KEYUP: u16 = 0x0002;

#[derive(Deserialize)]
pub struct AgentRsp {
    pub version: u8,
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
    let r2 = settings.get_str("password");
    let r3 = settings.get_str("base_url");
    if let (Ok(channel), Ok(password), Ok(base_url)) = (r1, r2, r3) {
        loop {
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
                            if VERSION >= json.version {
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
                                                                wVk: num,
                                                                dwFlags: 0,
                                                                dwExtraInfo: 0,
                                                                wScan: 0,
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

                                                for arg in args.clone() {
                                                    let res: Result<u16,_> = arg.parse();
                                                    if let Ok(num) = res {
                                                        let mut input_u: INPUT_u = unsafe { std::mem::zeroed() };
                                                        unsafe {
                                                            *input_u.ki_mut() = KEYBDINPUT {
                                                                wVk: num,
                                                                dwFlags: KEYUP,
                                                                dwExtraInfo: 0,
                                                                wScan: 0,
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
                            } else {
                                let r1 = settings.get_str("github_owner");
                                let r2 = settings.get_str("github_name");
                                if let (Ok(owner), Ok(name)) = (r1, r2) {
                                    let res = self_update::backends::github::Update::configure()
                                        .repo_owner(&owner)
                                        .repo_name(&name)
                                        .bin_name("babblebot")
                                        .show_download_progress(true)
                                        .current_version(cargo_crate_version!())
                                        .build();
                                    match res {
                                        Err(e) => println!("update error: {}", &e.to_string()),
                                        Ok(update) => {
                                            match update.update() {
                                                Err(e) => println!("update error: {}", &e.to_string()),
                                                Ok(status) => {
                                                    println!("updated to version: {}", status.version());
                                                }
                                            }
                                        }
                                    }

                                } else {
                                    println!("version error: your client is out of date");
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
