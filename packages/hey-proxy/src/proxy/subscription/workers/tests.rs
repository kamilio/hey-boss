use super::*;
use hey_proxy::usage::{Client, State as UsageState};

fn config() -> Arc<Config> {
    Arc::new(serde_json::from_str(include_str!("../../../../examples/routes.config.json")).unwrap())
}
fn readings(config: &Config) -> BTreeMap<String, AccountUsage> {
    config.worker_candidates.iter().map(|c| {
        let provider = config.accounts[&c.provider].implementation();
        let usage = serde_json::from_value(json!({"schema_version":1,"account":{"provider":provider,"id":c.provider},"state":"ok","updated_at":1000,"retry_after_seconds":60,
            "data":{"windows":[{"id":"five_hour","label":"Session","remaining_percent":50,"resets_at":"2000"}],"extra_usage":{"enabled":true,"remaining_percent":100}}})).unwrap();
        (c.provider.clone(), usage)
    }).collect()
}
#[test]
fn preference_capabilities_named_accounts_and_no_result_are_deterministic() {
    let c = config();
    let mut readings = readings(&c);
    let before = serde_json::to_value(&*c).unwrap();
    let runtimes = [Runtime::Codex, Runtime::Claude];
    let rec = from_readings(&c, &runtimes, &readings, 1000);
    assert_eq!(rec.selected.unwrap().candidate.provider, "codex-personal");
    assert_eq!(rec.config_revision, c.revision);
    readings.get_mut("codex-personal").unwrap().reading.state = UsageState::Stale;
    assert_eq!(
        from_readings(&c, &runtimes, &readings, 1000)
            .selected
            .unwrap()
            .candidate
            .provider,
        "codex-work"
    );
    let rec = from_readings(&c, &[Runtime::Claude], &readings, 1000);
    assert_eq!(rec.selected.unwrap().candidate.model, "claude-sonnet-5-5");
    assert_eq!(
        rec.candidates[0].skip_reason,
        Some(SkipReason::RuntimeUnavailable)
    );
    for u in readings.values_mut() {
        u.reading.data.as_mut().unwrap().windows[0].remaining_percent = Some(0.0);
    }
    let rec = from_readings(&c, &runtimes, &readings, 1000);
    assert_eq!(rec.status, RecommendationStatus::NoRecommendation);
    assert!(rec.selected.is_none());
    assert_eq!(rec.expires_at, 1000);
    assert_eq!(rec.recheck_at, 1060);
    assert_eq!(serde_json::to_value(&*c).unwrap(), before);
    assert!(!serde_json::to_string(&rec).unwrap().contains("op://"));
    let mut c = (*c).clone();
    c.worker_candidates[0].model = "gpt-6-astra".into();
    let rec = from_readings(&Arc::new(c), &runtimes, &BTreeMap::new(), 1000);
    assert_eq!(
        rec.candidates[0].skip_reason,
        Some(SkipReason::NoSubscriptionRoute)
    );
}

async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (
        url,
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
    )
}

