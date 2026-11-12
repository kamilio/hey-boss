//! cargo run --example workspace-chat-check -- /path/to/hey-boss
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, BufRead, Write},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

const SESSION: &str = "chat-fixture";

fn provider() {
    let mut turn = 0;
    for line in io::stdin().lock().lines() {
        let q: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let turn_id = format!("turn-{turn}");
        let send = |method: &str, item: Value| {
            println!(
                "{}",
                json!({"method":method,"params":{"threadId":SESSION,"turnId":turn_id,"item":item}})
            )
        };
        match (q["method"].as_str(), q["id"].as_str()) {
            (Some("thread/start"), _) => {
                assert_eq!(q["params"]["config"]["model_verbosity"], "low");
                println!(
                    "{}",
                    json!({"id":q["id"],"result":{"thread":{"id":SESSION}}})
                );
            }
            (Some("turn/start"), _) => {
                turn += 1;
                let turn_id = format!("turn-{turn}");
                println!("{}", json!({"id":q["id"],"result":{"turn":{"id":turn_id}}}));
                thread::sleep(Duration::from_millis(150));
                fs::write("recorded", turn.to_string()).unwrap();
                if turn == 1 {
                    println!(
                        "{}",
                        json!({"method":"item/completed","params":{"threadId":SESSION,"turnId":turn_id,"item":{"id":"user-1","type":"userMessage"}}})
                    );
                }
                println!(
                    "{}",
                    json!({"method":"item/started","params":{"threadId":SESSION,"turnId":turn_id,"item":{"id":"cmd","type":"commandExecution","command":"pwd"}}})
                );
                println!(
                    "{}",
                    json!({"id":"ask","method":"item/commandExecution/requestApproval","params":{"threadId":SESSION,"turnId":turn_id,"command":"pwd"}})
                );
            }
            (_, Some("ask")) => {
                assert_eq!(
                    q["result"]["decision"],
                    if turn == 1 { "decline" } else { "accept" }
                );
                for (id, thread, tid) in [
                    ("other-chat", "other", &*turn_id),
                    ("other-turn", SESSION, "other"),
                    ("form", SESSION, &*turn_id),
                ] {
                    println!(
                        "{}",
                        json!({"id":id,"method":"mcpServer/elicitation/request","params":{"threadId":thread,"turnId":tid,"serverName":"fixture","mode":"form","message":"Allow access?","requestedSchema":{"type":"object","properties":{}}}})
                    );
                }
            }
            (_, Some("form")) => {
                assert_eq!(
                    q["result"],
                    if turn == 1 {
                        json!({"action":"decline","content":null})
                    } else {
                        json!({"action":"accept","content":{}})
                    }
                );
                send(
                    "item/completed",
                    json!({"id":"cmd","type":"commandExecution","status":if turn==2 {"completed"}else{"declined"}}),
                );
                let reply = format!("reply {turn}");
                println!(
                    "{}",
                    json!({"method":"item/agentMessage/delta","params":{"threadId":SESSION,"turnId":turn_id,"delta":reply}})
                );
                send(
                    "item/completed",
                    json!({"type":"agentMessage","text":reply}),
                );
                println!(
                    "{}",
                    json!({"method":"turn/completed","params":{"threadId":SESSION,"turn":{"id":turn_id,"status":"completed"}}})
                );
            }
            (Some(_), _) if !q["id"].is_null() => println!("{}", json!({"id":q["id"],"result":{}})),
            _ => (),
        }
    }
}

fn main() {
    let cli = env::args_os().nth(1).expect("expected a hey-boss binary");
    if env::args_os().skip(1).any(|a| a == "app-server") {
        provider();
        return;
    }
    let dir = tempfile::tempdir_in(".").unwrap();
    let mut child = Command::new(fs::canonicalize(cli).unwrap())
        .arg("workspace-chat")
        .current_dir(dir.path())
        .env("HEY_BOSS_CODEX", env::current_exe().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in io::BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut input = child.stdin.take();
    let prompt = json!({"turn":"Hello"});
    writeln!(input.as_mut().unwrap(), "{prompt}").unwrap();
    let mut turn = 1;
    let mut seen = Vec::new();
    loop {
        let line = match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(l) => l,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("workspace chat timed out on turn {turn}: {seen:?}");
            }
        };
        let event: Value = serde_json::from_str(&line).unwrap();
        let (kind, data) = event.as_object().unwrap().iter().next().unwrap();
        seen.push(kind.clone());
        let reply = format!("reply {turn}");
        match kind.as_str() {
            "Session" => {
                assert_eq!(
                    fs::read_to_string(dir.path().join("recorded")).unwrap(),
                    turn.to_string()
                );
                assert_eq!(data["id"], SESSION);
                assert_eq!(data["accepted"], true);
            }
            "ToolStarted" => assert_eq!(data["input"]["command"], "pwd"),
            "Approval" => {
                assert!(matches!(
                    data["payload"]["id"].as_str(),
                    Some("ask" | "form")
                ));
                writeln!(
                    input.as_mut().unwrap(),
                    "{}",
                    json!({"decision":data["id"],"allow":turn==2})
                )
                .unwrap();
            }
            "ToolCompleted" => assert_eq!(data["failed"], turn == 1),
            "TextDelta" | "Message" => assert_eq!(data["text"], reply),
            "TurnCompleted" => {
                assert_eq!(data["output"], reply);
                assert_eq!(
                    seen,
                    [
                        "Session",
                        "ToolStarted",
                        "Approval",
                        "Approval",
                        "ToolCompleted",
                        "TextDelta",
                        "Message",
                        "TurnCompleted"
                    ]
                );
                seen.clear();
                turn += 1;
                if turn == 2 {
                    writeln!(input.as_mut().unwrap(), "{prompt}").unwrap();
                } else {
                    input.take();
                }
            }
            _ => panic!("unexpected workspace event: {event}"),
        }
    }
    assert!(child.wait().unwrap().success());
    assert_eq!(turn, 3);
    println!("Workspace chat: both turns, streamed text, approvals and disconnect passed.");
}
