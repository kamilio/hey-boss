use std::{
    fs,
    io::{Read, Write},
    os::unix::net::UnixListener,
    process::Command,
};

#[test]
fn terminal_chat_stream_and_exit_status() {
    check_terminal_chat(false);
    check_terminal_chat(true);
}

fn check_terminal_chat(companion: bool) {
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
    if companion {
        fs::write(dir.path().join("hey-boss.companion"), "").unwrap();
        fs::write(dir.path().join("bridge-protocol"), "1").unwrap();
    }
    let socket = UnixListener::bind(dir.path().join(if companion {
        "bridge.sock"
    } else {
        "daemon.sock"
    }))
    .unwrap();
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

#[test]
fn chat_commands_use_the_mac_without_local_fallback_or_queueing() {
    use serde_json::{Value, json};
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };

    let dir = tempfile::Builder::new()
        .prefix(".chats-test-")
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
    let local = UnixListener::bind(dir.path().join("daemon.sock")).unwrap();
    let bridge = UnixListener::bind(dir.path().join("bridge.sock")).unwrap();
    local.set_nonblocking(true).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let id = "5F60A116-5CF2-4AE9-87C1-E3373958223F";
    for companion in [false, true] {
        if companion {
            fs::write(dir.path().join("hey-boss.companion"), "").unwrap();
            fs::write(dir.path().join("bridge-protocol"), "1").unwrap();
        }
        let socket = if companion { &bridge } else { &local };
        for (args, expected, response, stdout, success) in [
            (
                vec!["read", id],
                json!({"command":"chat_read","chat_id":id}),
                json!({"status":"ok","transcript":"You: Hello\n\nHey Boss: Hi"}),
                "You: Hello\n\nHey Boss: Hi\n",
                true,
            ),
            (
                vec!["list", "--archived", "--after", id, "--json"],
                json!({"command":"chat_list","archived":true,"chat_id":id}),
                json!({"status":"ok","chats":[{"id":"next","title":"Saved chat"}],"next":null}),
                "",
                true,
            ),
            (
                vec!["rename", id, "New title"],
                json!({"command":"chat_rename","chat_id":id,"title":"New title"}),
                json!({"status":"ok"}),
                "",
                true,
            ),
            (
                vec!["archive", id],
                json!({"command":"chat_archive","chat_id":id}),
                json!({"status":"ok"}),
                "",
                true,
            ),
            (
                vec!["restore", id],
                json!({"command":"chat_restore","chat_id":id}),
                json!({"status":"ok"}),
                "",
                true,
            ),
            (
                vec!["delete", id],
                json!({"command":"chat_delete","chat_id":id}),
                json!({"status":"ok"}),
                "",
                true,
            ),
            (
                vec!["read", "missing"],
                json!({"command":"chat_read","chat_id":"missing"}),
                json!({"status":"error","error":"Chat not found"}),
                "",
                false,
            ),
        ] {
            let mut child = Command::new(&cli)
                .args(["agent", "chats"])
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut connection = loop {
                if let Ok((connection, _)) = socket.accept() {
                    break connection;
                }
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "CLI exited before sending {args:?}"
                );
                assert!(Instant::now() < deadline, "CLI did not send {args:?}");
                std::thread::sleep(Duration::from_millis(10));
            };
            connection
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            connection.read_to_end(&mut bytes).unwrap();
            let mut expected = expected;
            expected["sync"] = false.into();
            assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), expected);
            serde_json::to_writer(&mut connection, &response).unwrap();
            drop(connection);
            let output = child.wait_with_output().unwrap();
            assert_eq!(output.status.success(), success, "{output:?}");
            if args[0] == "list" {
                assert_eq!(
                    serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                    response
                );
            } else {
                assert_eq!(String::from_utf8_lossy(&output.stdout), stdout);
            }
            if !success {
                assert!(String::from_utf8_lossy(&output.stderr).contains("Chat not found"));
            }
        }
    }
    // Neither a missing/incompatible bridge nor an offline Mac may hit the local
    // broker or save a command for later execution.
    for protocol in [None, Some("2"), Some("1")] {
        match protocol {
            None => fs::remove_file(dir.path().join("bridge-protocol")).unwrap(),
            Some(value) => fs::write(dir.path().join("bridge-protocol"), value).unwrap(),
        }
        if protocol == Some("1") {
            fs::remove_file(dir.path().join("bridge.sock")).unwrap();
        }
        let output = Command::new(&cli)
            .args(["agent", "chats", "delete", id])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("not connected"));
        assert!(local.accept().is_err());
        assert!(bridge.accept().is_err());
        assert!(!dir.path().join("queue").exists());
    }
}
