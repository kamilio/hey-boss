//! Opt-in transport verification; creates an isolated thread, never starts a turn.
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tungstenite::Message;
struct Server {
    child: Child,
    root: PathBuf,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
#[test]
#[ignore = "requires a local Codex executable; run explicitly with CODEX_TEST_BIN"]
fn actual_unix_server_ownership_and_goal_inspection() {
    let codex = std::env::var("CODEX_TEST_BIN").expect("CODEX_TEST_BIN");
    let root = std::env::temp_dir().join(format!("hb-real-{}", std::process::id()));
    std::fs::create_dir_all(root.join("codex")).unwrap();
    let path = root.join("rpc.sock");
    let child = Command::new(codex)
        .args([
            "app-server",
            "--listen",
            &format!("unix://{}", path.display()),
        ])
        .env("CODEX_HOME", root.join("codex"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let server = Server { child, root };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < deadline, "server startup timeout");
        std::thread::sleep(Duration::from_millis(20));
    }
    let stream = UnixStream::connect(&path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let (mut ws, _) = tungstenite::client("ws://localhost/rpc", stream).unwrap();
    let mut sequence = 0;
    fn rpc(
        ws: &mut tungstenite::WebSocket<UnixStream>,
        sequence: &mut i32,
        method: &str,
        params: Value,
    ) -> Value {
        *sequence += 1;
        ws.send(Message::Text(
            json!({"id":sequence,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .unwrap();
        loop {
            if let Message::Text(text) = ws.read().unwrap() {
                let v: Value = serde_json::from_str(&text).unwrap();
                if v.get("id") == Some(&json!(sequence)) {
                    assert!(v.get("error").is_none(), "{v}");
                    return v["result"].clone();
                }
            }
        }
    }
    rpc(
        &mut ws,
        &mut sequence,
        "initialize",
        json!({"clientInfo":{"name":"hey_boss_isolated_audit","version":"0.1.0"},"capabilities":{"experimentalApi":true}}),
    );
    ws.send(Message::Text(
        json!({"method":"initialized","params":{}})
            .to_string()
            .into(),
    ))
    .unwrap();
    let started = rpc(
        &mut ws,
        &mut sequence,
        "thread/start",
        json!({"cwd":server.root,"approvalPolicy":"never","sandbox":"read-only"}),
    );
    let thread = started["thread"]["id"].as_str().unwrap();
    let config = server.root.join(".config/hey-boss");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("agent-control.json"),
        json!({"sockets":[path]}).to_string(),
    )
    .unwrap();
    let mut cli = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args(["agent-control", "--thread", thread, "inspect"])
        .env("HOME", &server.root)
        .env("CODEX_HOME", server.root.join("codex"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    cli.stdin.take().unwrap().write_all(b"{}").unwrap();
    let output = cli.wait_with_output().unwrap();
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["threadId"], thread);
    assert!(result["goal"].is_null());
    assert_eq!(result["canSteer"], false);
}
