#[cfg(windows)] use winapi::um::winuser::{INPUT_u, INPUT, INPUT_MOUSE, INPUT_KEYBOARD, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEINPUT, KEYBDINPUT, SendInput};
use std::{thread,time,io};
use config;
use semver::Version;
use redis::{self,Commands};
use http::header::{self,HeaderValue};
use reqwest::Client;
use serde::Deserialize;

const VERSION: &str = "1.0.0";

#[derive(Deserialize)]
pub struct AgentRsp {
    pub version: String,
    pub success: bool,
    pub actions: Vec<AgentAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>
}

#[derive(Deserialize)]
pub struct AgentAction {
    pub name: String,
    pub keys: Vec<u8>,
    pub hold: u8,
    pub delay: u8
}

#[cfg(not(windows))]
fn main() {
    println!("os error: windows not detected");
    println!("");
    println!("press enter to exit");
    let mut input = String::new();
    io::stdin().read_line(&mut input);
}

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
                                    break;
                                } else {
                                    if json.success {
                                        for action in json.actions {
                                            println!("received action: {}", action.name);
                                            match action.name.as_ref() {
                                                "MOUSE" => {
                                                    press_mouse(MOUSEEVENTF_LEFTDOWN);
                                                    thread::sleep(time::Duration::from_millis(100 * action.hold));
                                                    press_mouse(MOUSEEVENTF_LEFTUP);
                                                }
                                                "KEYBD" => {
                                                    for key in action.keys.iter() {
                                                        press_key(key, KEYEVENTF_SCANCODE);
                                                    }

                                                    thread::sleep(time::Duration::from_millis(100));

                                                    for key in action.keys.iter() {
                                                        press_key(key, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP);
                                                    }
                                                }
                                                _ => {}
                                            }

                                            thread::sleep(time::Duration::from_millis(100 * action.delay));
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
    } else {
        println!("config error: cannot read config file");
    }

    println!("");
    println!("press enter to exit");
    let mut input = String::new();
    io::stdin().read_line(&mut input);
}

#[cfg(windows)]
fn press_mouse(flag: u32) {
    let mut input_u: INPUT_u = unsafe { std::mem::zeroed() };
    unsafe {
        *input_u.mi_mut() = MOUSEINPUT {
            dwFlags: flag,
            dx: 0,
            dy: 0,
            time: 0,
            mouseData: 0,
            dwExtraInfo: 0
        }
    }
    let mut input = INPUT {
        type_: INPUT_MOUSE,
        u: input_u
    };
    let ipsize = std::mem::size_of::<INPUT>() as i32;
    unsafe {
        SendInput(1, &mut input, ipsize);
    };
}

#[cfg(windows)]
fn press_key(key: u8, flag: u32) {
    let mut input_u: INPUT_u = unsafe { std::mem::zeroed() };
    unsafe {
        *input_u.ki_mut() = KEYBDINPUT {
            wScan: key,
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

#[cfg(not(windows))]
fn press_mouse() {}

#[cfg(not(windows))]
fn press_key() {}
