use super::*;
use serde_json::json;

fn fixture() -> serde_json::Value {
    json!({"listen":"127.0.0.1:8080","account_schema_version":1,
        "accounts": {
            "personal":{"implementation":"codex","auth":"subscription","credentials_file":"personal.json"},
            "work":{"implementation":"codex","auth":"subscription","credentials_file":"work.json"},
            "ultima":{"implementation":"openai","auth":"api","endpoint":"https://example.com","credential":"op://Private/Ultima/key"}
        },
        "routes":[{"model":"logical","legs":[{"provider":"personal"},{"provider":"work"},{"provider":"ultima","override":"paid"}]}],
        "overrides":{"ultima":{"paid":{"from":"logical","to":"upstream","reasoning":"high","reasoning_routes":{"low":{"to":"upstream-fast"}}}}}
    })
}
fn config(value: serde_json::Value) -> Result<Config> {
    let config: Config = serde_json::from_value(value)?;
    config.validate()?;
    Ok(config)
}

#[test]
fn ordered_routes_roundtrip() {
    let raw = fixture();
    let c = config(raw.clone()).unwrap();
    let serialized = serde_json::to_value(c).unwrap();
    assert_eq!(serialized["routes"], raw["routes"]);
    assert_eq!(serialized["overrides"], raw["overrides"]);
}

#[test]
fn provider_selection_isolated_and_rewrites_exactly_once() {
    let mut raw = fixture();
    raw["aliases"] = json!([{"from":"logical","to":"wrong"},{"from":"upstream-fast","to":"twice"}]);
    raw["fallbacks"] = json!({"upstream-fast":["wrong"]});
    let c = Arc::new(config(raw).unwrap());
    let plan = c
        .route_plan("logical", "/v1/responses", Some("low"))
        .unwrap();
    assert_eq!(plan.len(), 3);
    for (i, name) in ["personal", "work", "ultima"].iter().enumerate() {
        let (selected, leg) = plan.select(i).unwrap();
        assert_eq!(leg.provider, *name);
        assert_eq!(leg.source_model, "logical");
        assert_eq!(leg.config_revision, c.revision);
        assert!(
            selected.aliases.is_empty()
                && selected.fallbacks.is_empty()
                && selected.routes.is_empty()
                && selected.overrides.is_empty()
        );
        if i < 2 {
            assert_eq!(leg.upstream_model, "logical");
            assert_eq!(leg.reasoning.as_deref(), Some("low"));
            assert_eq!(leg.billing_mode, BillingMode::IncludedSubscription);
            assert!(selected.api_keys.is_empty());
            assert_eq!(
                selected.codex.unwrap().credentials_file.unwrap().to_str(),
                Some(format!("{name}.json").as_str())
            );
        } else {
            assert_eq!(leg.upstream_model, "upstream-fast");
            assert_eq!(leg.reasoning.as_deref(), Some("high"));
            assert_eq!(leg.billing_mode, BillingMode::PayPerToken);
            assert!(selected.codex.is_none());
            let safe = serde_json::to_string(&leg).unwrap();
            assert!(!safe.contains("op://") && !safe.contains("credentials_file"));
        }
    }
    assert!(plan.select(3).is_err());
    assert_eq!(
        c.alias_for("logical", "/v1/responses")
            .unwrap()
            .to
            .as_deref(),
        Some("wrong")
    );
    assert!(c.route_plan("unmatched", "/v1/responses", None).is_none());
}

#[test]
fn rejects_ambiguous_routes_and_invalid_scopes() {
    let base = fixture();
    let cases = [
        ("routes", json!([]), true),
        ("routes", json!([{"model":"logical","legs":[]}]), false),
        (
            "routes",
            json!([{"model":"logical","legs":[{"provider":"missing"}]}]),
            false,
        ),
        (
            "routes",
            json!([{"model":"logical","legs":[{"provider":"work","override":"paid"}]}]),
            false,
        ),
        (
            "routes",
            json!([{"model":"logical","legs":[{"provider":"ultima","model":"different","override":"paid"}]}]),
            false,
        ),
        (
            "routes",
            json!([{"model":"logical","legs":[{"provider":"work"},{"provider":"work"}]}]),
            false,
        ),
        (
            "routes",
            json!([{"model":"logical","legs":[{"provider":"work"}]},{"model":"logical","api_shape":"responses","legs":[{"provider":"personal"}]}]),
            false,
        ),
        (
            "routes",
            json!([{"model":"logical","api_shape":"messages","legs":[{"provider":"work"}]},{"model":"logical","api_shape":"responses","legs":[{"provider":"personal"}]}]),
            true,
        ),
        (
            "overrides",
            json!({"missing":{"paid":{"from":"logical","to":"upstream"}}}),
            false,
        ),
        (
            "overrides",
            json!({"ultima":{"paid":{"from":"logical","api_key":"default"}}}),
            false,
        ),
        (
            "overrides",
            json!({"ultima":{"paid":{"from":"logical","reasoning_routes":{"low":{"to":"upstream","api_key":"default"}}}}}),
            false,
        ),
        (
            "overrides",
            json!({"ultima":{"paid":{"from":"logical","api_shape":"messages"}}}),
            false,
        ),
        // Route/model destinations are terminal, not references to another route.
        // Recursive route or override references are rejected rather than traversed.
        (
            "routes",
            json!([{"model":"logical","legs":[{"provider":"work","route":"logical"}]}]),
            false,
        ),
        (
            "overrides",
            json!({"ultima":{"paid":{"from":"logical","override":"paid"}}}),
            false,
        ),
        ("fallbacks", json!({"a":["b"],"b":["a"]}), false),
    ];
    for (field, value, valid) in cases {
        let mut raw = base.clone();
        raw[field] = value;
        assert_eq!(
            config(raw.clone()).is_ok(),
            valid,
            "{field}: {}",
            raw[field]
        );
    }
}

