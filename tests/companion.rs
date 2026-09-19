use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

struct Broker(Child);
impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start(state: &Path) -> Broker {
    // Mock bridges represent a protocol-negotiated connection from current Mac CLI.
    std::fs::create_dir_all(state).unwrap();
    std::fs::write(state.join("bridge-protocol"), "1").unwrap();
    let broker = Broker(
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(["companion", "serve", "--state"])
            .arg(state)
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(state.join("daemon.sock")).is_err() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    broker
}
fn call(state: &Path, request: serde_json::Value) -> serde_json::Value {
    let mut stream = UnixStream::connect(state.join("daemon.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
#[test]
fn overview_controls_are_forwarded_and_never_queued() {
    let state = std::env::temp_dir().join(format!("hb-overview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let broker = start(&state);
    assert_eq!(
        call(
            &state,
            serde_json::json!({"command":"overview_snapshot","sync":false})
        )["status"],
        "error"
    );
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    let peer = std::thread::spawn(move || {
        loop {
            let (mut stream, _) = bridge.accept().unwrap();
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let control = request["command"] == "overview_snapshot";
            let response = if control {
                serde_json::json!({"task_id":"overview","status":"ok","result":"{\"rows\":[]}"})
            } else {
                assert_eq!(request["command"], "agents_snapshot");
                serde_json::json!({"task_id":"agents","status":"ok"})
            };
            stream
                .write_all(&serde_json::to_vec(&response).unwrap())
                .unwrap();
            if control {
                break;
            }
        }
    });
    let result = call(
        &state,
        serde_json::json!({"command":"overview_snapshot","sync":false}),
    );
    peer.join().unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["result"], "{\"rows\":[]}");
    assert_eq!(std::fs::read_dir(state.join("queue")).unwrap().count(), 0);
    drop(broker);
    std::fs::remove_dir_all(state).unwrap();
}
#[test]
fn desktop_actions_fail_offline_and_stamp_connection_identity_without_queueing() {
    let state = std::env::temp_dir().join(format!("hb-desktop-actions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let broker = start(&state);
    let action = serde_json::json!({"command":"action","sync":false,"question":"{}","bridge_host":"spoof","bridge_generation":"spoof"});
    let began = Instant::now();
    let offline = call(&state, action.clone());
    assert_eq!(offline["status"], "error");
    assert!(offline["error"].as_str().unwrap().contains("not connected"));
    assert!(began.elapsed() < Duration::from_secs(2));
    assert_eq!(std::fs::read_dir(state.join("queue")).unwrap().count(), 0);
    std::fs::write(state.join("bridge-host"), "devbox").unwrap();
    std::fs::write(state.join("bridge-generation"), "connection-1").unwrap();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    let peer = std::thread::spawn(move || {
        loop {
            let (mut stream, _) = bridge.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let is_action = request["command"] == "action";
            if is_action {
                assert_eq!(request["bridge_host"], "devbox");
                assert_eq!(request["bridge_generation"], "connection-1");
            } else {
                assert_eq!(request["command"], "agents_snapshot");
            }
            stream
                .write_all(br#"{"task_id":"test","status":"ok"}"#)
                .unwrap();
            if is_action {
                break;
            }
        }
    });
    assert_eq!(call(&state, action)["status"], "ok");
    peer.join().unwrap();
    assert_eq!(std::fs::read_dir(state.join("queue")).unwrap().count(), 0);
    drop(broker);
    std::fs::remove_dir_all(state).unwrap();
}
#[test]
fn replayed_question_hide_is_forwarded_and_cancellation_survives_disconnect() {
    replay_question_control("hide");
}
#[test]
fn replayed_question_wait_maps_unicode_answer_and_caches_it_after_disconnect() {
    replay_question_control("wait");
}
fn bridge_action(bridge: &UnixListener) -> (UnixStream, serde_json::Value) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let (mut stream, _) = match bridge.accept() {
            Ok(pair) => pair,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < until, "Bridge action timed out");
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) => panic!("{error}"),
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if request["command"] == "agents_snapshot" {
            stream
                .write_all(br#"{"task_id":"agents","status":"ok"}"#)
                .unwrap();
            continue;
        }
        return (stream, request);
    }
}
fn replay_question_control(control: &'static str) {
    let state = std::env::temp_dir().join(format!("hb-{}-{control}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let broker = start(&state);
    assert_eq!(
        call(&state, serde_json::json!({"command":"ask","sync":false}))["status"],
        "error"
    );
    let request = serde_json::json!({"command":"ask","question":"Ready?","project":"Test","title":"Queue","sync":false});
    let queued = call(&state, request);
    assert_eq!(queued["status"], "pending");
    let id = queued["task_id"].as_str().unwrap();
    drop(broker);
    std::fs::remove_file(state.join("daemon.sock")).unwrap();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let broker = start(&state);
    let (mut stream, replay) = bridge_action(&bridge);
    assert_eq!(replay["question"], "Ready?");
    assert!(!replay["source_host"].as_str().unwrap().is_empty());
    assert_eq!(replay["sync"], false);
    stream
        .write_all(br#"{"task_id":"mac-question","status":"pending"}"#)
        .unwrap();
    drop(stream);
    let answer = std::thread::spawn(move || {
        let (mut stream, action) = bridge_action(&bridge);
        assert_eq!(
            action["command"],
            if control == "wait" { "status" } else { "hide" }
        );
        assert_eq!(action["task_id"], "mac-question");
        let response = if control == "wait" {
            serde_json::json!({"task_id":"mac-question","status":"ok","result":"Résumé α"})
        } else {
            serde_json::json!({"task_id":"mac-question","status":"cancelled"})
        };
        stream
            .write_all(&serde_json::to_vec(&response).unwrap())
            .unwrap();
    });
    let result = call(
        &state,
        serde_json::json!({"command":control,"task_id":id,"sync":false}),
    );
    answer.join().unwrap();
    assert_eq!(result["task_id"], id);
    assert_eq!(
        result["status"],
        if control == "wait" { "ok" } else { "cancelled" }
    );
    if control == "wait" {
        assert_eq!(result["result"], "Résumé α");
    } else {
        assert!(result.get("result").is_none());
    }
    std::fs::remove_file(state.join("bridge.sock")).unwrap();
    drop(broker);
    std::fs::remove_file(state.join("daemon.sock")).unwrap();
    let broker = start(&state);
    assert_eq!(
        call(
            &state,
            serde_json::json!({"command":"wait","task_id":id,"sync":false})
        ),
        result
    );
    drop(broker);
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn rejected_legacy_item_does_not_block_later_notifications() {
    let state = std::env::temp_dir().join(format!("hb-rejected-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(state.join("queue")).unwrap();
    let bad = state.join("queue/remote-1-1.json");
    std::fs::write(&bad, serde_json::to_vec(&serde_json::json!({"request":{"command":"alert","sync":false},"upstream":null,"terminal":null})).unwrap()).unwrap();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let peer = std::thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(8);
        let mut creations = 0;
        while creations < 2 {
            let mut stream = match bridge.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < until, "Replay stalled");
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let response = if request["command"] == "agents_snapshot" {
                serde_json::json!({"task_id":"agents","status":"ok"})
            } else {
                creations += 1;
                if request["project"].is_null() {
                    serde_json::json!({"status":"error","error":"Missing project"})
                } else {
                    serde_json::json!({"task_id":"good-mac-task","status":"pending"})
                }
            };
            stream
                .write_all(&serde_json::to_vec(&response).unwrap())
                .unwrap();
        }
    });
    let broker = start(&state);
    let queued = call(
        &state,
        serde_json::json!({"command":"alert","project":"Test","title":"Later","question":"Healthy item","sync":false}),
    );
    peer.join().unwrap();
    assert_eq!(
        call(
            &state,
            serde_json::json!({"command":"status","task_id":"remote-1-1","sync":false})
        )["status"],
        "error"
    );
    let good = state
        .join("queue")
        .join(format!("{}.json", queued["task_id"].as_str().unwrap()));
    let entry: serde_json::Value = serde_json::from_slice(&std::fs::read(good).unwrap()).unwrap();
    assert_eq!(entry["upstream"], "good-mac-task");
    drop(broker);
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn queued_question_can_be_cancelled_offline_and_stays_cancelled_after_restart() {
    let state = std::env::temp_dir().join(format!("hb-offline-cancel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let broker = start(&state);
    let queued = call(
        &state,
        serde_json::json!({"command":"ask","question":"Proceed?","project":"Test","title":"Cancel","sync":false}),
    );
    let id = queued["task_id"].as_str().unwrap();
    let cancelled = call(
        &state,
        serde_json::json!({"command":"hide","task_id":id,"sync":false}),
    );
    assert_eq!(cancelled["status"], "cancelled");
    assert!(cancelled.get("result").is_none());
    drop(broker);
    let broker = start(&state);
    assert_eq!(
        call(
            &state,
            serde_json::json!({"command":"wait","task_id":id,"sync":false})
        ),
        cancelled
    );
    let entry: serde_json::Value = serde_json::from_slice(
        &std::fs::read(state.join("queue").join(format!("{id}.json"))).unwrap(),
    )
    .unwrap();
    assert!(entry["upstream"].is_null());
    assert_eq!(entry["terminal"]["status"], "cancelled");
    drop(broker);
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn stalled_replay_does_not_block_queueing_and_racing_hide_is_delivered() {
    let state = std::env::temp_dir().join(format!("hb-stall-{}", std::process::id()));
    let broker = start(&state);
    let queued = call(
        &state,
        serde_json::json!({"command":"ask","project":"Test","title":"Race","question":"Proceed?","sync":false}),
    );
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let (mut replay, request) = bridge_action(&bridge);
    assert_eq!(request["command"], "ask");
    let start = Instant::now();
    let next = call(
        &state,
        serde_json::json!({"command":"alert","project":"Test","title":"Next","question":"Saved","sync":false}),
    );
    assert_eq!(next["status"], "pending");
    assert!(start.elapsed() < Duration::from_secs(2));
    let cancelled = call(
        &state,
        serde_json::json!({"command":"hide","task_id":queued["task_id"],"sync":false}),
    );
    assert_eq!(cancelled["status"], "cancelled");
    replay
        .write_all(br#"{"task_id":"racing-question","status":"pending"}"#)
        .unwrap();
    drop(replay);
    loop {
        let (mut stream, action) = bridge_action(&bridge);
        if action["command"] == "hide" {
            assert_eq!(action["task_id"], "racing-question");
            stream
                .write_all(br#"{"task_id":"racing-question","status":"cancelled"}"#)
                .unwrap();
            break;
        }
        assert_eq!(action["command"], "alert");
        stream
            .write_all(br#"{"task_id":"next","status":"pending"}"#)
            .unwrap();
    }
    drop(broker);
    std::fs::remove_dir_all(state).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn invalid_autoconnect_config_reports_error_and_recovers_without_relaunch() {
    let root = std::env::temp_dir().join(format!("hb-config-{}", std::process::id()));
    let state = root.join(".local/share/hey-boss");
    let config = root.join(".hey-boss/config.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, r#"{"ssh_hosts":[}"#).unwrap();
    let mut connector = Broker(
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(["companion", "auto"])
            .env("HOME", &root)
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(6);
    loop {
        let status = std::fs::read(state.join("connections.json"))
            .ok()
            .and_then(|d| serde_json::from_slice::<serde_json::Value>(&d).ok());
        if status.is_some_and(|s| s["config_error"].is_string()) {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(connector.0.try_wait().unwrap().is_none());
    std::fs::write(
        &config,
        r#"{"ssh_hosts":[{"host":"devbox","enabled":false}]}"#,
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(7);
    loop {
        let status: serde_json::Value =
            serde_json::from_slice(&std::fs::read(state.join("connections.json")).unwrap())
                .unwrap();
        if status["config_error"].is_null() && status["machines"].as_array().unwrap().is_empty() {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    // Disabled machines make no connections, while the manager keeps watching edits.
    assert!(connector.0.try_wait().unwrap().is_none());
    drop(connector);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn disconnected_waiters_release_client_capacity_while_offline() {
    let state = std::env::temp_dir().join(format!("hb-waiters-{}", std::process::id()));
    let broker = start(&state);
    let question = call(
        &state,
        serde_json::json!({"command":"ask","project":"Test","title":"Wait","question":"Proceed?","sync":false}),
    );
    for _ in 0..2 {
        let mut clients = Vec::new();
        for _ in 0..64 {
            let mut stream = UnixStream::connect(state.join("daemon.sock")).unwrap();
            stream.write_all(&serde_json::to_vec(&serde_json::json!({"command":"wait","task_id":question["task_id"],"sync":false})).unwrap()).unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            clients.push(stream);
        }
        std::thread::sleep(Duration::from_millis(100));
        drop(clients);
        std::thread::sleep(Duration::from_millis(1000));
    }
    assert_eq!(
        call(
            &state,
            serde_json::json!({"command":"status","task_id":question["task_id"],"sync":false})
        )["status"],
        "pending"
    );
    drop(broker);
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn unnegotiated_legacy_bridge_receives_no_control_commands() {
    let state = std::env::temp_dir().join(format!("hb-legacy-{}", std::process::id()));
    let broker = start(&state);
    std::fs::remove_file(state.join("bridge-protocol")).unwrap();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    assert_eq!(
        call(
            &state,
            serde_json::json!({"command":"overview_snapshot","sync":false})
        )["status"],
        "error"
    );
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        bridge.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(broker);
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn server_markdown_file_is_snapshotted_offline_and_replayed_after_restart() {
    let root = std::env::temp_dir().join(format!("hb-server-markdown-{}", std::process::id()));
    let state = root.join("state");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let executable = bin.join("hey-boss");
    std::fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &executable).unwrap();
    std::fs::write(bin.join("hey-boss.state"), state.to_str().unwrap()).unwrap();
    let markdown = "# Server report 🌍\n\n| Result | Status |\n|---|---|\n| migration | ready |\n\n> [!WARNING]\n> Review the rollout.\n\n```rust\nlet ready = true;\n```";
    let source = root.join("report.md");
    std::fs::write(&source, markdown).unwrap();
    let broker = start(&state);
    let result = Command::new(&executable)
        .args([
            "update",
            "--project",
            "Server migration",
            "--title",
            "Ready",
            "Review the migration",
            "--file",
        ])
        .arg(&source)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let queued: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(queued["status"], "pending");
    std::fs::remove_file(&source).unwrap();
    drop(broker);
    std::fs::remove_file(state.join("daemon.sock")).unwrap();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let restarted = start(&state);
    let (mut stream, replay) = bridge_action(&bridge);
    assert_eq!(replay["command"], "update");
    assert_eq!(replay["question"], markdown);
    assert_eq!(replay["description"], "Review the migration");
    assert!(!replay["source_host"].as_str().unwrap().is_empty());
    assert_ne!(replay["source_host"], "This Mac");
    let rendered = hey_boss::markdown::render_document(replay["question"].as_str().unwrap());
    assert!(
        rendered.contains("<table>")
            && rendered.contains("markdown-alert-warning")
            && rendered.contains("language-rust")
    );
    stream
        .write_all(br#"{"task_id":"mac-markdown","status":"pending"}"#)
        .unwrap();
    drop(stream);
    drop(restarted);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn document_review_status_returns_comments_and_caches_them_offline() {
    let root = std::env::temp_dir().join(format!("hb-review-comments-{}", std::process::id()));
    let state = root.join("state");
    let broker = start(&state);
    let queued = call(
        &state,
        serde_json::json!({"command":"update","project":"Synthetic","title":"Review","description":"Ready","question":"# Review","comments_enabled":true,"sync":false}),
    );
    let id = queued["task_id"].as_str().unwrap().to_owned();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let (mut stream, replay) = bridge_action(&bridge);
    assert_eq!(replay["comments_enabled"], true);
    stream
        .write_all(br#"{"task_id":"mac-review","status":"pending"}"#)
        .unwrap();
    drop(stream);
    let status_state = state.clone();
    let status_id = id.clone();
    let status = std::thread::spawn(move || {
        call(
            &status_state,
            serde_json::json!({"command":"status","task_id":status_id,"sync":false}),
        )
    });
    let (mut stream, action) = bridge_action(&bridge);
    assert_eq!(action["command"], "status");
    let open = serde_json::json!({"task_id":"mac-review","status":"pending","review_status":"open","comments":[{"id":"comment-1","text":"Clarify the rollout 🌍","quote":"selected paragraph","created_at":1.0,"selection":{"line_start":3,"line_end":4,"source_text":"Selected **paragraph**.\nNext line 🌍.\n"}}]});
    stream
        .write_all(&serde_json::to_vec(&open).unwrap())
        .unwrap();
    drop(stream);
    let current = status.join().unwrap();
    assert_eq!(current["comments"][0]["text"], "Clarify the rollout 🌍");
    drop(bridge);
    let offline = call(
        &state,
        serde_json::json!({"command":"status","task_id":id,"sync":false}),
    );
    assert_eq!(offline["comments"], current["comments"]);
    drop(broker);
    std::fs::remove_file(state.join("daemon.sock")).unwrap();
    let restarted = start(&state);
    let durable = call(
        &state,
        serde_json::json!({"command":"status","task_id":id,"sync":false}),
    );
    assert_eq!(durable["comments"], current["comments"]);
    std::fs::remove_file(state.join("bridge.sock")).unwrap();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let wait_state = state.clone();
    let wait_id = id.clone();
    let waiting = std::thread::spawn(move || {
        call(
            &wait_state,
            serde_json::json!({"command":"wait","task_id":wait_id,"sync":false}),
        )
    });
    let (mut stream, action) = bridge_action(&bridge);
    assert_eq!(action["command"], "status");
    let mut finished = open;
    finished["status"] = "ok".into();
    finished["review_status"] = "finished".into();
    stream
        .write_all(&serde_json::to_vec(&finished).unwrap())
        .unwrap();
    drop(stream);
    let terminal = waiting.join().unwrap();
    assert_eq!(terminal["review_status"], "finished");
    assert_eq!(terminal["comments"], current["comments"]);
    drop(bridge);
    assert_eq!(
        call(
            &state,
            serde_json::json!({"command":"status","task_id":id,"sync":false})
        )["comments"],
        current["comments"]
    );
    // Exercise the actual CLI, including default human output: no separate
    // comments command or opt-in on status should be necessary.
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let executable = bin.join("hey-boss");
    std::fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &executable).unwrap();
    std::fs::write(bin.join("hey-boss.state"), state.to_str().unwrap()).unwrap();
    for mode in [None, Some("--async"), Some("--sync")] {
        let mut command = Command::new(&executable);
        command.args(["status", &id]);
        if let Some(flag) = mode {
            command.arg(flag);
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let output = String::from_utf8(result.stdout).unwrap();
        assert!(output.contains("Review: finished"));
        assert!(output.contains("Lines: 3–4"));
        assert!(output.contains("Selected **paragraph**.\nNext line 🌍.\n"));
        assert!(output.contains("On: selected paragraph"));
        assert!(output.contains("Comment: Clarify the rollout 🌍"));
    }
    let client = hey_boss::Client::new(state.join("daemon.sock"));
    let response = client.status(&id);
    let comments = response.comments.unwrap();
    assert_eq!(comments[0].text, "Clarify the rollout 🌍");
    assert_eq!(
        comments[0].selection.as_ref().unwrap().source_text,
        "Selected **paragraph**.\nNext line 🌍.\n"
    );
    drop(restarted);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn server_source_file_is_snapshotted_offline_and_replayed_after_restart() {
    let root = std::env::temp_dir().join(format!("hb-server-source-{}", std::process::id()));
    let state = root.join("state");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let executable = bin.join("hey-boss");
    std::fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &executable).unwrap();
    std::fs::write(bin.join("hey-boss.state"), state.to_str().unwrap()).unwrap();
    let markdown = "// Remote source 🌍\nfn main() { println!(\"ready\"); }\n";
    let source = root.join("main.rs");
    std::fs::write(&source, markdown).unwrap();
    let broker = start(&state);
    let result = Command::new(&executable)
        .args([
            "update",
            "--project",
            "Server migration",
            "--title",
            "Ready",
            "Review the migration",
            "--file",
        ])
        .arg(&source)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let queued: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(queued["status"], "pending");
    std::fs::remove_file(&source).unwrap();
    drop(broker);
    std::fs::remove_file(state.join("daemon.sock")).unwrap();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let restarted = start(&state);
    let (mut stream, replay) = bridge_action(&bridge);
    assert_eq!(replay["command"], "update");
    assert_eq!(replay["document_name"], "main.rs");
    assert!(replay["question"].as_str().unwrap().contains(markdown));
    assert_eq!(replay["description"], "Review the migration");
    assert!(!replay["source_host"].as_str().unwrap().is_empty());
    assert_ne!(replay["source_host"], "This Mac");
    let rendered = hey_boss::markdown::render_document(replay["question"].as_str().unwrap());
    assert!(rendered.contains("language-rust") && rendered.contains("token-keyword"));
    stream
        .write_all(br#"{"task_id":"mac-markdown","status":"pending"}"#)
        .unwrap();
    drop(stream);
    drop(restarted);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn server_image_file_is_snapshotted_offline_and_replayed_after_restart() {
    let root = std::env::temp_dir().join(format!("hb-server-image-{}", std::process::id()));
    let state = root.join("state");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let executable = bin.join("hey-boss");
    std::fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &executable).unwrap();
    std::fs::write(bin.join("hey-boss.state"), state.to_str().unwrap()).unwrap();
    use base64::Engine;
    let image = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aL1sAAAAASUVORK5CYII=").unwrap();
    let source = root.join("preview.png");
    std::fs::write(&source, &image).unwrap();
    let broker = start(&state);
    let result = Command::new(&executable)
        .args([
            "update",
            "--project",
            "Server migration",
            "--title",
            "Ready",
            "Review the migration",
            "--file",
        ])
        .arg(&source)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let queued: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(queued["status"], "pending");
    std::fs::remove_file(&source).unwrap();
    drop(broker);
    std::fs::remove_file(state.join("daemon.sock")).unwrap();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let restarted = start(&state);
    let (mut stream, replay) = bridge_action(&bridge);
    assert_eq!(replay["command"], "update");
    assert_eq!(replay["attachment"]["name"], "preview.png");
    assert_eq!(replay["attachment"]["mime"], "image/png");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(replay["attachment"]["data"].as_str().unwrap())
            .unwrap(),
        image
    );
    assert_ne!(replay["source_host"], "This Mac");
    stream
        .write_all(br#"{"task_id":"mac-markdown","status":"pending"}"#)
        .unwrap();
    drop(stream);
    drop(restarted);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn synchronous_status_and_sdk_receive_open_review_feedback_without_reader_close() {
    let state = std::env::temp_dir().join(format!("hb-feedback-wait-{}", std::process::id()));
    let broker = start(&state);
    let queued = call(
        &state,
        serde_json::json!({"command":"update","project":"Synthetic","title":"Feedback","description":"Fixture","question":"# Review","comments_enabled":true,"sync":false}),
    );
    let id = queued["task_id"].as_str().unwrap().to_owned();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let (mut stream, _) = bridge_action(&bridge);
    stream
        .write_all(br#"{"task_id":"native-feedback","status":"pending"}"#)
        .unwrap();
    drop(stream);
    let client_state = state.clone();
    let client_id = id.clone();
    let waiting = std::thread::spawn(move || {
        hey_boss::Client::new(client_state.join("daemon.sock"))
            .try_wait_for_feedback(&client_id)
            .unwrap()
    });
    let (mut stream, action) = bridge_action(&bridge);
    assert_eq!(action["command"], "status");
    assert_eq!(
        action["sync"], false,
        "Server wait must not leave timed-out upstream waiters"
    );
    stream
        .write_all(br#"{"task_id":"native-feedback","status":"pending","review_status":"open"}"#)
        .unwrap();
    drop(stream);
    let (mut stream, action) = bridge_action(&bridge);
    assert_eq!(action["command"], "status");
    stream.write_all(br#"{"task_id":"native-feedback","status":"pending","review_status":"open","comments":[{"id":"new-feedback","text":"This needs a decision.","created_at":1.0}]}"#).unwrap();
    drop(stream);
    let response = waiting.join().unwrap();
    assert_eq!(response.status.as_deref(), Some("pending"));
    assert_eq!(response.review_status.as_deref(), Some("open"));
    assert_eq!(response.comments.unwrap()[0].text, "This needs a decision.");
    drop(bridge);
    drop(broker);
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn fifo_replay_retains_rejected_updates_and_retries_same_id_after_restart_and_lost_ack() {
    let state = std::env::temp_dir().join(format!("hb-retry-{}", std::process::id()));
    let broker = start(&state);
    let first = call(
        &state,
        serde_json::json!({"command":"update","project":"Offline","title":"First","description":"Finished","question":"# First complete","sync":false}),
    );
    let second = call(
        &state,
        serde_json::json!({"command":"update","project":"Offline","title":"Second","description":"Finished","question":"# Second complete","sync":false}),
    );
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let (mut stream, rejected) = bridge_action(&bridge);
    assert_eq!(rejected["title"], "First");
    assert_eq!(rejected["task_id"], first["task_id"]);
    stream
        .write_all(br#"{"task_id":"retryable","status":"error"}"#)
        .unwrap();
    drop(stream);
    let first_path = state
        .join("queue")
        .join(format!("{}.json", first["task_id"].as_str().unwrap()));
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&first_path).unwrap()).unwrap();
        if !saved["delivery_error"].is_null() {
            assert!(
                saved["terminal"].is_null(),
                "Failed delivery must not become terminal"
            );
            assert!(saved["upstream"].is_null());
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(broker);
    std::fs::remove_file(state.join("daemon.sock")).unwrap();
    let restarted = start(&state);
    let (stream, replay) = bridge_action(&bridge);
    assert_eq!(replay["task_id"], first["task_id"]);
    assert_eq!(replay["question"], rejected["question"]);
    drop(stream); // Receiver could have accepted it; acknowledgment was lost.
    let (mut stream, retry) = bridge_action(&bridge);
    assert_eq!(retry["task_id"], first["task_id"]);
    stream
        .write_all(br#"{"task_id":"accepted-first","status":"pending"}"#)
        .unwrap();
    drop(stream);
    let (mut stream, next) = bridge_action(&bridge);
    assert_eq!(
        next["task_id"], second["task_id"],
        "FIFO must wait for first acknowledgment"
    );
    assert_eq!(next["title"], "Second");
    stream
        .write_all(br#"{"task_id":"accepted-second","status":"pending"}"#)
        .unwrap();
    drop(stream);
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&first_path).unwrap()).unwrap();
        if saved["upstream"] == "accepted-first" {
            assert!(saved["delivery_error"].is_null());
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(restarted);
    std::fs::remove_dir_all(state).unwrap();
}
