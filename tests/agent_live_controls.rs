//! Opt-in control checks against authenticated, installed CLIs. All file/tool
//! effects are confined to a fresh scratch directory.
use hey_boss::agent_runtime::{AgentSession, Event, Launch, Provider, TurnStatus};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn scratch(provider: Provider) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "hey-boss-live-controls-{}-{}-{}",
        provider.name(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    eprintln!("SCRATCH {}", root.display());
    root
}
fn launch(provider: Provider, root: &Path) -> AgentSession {
    AgentSession::launch(Launch {
        codex_execution: Default::default(),
        model: None,
        provider,
        binary: None,
        cwd: root.into(),
        resume: None,
        env: BTreeMap::new(),
        output_schema: None,
    })
    .unwrap()
}
fn event(agent: &mut AgentSession, deadline: Instant) -> Event {
    loop {
        assert!(
            Instant::now() < deadline,
            "Timed out; state {:?}",
            agent.state()
        );
        if let Some(event) = agent.receive(Duration::from_millis(100)).unwrap() {
            if !matches!(event, Event::TextDelta { .. } | Event::Other(_)) {
                eprintln!("EVENT {event:?}");
            }
            return event;
        }
    }
}
fn completed(agent: &mut AgentSession, approve: bool) -> (String, TurnStatus, String) {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        match event(agent, deadline) {
            Event::Approval { id, .. } => agent.decide(&id, approve).unwrap(),
            Event::TurnCompleted {
                id, status, output, ..
            } => return (id, status, output),
            _ => {}
        }
    }
}
fn running_tool(agent: &mut AgentSession, root: &Path, started: &str) {
    let deadline = Instant::now() + Duration::from_secs(180);
    while !root.join(started).exists() {
        assert!(Instant::now() < deadline, "Timed tool did not start");
        if let Some(event) = agent.receive(Duration::from_millis(100)).unwrap() {
            if !matches!(event, Event::TextDelta { .. } | Event::Other(_)) {
                eprintln!("EVENT {event:?}");
            }
            match event {
                Event::Approval { id, .. } => agent.decide(&id, true).unwrap(),
                Event::TurnCompleted { .. } => panic!("Completed without starting timed tool"),
                _ => {}
            }
        }
    }
}
fn controls(provider: Provider, root: &Path) {
    let mut agent = launch(provider, root);
    let turn = agent.prompt("Use the bash tool to run exactly: printf started > steer-started; sleep 8. Wait for that tool, then respond ORIGINAL_DONE. Run in the foreground. Do not use any other tools.", None).unwrap();
    running_tool(&mut agent, root, "steer-started");
    assert!(agent.steer("stale-turn", "wrong instruction").is_err());
    let delivery = agent.steer(&turn, "After the current tool finishes, write steering.txt containing exactly STEERED. Then reply STEERED_DONE. Use the file write tool; do not run more bash commands.").unwrap();
    assert_eq!(delivery, provider.capabilities().steering);
    let (_, status, output) = completed(&mut agent, true);
    assert_eq!(status, TurnStatus::Completed);
    let output = if provider == Provider::Claude {
        assert!(
            output.contains("ORIGINAL_DONE"),
            "Next-turn steering was folded into current result: {output}"
        );
        let (next, status, output) = completed(&mut agent, true);
        assert_ne!(turn, next);
        assert_eq!(status, TurnStatus::Completed);
        output
    } else {
        output
    };
    assert!(output.contains("STEERED_DONE"), "{output}");
    assert_eq!(
        fs::read_to_string(root.join("steering.txt"))
            .unwrap()
            .trim(),
        "STEERED"
    );
    assert!(agent.steer(&turn, "stale after completion").is_err());
    eprintln!("PASS {} live steering", provider.name());

    agent.prompt("Use the bash tool to run exactly: printf started > interrupt-started; sleep 30; printf BAD > interrupted-finished. Run in the foreground. Do not use any other tools.", None).unwrap();
    running_tool(&mut agent, root, "interrupt-started");
    let started = Instant::now();
    agent.interrupt().unwrap();
    let (_, status, _) = completed(&mut agent, false);
    assert_eq!(status, TurnStatus::Interrupted);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "Interrupt was not prompt"
    );
    assert!(!root.join("interrupted-finished").exists());
    assert!(agent.inspect().unwrap().turn.is_none());
    agent
        .prompt("Reply RECOVERED only. Do not use any tools.", None)
        .unwrap();
    let (_, status, output) = completed(&mut agent, false);
    assert_eq!(status, TurnStatus::Completed);
    assert!(output.contains("RECOVERED"));
    // Keep the session alive beyond the original sleep deadline: stopping the
    // process earlier would hide a tool that survived the native interrupt.
    while started.elapsed() < Duration::from_secs(32) {
        if let Some(event) = agent.receive(Duration::from_millis(100)).unwrap() {
            assert!(
                !matches!(event, Event::TurnStarted { .. } | Event::ToolStarted { .. }),
                "Work restarted after interrupt: {event:?}"
            );
        }
    }
    assert!(
        !root.join("interrupted-finished").exists(),
        "Interrupted tool continued running"
    );
    eprintln!(
        "PASS {} live interruption and post-interrupt recovery",
        provider.name()
    );

    let turn = agent.prompt("Use the bash tool to run exactly: printf started > queue-started; sleep 30; printf BAD > queue-finished. Run in the foreground. Do not use other tools.", None).unwrap();
    running_tool(&mut agent, root, "queue-started");
    let started = Instant::now();
    agent
        .steer(
            &turn,
            "Use the write tool to create queued.txt containing BAD. Then reply BAD.",
        )
        .unwrap();
    agent.interrupt().unwrap();
    assert_eq!(completed(&mut agent, false).1, TurnStatus::Interrupted);
    assert!(!root.join("queued.txt").exists());
    assert!(agent.state().turn.is_none());
    agent.stop().unwrap();
    while started.elapsed() < Duration::from_secs(32) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !root.join("queue-finished").exists(),
        "Owned foreground tool survived queue cancellation"
    );
    assert!(
        !root.join("queued.txt").exists(),
        "Queued instruction restarted after shutdown"
    );
    eprintln!(
        "PASS {} interrupt discards queued steering",
        provider.name()
    );
}

