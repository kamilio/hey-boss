// JSONL worker used by process-level tests. A delivery report and the issue state are independent.
mod protocol;
use protocol::*;
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{ErrorKind, Write},
    path::Path,
    process::{self, Command},
    thread,
    time::Duration,
};

fn optional(file: &str) -> Option<String> {
    match fs::read_to_string(file) {
        Ok(v) => Some(v),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => panic!("{file}: {e}"),
    }
}
fn report(session: &str, turn: &str, status: &str, summary: &str) {
    agent_message(
        session,
        json!({"status":status,"summary":summary}).to_string(),
    );
    send(
        json!({"method":"turn/completed","params":{"threadId":session,"turn":{"id":turn,"status":"completed"}}}),
    );
}
fn agent_message(session: &str, text: String) {
    send(
        json!({"method":"item/completed","params":{"threadId":session,"item":{"type":"agentMessage","text":text}}}),
    );
}

fn turn_events(mode: &str, session: &str, turn: &str, task: Option<&Value>) {
    if mode == "disconnect" {
        process::exit(9);
    }
    if matches!(
        mode,
        "disconnect-after-text"
            | "failed-after-text"
            | "stale-turn-success"
            | "malformed-completion"
            | "terminal-error-event"
    ) {
        issue(session, &["close", &task.unwrap()["number"].to_string()]);
        let event_turn = if mode == "stale-turn-success" {
            "stale-turn"
        } else {
            turn
        };
        if event_turn != turn {
            send(
                json!({"method":"turn/started","params":{"threadId":session,"turn":{"id":event_turn}}}),
            );
        }
        let text = if mode == "malformed-completion" {
            "invalid completion report".to_owned()
        } else {
            json!({"status":"completed","summary":"Looks done, but the transport has not finished"})
                .to_string()
        };
        send(
            json!({"method":"item/completed","params":{"threadId":session,"turnId":event_turn,"item":{"type":"agentMessage","text":text}}}),
        );
        if mode == "disconnect-after-text" {
            process::exit(9);
        }
        if mode == "terminal-error-event" {
            send(
                json!({"method":"error","params":{"threadId":session,"turnId":turn,"willRetry":false,"error":{"message":"Model server is unavailable"}}}),
            );
            return;
        }
        if event_turn != turn {
            send(
                json!({"method":"turn/completed","params":{"threadId":session,"turn":{"id":event_turn,"status":"completed"}}}),
            );
        }
        send(
            json!({"method":"turn/completed","params":{"threadId":session,"turn":{"id":turn,"status":if mode=="malformed-completion"{"completed"}else{"failed"},"error":{"message":"actual active turn failed"}}}}),
        );
        return;
    }
    if mode == "delay-model-start" {
        thread::sleep(Duration::from_secs(6));
    }
    if mode == "delay-release" {
        while !Path::new("release").exists() {
            thread::sleep(Duration::from_millis(50));
        }
    }
    if matches!(mode, "delay" | "delay-unclaimed" | "delay-model-start") {
        send(
            json!({"method":"item/started","params":{"threadId":session,"item":{"type":"reasoning"}}}),
        );
        return;
    }
    if mode == "transient-error-recovered" {
        send(
            json!({"method":"error","params":{"threadId":session,"turnId":turn,"willRetry":true,"error":{"message":"Temporary disconnect; retry in progress"}}}),
        );
    }
    let approval = |request| {
        send(
            json!({"id":request,"method":"item/commandExecution/requestApproval","params":{"threadId":session,"command":"synthetic privileged operation"}}),
        )
    };
    if mode == "approval" {
        approval("approval-1");
        return;
    }
    if mode.starts_with("database-outage") || mode.starts_with("proxy-outage") {
        let outage = if mode.starts_with("database-outage") {
            "Database service transport failed: Broken pipe (os error 32). Write outcome may be unknown; mutations are never automatically replayed."
        } else {
            "HTTP 504 from local hey-proxy: internal recovery time budget exhausted before a response could be forwarded."
        };
        if mode.ends_with("-rpc") {
            send(
                json!({"method":"turn/completed","params":{"threadId":session,"turn":{"id":turn,"status":"failed","error":{"message":outage}}}}),
            );
            return;
        }
        if !mode.ends_with("-summary") {
            send(
                json!({"method":"item/completed","params":{"threadId":session,"item":{"type":"commandExecution","exitCode":1,"aggregatedOutput":outage}}}),
            );
        }
        if mode.ends_with("-approval") {
            approval("approval-after-outage");
            return;
        }
        if !mode.ends_with("-recovered") {
            report(
                session,
                turn,
                "blocked",
                if mode.ends_with("-summary") {
                    outage
                } else {
                    "Delivery cannot continue; saved work remains."
                },
            );
            return;
        }
    }
    if matches!(mode, "approval-outage-tool" | "approval-outage-recovered") {
        send(
            json!({"method":"item/completed","params":{"threadId":session,"item":{"type":"mcpToolCall","status":"failed","result":{"content":[{"type":"text","text":"Automatic approval review failed: unexpected status 404 Not Found: The model `wisp-alpha` does not exist or you do not have access to it. The action was not executed because automatic approval review could not be completed."}]}}}}),
        );
    }
    if mode.starts_with("approval-outage") && mode != "approval-outage-recovered" {
        report(
            session,
            turn,
            "blocked",
            if mode == "approval-outage-tool" {
                "Final verification cannot continue. Worktree and repairs retained."
            } else {
                "Automatic approval review still fails with HTTP 404 for missing `wisp-alpha`, preventing GitHub CI/review reads. Restore the approval service to finish notification and cleanup."
            },
        );
        return;
    }
    let status = match mode {
        "delay-release"
        | "fresh-claim"
        | "resume-unavailable"
        | "unclaimed"
        | "offline-updates"
        | "subtasks-completed"
        | "partial"
        | "partial-goal"
        | "approval-outage-recovered"
        | "database-outage-recovered"
        | "proxy-outage-recovered"
        | "transient-error-recovered" => "completed",
        other => other,
    };
    if let Some(task) = task
        && status == "completed"
        && !mode.starts_with("partial")
    {
        let args: Vec<String> = optional("worker-args.json")
            .map(|v| serde_json::from_str(&v).unwrap())
            .unwrap_or_default();
        let artifact = task["labels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "task:plan" || v == "task:research");
        if artifact || !args.iter().any(|v| v == "--prs") {
            issue(
                session,
                &[
                    "close",
                    &task["number"].to_string(),
                    "--comment",
                    "All fixture requirements resolved and verified.",
                ],
            );
        }
    }
    if mode == "partial-goal" {
        agent_message(session,"Pushed the partial fix. Genuine filter and citeproc engines remain unimplemented; leave the issue open.".into());
        send(
            json!({"method":"turn/completed","params":{"threadId":session,"turn":{"id":turn,"status":"completed"}}}),
        );
    } else {
        report(
            session,
            turn,
            status,
            "Implemented fixture. Meaningful checks passed.",
        );
    }
}

fn main() {
    if env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--print-path")) {
        println!("{}", env::current_exe().unwrap().display());
        return;
    }
    let mode = optional("mode.txt").unwrap_or_else(|| "completed".into());
    let mode = mode.trim();
    let mut session = id();
    let mut turns = 0;
    let mut transcript = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("protocol.jsonl")
        .unwrap();
    for msg in messages() {
        // One append per line: several workers can share the same checkout.
        transcript.write_all(format!("{msg}\n").as_bytes()).unwrap();
        let (Some(method), Some(request)) = (msg["method"].as_str(), msg.get("id")) else {
            continue;
        };
        let p = &msg["params"];
        if let Some(case) = mode.strip_prefix("live-prompt-") {
            let session = "live-prompt-session";
            match method {
                "initialize" => result(request, json!({})),
                "thread/start" => result(request, json!({"thread":{"id":session}})),
                "turn/start" => {
                    turns += 1;
                    issue(session, &["claim", "1"]);
                    result(
                        request,
                        json!({"turn":{"id":if turns==1{"owned-turn"}else{"updated-turn"}}}),
                    );
                    send(
                        json!({"method":"item/started","params":{"threadId":session,"item":{"type":"agentMessage"}}}),
                    );
                }
                "turn/steer" => match case {
                    "message-race" => {
                        report(session, "owned-turn", "completed", "Old instructions done");
                        send(
                            json!({"id":request,"error":{"message":"Turn has already completed"}}),
                        );
                    }
                    "message-uncertain" => result(request, json!({"turnId":"wrong-turn"})),
                    "message-rejected" => {
                        send(json!({"id":request,"error":{"message":"Temporary steering failure"}}))
                    }
                    _ => result(request, json!({"turnId":p["expectedTurnId"]})),
                },
                _ => {}
            }
            continue;
        }
        if mode == "retry-history" {
            match method {
                "initialize" => result(request, json!({})),
                "thread/start" | "thread/resume" => {
                    session = p["threadId"].as_str().unwrap_or("fresh-session").to_owned();
                    result(request, json!({"thread":{"id":session,"turns":[]}}));
                }
                "turn/start" if session != "fresh-session" => send(
                    json!({"id":request,"error":{"code":-32600,"message":"failed to submit turn input: ActiveTurnOutputSchemaMismatch"}}),
                ),
                "turn/start" => {
                    issue(&session, &["claim", "1"]);
                    issue(&session, &["close", "1"]);
                    result(request, json!({"turn":{"id":"turn"}}));
                    finish(&session, "turn", "Recovery verified");
                }
                _ => {}
            }
            continue;
        }
        match method {
            "initialize" => {
                assert_eq!(p["capabilities"]["experimentalApi"], true);
                result(request, json!({}));
            }
            "thread/start" => {
                assert_eq!(p["ephemeral"], false);
                fs::write(format!(".codex-fixture-{session}.json"), r#"{"turns":[]}"#).unwrap();
                result(request, json!({"thread":{"id":session}}));
            }
            "thread/resume" => {
                if mode == "resume-unavailable" {
                    send(
                        json!({"id":request,"error":{"code":-32600,"message":"Saved session is locked by another writer"}}),
                    );
                    continue;
                }
                session = p["threadId"].as_str().unwrap().to_owned();
                let saved: Value = serde_json::from_slice(
                    &fs::read(format!(".codex-fixture-{session}.json")).unwrap(),
                )
                .unwrap();
                assert!(!saved["turns"].as_array().unwrap().is_empty());
                result(request, json!({"thread":{"id":session}}));
            }
            "thread/goal/set" => {
                assert!(turns > 0);
                assert_eq!(p["status"], "active");
                result(request, json!({"goal":p}));
            }
            "thread/goal/get" => panic!("Workers must not poll goal status"),
            "turn/start" => {
                turns += 1;
                let text = p["input"][0]["text"].as_str().unwrap();
                let path = format!(".codex-fixture-{session}.json");
                let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                saved["turns"].as_array_mut().unwrap().push(json!(text));
                fs::write(path, saved.to_string()).unwrap();
                if mode == "offline-updates" {
                    let out = Command::new(env::var_os("HEY_BOSS_TEST_CLI").unwrap())
                        .args([
                            "update",
                            "--project",
                            "Offline worker QA",
                            "--title",
                            "Issue finished",
                            "Worker finished without waiting for Mac",
                            text,
                            "--json",
                        ])
                        .output()
                        .unwrap();
                    assert!(
                        out.status.success(),
                        "{}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                    assert_eq!(
                        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
                        "pending"
                    );
                }
                let task = if matches!(mode, "unclaimed" | "delay-unclaimed" | "delay-model-start")
                {
                    None
                } else {
                    let rest = text.split_once("issue view ").expect(text).1;
                    let number = &rest[..rest
                        .find(|c: char| !c.is_ascii_digit())
                        .unwrap_or(rest.len())];
                    assert_eq!(
                        env::var("HEY_BOSS_ISSUE_PROJECT").unwrap(),
                        "named:Worker fixture"
                    );
                    let view = issue(&session, &["view", number]);
                    assert_eq!(view["issue"]["number"], number.parse::<u64>().unwrap());
                    if mode == "fresh-claim" {
                        assert_eq!(view["issue"]["assignee"], Value::Null);
                    }
                    issue(&session, &["claim", number]);
                    Some(view)
                };
                assert_eq!(p["threadId"], session);
                assert_eq!(p["outputSchema"]["required"], json!(["status", "summary"]));
                let turn = id();
                result(request, json!({"turn":{"id":turn}}));
                turn_events(mode, &session, &turn, task.as_ref().map(|v| &v["issue"]));
            }
            _ => result(request, json!({})),
        }
    }
}
