//! Synthetic JSONL agent used by runtime, scheduler, and worker integration tests.
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
mod claude;
mod codex;
mod pi;

fn setting(k: &str) -> Option<String> {
    env::var(format!("HEY_BOSS_FIXTURE_{k}")).ok()
}
fn enabled(k: &str) -> bool {
    setting(k).is_some_and(|v| !v.is_empty())
}
fn send(v: Value) {
    println!("{v}");
}
fn value(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
struct Agent {
    provider: String,
    session: String,
    model: String,
    route: String,
    file: Option<PathBuf>,
    output: String,
    streaming: bool,
    turn: usize,
    preflight: Option<(Value, bool)>,
    native: Option<Child>,
    deadline: Option<Instant>,
}
impl Agent {
    fn turn(&self) -> String {
        if self.turn == 0 {
            "fixture-turn".into()
        } else {
            format!("fixture-turn-{}", self.turn)
        }
    }
    fn later(&mut self, ms: u64) {
        self.deadline = Some(Instant::now() + Duration::from_millis(ms));
    }
    fn check_model(&self, model: &str) {
        if let Some(expected) = setting("EXPECT_MODEL") {
            assert_eq!(model, expected)
        }
    }
    fn delta(&self, text: &str) {
        send(match self.provider.as_str() {
            "codex" => {
                json!({"method":"item/agentMessage/delta","params":{"threadId":self.session,"delta":text}})
            }
            "claude" => {
                json!({"type":"stream_event","session_id":self.session,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":text}}})
            }
            _ => {
                json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":text}})
            }
        })
    }
    fn complete(&mut self, interrupted: bool) {
        self.streaming = false;
        match self.provider.as_str() {
            "codex" => {
                send(
                    json!({"method":"item/completed","params":{"threadId":self.session,"item":{"type":"agentMessage","text":self.output}}}),
                );
                send(
                    json!({"method":"turn/completed","params":{"threadId":self.session,"turn":{"id":self.turn(),"status":if interrupted{"interrupted"}else{"completed"}}}}),
                )
            }
            "claude" => send(
                json!({"type":"result","session_id":self.session,"subtype":if interrupted{"error_during_execution"}else{"success"},"is_error":interrupted,"result":self.output,"errors":if interrupted{vec!["interrupted"]}else{vec![]}}),
            ),
            _ => {
                send(
                    json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":self.output}],"stopReason":if interrupted{"aborted"}else{"stop"}}}),
                );
                send(json!({"type":"agent_end","messages":[],"willRetry":false}));
                send(json!({"type":"agent_settled"}));
            }
        }
    }
    fn approval(&self, id: &str, cmd: &str) {
        send(match self.provider.as_str() {
            "codex" => {
                json!({"id":id,"method":"item/commandExecution/requestApproval","params":{"threadId":self.session,"turnId":self.turn(),"command":cmd}})
            }
            "claude" => {
                json!({"type":"control_request","request_id":id,"request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":cmd}}})
            }
            _ => {
                json!({"type":"extension_ui_request","id":id,"method":"select","title":cmd,"options":["one","two"]})
            }
        })
    }
    fn prompt(&mut self, text: &str) {
        self.check_model(&self.model);
        if let Some(path) = setting("JOB_AUDIT") {
            writeln!(fs::OpenOptions::new().create(true).append(true).open(path).unwrap(),"{}",json!({"text":text,"model":self.model,"route":self.route,"pid":std::process::id(),"session":self.session,"run":env::var("HEY_BOSS_JOB_RUN").ok()})).unwrap();
        }
        self.streaming = true;
        self.output = if text == "owned identity" {
            env::var("HEY_BOSS_AGENT_ID").unwrap_or("missing".into())
        } else {
            text.into()
        };
        if self.provider == "claude" && setting("MODEL_FAILURE").as_deref() != Some("missing") {
            send(
                json!({"type":"system","subtype":"init","session_id":self.session,"model":if enabled("MODEL_FAILURE"){"default-model"}else{&self.model}}),
            )
        }
        if self.provider == "pi" {
            send(json!({"type":"agent_start"}))
        }
        self.delta(text);
        if enabled("JOB_HOLD") {
            if enabled("JOB_RELEASE") {
                self.later(50)
            }
            return;
        }
        match text {
            "question fixture" if self.provider == "claude" => send(
                json!({"type":"control_request","request_id":"question","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Choose a color","options":[{"label":"Blue"},{"label":"Green"}],"multiSelect":false}]}}}),
            ),
            "closed input" => {
                // exec closes the reader thread's input before announcing the marker.
                use std::os::unix::process::CommandExt;
                let event = if self.provider == "codex" {
                    json!({"method":"item/agentMessage/delta","params":{"threadId":self.session,"delta":"INPUT_CLOSED"}})
                } else {
                    json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"INPUT_CLOSED"}})
                };
                panic!(
                    "{}",
                    Command::new("/bin/sh")
                        .args([
                            "-c",
                            "exec 0<&-; echo \"$1\"; exec sleep 30",
                            "fixture",
                            &event.to_string()
                        ])
                        .exec()
                );
            }
            "duplicate requests" => {
                for cmd in ["first command", "different command"] {
                    self.approval("duplicate", cmd)
                }
            }
            "missing approval ownership" => send(
                json!({"id":"unowned-approval","method":"item/commandExecution/requestApproval","params":{"command":"unowned command"}}),
            ),
            "invalid approval identity" => send(
                json!({"id":{"invalid":true},"method":"item/commandExecution/requestApproval","params":{"threadId":self.session,"turnId":self.turn(),"command":"unowned command"}}),
            ),
            "tool event classifications" => {
                for item in [
                    json!({"id":"user","type":"userMessage"}),
                    json!({"id":"reasoning","type":"reasoning"}),
                    json!({"id":"command","type":"commandExecution","status":"declined"}),
                    json!({"id":"dynamic","type":"dynamicToolCall","status":"completed","success":false}),
                ] {
                    send(
                        json!({"method":"item/completed","params":{"threadId":self.session,"item":item}}),
                    )
                }
                self.complete(false)
            }
            "burst completion" | "large burst completion" => {
                for _ in 0..if text == "burst completion" { 160 } else { 600 } {
                    self.delta("x")
                }
                self.complete(false)
            }
            "coalesce boundaries" => {
                for c in ["é", "b", "c"] {
                    self.delta(&c.repeat(10000))
                }
                send(
                    json!({"method":"item/started","params":{"threadId":self.session,"item":{"id":"boundary-tool","type":"commandExecution"}}}),
                );
                self.delta("after tool");
                self.complete(false)
            }
            "completion then disconnect" => {
                self.complete(false);
                std::process::exit(0)
            }
            "background killed" => {
                send(
                    json!({"type":"system","subtype":"task_started","task_id":"task-1","task_type":"local_agent"}),
                );
                send(
                    json!({"type":"result","session_id":self.session,"subtype":"success","is_error":false,"result":"provisional"}),
                );
                send(
                    json!({"type":"system","subtype":"task_updated","task_id":"task-1","patch":{"status":"killed"}}),
                );
                self.complete(false)
            }
            "hold native tool" => {
                self.native = Some(
                    Command::new("/bin/sh")
                        .args(["-c", "sleep 0.6; printf BAD > native-finished"])
                        .current_dir(setting("EFFECTS").unwrap())
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .unwrap(),
                )
            }
            "hold idle provider" if self.provider == "pi" => {
                self.streaming = false;
                self.later(600)
            }
            "queued turns" => self.later(300),
            "retry" if self.provider == "pi" => {
                send(json!({"type":"agent_end","messages":[],"willRetry":true}));
                self.later(100)
            }
            "input" if self.provider == "pi" => send(
                json!({"type":"extension_ui_request","id":"input-1","method":"select","title":"Fixture input","options":["one","two"]}),
            ),
            "malformed" => println!("{{invalid JSON}}"),
            "malformed terminal" => match self.provider.as_str() {
                "codex" => send(
                    json!({"method":"turn/completed","params":{"turn":{"id":self.turn(),"status":"completed"}}}),
                ),
                "claude" => send(
                    json!({"type":"result","session_id":self.session,"subtype":"success","result":"unverified"}),
                ),
                _ => {
                    send(
                        json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"unverified"}],"stopReason":"invalid"}}),
                    );
                    send(json!({"type":"agent_settled"}))
                }
            },
            "deferred assistant" if self.provider == "pi" => {
                send(
                    json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"waiting for external work"}],"stopReason":"deferred"}}),
                );
                send(json!({"type":"agent_settled"}))
            }
            "approval" => {
                if self.provider == "codex" {
                    send(
                        json!({"id":"foreign","method":"item/commandExecution/requestApproval","params":{"threadId":"other-session","turnId":self.turn()}}),
                    )
                }
                self.approval("approval-1", "fixture command")
            }
            s if s.starts_with("hold") || s == "missing session acknowledgement" => {}
            _ => self.complete(false),
        }
    }
    fn timer(&mut self) {
        self.deadline = None;
        if enabled("JOB_HOLD") && setting("JOB_RELEASE").is_some_and(|p| !Path::new(&p).exists()) {
            self.later(50);
            return;
        }
        if self.output == "retry" {
            self.output = "retry settled".into()
        }
        self.complete(false)
    }
}
fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    if args == ["--print-path"] {
        println!("{}", env::current_exe().unwrap().display());
        return;
    }
    let flag = |k| args.windows(2).find(|a| a[0] == k).map(|a| a[1].clone());
    let mut agent = Agent {
        provider: setting("PROVIDER").unwrap(),
        session: if enabled("FRESH_THREADS") {
            uuid::Uuid::new_v4().to_string()
        } else {
            setting("SESSION_ID").unwrap_or("00000000-0000-0000-0000-000000000001".into())
        },
        model: flag("--model").unwrap_or("default-model".into()),
        route: "proxy".into(),
        file: None,
        output: String::new(),
        streaming: false,
        turn: 0,
        preflight: None,
        native: None,
        deadline: None,
    };
    match agent.provider.as_str() {
        "codex" => codex::check_args(&args),
        "pi" => {
            if let Some(p) = flag("--session") {
                agent.restore_pi(p.into())
            }
        }
        "claude" => {}
        p => panic!("unknown provider: {p}"),
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if tx
                .send(serde_json::from_str::<Value>(&line.unwrap()).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    loop {
        let msg = if let Some(deadline) = agent.deadline {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(v) => v,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    agent.timer();
                    continue;
                }
                Err(_) => break,
            }
        } else {
            match rx.recv() {
                Ok(v) => v,
                Err(_) => break,
            }
        };
        match agent.provider.as_str() {
            "codex" => agent.codex(&msg),
            "claude" => agent.claude(&msg),
            _ => agent.pi(&msg),
        }
    }
    // A held job outlives a scheduler that closes its end of the pipe during reload.
    if enabled("JOB_HOLD") || enabled("JOB_HOLD_INIT") {
        loop {
            if let Some(d) = agent.deadline {
                thread::sleep(d.saturating_duration_since(Instant::now()));
                agent.timer()
            } else {
                thread::park()
            }
        }
    }
}
