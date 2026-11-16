use std::{
    fs,
    io::{Read, Write},
    os::unix::net::UnixListener,
    process::Command,
};

#[test]
fn terminal_chat_stream_and_exit_status() {
    let dir = tempfile::Builder::new()
        .prefix(".agent-test-")
        .tempdir_in(".")
        .unwrap();
    let cli = dir.path().join("hey-boss");
    fs::hard_link(env!("CARGO_BIN_EXE_hey-boss"), &cli)
        .or_else(|_| fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &cli).map(|_| ()))
        .unwrap();
    fs::write(
        dir.path().join("hey-boss.state"),
        dir.path()
            .canonicalize()
            .unwrap()
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();
    let socket = UnixListener::bind(dir.path().join("daemon.sock")).unwrap();
    socket.set_nonblocking(true).unwrap();
    for (json, fail) in [(false, false), (true, false), (true, true)] {
        let mut cmd = Command::new(&cli);
        cmd.args(["agent", "exec", "hello"]);
        if json {
            cmd.arg("--json");
        }
        use std::process::Stdio;
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut conn = loop {
            if let Ok((c, _)) = socket.accept() {
                break c;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "CLI exited before sending a turn"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let mut req = String::new();
        conn.read_to_string(&mut req).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&req).unwrap()["question"],
            "hello"
        );
        let mut events = vec![
            r#"{"type":"thread.started","thread_id":"abc"}"#,
            r#"{"type":"turn.started"}"#,
        ];
        if fail {
            events.push(r#"{"type":"turn.failed","error":{"message":"offline"}}"#);
        } else {
            events.extend([r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"hello back"}}"#,r#"{"type":"turn.completed","usage":null}"#]);
        }
        let wire = events.join("\n") + "\n";
        conn.write_all(wire.as_bytes()).unwrap();
        drop(conn);
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.success(), !fail);
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            if json { wire } else { "hello back\n".into() }
        );
    }
}
