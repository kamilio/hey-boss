use serde_json::json;
use std::io::Write;
use std::{fs, path::Path, thread, time::Duration};
mod protocol;
use protocol::*;

fn main() {
    let session = id();
    let mut started = false;
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("github-worker.jsonl")
        .unwrap();
    for msg in messages() {
        let (Some(method), Some(request)) = (msg["method"].as_str(), msg.get("id")) else {
            continue;
        };
        match method {
            "thread/start" => result(request, json!({"thread":{"id":session}})),
            "thread/resume" | "turn/steer" => panic!("Watcher events must start a fresh session"),
            "turn/start" => {
                assert!(!started, "Each watcher claim starts its own thread");
                started = true;
                let phase = match fs::read_to_string("github-launches.txt") {
                    Ok(n) => n.parse::<usize>().unwrap() + 1,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => 1,
                    Err(e) => panic!("{e}"),
                };
                fs::write("github-launches.txt", phase.to_string()).unwrap();
                let text = msg["params"]["input"][0]["text"].as_str().unwrap();
                assert!(text.starts_with("Claim and implement `hey-boss issue view 1`."));
                assert!(!text.contains("hey-boss issue pr list 1"));
                assert!(text.ends_with("The GitHub status changed."));
                let view = issue(&session, &["view", "1"]);
                let status = &view["issue"]["github_status"];
                assert!(status["event"].is_string());
                assert_eq!(status["monitoring"], true);
                let claim = issue(&session, &["claim", "1"]);
                assert_eq!(claim["issue"]["github_status"]["event"], status["event"]);
                writeln!(
                    log,
                    "{}",
                    json!({"type":"claim","phase":phase,"session":session,"status":status})
                )
                .unwrap();
                let turn = id();
                result(request, json!({"turn":{"id":turn}}));
                if phase == 1 {
                    // Continue reading requests while waiting, so a stray steer fails the test.
                    let (session, turn) = (session.clone(), turn.clone());
                    thread::spawn(move || {
                        while !Path::new("release-first").exists() {
                            thread::sleep(Duration::from_millis(50));
                        }
                        finish(&session, &turn, "Synthetic GitHub findings handled.");
                    });
                } else {
                    finish(&session, &turn, "Synthetic GitHub findings handled.");
                }
            }
            _ => result(request, json!({})),
        }
    }
}
