//! Controls only loaded threads on an explicitly configured owning Codex server.
use serde_json::{Value, json};
use std::{
    io,
    os::unix::net::UnixStream,
    path::PathBuf,
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};
fn fail(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
struct Connection {
    socket: WebSocket<UnixStream>,
    id: u64,
    deadline: Instant,
}
impl Connection {
    fn open(path: &str) -> io::Result<Self> {
        let deadline = Instant::now() + Duration::from_secs(8);
        let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
        socket.connect_timeout(&socket2::SockAddr::unix(path)?, Duration::from_secs(8))?;
        let fd: std::os::fd::OwnedFd = socket.into();
        let stream = UnixStream::from(fd);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(fail("Codex connection timed out"));
        }
        stream.set_read_timeout(Some(remaining))?;
        stream.set_write_timeout(Some(remaining))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(2 * 1024 * 1024))
            .max_frame_size(Some(2 * 1024 * 1024));
        let (socket, _) =
            tungstenite::client::client_with_config("ws://localhost/rpc", stream, Some(config))
                .map_err(|e| fail(format!("Codex WebSocket handshake failed: {e}")))?;
        let mut c = Self {
            socket,
            id: 0,
            deadline,
        };
        c.rpc("initialize", json!({"clientInfo":{"name":"hey_boss","title":"Hey Boss","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}))?;
        c.write(json!({"method":"initialized","params":{}}))?;
        Ok(c)
    }
    fn timeout(&self) -> io::Result<()> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(fail(
                "Codex connection timed out. Check action state before retrying.",
            ));
        }
        self.socket
            .get_ref()
            .set_read_timeout(Some(remaining))
            .map_err(|e| fail(format!("read timeout {remaining:?}: {e}")))?;
        self.socket
            .get_ref()
            .set_write_timeout(Some(remaining))
            .map_err(|e| fail(format!("write timeout {remaining:?}: {e}")))
    }
    fn write(&mut self, value: Value) -> io::Result<()> {
        self.timeout()?;
        self.socket
            .send(Message::Text(serde_json::to_string(&value)?.into()))
            .map_err(|e| {
                fail(format!(
                    "Codex write failed ({e}). Check action state before retrying."
                ))
            })
    }
    fn rpc(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.id += 1;
        let id = self.id;
        self.write(json!({"id":id,"method":method,"params":params}))?;
        loop {
            self.timeout()?;
            let message = self.socket.read().map_err(|_| fail("Codex disconnected or timed out. If an action was pending, check its state before retrying."))?;
            let value: Value = match message {
                Message::Text(text) => serde_json::from_str(&text)?,
                Message::Close(_) => {
                    return Err(fail(
                        "Codex disconnected; check action state before retrying",
                    ));
                }
                Message::Ping(_) | Message::Pong(_) => continue,
                _ => return Err(fail("Unexpected Codex WebSocket message")),
            };
            if value.get("id") == Some(&json!(id)) {
                if let Some(error) = value.get("error") {
                    return Err(fail(format!(
                        "Codex: {}",
                        error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("request rejected")
                    )));
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| fail("Invalid Codex acknowledgement"));
            }
            // Never respond to approvals/tool calls from another controlling client.
        }
    }
    fn owns(&mut self, thread: &str) -> io::Result<bool> {
        let mut cursor = Value::Null;
        for _ in 0..32 {
            let page = self.rpc("thread/loaded/list", json!({"limit":100,"cursor":cursor}))?;
            let ids = page["data"]
                .as_array()
                .ok_or_else(|| fail("Invalid loaded-thread list"))?;
            if ids.iter().any(|id| id.as_str() == Some(thread)) {
                return Ok(true);
            }
            cursor = page["nextCursor"].clone();
            if cursor.is_null() {
                return Ok(false);
            }
        }
        Err(fail("Loaded-thread list exceeds limit"))
    }
}

fn apply(c: &mut Connection, thread: &str, action: &str, input: &Value) -> io::Result<Value> {
    let runtime = c.rpc("thread/read", json!({"threadId":thread}))?;
    if runtime["thread"]["id"].as_str() != Some(thread) {
        return Err(fail("Codex returned a different thread"));
    }
    let mut goal = c.rpc("thread/goal/get", json!({"threadId":thread}))?["goal"].clone();
    let mut turn = Value::Null;
    if runtime["thread"]["status"]["type"] == "active" {
        let turns = c.rpc(
            "thread/turns/list",
            json!({"threadId":thread,"limit":10,"itemsView":"summary","sortDirection":"desc"}),
        )?;
        if let Some(data) = turns["data"].as_array() {
            turn = data
                .iter()
                .find(|t| t["status"] == "inProgress")
                .map(|t| t["id"].clone())
                .unwrap_or(Value::Null);
        }
    }
    match action {
        "inspect" => {}
        "enable-goal" | "disable-goal" => {
            if goal.is_null() {
                return Err(fail("This session has no saved goal to re-enable"));
            }
            goal = c.rpc("thread/goal/set", goal_params(thread, action))?["goal"].clone();
            let expected = if action == "enable-goal" {
                "active"
            } else {
                "paused"
            };
            if goal["status"] != expected {
                return Err(fail("Goal change was not acknowledged"));
            }
        }
        "steer" => {
            let text = input["text"]
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 32_000)
                .ok_or_else(|| fail("Instruction must contain 1–32000 bytes"))?;
            let expected = input["expectedTurnId"]
                .as_str()
                .ok_or_else(|| fail("Refresh controls before sending"))?;
            if turn.as_str() != Some(expected) || runtime["thread"]["canAcceptDirectInput"] != true
            {
                return Err(fail(
                    "The active turn changed or cannot accept steering. Refresh controls; your draft is preserved.",
                ));
            }
            let ack = c.rpc("turn/steer", json!({"threadId":thread,"expectedTurnId":expected,"input":[{"type":"text","text":text}]}))?;
            if ack["turnId"].as_str() != Some(expected) {
                return Err(fail("Unexpected steering acknowledgement"));
            }
        }
        _ => return Err(fail("Unknown agent-control action")),
    }
    Ok(
        json!({"ok":true,"threadId":thread,"goal":goal,"turnId":turn,"canSteer":!turn.is_null() && runtime["thread"]["canAcceptDirectInput"] == true}),
    )
}
fn goal_params(thread: &str, action: &str) -> Value {
    // Deliberately omit objective and budget: preserve Codex's saved goal and usage.
    json!({"threadId":thread,"status":if action == "enable-goal" {"active"} else {"paused"}})
}