#[tokio::test]
async fn authenticated_host_and_client_relay_preserve_capabilities_and_no_result() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("config.json");
    let keys = crate::access::ensure(&source, &[]).unwrap();
    let mut c = (*config()).clone();
    c.mode = Mode::Host;
    // No capabilities => no OAuth reads, so nonexistent synthetic stores are intentional.
    let (host, host_task) = serve(
        router_with(
            c,
            Options {
                access_config: Some(source),
                ..Default::default()
            },
        )
        .unwrap(),
    )
    .await;
    assert_eq!(
        reqwest::get(format!("{host}/usage/v2/recommend"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let relay: Config = serde_json::from_value(json!({"listen":"127.0.0.1:8080","mode":"client","connection":{"url":host,"api_key":keys.local}})).unwrap();
    let (relay, relay_task) = serve(router(relay).unwrap()).await;
    let rec = Client::new(&relay, Some("caller-token-never-forwarded"))
        .unwrap()
        .recommend_workers(&[])
        .await
        .unwrap();
    assert_eq!(rec.status, RecommendationStatus::NoRecommendation);
    assert!(
        rec.candidates
            .iter()
            .all(|c| c.skip_reason == Some(SkipReason::RuntimeUnavailable))
    );
    let response = reqwest::get(format!("{relay}/usage/v2/recommend?runtimes=unknown"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = reqwest::get(format!("{relay}/usage/v2/recommend?runtimes="))
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    relay_task.abort();
    host_task.abort();
}

#[tokio::test]
async fn named_quota_reads_select_work_account_without_any_inference() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let (upstream, upstream_task) = serve(Router::new().fallback(axum::routing::any(move |request: Request| {
        let count = count.clone(); async move {
            assert_eq!(request.uri().path(), "/backend-api/wham/usage", "recommendation must never perform inference");
            count.fetch_add(1, Ordering::SeqCst);
            let exhausted = request.headers()["authorization"] == "Bearer synthetic-personal";
            axum::Json(json!({"rate_limit":{"primary_window":{"used_percent":if exhausted {100} else {20},"limit_window_seconds":18000}},"credits":{"has_credits":true,"balance":1000}}))
        }
    }))).await;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("config.json");
    let mut c = (*config()).clone();
    for (name, suffix) in [("codex-personal", "personal"), ("codex-work", "work")] {
        let path = dir.path().join(format!("{suffix}.json"));
        crate::codex_auth::save(
            &path,
            &crate::codex_auth::Tokens {
                access_token: format!("synthetic-{suffix}"),
                refresh_token: "synthetic-refresh".into(),
                expires_at: crate::codex_auth::now() + 3600,
                account_id: Some(format!("synthetic-account-{suffix}")),
            },
        )
        .unwrap();
        if let crate::config::accounts::AccountConfig::Codex {
            endpoint,
            credentials_file,
            ..
        } = c.accounts.get_mut(name).unwrap()
        {
            *endpoint = Some(upstream.clone());
            *credentials_file = path;
        }
    }
    std::fs::write(&source, serde_json::to_vec(&c).unwrap()).unwrap();
    let proxy = local_snapshot(c, Some(source)).unwrap();
    let rec = evaluate_workers(&proxy, &[Runtime::Codex]).await;
    assert_eq!(
        rec.selected.as_ref().unwrap().candidate.provider,
        "codex-work"
    );
    assert_eq!(
        rec.candidates[0].skip_reason,
        Some(SkipReason::IncludedExhausted)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let text = serde_json::to_string(&rec).unwrap();
    assert!(
        !text.contains("synthetic-")
            && !text.contains("credentials_file")
            && !text.contains("op://")
    );
    evaluate_workers(&proxy, &[Runtime::Codex]).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "cached reading must be shared"
    );
    upstream_task.abort();
}

#[test]
fn normalized_provider_denials_and_both_model_limits_reach_the_evaluator() {
    let normalized = codex::usage::normalize(&json!({
        "rate_limit":{"primary_window":{"used_percent":20}},
        "additional_rate_limits":[{"metered_feature":"gpt-6-astra","rate_limit":{
            "primary_window":{"used_percent":20}, "secondary_window":{"used_percent":100}}}]
    }))
    .unwrap();
    let usage: AccountUsage = serde_json::from_value(json!({"schema_version":1,"account":{"provider":"codex","id":"work"},"state":"ok","updated_at":1000,"data":normalized})).unwrap();
    assert!(included_quota(&usage, "gpt-6.1-sol", 1000).is_ok());
    assert_eq!(
        included_quota(&usage, "gpt-6-astra", 1000).unwrap_err(),
        SkipReason::IncludedExhausted
    );
    for flags in [json!({"allowed":false}), json!({"limit_reached":true})] {
        let mut raw = flags;
        raw["primary_window"] = json!({"used_percent":20});
        let mut u = usage.clone();
        u.reading.data = Some(
            serde_json::from_value(codex::usage::normalize(&json!({"rate_limit":raw})).unwrap())
                .unwrap(),
        );
        assert!(included_quota(&u, "gpt-6.1-sol", 1000).is_err());
    }
}

#[test]
fn scoped_denials_without_numeric_windows_cannot_establish_included_capacity() {
    for flags in [
        json!({"limit_reached":true}),
        json!({"allowed":false}),
        json!({"allowed":false,"primary_window":{"used_percent":20}}),
    ] {
        let normalized = codex::usage::normalize(&json!({"rate_limit":{"primary_window":{"used_percent":20}},
            "additional_rate_limits":[{"model":"gpt-6.1-sol","metered_feature":"gpt-6.1-sol","rate_limit":flags}]})).unwrap();
        let usage: AccountUsage = serde_json::from_value(json!({"schema_version":1,"account":{"provider":"codex","id":"work"},"state":"ok","updated_at":1000,"data":normalized})).unwrap();
        assert!(included_quota(&usage, "gpt-6.1-sol", 1000).is_err());
        assert!(included_quota(&usage, "gpt-6-astra", 1000).is_ok());
    }
}
