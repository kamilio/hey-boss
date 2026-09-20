use hey_boss::agent_runtime::{AgentSession, Event, Launch, Provider, SessionRef, TurnStatus};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

fn launch(provider: Provider, resume: Option<SessionRef>) -> AgentSession {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    AgentSession::launch(Launch {
        provider,
        binary: Some(root.join("tests/fixtures/agent-runtime.mjs")),
        cwd: root,
        resume,
        env: BTreeMap::from([("HEY_BOSS_FIXTURE_PROVIDER".into(), provider.name().into())]),
        output_schema: None,
    })
    .unwrap()
}

fn until(session: &mut AgentSession, predicate: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(event) = session.receive(Duration::from_millis(50)).unwrap()
            && predicate(&event)
        {
            return event;
        }
        assert!(Instant::now() < deadline, "No expected event");
    }
}

#[test]
fn providers_complete_resume_and_control_owned_sessions() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let mut session = launch(provider, None);
        let turn = session.prompt("fixture completed", None).unwrap();
        let event = until(&mut session, |e| matches!(e, Event::TurnCompleted { .. }));
        match event {
            Event::TurnCompleted { status, output, .. } => {
                assert_eq!(status, TurnStatus::Completed);
                assert!(output.contains("fixture completed"));
            }
            _ => unreachable!(),
        }
        let saved = session.inspect().unwrap().session.unwrap();
        assert_eq!(saved.provider, provider);
        assert!(session.steer(&turn, "stale instruction").is_err());
        session.stop().unwrap();
        let mut resumed = launch(provider, Some(saved.clone()));
        resumed.prompt("hold", None).unwrap();
        until(&mut resumed, |e| matches!(e, Event::TextDelta { .. }));
        let state = resumed.inspect().unwrap();
        assert_eq!(state.session.unwrap().id, saved.id);
        resumed
            .steer(state.turn.as_deref().unwrap(), "new instruction")
            .unwrap();
        resumed.interrupt().unwrap();
        until(&mut resumed, |e| {
            matches!(
                e,
                Event::TurnCompleted {
                    status: TurnStatus::Interrupted,
                    ..
                }
            )
        });
    }
}

#[test]
fn approvals_require_an_explicit_owned_pending_request() {
    for provider in [Provider::Codex, Provider::Claude] {
        let mut session = launch(provider, None);
        session.prompt("approval", None).unwrap();
        let event = until(&mut session, |e| matches!(e, Event::Approval { .. }));
        let Event::Approval { id, .. } = event else {
            unreachable!()
        };
        assert!(session.decide("unknown", true).is_err());
        session.decide(&id, false).unwrap();
        assert!(session.decide(&id, true).is_err());
        until(&mut session, |e| matches!(e, Event::TurnCompleted { .. }));
    }
}

#[test]
fn cross_provider_resume_is_rejected_before_spawning() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let result = AgentSession::launch(Launch {
        provider: Provider::Claude,
        binary: Some(root.join("tests/fixtures/agent-runtime.mjs")),
        cwd: root,
        resume: Some(SessionRef {
            provider: Provider::Codex,
            id: "00000000-0000-0000-0000-000000000001".into(),
            path: None,
        }),
        env: BTreeMap::new(),
        output_schema: None,
    });
    assert!(result.is_err());
}

#[test]
fn pi_retries_are_not_terminal_and_extension_input_can_be_cancelled() {
    let mut session = launch(Provider::Pi, None);
    session.prompt("retry", None).unwrap();
    let event = until(&mut session, |e| matches!(e, Event::TurnCompleted { .. }));
    let Event::TurnCompleted { output, .. } = event else {
        unreachable!()
    };
    assert_eq!(output, "retry settled");
    session.prompt("input", None).unwrap();
    until(&mut session, |e| matches!(e, Event::Input { .. }));
    assert!(
        session
            .respond_input("input-1", Some("not offered"))
            .is_err()
    );
    session.respond_input("input-1", None).unwrap();
    assert!(session.respond_input("input-1", Some("one")).is_err());
    until(&mut session, |e| matches!(e, Event::TurnCompleted { .. }));
}