#[test]
#[ignore = "real authenticated Codex CLI; invokes models and scratch-only tools"]
fn real_codex_steering_and_interruption() {
    let root = scratch(Provider::Codex);
    controls(Provider::Codex, &root);
}

#[test]
#[ignore = "real authenticated Codex CLI; invokes models and scratch-only tools"]
fn real_codex_stop_and_drop_cancel_foreground_tools() {
    for stop in [true, false] {
        let root = scratch(Provider::Codex);
        let mut agent = launch(Provider::Codex, &root);
        agent.prompt("Use the bash tool to run exactly: printf started > shutdown-started; sleep 10; printf BAD > shutdown-finished. Run in the foreground. Do not use other tools.", None).unwrap();
        running_tool(&mut agent, &root, "shutdown-started");
        let started = Instant::now();
        if stop {
            agent.stop().unwrap();
            while started.elapsed() < Duration::from_secs(12) {
                std::thread::sleep(Duration::from_millis(100));
            }
            assert!(agent.state().stopped);
        } else {
            drop(agent);
            while started.elapsed() < Duration::from_secs(12) {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        assert!(
            !root.join("shutdown-finished").exists(),
            "Codex tool survived {}",
            if stop { "stop" } else { "drop" }
        );
        eprintln!("PASS Codex owned tool shutdown with stop={stop}");
    }
}

#[test]
#[ignore = "real authenticated Claude CLI; invokes models and scratch-only tools"]
fn real_claude_steering_interruption_and_approvals() {
    let root = scratch(Provider::Claude);
    fs::create_dir_all(root.join(".claude")).unwrap();
    fs::write(
        root.join(".claude/settings.local.json"),
        json!({"permissions":{"ask":["Bash", "Write", "Edit"]}}).to_string(),
    )
    .unwrap();
    controls(Provider::Claude, &root);
    let mut agent = launch(Provider::Claude, &root);
    for (name, allow) in [("denied.txt", false), ("allowed.txt", true)] {
        agent.prompt(&format!("Use the Write tool once to create {name} with content APPROVAL_TEST. If permission is denied, do not retry, do not use alternative tools, and reply DENIED. If it succeeds, reply ALLOWED."), None).unwrap();
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if let Event::Approval { id, payload, .. } = event(&mut agent, deadline) {
                assert_eq!(payload["request"]["tool_name"], "Write");
                assert!(agent.decide("not-pending", true).is_err());
                assert!(!root.join(name).exists(), "Write occurred before approval");
                agent.decide(&id, allow).unwrap();
                assert!(agent.decide(&id, !allow).is_err());
                break;
            }
        }
        let (_, status, _) = completed(&mut agent, false);
        assert_eq!(status, TurnStatus::Completed);
        assert_eq!(root.join(name).exists(), allow);
        if allow {
            assert_eq!(
                fs::read_to_string(root.join(name)).unwrap().trim(),
                "APPROVAL_TEST"
            );
        }
        eprintln!("PASS live Claude approval {allow}");
    }
    agent.stop().unwrap();
}

#[test]
#[ignore = "real authenticated Pi CLI; invokes models and scratch-only tools"]
fn real_pi_steering_and_interruption() {
    controls(Provider::Pi, &scratch(Provider::Pi));
}

#[test]
#[ignore = "requires installed Pi with its bundled example extensions; no model calls"]
fn real_pi_extension_dialogs() {
    let binary = Provider::Pi.binary().unwrap().canonicalize().unwrap();
    let examples = binary
        .ancestors()
        .map(|dir| dir.join("examples/extensions"))
        .find(|dir| dir.join("rpc-demo.ts").is_file())
        .expect("install Pi with its example extensions");
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let extensions = root.path().join("extensions");
    fs::create_dir(&extensions).unwrap();
    for example in ["rpc-demo.ts", "timed-confirm.ts"] {
        fs::copy(examples.join(example), extensions.join(example)).unwrap();
    }
    let mut agent = AgentSession::launch(Launch {
        codex_execution: Default::default(),
        model: None,
        provider: Provider::Pi,
        binary: Some(binary),
        cwd: root.path().into(),
        resume: None,
        env: [
            ("PI_CODING_AGENT_DIR".into(), root.path().into()),
            ("HOME".into(), root.path().into()),
            ("PI_OFFLINE".into(), "1".into()),
        ]
        .into(),
        output_schema: None,
    })
    .unwrap();
    assert!(!agent.capabilities().tool_approvals);
    for (command, method, answer, notification) in [
        (
            "/timed-select",
            "select",
            Some("Option B"),
            "Selected: Option B",
        ),
        ("/timed", "confirm", Some("true"), "Confirmed by user!"),
        ("/timed", "confirm", Some("false"), "Cancelled or timed out"),
        (
            "/rpc-input",
            "input",
            Some("typed answer"),
            "You entered: typed answer",
        ),
        ("/rpc-editor", "editor", None, "Editor cancelled"),
        (
            "/timed-select",
            "select",
            None,
            "Selection cancelled or timed out",
        ),
    ] {
        let turn = agent.prompt(command, None).unwrap();
        assert!(agent.state().awaiting_prompt_ack);
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut answered = false;
        let mut notified = false;
        loop {
            match event(&mut agent, deadline) {
                Event::Input { id, payload } => {
                    assert!(!answered);
                    assert_eq!(payload["method"], method);
                    assert!(agent.respond_input("unknown-input", answer).is_err());
                    if method == "select" {
                        assert!(agent.respond_input(&id, Some("invalid")).is_err());
                    }
                    agent.respond_input(&id, answer).unwrap();
                    assert!(agent.respond_input(&id, answer).is_err());
                    answered = true;
                }
                Event::Other(v) if v["method"] == "notify" => {
                    assert_eq!(v["message"], notification);
                    notified = true;
                }
                Event::TurnCompleted { id, status, .. } => {
                    assert_eq!(id, turn);
                    assert_eq!(status, TurnStatus::Completed);
                    assert!(answered && notified);
                    break;
                }
                _ => {}
            }
        }
        assert!(!agent.state().awaiting_prompt_ack);
    }
    agent.prompt("/rpc-input", None).unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    let pending = loop {
        if let Event::Input { id, .. } = event(&mut agent, deadline) {
            break id;
        }
    };
    agent.interrupt().unwrap();
    assert_eq!(completed(&mut agent, false).1, TurnStatus::Interrupted);
    assert!(agent.state().stopped);
    assert!(agent.respond_input(&pending, Some("late")).is_err());
}