#[test]
fn shape_matching_covers_native_and_adapter_endpoints() {
    for (shape, paths) in [
        ("responses", vec!["/v1/responses", "/responses/compact"]),
        (
            "chat_completions",
            vec!["/v1/chat/completions", "/v1/custom/chat/completions"],
        ),
        (
            "messages",
            vec![
                "/v1/messages",
                "/messages",
                "/custom/v1/messages",
                "/v1/messages/count_tokens",
            ],
        ),
        (
            "gemini",
            vec![
                "/v1beta/models/test:generateContent",
                "/v1/models/test:streamGenerateContent",
            ],
        ),
        ("realtime", vec!["/v1/realtime", "/v1/realtime/sessions"]),
        ("completions", vec!["/v1/completions"]),
    ] {
        let mut raw = fixture();
        raw["routes"][0]["api_shape"] = json!(shape);
        let c = Arc::new(config(raw).unwrap());
        for path in paths {
            assert!(c.route_plan("logical", path, None).is_some(), "{path}");
        }
        assert!(c.route_plan("logical", "/unsupported", None).is_none());
    }
}

#[test]
fn legacy_config_and_client_policy_remain_separate() {
    let c = Arc::new(config(json!({"listen":"127.0.0.1:8080","aliases":[{"from":"a","to":"b"},{"from":"b","to":"c"}],"fallbacks":{"b":["d"]}})).unwrap());
    assert!(c.route_plan("a", "/v1/responses", None).is_none());
    assert_eq!(
        c.alias_for("a", "/v1/responses")
            .unwrap()
            .destination(None)
            .0,
        Some("b")
    );
    assert_eq!(hey_proxy::fallback::targets(&c.fallbacks, "b"), ["d"]);
    let mut raw = fixture();
    raw["accounts"] = json!({});
    raw["overrides"] = json!({});
    raw["mode"] = json!("client");
    raw["connection"] = json!({"url":"http://localhost:9090","api_key":"synthetic"});
    assert!(config(raw).is_err());
}

#[test]
fn file_parser_rejects_duplicate_keys_without_echoing_values() {
    for raw in [
        r#"{"listen":"127.0.0.1:8080","routes":[],"routes":[]}"#,
        r#"{"listen":"127.0.0.1:8080","api_keys":{"default":"synthetic-private","default":"other"}}"#,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, raw).unwrap();
        let error = match super::super::load(&path) {
            Ok(_) => panic!("accepted duplicate keys"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("Invalid config JSON"));
        assert!(!error.contains("synthetic-private"));
    }
}

#[test]
fn paid_only_routes_and_explicit_leg_models_need_no_recommendations() {
    let mut raw = fixture();
    raw["routes"] = json!([{"model":"logical","legs":[{"provider":"ultima","model":"terminal"}]}]);
    let c = Arc::new(config(raw).unwrap());
    let (_, leg) = c
        .route_plan("logical", "/v1/responses", None)
        .unwrap()
        .select(0)
        .unwrap();
    assert_eq!(leg.upstream_model, "terminal");
    assert_eq!(leg.billing_mode, BillingMode::PayPerToken);
}

#[test]
fn documented_rules_resolve_to_the_expected_order_and_billing() {
    let c = Arc::new(
        config(serde_json::from_str(include_str!("../../../examples/routes.config.json")).unwrap())
            .unwrap(),
    );
    for (model, path, expected) in [
        (
            "gpt-6-astra",
            "/v1/responses",
            vec![("ultima", "ultima-alpha", BillingMode::PayPerToken)],
        ),
        (
            "gpt-6.1-sol",
            "/v1/responses",
            vec![
                (
                    "codex-personal",
                    "gpt-6.1-sol",
                    BillingMode::IncludedSubscription,
                ),
                (
                    "codex-work",
                    "gpt-6.1-sol",
                    BillingMode::IncludedSubscription,
                ),
                ("ultima", "gpt-6.1-sol", BillingMode::PayPerToken),
            ],
        ),
        (
            "claude-sonnet-5-5",
            "/v1/messages",
            vec![
                (
                    "claude-personal",
                    "claude-sonnet-5-5",
                    BillingMode::IncludedSubscription,
                ),
                ("sonnet-api", "claude-sonnet-5-5", BillingMode::PayPerToken),
            ],
        ),
    ] {
        let plan = c.route_plan(model, path, None).unwrap();
        assert_eq!(plan.len(), expected.len());
        for (i, (provider, upstream, billing)) in expected.iter().enumerate() {
            let (_, leg) = plan.select(i).unwrap();
            assert_eq!(leg.provider, *provider);
            assert_eq!(leg.upstream_model, *upstream);
            assert_eq!(leg.billing_mode, *billing);
        }
    }
}
