use hey_boss::agent_runtime::{AgentSession, GoalStatus, Launch, ManagedGoal, Provider};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

#[test]
fn goals_continue_without_a_report_and_preserve_pause_and_resume() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut session = AgentSession::launch(Launch {
            provider,
            binary: Some(root.join("tests/fixtures/agent-runtime.mjs")),
            cwd: root,
            resume: None,
            env: BTreeMap::from([("HEY_BOSS_FIXTURE_PROVIDER".into(), provider.name().into())]),
            output_schema: None,
        })
        .unwrap();
        let mut goal = ManagedGoal::new("goal fixture").unwrap();
        goal.start(&mut session).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while goal.status() == GoalStatus::Active {
            if let Some(event) = session.receive(Duration::from_millis(50)).unwrap() {
                goal.observe(&mut session, &event).unwrap();
            }
            assert!(Instant::now() < deadline);
        }
        assert_eq!(goal.status(), GoalStatus::Complete);
        assert_eq!(goal.turns_completed(), 2);
        assert_eq!(goal.summary(), Some("Goal fixture verified"));
        let restored: ManagedGoal =
            serde_json::from_slice(&serde_json::to_vec(&goal).unwrap()).unwrap();
        assert_eq!(restored.objective(), "goal fixture");
        assert!(goal.start(&mut session).is_err());

        let mut goal = ManagedGoal::new("hold").unwrap();
        goal.start(&mut session).unwrap();
        goal.pause(&mut session).unwrap();
        assert_eq!(goal.status(), GoalStatus::Paused);
        // An interrupted turn delivered after pause cannot resume the goal.
        for _ in 0..8 {
            if let Some(event) = session.receive(Duration::from_millis(20)).unwrap() {
                goal.observe(&mut session, &event).unwrap();
            }
        }
        assert_eq!(goal.status(), GoalStatus::Paused);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut other = AgentSession::launch(Launch {
            provider,
            binary: Some(root.join("tests/fixtures/agent-runtime.mjs")),
            cwd: root,
            resume: None,
            env: BTreeMap::from([("HEY_BOSS_FIXTURE_PROVIDER".into(), provider.name().into())]),
            output_schema: None,
        })
        .unwrap();
        assert!(goal.pause(&mut other).is_err());
        goal.start(&mut session).unwrap();
        assert_eq!(goal.status(), GoalStatus::Active);
        goal.pause(&mut session).unwrap();
        let saved = session.inspect().unwrap().session.unwrap();
        let encoded = serde_json::to_vec(&goal).unwrap();
        session.stop().unwrap();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut recovered = AgentSession::launch(Launch {
            provider,
            binary: Some(root.join("tests/fixtures/agent-runtime.mjs")),
            cwd: root,
            resume: Some(saved.clone()),
            env: BTreeMap::from([("HEY_BOSS_FIXTURE_PROVIDER".into(), provider.name().into())]),
            output_schema: None,
        })
        .unwrap();
        let mut restored: ManagedGoal = serde_json::from_slice(&encoded).unwrap();
        restored.resume(&mut recovered).unwrap();
        assert_eq!(restored.objective(), "hold");
        assert_eq!(restored.session().unwrap().id, saved.id);
        assert_eq!(restored.status(), GoalStatus::Active);
        restored.pause(&mut recovered).unwrap();
    }
}

#[test]
fn queued_claude_instructions_must_finish_before_the_goal_completes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut agent = AgentSession::launch(Launch {
        provider: Provider::Claude,
        binary: Some(root.join("tests/fixtures/agent-runtime.mjs")),
        cwd: root,
        resume: None,
        env: BTreeMap::from([("HEY_BOSS_FIXTURE_PROVIDER".into(), "claude".into())]),
        output_schema: None,
    })
    .unwrap();
    let mut goal = ManagedGoal::new("queued goal").unwrap();
    let turn = goal.start(&mut agent).unwrap();
    loop {
        let event = agent.receive(Duration::from_millis(50)).unwrap();
        if let Some(event) = event {
            goal.observe(&mut agent, &event).unwrap();
            if matches!(event, hey_boss::agent_runtime::Event::TextDelta { .. }) {
                break;
            }
        }
    }
    agent
        .steer(
            &turn,
            r#"{"status":"completed","summary":"Queued instruction verified"}"#,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while goal.status() == GoalStatus::Active {
        if let Some(event) = agent.receive(Duration::from_millis(50)).unwrap() {
            goal.observe(&mut agent, &event).unwrap();
        }
        assert!(Instant::now() < deadline);
    }
    assert_eq!(goal.status(), GoalStatus::Complete);
    assert_eq!(goal.turns_completed(), 2);
    assert_eq!(goal.summary(), Some("Queued instruction verified"));
}

