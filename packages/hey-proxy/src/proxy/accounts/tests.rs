use super::*;
use crate::codex_auth::{Tokens, save};
use std::path::Path;

fn fixture(dir: &Path) -> (Config, PathBuf) {
    let path = dir.join("config.json");
    let config: Config = serde_json::from_value(json!({"listen":"127.0.0.1:8080","account_schema_version":1,
        "accounts":{
            "personal":{"implementation":"codex","auth":"subscription","credentials_file":"personal.json"},
            "work":{"implementation":"codex","auth":"subscription","credentials_file":"work.json"},
            "alias":{"implementation":"codex","auth":"subscription","credentials_file":"alias.json"}
        }})).unwrap();
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    for (name, id) in [
        ("personal", "synthetic-account-one"),
        ("work", "synthetic-account-two"),
        ("alias", "synthetic-account-one"),
    ] {
        save(
            &dir.join(format!("{name}.json")),
            &Tokens {
                access_token: format!("synthetic-access-{name}"),
                refresh_token: format!("synthetic-refresh-{name}"),
                expires_at: crate::codex_auth::now() + 3600,
                account_id: Some(id.into()),
            },
        )
        .unwrap();
    }
    (config, path)
}

#[tokio::test]
async fn isolated_credentials_shared_identity_and_restart_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let (config, path) = fixture(dir.path());
    let proxy = local_snapshot(config.clone(), Some(path.clone())).unwrap();
    let personal = proxy.select("personal").await.unwrap();
    let work = proxy.select("work").await.unwrap();
    let alias = proxy.select("alias").await.unwrap();
    assert!(!Arc::ptr_eq(&personal.codex, &work.codex));
    assert!(!Arc::ptr_eq(&personal.codex, &alias.codex));
    let p = personal.binding.as_ref().unwrap();
    let w = work.binding.as_ref().unwrap();
    let a = alias.binding.as_ref().unwrap();
    assert_ne!(p.token, w.token);
    assert_ne!(p.reference, w.reference);
    assert_eq!(p.reference, a.reference);
    assert!(Arc::ptr_eq(&p.quota, &a.quota));
    assert!(!Arc::ptr_eq(&p.quota, &w.quota));
    assert!(!p.reference.contains("synthetic"));
    let restarted = local_snapshot(config, Some(path))
        .unwrap()
        .select("personal")
        .await
        .unwrap();
    assert_eq!(p.reference, restarted.binding.unwrap().reference);
    assert!(proxy.select("missing").await.is_err());
}

