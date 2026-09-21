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
fn ambiguous_duplicate_request_ids_disable_controls() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let mut session = launch(provider, None);
        session.prompt("duplicate requests", None).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match session.receive(Duration::from_millis(50)) {
                Err(_) => break,
                _ => assert!(Instant::now() < deadline),
            }
        }
        assert!(
            session.state().outcome_uncertain,
            "{provider:?} accepted ambiguous request IDs"
        );
        let request = session.state().pending_requests.first().unwrap().clone();
        if provider == Provider::Pi {
            assert!(session.respond_input(&request, Some("one")).is_err());
        } else {
            assert!(session.decide(&request, true).is_err());
        }
        session.stop().unwrap();
    }
}

#[test]
fn broken_rpc_input_marks_delivery_uncertain_and_prevents_replay() {
    for provider in [Provider::Codex, Provider::Pi] {
        let mut session = launch(provider, None);
        let turn = session.prompt("closed input", None).unwrap();
        until(
            &mut session,
            |event| matches!(event, Event::TextDelta { text } if text == "INPUT_CLOSED"),
        );
        assert!(
            session
                .steer(&turn, "must not replay uncertain delivery")
                .is_err()
        );
        assert!(
            session.state().outcome_uncertain,
            "{provider:?} did not taint a failed RPC write"
        );
        assert!(
            session
                .steer(&turn, "retry must be explicitly recovered")
                .is_err()
        );
        session.stop().unwrap();
    }
}

#[test]
fn inspecting_a_burst_preserves_events_and_terminal_state() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let mut session = launch(provider, None);
        session.prompt("burst completion", None).unwrap();
        // Let the fixture fill the bounded transport before inspecting it.
        std::thread::sleep(Duration::from_millis(100));
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.inspect().unwrap().turn.is_some() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut text = String::new();
        loop {
            match session.receive(Duration::ZERO).unwrap() {
                Some(Event::TextDelta { text: delta }) => text.push_str(&delta),
                Some(Event::TurnCompleted { status, .. }) => {
                    assert_eq!(status, TurnStatus::Completed);
                    break;
                }
                Some(_) => {}
                None => panic!("{provider:?} lost completion"),
            }
        }
        assert_eq!(text, format!("burst completion{}", "x".repeat(160)));
    }
}

#[test]
fn consecutive_streaming_deltas_do_not_exhaust_the_control_queue() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let mut session = launch(provider, None);
        session.prompt("large burst completion", None).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.inspect().unwrap().turn.is_some() {
            assert!(Instant::now() < deadline);
        }
        let mut text = String::new();
        while let Some(event) = session.receive(Duration::ZERO).unwrap() {
            match event {
                Event::TextDelta { text: delta } => text.push_str(&delta),
                Event::TurnCompleted { status, .. } => assert_eq!(status, TurnStatus::Completed),
                _ => {}
            }
        }
        assert_eq!(text, format!("large burst completion{}", "x".repeat(600)));
        assert!(!session.state().outcome_uncertain);
    }
}

#[test]
fn merged_deltas_preserve_utf8_size_and_tool_boundaries() {
    let mut session = launch(Provider::Codex, None);
    session.prompt("coalesce boundaries", None).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let deadline = Instant::now() + Duration::from_secs(5);
    while session.inspect().unwrap().turn.is_some() {
        assert!(Instant::now() < deadline);
    }
    let mut fragments = Vec::new();
    let mut saw_tool = false;
    while let Some(event) = session.receive(Duration::ZERO).unwrap() {
        match event {
            Event::TextDelta { text } => {
                assert!(text.len() <= 32000);
                if saw_tool {
                    assert_eq!(text, "after tool");
                }
                fragments.push(text);
            }
            Event::ToolStarted { .. } => saw_tool = true,
            _ => {}
        }
    }
    assert!(saw_tool);
    assert_eq!(fragments.len(), 3);
    assert_eq!(
        fragments.concat(),
        format!(
            "coalesce boundaries{}{}{}after tool",
            "é".repeat(10000),
            "b".repeat(10000),
            "c".repeat(10000)
        )
    );
}

#[test]
fn disconnect_keeps_buffered_confirmed_completion_in_last_observed_state() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let mut session = launch(provider, None);
        session.prompt("completion then disconnect", None).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.inspect().is_ok() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(session.state().outcome_uncertain);
        assert!(
            session.state().turn.is_none(),
            "{provider:?} discarded confirmed completion on EOF"
        );
        assert!(
            session
                .prompt("must not restart a disconnected agent", None)
                .is_err()
        );
        // Confirmed events remain available even though new controls are disabled.
        let Event::TurnCompleted { status, .. } = until(&mut session, |event| {
            matches!(event, Event::TurnCompleted { .. })
        }) else {
            unreachable!()
        };
        assert_eq!(status, TurnStatus::Completed);
    }
}

#[test]
fn stopping_does_not_reactivate_a_buffered_turn_start() {
    let mut session = launch(Provider::Codex, None);
    let turn = session.prompt("start before ack", None).unwrap();
    session.stop().unwrap();
    let mut completions = 0;
    while let Some(event) = session.receive(Duration::ZERO).unwrap() {
        if let Event::TurnCompleted { id, status, .. } = event {
            assert_eq!(id, turn);
            assert_eq!(status, TurnStatus::Interrupted);
            completions += 1;
        }
    }
    assert_eq!(completions, 1);
    assert!(session.inspect().unwrap().turn.is_none());
}

