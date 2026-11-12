// Record callback replies; never execute an approved tool.
use serde_json::{Value, json};
use std::io::Write;
use std::{fs, thread, time::Duration};
mod protocol;
use protocol::*;

fn approval(mode: &str, thread: &str, turn: &str, i: usize, count: usize) {
    let item = if count == 2 {
        "shared-item".into()
    } else {
        format!("item-{i}")
    };
    let mut params = json!({"threadId":if mode == "foreign" { id() } else { thread.to_owned() },
        "turnId":turn,"itemId":item,"cwd":"/synthetic/repo","reason":"Review the exact requested action."});
    let command = "item/commandExecution/requestApproval";
    let method = match mode {
        "files" => {
            send(
                json!({"method":"item/started","params":{"threadId":thread,"item":{"id":item,"type":"fileChange","changes":[{"path":"/synthetic/repo/example.rs","diff":"-old\n+new"}]}}}),
            );
            params["grantRoot"] = json!("/synthetic/repo");
            "item/fileChange/requestApproval"
        }
        "permissions" => {
            params["permissions"] =
                json!({"network":{"enabled":true},"fileSystem":{"write":["/synthetic/repo"]}});
            "item/permissions/requestApproval"
        }
        "network" => {
            params["networkApprovalContext"] =
                json!({"host":"registry.example.com","protocol":"https"});
            command
        }
        m if m.starts_with("signin") => {
            params["mode"] = json!("url");
            params["serverName"] = json!("Okta");
            params["message"] = json!("Sign in to continue.");
            params["url"] = json!("https://login.example.invalid/authorize?state=synthetic");
            params["elicitationId"] = json!("login-1");
            "mcpServer/elicitation/request"
        }
        m if m.starts_with("mcp-confirm") => {
            params["mode"] = json!(if m == "mcp-confirm-openai" {
                "openai/form"
            } else {
                "form"
            });
            params["serverName"] = json!("cua_repl");
            params["message"] = json!("Allow Computer Use to use \"Skill Audit\"?");
            params["requestedSchema"] = json!({"type":"object","properties":{}});
            "mcpServer/elicitation/request"
        }
        m if m.starts_with("connector") => {
            let mut questions = vec![
                json!({"id":"okta-approval","question":"Allow the requested Okta action?","options":[{"label":"Accept","description":"Run this action once."},{"label":"Decline","description":"Do not run this action."},{"label":"Cancel","description":"Stop here."}]}),
            ];
            if m == "connector-multi" {
                questions.push(json!({"id":"second-question","question":"Choose the environment","options":[{"label":"Staging","description":"Use staging."},{"label":"Production","description":"Use production."}]}));
            }
            params["questions"] = json!(questions);
            if m == "connector-item" {
                "item/tool/requestUserInput"
            } else {
                "tool/requestUserInput"
            }
        }
        _ => {
            params["command"] = json!(if i == 0 {
                "cargo test --offline"
            } else {
                "cargo test --lib"
            });
            command
        }
    };
    let request = format!("approval-{i}");
    send(json!({"id":request,"method":method,"params":params}));
    if mode == "resolved" {
        send(
            json!({"method":"serverRequest/resolved","params":{"threadId":thread,"requestId":request}}),
        );
    }
}

fn main() {
    // Let the worker's final callback reach the transcript during cancellation.
    ctrlc::set_handler(|| {
        thread::sleep(Duration::from_millis(50));
        std::process::exit(0)
    })
    .unwrap();
    let (session, turn) = (id(), id());
    let mode = fs::read_to_string("mode.txt").unwrap();
    let mode = mode.trim();
    let mut pending = 0;
    let mut delayed_start: Option<Value> = None;
    let complete = || {
        finish(
            &session,
            &turn,
            "Explicit decisions received in the original session.",
        )
    };
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("protocol.jsonl")
        .unwrap();
    for msg in messages() {
        writeln!(log, "{msg}").unwrap();
        match msg["method"].as_str() {
            None => {
                pending -= 1;
                if pending == 0 {
                    if let Some(start) = delayed_start.take() {
                        result(&start, json!({"turn":{"id":turn}}));
                    }
                    complete();
                }
            }
            Some("thread/start") => result(&msg["id"], json!({"thread":{"id":session}})),
            Some("turn/start") => {
                issue(&session, &["claim", "1"]);
                if mode == "ack-race" {
                    delayed_start = Some(msg["id"].clone());
                } else {
                    result(&msg["id"], json!({"turn":{"id":turn}}));
                }
                pending = if mode == "concurrent" { 2 } else { 1 };
                for i in 0..pending as usize {
                    approval(mode, &session, &turn, i, pending as usize);
                }
                if mode == "resolved" {
                    let (s, t) = (session.clone(), turn.clone());
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(300));
                        finish(
                            &s,
                            &t,
                            "Explicit decisions received in the original session.",
                        );
                    });
                }
            }
            Some(_) => {
                // Notifications have no request id and receive no response.
                if let Some(request) = msg.get("id") {
                    result(request, json!({}));
                }
            }
        }
    }
}