#[tokio::test]
async fn session_pin_rejects_rebinding_and_old_snapshot_survives_edits() {
    let dir = tempfile::tempdir().unwrap();
    let (config, path) = fixture(dir.path());
    let proxy = local_snapshot(config.clone(), Some(path.clone())).unwrap();
    let selected = proxy.select("personal").await.unwrap();
    let reference = selected.binding.as_ref().unwrap().reference.clone();
    let request = || {
        Request::builder()
            .uri("/v1/responses")
            .header("x-hey-proxy-provider", "personal")
            .header("x-hey-proxy-account", &reference)
            .body(Body::empty())
            .unwrap()
    };
    let mut req = request();
    assert!(bind(&proxy, &mut req).await.unwrap().is_some());
    assert!(req.headers().get("x-hey-proxy-provider").is_none());
    assert_eq!(req.uri().path(), "/responses");
    let mut changed = config.clone();
    changed
        .accounts
        .insert("personal".into(), config.accounts["work"].clone());
    let next = local_snapshot(changed, Some(path)).unwrap();
    assert!(bind(&next, &mut request()).await.is_err());
    assert_eq!(selected.binding.unwrap().token, "synthetic-access-personal");
    assert!(
        bind(
            &proxy,
            &mut Request::builder()
                .uri("/v1/responses")
                .header("x-hey-proxy-provider", "personal")
                .body(Body::empty())
                .unwrap()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn catalog_redacts_identity_credentials_and_paths() {
    let dir = tempfile::tempdir().unwrap();
    let (config, path) = fixture(dir.path());
    let proxy = local_snapshot(config, Some(path)).unwrap();
    let response = catalog(State(proxy.service)).await;
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(!text.contains("synthetic"));
    assert!(!text.contains(".json"));
    assert!(!text.contains("refresh_token"));
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["connections"].as_array().unwrap().len(), 3);
    assert!(
        value["connections"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["ready"] == true)
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
async fn named_refresh_usage_cooldown_and_inference_are_account_scoped() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let refreshes = Arc::new(AtomicUsize::new(0));
    let usages = Arc::new(AtomicUsize::new(0));
    let (r, u) = (refreshes.clone(), usages.clone());
    let (url, task) = serve(Router::new().fallback(axum::routing::any(move |request: Request| {
        let (r,u)=(r.clone(),u.clone());
        async move {
            if request.uri().path() == "/oauth/token" {
                r.fetch_add(1, Ordering::SeqCst);
                let body = axum::body::to_bytes(request.into_body(), 65536).await.unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap();
                let name = body["refresh_token"].as_str().unwrap().strip_prefix("synthetic-refresh-").unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                return axum::Json(json!({"access_token":format!("fresh-token-{name}"),"refresh_token":format!("new-refresh-{name}"),"expires_in":3600})).into_response();
            }
            if request.uri().path() == "/backend-api/wham/usage" {
                u.fetch_add(1, Ordering::SeqCst);
                if request.headers()["chatgpt-account-id"] == "synthetic-account-one" {
                    return (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "120")], "synthetic-private-error").into_response();
                }
                assert_eq!(request.headers()["authorization"], "Bearer fresh-token-work");
                return axum::Json(json!({"rate_limit":{"primary_window":{"used_percent":23,"limit_window_seconds":18000}}})).into_response();
            }
            assert_eq!(request.uri().path(), "/backend-api/codex/responses");
            assert_eq!(request.headers()["authorization"], "Bearer fresh-token-work");
            assert_eq!(request.headers()["chatgpt-account-id"], "synthetic-account-two");
            assert!(request.headers().get("x-hey-proxy-provider").is_none());
            axum::Json(json!({"id":"synthetic-response","output":[]})).into_response()
        }
    }))).await;
    let dir = tempfile::tempdir().unwrap();
    let (mut config, path) = fixture(dir.path());
    for account in config.accounts.values_mut() {
        if let AccountConfig::Codex {
            endpoint, issuer, ..
        } = account
        {
            *endpoint = Some(url.clone());
            *issuer = Some(url.clone());
        }
    }
    config.codex = config.accounts["personal"].apply(&config).codex;
    for (name, id) in [
        ("personal", "synthetic-account-one"),
        ("work", "synthetic-account-two"),
    ] {
        save(
            &dir.path().join(format!("{name}.json")),
            &Tokens {
                access_token: format!("expired-token-{name}"),
                refresh_token: format!("synthetic-refresh-{name}"),
                expires_at: 0,
                account_id: Some(id.into()),
            },
        )
        .unwrap();
    }
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let proxy = local_snapshot(config.clone(), Some(path.clone())).unwrap();
    let (personal, work) = tokio::join!(proxy.select("personal"), proxy.select("work"));
    let (personal, work) = (personal.unwrap(), work.unwrap());
    assert_eq!(refreshes.load(Ordering::SeqCst), 2);
    for _ in 0..3 {
        assert_eq!(
            proxy.select("work").await.unwrap().binding.unwrap().token,
            "fresh-token-work"
        );
    }
    assert_eq!(refreshes.load(Ordering::SeqCst), 2);
    work.rejected_binding().await;
    assert_eq!(refreshes.load(Ordering::SeqCst), 3);
    assert_eq!(
        personal.binding.as_ref().unwrap().token,
        "fresh-token-personal"
    );
    assert_eq!(codex::reading(&personal).await["state"], "error");
    let alias = proxy.select("alias").await.unwrap();
    let alias_usage = codex::reading(&alias).await;
    assert!(alias_usage["retry_after_seconds"].as_u64().unwrap() >= 119);
    assert!(!alias_usage.to_string().contains("synthetic-private"));
    assert!(
        codex::reading(&proxy).await["retry_after_seconds"]
            .as_u64()
            .unwrap()
            >= 119
    );
    assert_eq!(codex::reading(&work).await["state"], "ok");
    assert_eq!(usages.load(Ordering::SeqCst), 2);
    let (base, server) = serve(
        router_with(
            config,
            Options {
                source: Some((path.clone(), config::fingerprint(&path))),
                ..Default::default()
            },
        )
        .unwrap(),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .header("x-hey-proxy-provider", "work")
        .header("x-hey-proxy-account", &work.binding.unwrap().reference)
        .header("chatgpt-account-id", "malicious-client-id")
        .json(&json!({"model":"test-model","input":"synthetic"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    server.abort();
    task.abort();
}

#[tokio::test]
async fn invalid_reload_retains_previous_config_and_removal_never_rebinds() {
    let dir = tempfile::tempdir().unwrap();
    let (config, path) = fixture(dir.path());
    let proxy = local_snapshot(config, Some(path.clone())).unwrap();
    let old = proxy
        .select("work")
        .await
        .unwrap()
        .binding
        .unwrap()
        .reference;
    std::fs::write(
        &path,
        br#"{"accounts":{"work":{"implementation":"unknown"}}}"#,
    )
    .unwrap();
    proxy.service.refresh_files();
    assert_eq!(
        proxy
            .service
            .snapshot()
            .select("work")
            .await
            .unwrap()
            .binding
            .unwrap()
            .reference,
        old
    );
    let mut next = (*proxy.config).clone();
    next.accounts.remove("work");
    std::fs::write(&path, serde_json::to_vec(&next).unwrap()).unwrap();
    proxy.service.refresh_files();
    assert!(proxy.service.snapshot().select("work").await.is_err());
    assert_eq!(
        proxy
            .select("work")
            .await
            .unwrap()
            .binding
            .unwrap()
            .reference,
        old
    );
}

#[tokio::test]
async fn claude_identity_is_private_persistent_and_shared_across_aliases() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let profiles = Arc::new(AtomicUsize::new(0));
    let usages = Arc::new(AtomicUsize::new(0));
    let (p, u) = (profiles.clone(), usages.clone());
    let (endpoint,server)=serve(Router::new().fallback(axum::routing::any(move |request:Request| {
        let (p,u)=(p.clone(),u.clone());
        async move {
            let second=request.headers()["authorization"]=="Bearer sk-ant-oat01-work";
            if request.uri().path()=="/api/oauth/profile" {
                p.fetch_add(1,Ordering::SeqCst);
                axum::Json(json!({"organization":{"uuid":if second {"synthetic-org-two"} else {"synthetic-org-one"}},"email":"synthetic-private-email"})).into_response()
            } else {
                assert_eq!(request.uri().path(),"/api/oauth/usage");
                u.fetch_add(1,Ordering::SeqCst);
                axum::Json(json!({"five_hour":{"utilization":if second {25} else {75}}})).into_response()
            }
        }
    }))).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let mut definitions = json!({});
    for name in ["personal", "work", "alias"] {
        let tokens:crate::claude_auth::Tokens=serde_json::from_value(json!({"access_token":format!("sk-ant-oat01-{name}"),"refresh_token":format!("synthetic-{name}"),"expires_at":crate::claude_auth::now()+3600})).unwrap();
        crate::claude_auth::save(&dir.path().join(format!("{name}.json")), &tokens).unwrap();
        definitions[name] = json!({"implementation":"claude","auth":"subscription","endpoint":endpoint,"credentials_file":format!("{name}.json")});
    }
    let config: Config = serde_json::from_value(
        json!({"listen":"127.0.0.1:8080","account_schema_version":1,"accounts":definitions}),
    )
    .unwrap();
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let proxy = local_snapshot(config.clone(), Some(path.clone())).unwrap();
    let personal = proxy.select("personal").await.unwrap();
    let work = proxy.select("work").await.unwrap();
    let alias = proxy.select("alias").await.unwrap();
    assert!(!Arc::ptr_eq(&personal.claude, &work.claude));
    assert_eq!(
        personal.binding.as_ref().unwrap().reference,
        alias.binding.as_ref().unwrap().reference
    );
    assert_ne!(
        personal.binding.as_ref().unwrap().reference,
        work.binding.as_ref().unwrap().reference
    );
    assert_eq!(
        claude::reading(&personal).await["data"]["windows"][0]["used_percent"],
        75.0
    );
    assert_eq!(
        claude::reading(&alias).await["data"]["windows"][0]["used_percent"],
        75.0
    );
    assert_eq!(
        claude::reading(&work).await["data"]["windows"][0]["used_percent"],
        25.0
    );
    assert_eq!(usages.load(Ordering::SeqCst), 2);
    assert_eq!(profiles.load(Ordering::SeqCst), 3);
    let restarted = local_snapshot(config, Some(path))
        .unwrap()
        .select("personal")
        .await
        .unwrap();
    assert_eq!(
        personal.binding.unwrap().reference,
        restarted.binding.unwrap().reference
    );
    assert_eq!(profiles.load(Ordering::SeqCst), 3);
    let stored = std::fs::read_to_string(dir.path().join("personal.json")).unwrap();
    assert!(!stored.contains("synthetic-org"));
    assert!(!stored.contains("sk-ant-oat"));
    server.abort();
}

#[test]
fn route_plans_keep_provider_overrides_and_revision_across_atomic_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let raw = json!({"listen":"127.0.0.1:8080","account_schema_version":1,
        "accounts":{"paid":{"implementation":"openai","auth":"api","endpoint":"https://before.example","credential":"op://Private/Before/key"}},
        "routes":[{"model":"logical","legs":[{"provider":"paid","override":"mapping"}]}],
        "overrides":{"paid":{"mapping":{"from":"logical","to":"before"}}}
    });
    std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let proxy = local_snapshot(crate::config::load(&path).unwrap(), Some(path.clone())).unwrap();
    let old = proxy
        .config
        .route_plan("logical", "/v1/responses", None)
        .unwrap();
    let (_, old_leg) = old.select(0).unwrap();
    let mut next = raw.clone();
    next["routes"][0]["legs"][0]["override"] = json!("missing");
    std::fs::write(&path, serde_json::to_vec(&next).unwrap()).unwrap();
    proxy.service.refresh_files();
    assert_eq!(
        proxy.service.snapshot().config.revision,
        old_leg.config_revision
    );
    next = raw;
    next["accounts"]["paid"]["endpoint"] = json!("https://after.example");
    next["overrides"]["paid"]["mapping"]["to"] = json!("after-reload");
    std::fs::write(&path, serde_json::to_vec(&next).unwrap()).unwrap();
    proxy.service.refresh_files();
    let new = proxy
        .service
        .snapshot()
        .config
        .route_plan("logical", "/v1/responses", None)
        .unwrap();
    let (old_config, old_again) = old.select(0).unwrap();
    let (new_config, new_leg) = new.select(0).unwrap();
    assert_eq!(old_config.upstream_url, "https://before.example");
    assert_eq!(old_again.upstream_model, "before");
    assert_eq!(old_again.config_revision, old_leg.config_revision);
    assert_eq!(new_config.upstream_url, "https://after.example");
    assert_eq!(new_leg.upstream_model, "after-reload");
    assert_ne!(new_leg.config_revision, old_leg.config_revision);
}
