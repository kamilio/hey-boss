//! Opt-in control checks against authenticated, installed CLIs. All file/tool
//! effects are confined to a fresh scratch directory.
use hey_boss::agent_runtime::{
    AgentSession, Event, Launch, Provider, SteeringDelivery, TurnStatus,
};
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
fn launch(provider: Provider, root: &Path, binary: Option<PathBuf>) -> AgentSession {
    AgentSession::launch(Launch {
        provider,
        binary,
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
fn controls(provider: Provider, root: &Path, binary: Option<PathBuf>) {
    let mut agent = launch(provider, root, binary);
    let turn = agent.prompt("Use the bash tool to run exactly: printf started > steer-started; sleep 8. Wait for that tool, then respond ORIGINAL_DONE. Run in the foreground. Do not use any other tools.", None).unwrap();
    running_tool(&mut agent, root, "steer-started");
    assert!(agent.steer("stale-turn", "wrong instruction").is_err());
    let delivery = agent.steer(&turn, "After the current tool finishes, write steering.txt containing exactly STEERED. Then reply STEERED_DONE. Use the file write tool; do not run more bash commands.").unwrap();
    assert_eq!(
        delivery,
        if provider == Provider::Claude {
            SteeringDelivery::NextTurn
        } else {
            SteeringDelivery::BeforeNextModelCall
        }
    );
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

    let turn = agent.prompt("Use the bash tool to run exactly: printf started > queue-started; sleep 30. Run in the foreground. Do not use other tools.", None).unwrap();
    running_tool(&mut agent, root, "queue-started");
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
    eprintln!(
        "PASS {} interrupt discards queued steering",
        provider.name()
    );
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
    controls(Provider::Claude, &root, None);
    let mut agent = launch(Provider::Claude, &root, None);
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
#[ignore = "real authenticated Pi CLI; invokes models and scratch-only tools/extension UI"]
fn real_pi_steering_interruption_and_extension_input() {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch(Provider::Pi);
    let extension = root.join("live-dialogs.ts");
    fs::write(&extension, r#"import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
export default function(pi) {
  pi.on('before_agent_start', async (event, ctx) => {
    if (!event.prompt.startsWith('HB_DIALOG')) return;
    const selected = await ctx.ui.select('Live select', ['one', 'two']);
    if (event.prompt.startsWith('HB_DIALOG_CANCEL')) {
      writeFileSync(join(ctx.cwd, 'dialog-cancel.json'), JSON.stringify({cancelled: selected === undefined}));
      return;
    }
    const confirmed = await ctx.ui.confirm('Live confirm', 'Approve scratch operation?');
    const typed = await ctx.ui.input('Live input', 'type here');
    const edited = await ctx.ui.editor('Live editor', 'initial');
    writeFileSync(join(ctx.cwd, 'dialog-results.json'), JSON.stringify({selected, confirmed, typed, editorCancelled: edited === undefined}));
  });
}
"#).unwrap();
    let wrapper = root.join("real-pi");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexec '{}' -e '{}' \"$@\"\n",
            Provider::Pi.binary().unwrap().display(),
            extension.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    controls(Provider::Pi, &root, Some(wrapper.clone()));
    let mut agent = launch(Provider::Pi, &root, Some(wrapper));
    agent
        .prompt("HB_DIALOG: Reply DIALOG_DONE only; do not use tools.", None)
        .unwrap();
    assert!(agent.state().awaiting_prompt_ack);
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut methods = Vec::new();
    loop {
        match event(&mut agent, deadline) {
            Event::Input { id, payload } => {
                let method = payload["method"].as_str().unwrap();
                methods.push(method.to_owned());
                let answer = match method {
                    "select" => {
                        assert!(agent.respond_input(&id, Some("invalid")).is_err());
                        Some("two")
                    }
                    "confirm" => Some("false"),
                    "input" => Some("typed answer"),
                    "editor" => None,
                    _ => panic!("Unexpected dialog {method}"),
                };
                assert!(agent.respond_input("unknown-input", Some("two")).is_err());
                agent.respond_input(&id, answer).unwrap();
                assert!(agent.respond_input(&id, answer).is_err());
            }
            Event::TurnCompleted { status, .. } => {
                assert_eq!(status, TurnStatus::Completed);
                break;
            }
            _ => {}
        }
    }
    assert_eq!(methods, ["select", "confirm", "input", "editor"]);
    let results: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("dialog-results.json")).unwrap()).unwrap();
    assert_eq!(
        results,
        json!({"selected":"two", "confirmed":false, "typed":"typed answer", "editorCancelled":true})
    );
    assert!(!agent.state().awaiting_prompt_ack);
    agent
        .prompt(
            "HB_DIALOG_CANCEL: Reply CANCEL_DONE only; do not use tools.",
            None,
        )
        .unwrap();
    loop {
        if let Event::Input { id, .. } = event(&mut agent, deadline) {
            agent.respond_input(&id, None).unwrap();
            break;
        }
    }
    assert_eq!(completed(&mut agent, false).1, TurnStatus::Completed);
    let results: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("dialog-cancel.json")).unwrap()).unwrap();
    assert_eq!(results, json!({"cancelled":true}));
    assert!(!agent.capabilities().tool_approvals);
    agent
        .prompt(
            "HB_DIALOG_PENDING: Reply MUST_NOT_RUN only; do not use tools.",
            None,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let pending = loop {
        if let Event::Input { id, .. } = event(&mut agent, deadline) {
            break id;
        }
    };
    agent.interrupt().unwrap();
    assert_eq!(completed(&mut agent, false).1, TurnStatus::Interrupted);
    assert!(agent.state().stopped);
    assert!(agent.respond_input(&pending, Some("two")).is_err());
    eprintln!("PASS live Pi pending-dialog interruption");
    agent.stop().unwrap();
    eprintln!(
        "PASS live Pi select, confirm, input, editor, invalid/repeated responses and cancellation"
    );
}