#[test]
fn buffered_claude_completions_preserve_queued_goal_scope() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut agent = AgentSession::launch(Launch {
        provider: Provider::Claude,
        binary: Some(root.join("tests/fixtures/agent-runtime.mjs")),
        cwd: root,
        resume: None,
        env: BTreeMap::from([("HEY_BOSS_FIXTURE_PROVIDER".into(), "claude".into())]),
        output_schema: None,
    })
    .unwrap();
    let mut goal = ManagedGoal::new("queued goal").unwrap();
    let turn = goal.start(&mut agent).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline);
        if let Some(event) = agent.receive(Duration::from_millis(50)).unwrap() {
            goal.observe(&mut agent, &event).unwrap();
            if matches!(event, hey_boss::agent_runtime::Event::TextDelta { .. }) {
                break;
            }
        }
    }
    agent
        .steer(
            &turn,
            r#"{"status":"completed","summary":"Queued instruction verified"}"#,
        )
        .unwrap();
    // A UI may inspect all arrived events before delivering them to the goal.
    while agent.inspect().unwrap().turn.is_some() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut buffered = Vec::new();
    while let Some(event) = agent.receive(Duration::ZERO).unwrap() {
        buffered.push(event);
    }
    for event in buffered {
        let initial_completion = matches!(&event, hey_boss::agent_runtime::Event::TurnCompleted { id, .. } if id == &turn);
        goal.observe(&mut agent, &event).unwrap();
        if initial_completion {
            assert_eq!(
                goal.status(),
                GoalStatus::Active,
                "An earlier report discarded queued instructions"
            );
        }
    }
    assert_eq!(goal.status(), GoalStatus::Complete);
    assert_eq!(goal.turns_completed(), 2);
    assert_eq!(goal.summary(), Some("Queued instruction verified"));
}

#[test]
#[ignore = "real authenticated Codex, Claude and Pi CLIs; models and scratch-only tools"]
fn real_managed_goals_verify_tool_effects() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let root = std::env::temp_dir().join(format!(
            "hey-boss-live-goal-{}-{}-{}",
            provider.name(),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut agent = AgentSession::launch(Launch {
            provider,
            binary: None,
            cwd: root.clone(),
            resume: None,
            env: BTreeMap::new(),
            output_schema: None,
        })
        .unwrap();
        let mut goal = ManagedGoal::new("Create proof.txt in the current directory containing exactly VERIFIED_GOAL. Use the file write tool, then read the file back with the file read tool and verify the exact contents. Do not change any other files. Return only the required JSON status/summary object, without Markdown or code fences. Report completed only after reading and verifying the file.").unwrap();
        goal.start(&mut agent).unwrap();
        let deadline = Instant::now() + Duration::from_secs(180);
        let mut tools = 0;
        while goal.status() == GoalStatus::Active {
            assert!(
                Instant::now() < deadline,
                "{} timed out: {:?}",
                provider.name(),
                agent.state()
            );
            if let Some(event) = agent.receive(Duration::from_millis(100)).unwrap() {
                match &event {
                    hey_boss::agent_runtime::Event::Approval { id, .. } => {
                        agent.decide(id, true).unwrap()
                    }
                    hey_boss::agent_runtime::Event::ToolStarted { .. } => tools += 1,
                    _ => {}
                }
                goal.observe(&mut agent, &event).unwrap();
                assert!(
                    goal.turns_completed() <= 3,
                    "{} did not return a valid goal report",
                    provider.name()
                );
            }
        }
        assert_eq!(
            goal.status(),
            GoalStatus::Complete,
            "{}: {:?}",
            provider.name(),
            goal.summary()
        );
        assert_eq!(
            std::fs::read_to_string(root.join("proof.txt"))
                .unwrap()
                .trim(),
            "VERIFIED_GOAL"
        );
        assert!(
            tools >= 2,
            "{} skipped required tool verification",
            provider.name()
        );
        assert!(goal.session().is_some());
        agent.stop().unwrap();
        eprintln!(
            "PASS {} real managed goal; {} turns; {tools} tools; scratch {}",
            provider.name(),
            goal.turns_completed(),
            root.display()
        );
    }
}
