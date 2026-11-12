// Synthetic Claude/Pi wire protocols. Claims and completions go through the real issue CLI.
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::Path,
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn append(file: &str, value: &Value) {
    let mut out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .unwrap();
    writeln!(out, "{value}").unwrap();
}

fn issue(action: &str) {
    let out = Command::new(env::var_os("HEY_BOSS_TEST_CLI").unwrap())
        .args(["issue", action, &env::var("HEY_BOSS_ISSUE_NUMBER").unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
}

fn main() {
    let provider = env::var("HEY_BOSS_FIXTURE_PROVIDER").unwrap();
    assert!(matches!(provider.as_str(), "claude" | "pi"));
    let claude = provider == "claude";
    let session = uuid::Uuid::new_v4().to_string();
    let file = env::current_dir().unwrap().join("session.jsonl");
    append(
        "launches.jsonl",
        &json!(env::args().skip(1).collect::<Vec<_>>()),
    );
    if !claude {
        fs::write(
            &file,
            format!("{}\n", json!({"type":"session","id":session})),
        )
        .unwrap();
    }

    let (stdin, messages) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if stdin
                .send(serde_json::from_str::<Value>(&line.unwrap()).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    let send = |msg: Value| println!("{msg}");
    let complete = |done| {
        let text = if done {
            issue("close");
            json!({"status":"completed","summary":"Provider worker verified"}).to_string()
        } else {
            "More work remains".to_owned()
        };
        if claude {
            send(
                json!({"type":"result","session_id":session,"subtype":"success","is_error":false,"result":text}),
            );
        } else {
            send(
                json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":text}],"stopReason":"stop"}}),
            );
            send(json!({"type":"agent_settled"}));
        }
    };
    let mut streaming = false;
    let mut goal_deadline = None;
    let mut turns = 0;
    loop {
        let msg = if streaming {
            let incomplete = goal_deadline.is_some_and(|end| Instant::now() >= end);
            if incomplete || (goal_deadline.is_none() && Path::new("finish").exists()) {
                complete(!incomplete);
                streaming = false;
                goal_deadline = None;
                continue;
            }
            match messages.recv_timeout(Duration::from_millis(50)) {
                Ok(v) => v,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else if let Ok(v) = messages.recv() {
            v
        } else {
            break;
        };
        append("protocol.jsonl", &msg);
        let kind = msg["type"].as_str().unwrap();
        let input = if claude {
            if kind == "control_request" {
                send(
                    json!({"type":"control_response","response":{"subtype":"success","request_id":msg["request_id"],"response":{}}}),
                );
            }
            if kind != "user" {
                continue;
            }
            &msg["message"]["content"]
        } else {
            let state = if kind == "get_state" {
                json!({"sessionId":session,"sessionFile":file,"isStreaming":streaming})
            } else {
                json!({})
            };
            send(
                json!({"type":"response","id":msg["id"],"command":kind,"success":true,"data":state}),
            );
            if kind == "steer" {
                append("inputs.jsonl", &msg["message"]);
                complete(true);
                streaming = false;
                goal_deadline = None;
                continue;
            }
            if kind != "prompt" {
                continue;
            }
            &msg["message"]
        };
        let text = input.as_str().unwrap();
        append("inputs.jsonl", input);
        turns += 1;
        if claude {
            send(json!({"type":"system","subtype":"init","session_id":session}));
        }
        if turns == 1 {
            issue("claim");
            fs::write("claimed", &provider).unwrap();
        }
        if claude {
            send(
                json!({"type":"stream_event","session_id":session,"event":{"delta":{"type":"text_delta","text":"Working"}}}),
            );
        } else {
            send(
                json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"Working"}}),
            );
        }
        if text.contains("STEER") {
            complete(true);
            streaming = false;
            goal_deadline = None;
        } else {
            streaming = true;
            goal_deadline = (turns == 1 && text.contains("goal fixture"))
                .then(|| Instant::now() + Duration::from_millis(100));
        }
    }
}