pub fn run(thread: &str, action: &str, input: &Value) -> io::Result<Value> {
    if thread.len() != 36
        || !thread.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        return Err(fail("Invalid Codex session ID"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| fail("Home directory unavailable"))?;
    let path = PathBuf::from(home).join(".config/hey-boss/agent-control.json");
    let bytes = std::fs::read(&path).map_err(|_| fail("Codex control is not configured. Add an owning app-server socket to ~/.config/hey-boss/agent-control.json; standalone terminal sessions cannot be controlled."))?;
    if bytes.len() > 32_768 {
        return Err(fail("Control config exceeds limit"));
    }
    let config: Value = serde_json::from_slice(&bytes)?;
    run_config(thread, action, input, &config)
}

fn run_config(thread: &str, action: &str, input: &Value, config: &Value) -> io::Result<Value> {
    let sockets = config["sockets"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 4)
        .ok_or_else(|| fail("Control config requires 1–4 sockets"))?;
    let mut owner = None;
    for socket in sockets {
        let socket = socket
            .as_str()
            .filter(|p| p.starts_with('/') && p.len() < 1024)
            .ok_or_else(|| fail("Control socket must be an absolute path"))?;
        let mut connection = Connection::open(socket)
            .map_err(|e| fail(format!("Cannot verify configured Codex server: {e}")))?;
        if connection.owns(thread)? {
            if owner.is_some() {
                return Err(fail(
                    "Session is loaded on multiple servers; refusing ambiguous control",
                ));
            }
            owner = Some(connection);
        }
    }
    let mut owner = owner.ok_or_else(|| fail("This session is not loaded on a configured Codex server. No session was resumed or started."))?;
    owner.deadline = Instant::now() + Duration::from_secs(8);
    apply(&mut owner, thread, action, input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::net::UnixListener,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        thread,
    };
    struct Mock {
        directory: PathBuf,
        path: String,
        log: Arc<Mutex<Vec<Value>>>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    impl Mock {
        fn new(mode: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "hey-boss-control-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&directory).unwrap();
            let path = directory.join("socket").to_string_lossy().into_owned();
            let listener = UnixListener::bind(&path).unwrap();
            listener.set_nonblocking(true).unwrap();
            let log = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let worker_log = log.clone();
            let worker_stop = stop.clone();
            let mode = mode.to_owned();
            let worker = thread::spawn(move || {
                let mut clients = Vec::new();
                while !worker_stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let log = worker_log.clone();
                            let mode = mode.clone();
                            let stop = worker_stop.clone();
                            clients.push(thread::spawn(move || {
                                stream.set_nonblocking(false).unwrap();
                                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                                let mut ws=tungstenite::accept(stream).unwrap();
                                let mut goal=json!({"objective":"Build the substantial feature","status":"paused","tokensUsed":4321,"timeUsedSeconds":87,"tokenBudget":9000});
                                while let Ok(Message::Text(text))=ws.read() {
                                    let r:Value=serde_json::from_str(&text).unwrap();log.lock().unwrap().push(r.clone());
                                    let Some(id)=r.get("id") else {continue};let m=r["method"].as_str().unwrap();let p=&r["params"];
                                    let result=match m {
                                        "initialize"=>json!({}),
                                        "thread/loaded/list"=>json!({"data":if mode=="absent" {vec![]}else{vec!["session"]},"nextCursor":null}),
                                        "thread/read"=>json!({"thread":{"id":if mode=="wrong" {"other"}else{"session"},"status":{"type":"active"},"canAcceptDirectInput":true}}),
                                        "thread/goal/get"=>json!({"goal":if mode=="no-goal" {Value::Null}else{goal.clone()}}),
                                        "thread/turns/list"=>json!({"data":[{"id":"turn","status":"inProgress"}]}),
                                        "thread/goal/set"=> {assert_eq!(p.as_object().unwrap().len(),2);if mode=="disconnect" {break}goal["status"]=p["status"].clone();json!({"goal":goal})},
                                        "turn/steer"=> {assert_eq!(p["expectedTurnId"],"turn");assert_eq!(p["threadId"],"session");if mode=="conflict" {ws.send(Message::Text(json!({"id":id,"error":{"message":"turn changed"}}).to_string().into())).unwrap();continue}json!({"turnId":"turn"})},
                                        _=>panic!("Unexpected method {m}"),
                                    };
                                    if mode=="timeout" && m!="initialize" {while !stop.load(Ordering::Relaxed) {thread::sleep(Duration::from_millis(10));}break}
                                    if ws.send(Message::Text(json!({"method":"unrelated/event"}).to_string().into())).is_err(){break}
                                    if ws.send(Message::Text(json!({"id":99999,"result":{}}).to_string().into())).is_err(){break}
                                    if ws.send(Message::Text(json!({"id":id,"result":result}).to_string().into())).is_err(){break}
                                }
                            }));
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(e) => panic!("{e}"),
                    }
                }
                for c in clients {
                    c.join().unwrap();
                }
            });
            Self {
                directory,
                path,
                log,
                stop,
                worker: Some(worker),
            }
        }
        fn config(&self) -> Value {
            json!({"sockets":[self.path]})
        }
        fn methods(&self) -> Vec<String> {
            self.log
                .lock()
                .unwrap()
                .iter()
                .map(|v| v["method"].as_str().unwrap().to_owned())
                .collect()
        }
    }
    #[test]
    fn invalid_identity_fails_before_connecting() {
        assert!(run("wrong", "enable-goal", &Value::Null).is_err());
    }
    #[test]
    fn reenable_and_pause_roundtrip_keep_goal_usage() {
        let mock = Mock::new("normal");
        for (action, status) in [("enable-goal", "active"), ("disable-goal", "paused")] {
            let result = run_config("session", action, &json!({}), &mock.config()).unwrap();
            assert_eq!(result["goal"]["status"], status);
            assert_eq!(result["goal"]["objective"], "Build the substantial feature");
            assert_eq!(result["goal"]["tokensUsed"], 4321);
            assert_eq!(result["goal"]["timeUsedSeconds"], 87);
        }
        assert!(
            !mock
                .methods()
                .iter()
                .any(|m| m == "thread/resume" || m == "thread/goal/clear" || m == "turn/start")
        );
    }
    #[test]
    fn absent_and_ambiguous_owners_never_mutate() {
        let absent = Mock::new("absent");
        assert!(run_config("session", "enable-goal", &json!({}), &absent.config()).is_err());
        assert!(!absent.methods().contains(&"thread/goal/set".into()));
        let a = Mock::new("normal");
        let b = Mock::new("normal");
        assert!(
            run_config(
                "session",
                "enable-goal",
                &json!({}),
                &json!({"sockets":[a.path,b.path]})
            )
            .is_err()
        );
        assert!(!a.methods().contains(&"thread/goal/set".into()));
        assert!(!b.methods().contains(&"thread/goal/set".into()));
    }
    #[test]
    fn no_saved_goal_and_wrong_identity_never_mutate() {
        for mode in ["no-goal", "wrong"] {
            let mock = Mock::new(mode);
            assert!(run_config("session", "enable-goal", &json!({}), &mock.config()).is_err());
            assert!(!mock.methods().contains(&"thread/goal/set".into()));
        }
    }
    #[test]
    fn steering_requires_exact_active_turn_and_acknowledgement() {
        let mock = Mock::new("normal");
        assert!(
            run_config(
                "session",
                "steer",
                &json!({"text":"Focus","expectedTurnId":"stale"}),
                &mock.config()
            )
            .is_err()
        );
        assert!(!mock.methods().contains(&"turn/steer".into()));
        assert_eq!(
            run_config(
                "session",
                "steer",
                &json!({"text":"Focus","expectedTurnId":"turn"}),
                &mock.config()
            )
            .unwrap()["ok"],
            true
        );
        let conflict = Mock::new("conflict");
        assert!(
            run_config(
                "session",
                "steer",
                &json!({"text":"Focus","expectedTurnId":"turn"}),
                &conflict.config()
            )
            .unwrap_err()
            .to_string()
            .contains("turn changed")
        );
    }
    #[test]
    fn disconnect_and_timeout_are_bounded_without_retries() {
        let mock = Mock::new("disconnect");
        assert!(run_config("session", "enable-goal", &json!({}), &mock.config()).is_err());
        assert_eq!(
            mock.methods()
                .iter()
                .filter(|m| m.as_str() == "thread/goal/set")
                .count(),
            1
        );
        let mock = Mock::new("timeout");
        let mut c = Connection::open(&mock.path).unwrap();
        c.deadline = Instant::now() + Duration::from_millis(100);
        let start = Instant::now();
        assert!(c.owns("session").is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(c);
    }
}
