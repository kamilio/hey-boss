use hey_boss::agent_runtime::{
    AgentSession, Event, Launch, ModelSelection, Provider, SessionRef, TurnStatus,
};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

fn model(provider: Provider, id: &str) -> ModelSelection {
    ModelSelection {
        id: id.into(),
        route: (provider == Provider::Pi).then(|| "proxy".into()),
    }
}
fn launch(
    provider: Provider,
    selection: Option<ModelSelection>,
    resume: Option<SessionRef>,
) -> Launch {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Launch {
        provider,
        model: selection,
        binary: Some(root.join("tests/fixtures/agent-runtime.mjs")),
        cwd: root,
        resume,
        env: BTreeMap::from([("HEY_BOSS_FIXTURE_PROVIDER".into(), provider.name().into())]),
        output_schema: None,
    }
}
fn completed(session: &mut AgentSession) {
    for _ in 0..100 {
        if let Some(Event::TurnCompleted { status, .. }) =
            session.receive(Duration::from_millis(50)).unwrap()
        {
            assert_eq!(status, TurnStatus::Completed);
            return;
        }
    }
    panic!("No completed turn");
}
#[test]
fn pinned_models_survive_turns_serialization_and_exact_resume() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let selected = model(provider, "custom-route");
        let mut config = launch(provider, Some(selected.clone()), None);
        config.env.insert(
            "HEY_BOSS_FIXTURE_EXPECT_MODEL".into(),
            "custom-route".into(),
        );
        let mut session = AgentSession::launch(config).unwrap();
        session.prompt("fixture completed", None).unwrap();
        completed(&mut session);
        session.prompt("fixture completed", None).unwrap();
        completed(&mut session);
        let saved = session.state().session.unwrap();
        assert_eq!(saved.model, Some(selected));
        let persisted: SessionRef =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        session.stop().unwrap();
        let mut config = launch(provider, None, Some(persisted));
        config.env.insert(
            "HEY_BOSS_FIXTURE_EXPECT_MODEL".into(),
            "custom-route".into(),
        );
        let mut session = AgentSession::launch(config).unwrap();
        session.prompt("fixture completed", None).unwrap();
        completed(&mut session);
        assert_eq!(session.state().session.as_ref(), Some(&saved));
        session.stop().unwrap();
        if let Some(path) = saved.path {
            std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }
}
#[test]
fn incompatible_resume_and_unsupported_route_fail_before_spawn() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let saved = SessionRef {
            provider,
            id: "saved-session".into(),
            path: None,
            model: Some(model(provider, "original")),
        };
        let error = AgentSession::launch(launch(
            provider,
            Some(model(provider, "different")),
            Some(saved),
        ))
        .err()
        .unwrap();
        assert!(error.to_string().contains("model"), "{error}");
    }
    let invalid = ModelSelection {
        id: "custom".into(),
        route: Some("proxy".into()),
    };
    let error = AgentSession::launch(launch(Provider::Codex, Some(invalid), None))
        .err()
        .unwrap();
    assert!(error.to_string().contains("route"));
    let error = AgentSession::launch(launch(
        Provider::Pi,
        Some(ModelSelection {
            id: "custom".into(),
            route: None,
        }),
        None,
    ))
    .err()
    .unwrap();
    assert!(error.to_string().contains("route"));
}
#[test]
fn refused_or_ignored_model_selection_never_runs_a_default() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        for mode in ["reject", "ignore"] {
            let mut config = launch(provider, Some(model(provider, "unavailable")), None);
            config
                .env
                .insert("HEY_BOSS_FIXTURE_MODEL_FAILURE".into(), mode.into());
            match AgentSession::launch(config) {
                Err(error) => assert!(
                    error.to_string().to_lowercase().contains("model"),
                    "{error}"
                ),
                Ok(mut session) => {
                    assert_eq!(provider, Provider::Claude);
                    let result = session.prompt("fixture completed", None);
                    if result.is_ok() {
                        let mut failed = false;
                        for _ in 0..100 {
                            match session.receive(Duration::from_millis(50)) {
                                Err(error) => {
                                    assert!(error.to_string().contains("model"));
                                    failed = true;
                                    break;
                                }
                                Ok(Some(Event::TurnCompleted { status, .. })) => {
                                    assert_eq!(status, TurnStatus::Failed);
                                    failed = true;
                                    break;
                                }
                                _ => {}
                            }
                        }
                        assert!(failed);
                    }
                }
            }
        }
    }
}
#[test]
fn model_catalog_retains_custom_ids_and_reports_discovery_failures() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let custom = model(provider, "configured-custom");
        let catalog = AgentSession::discover_models(
            launch(provider, None, None),
            std::slice::from_ref(&custom),
        );
        assert!(catalog.error.is_none(), "{:?}", catalog.error);
        assert!(
            catalog
                .models
                .iter()
                .any(|m| m.selection == custom && m.configured && !m.advertised)
        );
        assert!(
            catalog
                .models
                .iter()
                .any(|m| m.selection.id == "catalog-model" && m.advertised)
        );
        let mut config = launch(provider, None, None);
        config
            .env
            .insert("HEY_BOSS_FIXTURE_DISCOVERY_FAILURE".into(), "1".into());
        let catalog = AgentSession::discover_models(config, std::slice::from_ref(&custom));
        assert!(catalog.error.is_some());
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].selection, custom);
    }
}
#[test]
fn legacy_session_references_deserialize_without_a_model() {
    let saved: SessionRef =
        serde_json::from_str(r#"{"provider":"codex","id":"existing","path":null}"#).unwrap();
    assert_eq!(saved.model, None);
}

#[test]
fn pi_resume_requires_the_exact_file_and_saved_model() {
    let root = std::env::temp_dir().join(format!("hey-boss-model-resume-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("session.jsonl");
    let saved = SessionRef {
        provider: Provider::Pi,
        id: "saved-session".into(),
        path: Some(path.clone()),
        model: Some(model(Provider::Pi, "pinned")),
    };
    let error = AgentSession::launch(launch(Provider::Pi, None, Some(saved.clone())))
        .err()
        .unwrap();
    assert!(error.to_string().contains("exact saved session file"));
    std::fs::write(&path, "{\"type\":\"session\",\"id\":\"wrong-session\"}\n").unwrap();
    let error = AgentSession::launch(launch(Provider::Pi, None, Some(saved.clone())))
        .err()
        .unwrap();
    assert!(error.to_string().contains("saved session ID"));
    std::fs::write(&path, "{\"type\":\"session\",\"id\":\"saved-session\"}\n{\"type\":\"model_change\",\"provider\":\"proxy\",\"modelId\":\"changed\"}\n").unwrap();
    let error = AgentSession::launch(launch(Provider::Pi, None, Some(saved)))
        .err()
        .unwrap();
    assert!(error.to_string().contains("requested logical model"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_model_ids_and_pinning_legacy_resumes_are_rejected() {
    for id in ["", " model", "a\nb", "--option"] {
        let error = AgentSession::launch(launch(
            Provider::Codex,
            Some(model(Provider::Codex, id)),
            None,
        ))
        .err()
        .unwrap();
        assert!(error.to_string().contains("identifiers"));
    }
    let saved = SessionRef {
        provider: Provider::Codex,
        id: "saved-session".into(),
        path: None,
        model: None,
    };
    let error = AgentSession::launch(launch(
        Provider::Codex,
        Some(model(Provider::Codex, "new-model")),
        Some(saved),
    ))
    .err()
    .unwrap();
    assert!(error.to_string().contains("legacy session"));
}

#[test]
fn codex_catalog_paginates_and_exposes_cursor_failures() {
    for mode in ["pages", "loop"] {
        let mut config = launch(Provider::Codex, None, None);
        config
            .env
            .insert("HEY_BOSS_FIXTURE_CATALOG_PAGES".into(), mode.into());
        let catalog = AgentSession::discover_models(config, &[]);
        if mode == "pages" {
            assert!(catalog.error.is_none());
            assert_eq!(catalog.models.len(), 2);
            assert_eq!(catalog.models[1].selection.id, "second-model");
        } else {
            assert!(catalog.error.unwrap().contains("repeated"));
        }
    }
}

#[test]
fn replaced_session_identity_is_never_accepted() {
    for provider in [Provider::Codex, Provider::Claude] {
        let saved = SessionRef {
            provider,
            id: "saved-session".into(),
            path: None,
            model: Some(model(provider, "custom-route")),
        };
        match AgentSession::launch(launch(provider, None, Some(saved))) {
            Err(error) => assert!(error.to_string().contains("saved session")),
            Ok(mut session) => {
                assert_eq!(provider, Provider::Claude);
                session.prompt("fixture completed", None).unwrap();
                let mut rejected = false;
                for _ in 0..100 {
                    match session.receive(Duration::from_millis(50)) {
                        Err(error) => {
                            assert!(error.to_string().contains("saved session"));
                            rejected = true;
                            break;
                        }
                        Ok(Some(Event::TurnCompleted { .. })) => {
                            panic!("Replacement session completed")
                        }
                        _ => {}
                    }
                }
                assert!(rejected);
            }
        }
    }
}

#[test]
fn claude_cannot_complete_without_acknowledging_its_model() {
    let mut config = launch(
        Provider::Claude,
        Some(model(Provider::Claude, "custom-route")),
        None,
    );
    config
        .env
        .insert("HEY_BOSS_FIXTURE_MODEL_FAILURE".into(), "missing".into());
    let mut session = AgentSession::launch(config).unwrap();
    session.prompt("fixture completed", None).unwrap();
    for _ in 0..100 {
        match session.receive(Duration::from_millis(50)) {
            Err(error) => {
                assert!(error.to_string().contains("selected model"));
                return;
            }
            Ok(Some(Event::TurnCompleted { .. })) => panic!("Unconfirmed model completed"),
            _ => {}
        }
    }
    panic!("No model confirmation failure");
}

#[test]
fn claude_alias_catalog_without_resolved_ids_is_an_explicit_capability_failure() {
    let mut config = launch(Provider::Claude, None, None);
    config
        .env
        .insert("HEY_BOSS_FIXTURE_LEGACY_MODELS".into(), "1".into());
    let custom = model(Provider::Claude, "configured-custom");
    let catalog = AgentSession::discover_models(config, std::slice::from_ref(&custom));
    assert!(
        catalog
            .error
            .as_deref()
            .is_some_and(|e| e.contains("resolved model IDs"))
    );
    assert_eq!(catalog.models.len(), 1);
    assert_eq!(catalog.models[0].selection, custom);
    assert!(!catalog.models[0].advertised);
}