#[test]
fn malformed_prompt_acknowledgements_prevent_duplicate_work() {
    for prompt in ["missing turn acknowledgement", "malformed acknowledgement"] {
        let mut session = launch(Provider::Codex, None);
        assert!(session.prompt(prompt, None).is_err());
        assert!(
            session.state().outcome_uncertain,
            "Malformed acknowledgement allowed replay: {prompt}"
        );
        assert!(
            session
                .prompt("retry must not duplicate work", None)
                .is_err()
        );
    }
}

#[test]
fn malformed_native_control_and_state_replies_disable_controls() {
    let mut claude = launch(Provider::Claude, None);
    claude.prompt("hold malformed interrupt", None).unwrap();
    until(&mut claude, |event| {
        matches!(event, Event::TextDelta { .. })
    });
    assert!(claude.interrupt().is_err());
    assert!(claude.state().outcome_uncertain);
    let mut pi = launch(Provider::Pi, None);
    assert!(pi.prompt("malformed prompt acknowledgement", None).is_err());
    assert!(pi.state().outcome_uncertain);
    let mut pi = launch(Provider::Pi, None);
    pi.prompt("missing session acknowledgement", None).unwrap();
    until(&mut pi, |event| matches!(event, Event::TextDelta { .. }));
    assert!(pi.inspect().is_err());
    assert!(pi.state().outcome_uncertain);
}

#[test]
fn explicitly_rejected_steering_remains_recoverable() {
    for provider in [Provider::Codex, Provider::Pi] {
        let mut session = launch(provider, None);
        let turn = session.prompt("hold", None).unwrap();
        assert!(session.steer(&turn, "reject steering").is_err());
        assert!(!session.state().outcome_uncertain);
        session.steer(&turn, "valid instruction").unwrap();
    }
}

#[test]
fn rejected_interrupt_does_not_relabel_normal_completion() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let mut session = launch(provider, None);
        session.prompt("hold rejected interrupt", None).unwrap();
        until(&mut session, |event| {
            matches!(event, Event::TextDelta { .. })
        });
        assert!(session.interrupt().is_err());
        assert!(!session.state().outcome_uncertain);
        let Event::TurnCompleted { status, .. } = until(&mut session, |event| {
            matches!(event, Event::TurnCompleted { .. })
        }) else {
            unreachable!()
        };
        assert_eq!(
            status,
            TurnStatus::Completed,
            "{provider:?} relabeled a rejected interrupt"
        );
    }
}

#[test]
fn codex_interruption_terminates_native_tools_before_reporting_success() {
    let root = std::env::temp_dir().join(format!(
        "hey-boss-native-tool-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut session = AgentSession::launch(Launch {
        provider: Provider::Codex,
        binary: Some(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agent-runtime.mjs"),
        ),
        cwd: root.clone(),
        resume: None,
        env: BTreeMap::from([
            ("HEY_BOSS_FIXTURE_PROVIDER".into(), "codex".into()),
            (
                "HEY_BOSS_FIXTURE_EFFECTS".into(),
                root.clone().into_os_string(),
            ),
        ]),
        output_schema: None,
    })
    .unwrap();
    session.prompt("hold native tool", None).unwrap();
    until(&mut session, |event| {
        matches!(event, Event::TextDelta { .. })
    });
    session.interrupt().unwrap();
    let Event::TurnCompleted { status, .. } = until(&mut session, |event| {
        matches!(event, Event::TurnCompleted { .. })
    }) else {
        unreachable!()
    };
    assert_eq!(status, TurnStatus::Interrupted);
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        !root.join("native-finished").exists(),
        "Native interrupt left the tool running"
    );
    session.stop().unwrap();
    std::fs::remove_dir_all(root).unwrap();
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
fn pi_preflight_input_exposes_pending_ack_and_validates_the_late_response() {
    for reject in [false, true] {
        let mut session = launch(Provider::Pi, None);
        let turn = session
            .prompt(
                if reject {
                    "preflight reject"
                } else {
                    "preflight input"
                },
                None,
            )
            .unwrap();
        assert!(session.state().awaiting_prompt_ack);
        until(&mut session, |e| matches!(e, Event::Input { .. }));
        assert!(session.steer(&turn, "must not bypass preflight").is_err());
        assert!(
            session
                .respond_input("preflight-input", Some("invalid"))
                .is_err()
        );
        session
            .respond_input("preflight-input", Some("two"))
            .unwrap();
        let Event::TurnCompleted { status, output, .. } =
            until(&mut session, |e| matches!(e, Event::TurnCompleted { .. }))
        else {
            unreachable!()
        };
        assert_eq!(
            status,
            if reject {
                TurnStatus::Failed
            } else {
                TurnStatus::Completed
            }
        );
        assert!(output.contains(if reject {
            "preflight rejected"
        } else {
            "preflight accepted"
        }));
        assert!(!session.state().awaiting_prompt_ack);
        assert!(!session.state().outcome_uncertain);
    }
}

#[test]
fn pi_interrupt_stops_owned_preflight_without_answering_input() {
    let mut session = launch(Provider::Pi, None);
    session.prompt("preflight input", None).unwrap();
    until(&mut session, |e| matches!(e, Event::Input { .. }));
    session.interrupt().unwrap();
    until(&mut session, |e| {
        matches!(
            e,
            Event::TurnCompleted {
                status: TurnStatus::Interrupted,
                ..
            }
        )
    });
    assert!(session.state().stopped);
    assert!(!session.state().awaiting_prompt_ack);
    assert!(session.state().pending_requests.is_empty());
    assert!(
        session
            .respond_input("preflight-input", Some("two"))
            .is_err()
    );
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
fn killed_claude_task_without_notification_does_not_hold_parent_completion() {
    let mut session = launch(Provider::Claude, None);
    session.prompt("background killed", None).unwrap();
    until(&mut session, |event| {
        matches!(event, Event::TurnCompleted { .. })
    });
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
