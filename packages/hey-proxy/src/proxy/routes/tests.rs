use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    _dir: tempfile::TempDir,
    proxy: Arc<Proxy>,
    task: tokio::task::JoinHandle<()>,
    seen: Arc<Mutex<Vec<Value>>>,
    used: Arc<AtomicUsize>,
    work_used: Arc<AtomicUsize>,
    usage_status: Arc<AtomicUsize>,
    usage_calls: Arc<AtomicUsize>,
    failure: Arc<Mutex<Option<(u16, String, String)>>>,
    response_gate: Arc<Mutex<Option<Arc<tokio::sync::Notify>>>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let used = Arc::new(AtomicUsize::new(0));
        let work_used = Arc::new(AtomicUsize::new(0));
        let usage_status = Arc::new(AtomicUsize::new(200));
        let usage_calls = Arc::new(AtomicUsize::new(0));
        let failure = Arc::new(Mutex::new(None::<(u16, String, String)>));
        let response_gate = Arc::new(Mutex::new(None::<Arc<tokio::sync::Notify>>));
        let gate = response_gate.clone();
        let (s, u, c, f) = (
            seen.clone(),
            used.clone(),
            usage_calls.clone(),
            failure.clone(),
        );
        let (w, us) = (work_used.clone(), usage_status.clone());
        let app = Router::new().fallback(axum::routing::any(move |request: Request| {
            let gate = gate.clone();
            let (s, u, c, f) = (s.clone(), u.clone(), c.clone(), f.clone());
            let (w, us) = (w.clone(), us.clone());
            async move {
                let path = request.uri().path().to_owned();
                let auth = request.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
                let account = request.headers().get("chatgpt-account-id").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
                if path == "/api/oauth/profile" { return axum::Json(json!({"organization":{"uuid":"synthetic-claude-org"}})).into_response(); }
                if path == "/api/oauth/usage" {
                    c.fetch_add(1,Ordering::SeqCst);
                    assert_eq!(auth,"Bearer sk-ant-oat01-synthetic-old");
                    return axum::Json(json!({"five_hour":{"utilization":0},"seven_day_sonnet":{"utilization":u.load(Ordering::SeqCst)},"extra_usage":{"is_enabled":true,"monthly_limit":10000,"used_credits":0}})).into_response();
                }
                if path == "/backend-api/wham/usage" {
                    c.fetch_add(1, Ordering::SeqCst);
                    if us.load(Ordering::SeqCst) != 200 { return StatusCode::from_u16(us.load(Ordering::SeqCst) as u16).unwrap().into_response(); }
                    assert_eq!(auth, format!("Bearer oauth-{account}"));
                    let percent = if account == "personal" { u.load(Ordering::SeqCst) } else { w.load(Ordering::SeqCst) };
                    return axum::Json(json!({"rate_limit":{"primary_window":{"used_percent":percent,"reset_at":crate::codex_auth::now()+3600}},"credits":{"has_credits":true,"balance":100}})).into_response();
                }
                let bytes = axum::body::to_bytes(request.into_body(), 1024*1024).await.unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                s.lock().unwrap().push(json!({"path":path,"auth":auth,"account":account,"body":body}));
                if path == "/v1/messages" { return axum::Json(json!({"id":"synthetic-message","type":"message","model":body["model"],"role":"assistant","content":[],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":0}})).into_response(); }
                if account == "personal" && let Some((status, content_type, body)) = f.lock().unwrap().clone() {
                    let gate = gate.lock().unwrap().clone();
                    let body = if let Some(gate) = gate {
                        Body::from_stream(async_stream::stream! {
                            yield Ok::<_, std::io::Error>(Bytes::from(body));
                            gate.notified().await;
                        })
                    } else {
                        Body::from(body)
                    };
                    let mut response = Response::new(body);
                    *response.status_mut() = StatusCode::from_u16(status).unwrap();
                    if !content_type.is_empty() { response.headers_mut().insert(header::CONTENT_TYPE,content_type.parse().unwrap()); }
                    return response;
                }
                axum::Json(json!({"id":"synthetic-response","object":"response","status":"completed","output":[],"model":body["model"],"usage":{"input_tokens":1,"output_tokens":0}})).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        for name in ["personal", "work"] {
            crate::codex_auth::save(
                &dir.path().join(format!("{name}.json")),
                &crate::codex_auth::Tokens {
                    access_token: format!("oauth-{name}"),
                    refresh_token: format!("refresh-{name}"),
                    expires_at: crate::codex_auth::now() + 3600,
                    account_id: Some(name.into()),
                },
            )
            .unwrap();
        }
        let key = dir.path().join("api-key");
        std::fs::write(&key, "synthetic-paid-key").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let config: Config = serde_json::from_value(json!({
            "listen":"127.0.0.1:8080", "account_schema_version":1,
            "accounts":{
                "personal":{"implementation":"codex","auth":"subscription","credentials_file":"personal.json","endpoint":url},
                "work":{"implementation":"codex","auth":"subscription","credentials_file":"work.json","endpoint":url},
                "ultima":{"implementation":"openai","auth":"api","endpoint":url,"credential":format!("file://{}",key.display())}
            },
            "routes":[
                {"model":"gpt-6-astra","legs":[{"provider":"ultima","override":"astra"}]},
                {"model":"gpt-6.1-sol","legs":[{"provider":"personal"},{"provider":"work"},{"provider":"ultima"}]}
            ],
            "overrides":{"ultima":{"astra":{"from":"gpt-6-astra","to":"ultima-alpha","reasoning":"high"}}}
        })).unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let proxy = Arc::new(local_snapshot(config, Some(path)).unwrap());
        Self {
            _dir: dir,
            proxy,
            task,
            seen,
            used,
            work_used,
            usage_status,
            usage_calls,
            failure,
            response_gate,
        }
    }
    async fn request(&self, input: Value) -> Response {
        super::super::forward_api(
            self.proxy.clone(),
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::CONTENT_TYPE, "application/json")
                .header("authorization", "Bearer incoming-key")
                .header("chatgpt-account-id", "untrusted-account")
                .body(Body::from(input.to_string()))
                .unwrap(),
        )
        .await
    }
    fn claude(&mut self) {
        let mut config = (*self.proxy.config).clone();
        let endpoint = match &config.accounts["ultima"] {
            config::accounts::AccountConfig::Openai { endpoint, .. } => endpoint.clone(),
            _ => unreachable!(),
        };
        let path = self._dir.path().join("claude.json");
        crate::claude_auth::tests::fixture(&path, crate::claude_auth::now() + 3600);
        config.accounts.insert("sonnet".into(),serde_json::from_value(json!({"implementation":"claude","auth":"subscription","credentials_file":path,"endpoint":endpoint})).unwrap());
        config.routes.push(serde_json::from_value(json!({"model":"claude-sonnet-5-5","legs":[{"provider":"sonnet"},{"provider":"ultima"}]})).unwrap());
        self.proxy = Arc::new(local_snapshot(config, self.proxy.service.source.clone()).unwrap());
    }
}

#[tokio::test]
async fn sonnet_uses_included_native_messages_then_explicit_paid_responses_adapter() {
    for exhausted in [false, true] {
        let mut f = Fixture::new().await;
        f.claude();
        f.used
            .store(if exhausted { 100 } else { 0 }, Ordering::SeqCst);
        let response = super::super::forward_api(f.proxy.clone(),Request::builder().method("POST").uri("/v1/messages")
            .header(header::CONTENT_TYPE,"application/json")
            .body(Body::from(json!({"model":"claude-sonnet-5-5","messages":[{"role":"user","content":"synthetic"}],"max_tokens":16,"stream":false}).to_string())).unwrap()).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let seen = f.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0]["auth"],
            if exhausted {
                "Bearer synthetic-paid-key"
            } else {
                "Bearer sk-ant-oat01-synthetic-old"
            }
        );
        assert_eq!(
            seen[0]["path"],
            if exhausted {
                "/v1/responses"
            } else {
                "/v1/messages"
            }
        );
        assert_eq!(seen[0]["body"]["model"], "claude-sonnet-5-5");
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["type"],
            "message"
        );
    }
}

#[tokio::test]
async fn exhausted_subscriptions_use_paid_key_despite_available_overage() {
    let f = Fixture::new().await;
    f.used.store(100, Ordering::SeqCst);
    f.work_used.store(100, Ordering::SeqCst);
    let response = f
        .request(json!({"model":"gpt-6.1-sol","input":[],"stream":true}))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-hey-proxy-provider"], "ultima");
    let seen = f.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["auth"], "Bearer synthetic-paid-key");
    assert_eq!(seen[0]["account"], "");
    assert_eq!(seen[0]["body"]["model"], "gpt-6.1-sol");
    assert!(seen[0]["body"]["reasoning"].is_null());
}

#[tokio::test]
async fn quota_refresh_is_shared_and_failures_never_select_paid() {
    let f = Fixture::new().await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..12 {
        let proxy = f.proxy.clone();
        tasks.spawn(async move {
            let selected = proxy.select("personal").await.unwrap();
            quota::availability(&selected, "gpt-6.1-sol").await
        });
    }
    while let Some(result) = tasks.join_next().await {
        assert_eq!(result.unwrap(), quota::Availability::Included);
    }
    assert_eq!(f.usage_calls.load(Ordering::SeqCst), 1);
    for status in [401, 403, 429, 503] {
        let f = Fixture::new().await;
        f.usage_status.store(status, Ordering::SeqCst);
        let response = f
            .request(json!({"model":"gpt-6.1-sol","input":[],"stream":true}))
            .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(f.seen.lock().unwrap().len(), 0);
        let response = f
            .request(json!({"model":"gpt-6.1-sol","input":[],"stream":true}))
            .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(f.usage_calls.load(Ordering::SeqCst) <= 2);
    }
}

#[tokio::test]
async fn typed_429_advances_but_auth_policy_invalid_and_transient_failures_do_not() {
    for (status, code, advances) in [
        (429, "usage_limit_reached", true),
        (429, "insufficient_quota", true),
        (429, "rate_limit_exceeded", false),
        (503, "server_error", false),
        (403, "usage_limit_reached", false),
        (401, "authentication_error", false),
        (400, "invalid_request_error", false),
        (429, "content_policy_violation", false),
    ] {
        let f = Fixture::new().await;
        *f.failure.lock().unwrap() = Some((
            status,
            "application/json".into(),
            json!({"error":{"code":code}}).to_string(),
        ));
        let response = f
            .request(json!({"model":"gpt-6.1-sol","input":[],"stream":true}))
            .await;
        assert_eq!(
            response.status().as_u16(),
            if advances { 200 } else { status },
            "{code}"
        );
        assert_eq!(
            f.seen.lock().unwrap().len(),
            if advances { 2 } else { 1 },
            "{code}"
        );
        if advances {
            f.seen.lock().unwrap().clear();
            let response = f
                .request(json!({"model":"gpt-6.1-sol","input":[],"stream":true}))
                .await;
            assert_eq!(response.headers()["x-hey-proxy-provider"], "work");
            assert_eq!(f.seen.lock().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn stateful_and_signed_history_uses_first_provider_without_rewriting_history() {
    let f = Fixture::new().await;
    for extra in [
        json!({"previous_response_id":"resp_remote"}),
        json!({"conversation":"conv_remote"}),
        json!({"input":[{"type":"reasoning","encrypted_content":"synthetic"}]}),
        json!({"tools":[{"type":"web_search"}]}),
        json!({"input":[{"type":"thinking","signature":"signed"}]}),
    ] {
        let mut input = json!({"model":"gpt-6.1-sol","input":[],"stream":true});
        input
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let response = f.request(input).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-hey-proxy-provider"], "personal");
        let seen = f.seen.lock().unwrap();
        let sent = seen.last().unwrap();
        assert_eq!(sent["account"], "personal");
        for (key, value) in extra.as_object().unwrap() {
            assert_eq!(&sent["body"][key], value, "{key}");
        }
    }
    assert_eq!(f.usage_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.seen.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn account_bound_history_never_skips_an_exhausted_first_provider() {
    let f = Fixture::new().await;
    f.used.store(100, Ordering::SeqCst);
    let response = f
        .request(json!({"model":"gpt-6.1-sol","input":[
        {"type":"compaction","encrypted_content":"synthetic"}
    ],"stream":true}))
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(f.usage_calls.load(Ordering::SeqCst), 1);
    assert!(f.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stateful_stream_returns_prelude_without_waiting_for_generation() {
    let f = Fixture::new().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *f.response_gate.lock().unwrap() = Some(gate.clone());
    let prelude = "data: {\"type\":\"response.created\",\"response\":{\"output\":[]}}\n\n";
    *f.failure.lock().unwrap() = Some((200, "text/event-stream".into(), prelude.into()));
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        f.request(json!({
            "model":"gpt-6.1-sol", "stream":true,
            "input":[{"type":"compaction","encrypted_content":"synthetic"}]
        })),
    )
    .await
    .expect("A request that cannot switch providers must not buffer its prelude");
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first, prelude);
    gate.notify_one();
    assert!(stream.next().await.is_none());
    assert_eq!(f.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn account_bound_history_never_replays_upstream_quota_refusal() {
    for (status, content_type, body) in [
        (
            429,
            "application/json",
            r#"{"error":{"code":"usage_limit_reached"}}"#,
        ),
        (
            200,
            "text/event-stream",
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_reached\"}}}\n\n",
        ),
    ] {
        let f = Fixture::new().await;
        *f.failure.lock().unwrap() = Some((status, content_type.into(), body.into()));
        let response = f
            .request(json!({"model":"gpt-6.1-sol","input":[
            {"type":"compaction","encrypted_content":"synthetic"}
        ],"stream":true}))
            .await;
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(response.headers()["x-hey-proxy-provider"], "personal");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap(),
            body
        );
        assert_eq!(f.seen.lock().unwrap().len(), 1);
        assert_eq!(f.usage_calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn stale_cache_and_reset_require_a_new_successful_account_reading() {
    let f = Fixture::new().await;
    let selected = f.proxy.select("personal").await.unwrap();
    f.used.store(100, Ordering::SeqCst);
    assert_eq!(
        quota::availability(&selected, "gpt-6.1-sol").await,
        quota::Availability::Exhausted
    );
    f.used.store(0, Ordering::SeqCst);
    assert_eq!(
        quota::availability(&selected, "gpt-6.1-sol").await,
        quota::Availability::Exhausted
    );
    let binding = selected.binding.as_ref().unwrap();
    binding.quota.codex.lock().await.expire_before(u64::MAX);
    f.usage_status.store(503, Ordering::SeqCst);
    assert_eq!(
        quota::availability(&selected, "gpt-6.1-sol").await,
        quota::Availability::Unavailable
    );
    assert_eq!(
        quota::availability(&selected, "gpt-6.1-sol").await,
        quota::Availability::Unavailable
    );
    assert_eq!(f.usage_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stream_quota_refusals_advance_but_committed_output_never_replays() {
    let refusal = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_reached\"}}}\n\n";
    for streaming in [true, false] {
        let f = Fixture::new().await;
        *f.failure.lock().unwrap() = Some((200, "text/event-stream".into(), refusal.into()));
        let response = f
            .request(json!({"model":"gpt-6.1-sol","input":[],"stream":streaming}))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-hey-proxy-provider"], "work");
        assert_eq!(f.seen.lock().unwrap().len(), 2);
    }
    for event in [
        "response.output_text.delta",
        "response.output_item.added",
        "response.reasoning.delta",
    ] {
        let f = Fixture::new().await;
        let body = format!("data: {{\"type\":\"{event}\"}}\n\n{refusal}");
        *f.failure.lock().unwrap() = Some((200, "text/event-stream".into(), body.clone()));
        let response = f
            .request(json!({"model":"gpt-6.1-sol","input":[],"stream":true}))
            .await;
        assert_eq!(response.headers()["x-hey-proxy-provider"], "personal");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap(),
            body
        );
        assert_eq!(f.seen.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn fresh_reading_recovers_after_exhaustion_and_pin_bypasses_automatic_routes() {
    let f = Fixture::new().await;
    let selected = f.proxy.select("personal").await.unwrap();
    f.used.store(100, Ordering::SeqCst);
    assert_eq!(
        quota::availability(&selected, "gpt-6.1-sol").await,
        quota::Availability::Exhausted
    );
    selected
        .binding
        .as_ref()
        .unwrap()
        .quota
        .codex
        .lock()
        .await
        .expire_before(u64::MAX);
    f.used.store(10, Ordering::SeqCst);
    assert_eq!(
        quota::availability(&selected, "gpt-6.1-sol").await,
        quota::Availability::Included
    );
    assert_eq!(f.usage_calls.load(Ordering::SeqCst), 2);
    let response = super::super::forward_api(
        Arc::new(selected),
        Request::builder()
            .method("POST")
            .uri("/responses")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"model":"gpt-6-astra","input":[],"stream":true}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(f.seen.lock().unwrap()[0]["auth"], "Bearer oauth-personal");
    assert_eq!(f.seen.lock().unwrap()[0]["body"]["model"], "gpt-6-astra");
    assert_eq!(f.usage_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn codex_missing_content_type_remains_sse_and_nonstreaming_collects_terminal_object() {
    for streaming in [true, false] {
        let f = Fixture::new().await;
        let body = "data: {\"type\":\"response.created\",\"response\":{\"output\":[]}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"synthetic-done\",\"object\":\"response\",\"status\":\"completed\",\"output\":[]}}\n\n";
        *f.failure.lock().unwrap() = Some((200, String::new(), body.into()));
        let response = f
            .request(json!({"model":"gpt-6.1-sol","input":[],"stream":streaming}))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            if streaming {
                "text/event-stream"
            } else {
                "application/json"
            }
        );
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        if streaming {
            assert_eq!(bytes, body);
        } else {
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["id"],
                "synthetic-done"
            );
        }
        assert_eq!(f.seen.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn astra_routes_directly_without_quota_or_recommender_and_scopes_override() {
    let f = Fixture::new().await;
    let response = f
        .request(json!({"model":"gpt-6-astra","input":[],"stream":true}))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-hey-proxy-requested-model"],
        "gpt-6-astra"
    );
    assert_eq!(f.usage_calls.load(Ordering::SeqCst), 0);
    let seen = f.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["body"]["model"], "ultima-alpha");
    assert_eq!(seen[0]["body"]["reasoning"]["effort"], "high");
    assert_eq!(seen[0]["auth"], "Bearer synthetic-paid-key");
    assert_eq!(seen[0]["account"], "");
}

#[tokio::test]
async fn sol_uses_exact_subscription_credentials_and_next_account_on_exhaustion() {
    let f = Fixture::new().await;
    f.used.store(100, Ordering::SeqCst);
    let response = f
        .request(json!({"model":"gpt-6.1-sol","input":[],"stream":true}))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(f.usage_calls.load(Ordering::SeqCst), 2);
    let seen = f.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["path"], "/backend-api/codex/responses");
    assert_eq!(seen[0]["auth"], "Bearer oauth-work");
    assert_eq!(seen[0]["account"], "work");
    assert_eq!(seen[0]["body"]["model"], "gpt-6.1-sol");
    assert_eq!(seen[0]["body"]["store"], false);
}

#[tokio::test]
async fn reasoning_history_switches_on_quota_and_preserves_messages_and_tool_pairs() {
    for preflight_exhausted in [true, false] {
        let f = Fixture::new().await;
        if preflight_exhausted {
            f.used.store(100, Ordering::SeqCst);
        } else {
            *f.failure.lock().unwrap() = Some((
                429,
                "application/json".into(),
                json!({"error":{"code":"usage_limit_reached"}}).to_string(),
            ));
        }
        let history = json!([
            {"role":"user","content":"synthetic request"},
            {"type":"reasoning","encrypted_content":"opaque","summary":[]},
            {"type":"function_call","name":"lookup","call_id":"call_1","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_1","output":"synthetic result"},
            {"type":"custom_tool_call","name":"patch","call_id":"call_2","input":"synthetic patch"},
            {"type":"custom_tool_call_output","call_id":"call_2","output":"done"},
            {"role":"assistant","content":"visible answer"}
        ]);
        let response = f.request(json!({"model":"gpt-6.1-sol","input":history,"stream":true,
            "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}},{"type":"web_search"}]
        })).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-hey-proxy-provider"], "work");
        let seen = f.seen.lock().unwrap();
        assert_eq!(seen.len(), if preflight_exhausted { 1 } else { 2 });
        if !preflight_exhausted {
            assert_eq!(seen[0]["body"]["input"], history);
        }
        let mut expected = history.as_array().unwrap().clone();
        expected.remove(1);
        assert_eq!(seen.last().unwrap()["body"]["input"], json!(expected));
        assert_eq!(
            seen.last().unwrap()["body"]["tools"][1]["type"],
            "web_search"
        );
    }
}

#[test]
fn reasoning_cleanup_is_limited_to_history_and_preserves_compaction() {
    let mut body = json!({"input":[
        {"type":"reasoning","encrypted_content":"remove"},
        {"type":"compaction","encrypted_content":"keep"},
        {"type":"function_call_output","call_id":"c","output":{"type":"reasoning","signature":"tool data"}},
        {"role":"user","content":"reasoning is ordinary text"}
    ],"reasoning":{"effort":"high"},"tools":[{"type":"function","parameters":{"properties":{"signature":{"type":"string"}}}}]});
    let mut expected = body.clone();
    expected["input"].as_array_mut().unwrap().remove(0);
    assert_eq!(strip_reasoning(&mut body), 1);
    assert_eq!(body, expected);
    assert_eq!(replay_blocker(&body), Some("provider_bound_history"));
    assert_eq!(strip_reasoning(&mut body), 0);
    let mut native = json!({"messages":[
        {"role":"user","content":[{"type":"text","text":"synthetic"}]},
        {"role":"assistant","content":[{"type":"thinking","thinking":"optional","signature":"opaque"}]},
        {"role":"assistant","content":[{"type":"redacted_thinking","data":"opaque"},{"type":"tool_use","id":"t","name":"lookup","input":{"signature":"argument"}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"result"}]}
    ]});
    let mut expected = native.clone();
    expected["messages"].as_array_mut().unwrap().remove(1);
    expected["messages"][1]["content"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    assert_eq!(strip_reasoning(&mut native), 2);
    assert_eq!(native, expected);
}

#[tokio::test]
async fn native_thinking_can_fall_back_without_losing_tool_history() {
    let mut f = Fixture::new().await;
    f.claude();
    f.used.store(100, Ordering::SeqCst);
    let response=super::super::forward_api(f.proxy.clone(),Request::builder().method("POST").uri("/v1/messages")
        .header(header::CONTENT_TYPE,"application/json")
        .body(Body::from(json!({"model":"claude-sonnet-5-5","max_tokens":16,"stream":false,
            "messages":[
                {"role":"user","content":"synthetic"},
                {"role":"assistant","content":[{"type":"thinking","thinking":"optional","signature":"foreign"},{"type":"tool_use","id":"c","name":"lookup","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"c","content":"result"}]}
            ],"tools":[{"name":"lookup","input_schema":{"type":"object"}}]
        }).to_string())).unwrap()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-hey-proxy-provider"], "ultima");
    assert_eq!(response.headers()["x-hey-proxy-reasoning-stripped"], "1");
    let seen = f.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let input = seen[0]["body"]["input"].as_array().unwrap();
    assert!(
        input
            .iter()
            .any(|i| i["type"] == "function_call" && i["call_id"] == "c")
    );
    assert!(input.iter().any(|i| i["type"] == "function_call_output"
        && i["call_id"] == "c"
        && i["output"] == json!([{"type":"input_text","text":"result"}])));
}

#[tokio::test]
async fn switchable_stream_does_not_wait_seconds_for_generation() {
    let f = Fixture::new().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *f.response_gate.lock().unwrap() = Some(gate.clone());
    let prelude = "data: {\"type\":\"response.created\",\"response\":{\"output\":[]}}\n\n";
    *f.failure.lock().unwrap() = Some((200, "text/event-stream".into(), prelude.into()));
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        f.request(json!({
            "model":"gpt-6.1-sol", "stream":true,
            "input":[{"type":"reasoning","encrypted_content":"synthetic"}]
        })),
    )
    .await
    .expect("Fallback inspection must not stall response headers for model generation");
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap(), prelude);
    gate.notify_one();
    assert!(stream.next().await.is_none());
    assert_eq!(f.seen.lock().unwrap().len(), 1);
}