#[test]
fn claude_queued_input_keeps_a_distinct_guarded_turn() {
    let mut session = launch(Provider::Claude, None);
    let turn = session.prompt("queued turns", None).unwrap();
    until(&mut session, |e| matches!(e, Event::TextDelta { .. }));
    session.steer(&turn, "next turn").unwrap();
    until(&mut session, |e| matches!(e, Event::TurnCompleted { .. }));
    let event = until(&mut session, |e| matches!(e, Event::TurnCompleted { .. }));
    let Event::TurnCompleted { id, output, .. } = event else {
        unreachable!()
    };
    assert_ne!(id, turn);
    assert_eq!(output, "next turn");
}

#[test]
fn malformed_protocol_never_counts_as_completion() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let mut session = launch(provider, None);
        session.prompt("malformed", None).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match session.receive(Duration::from_millis(50)) {
                Err(_) => break,
                Ok(Some(Event::TurnCompleted { .. })) => panic!("Malformed stream completed"),
                _ => {}
            }
            assert!(Instant::now() < deadline);
        }
        assert!(
            session
                .prompt("retry must not duplicate uncertain work", None)
                .is_err()
        );
    }
}

#[test]
fn embedding_can_supply_the_owned_agents_explicit_identity() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut session = AgentSession::launch(Launch {
            provider,
            binary: Some(root.join("tests/fixtures/agent-runtime.mjs")),
            cwd: root,
            resume: None,
            env: BTreeMap::from([
                ("HEY_BOSS_FIXTURE_PROVIDER".into(), provider.name().into()),
                ("HEY_BOSS_AGENT_ID".into(), "fixture:owned".into()),
            ]),
            output_schema: None,
        })
        .unwrap();
        session.prompt("owned identity", None).unwrap();
        let Event::TurnCompleted { output, .. } =
            until(&mut session, |e| matches!(e, Event::TurnCompleted { .. }))
        else {
            unreachable!()
        };
        assert_eq!(output, "fixture:owned");
    }
}

#[test]
#[ignore = "requires installed, authenticated Codex, Claude and Pi CLIs; makes small model requests"]
fn real_agents_complete_and_resume_the_exact_conversation() {
    let root = std::env::temp_dir().join(format!("hey-boss-real-agents-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        eprintln!("Testing real {}", provider.name());
        let start = |resume| {
            AgentSession::launch(Launch {
                provider,
                binary: None,
                cwd: root.clone(),
                resume,
                env: BTreeMap::new(),
                output_schema: None,
            })
            .unwrap()
        };
        let mut session = start(None);
        session.prompt("Remember this exact word: cobblestone. Reply with only that word. Do not use tools.", None).unwrap();
        let completed = |session: &mut AgentSession| {
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                if let Some(Event::TurnCompleted { status, output, .. }) =
                    session.receive(Duration::from_millis(100)).unwrap()
                {
                    assert_eq!(
                        status,
                        TurnStatus::Completed,
                        "{}: {output}",
                        provider.name()
                    );
                    assert!(
                        output.to_lowercase().contains("cobblestone"),
                        "{}: {output}",
                        provider.name()
                    );
                    break;
                }
                assert!(Instant::now() < deadline, "{} timed out", provider.name());
            }
        };
        completed(&mut session);
        let saved = session.inspect().unwrap().session.unwrap();
        session.stop().unwrap();
        let mut resumed = start(Some(saved.clone()));
        resumed.prompt("What exact word did I ask you to remember? Reply with only that word. Do not use tools.", None).unwrap();
        completed(&mut resumed);
        assert_eq!(resumed.inspect().unwrap().session.unwrap().id, saved.id);
    }
    std::fs::remove_dir_all(root).unwrap();
}
