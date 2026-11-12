// Stay alive until the test releases the claimed issue.
use serde_json::json;
use std::{fs, path::Path, thread, time::Duration};
mod protocol;
use protocol::*;

fn main() {
    let session = id();
    for msg in messages() {
        let (Some(method), Some(request)) = (msg["method"].as_str(), msg.get("id")) else {
            continue;
        };
        match method {
            "thread/start" => result(request, json!({"thread":{"id":session}})),
            "turn/start" => {
                let turn = id();
                let text = msg["params"]["input"][0]["text"].as_str().unwrap();
                let rest = text.split_once("issue view ").unwrap().1;
                let number = &rest[..rest
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(rest.len())];
                issue(&session, &["claim", number]);
                result(request, json!({"turn":{"id":turn}}));
                fs::write("agent-started", std::process::id().to_string()).unwrap();
                while !Path::new("release-agent").exists() {
                    thread::sleep(Duration::from_millis(25));
                }
                issue(&session, &["close", number]);
                finish(&session, &turn, "Drain fixture finished normally.");
            }
            _ => result(request, json!({})),
        }
    }
}
