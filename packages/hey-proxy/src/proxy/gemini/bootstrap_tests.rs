use super::tests::{provider, serve};
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn stream_bootstrap_recovers_without_exposing_failed_attempts() {
    for path in ["/v1/responses", "/v1/custom/chat/completions"] {
        for failure in ["disconnect", "empty", "fragment", "http_then_disconnect"] {
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            let (upstream_url, upstream) = serve(Router::new().fallback(move || {
                let n = count.fetch_add(1, Ordering::SeqCst);
                async move {
                    if failure == "http_then_disconnect" && n == 0 {
                        return (StatusCode::SERVICE_UNAVAILABLE, "{}").into_response();
                    }
                    let failed = n == usize::from(failure == "http_then_disconnect");
                    let body = Body::from_stream(async_stream::stream! {
                        yield Ok::<Bytes, std::io::Error>(Bytes::from_static(b": accepted\n\n"));
                        if failed {
                            if failure == "fragment" {
                                yield Ok(Bytes::from_static(b"data: {\"candidates\":["));
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            if failure != "empty" { yield Err(std::io::Error::other("synthetic disconnect")); }
                        } else {
                            yield Ok(Bytes::from(format!("data: {}\n\n", json!({"candidates":[{"content":{"parts":[{"text":"recovered"}]},"finishReason":"STOP"}]}))));
                        }
                    });
                    ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
                }
            })).await;
            let mut config = Config {
                gemini: Some(provider(&upstream_url, "bearer")),
                ..Config::test_fixture()
            };
            config.retry.max_retries = 2;
            config.retry.recovery_timeout_ms = 0;
            config.retry.initial_delay_ms = 0;
            config.retry.max_delay_ms = 0;
            let (url, proxy) = serve(super::super::router(config).unwrap()).await;
            let request = if path == "/v1/responses" {
                json!({"model":"gemini/test","input":"fixture","stream":true})
            } else {
                json!({"model":"gemini/test","messages":[{"role":"user","content":"fixture"}],"stream":true})
            };
            let text = reqwest::Client::new()
                .post(format!("{url}{path}"))
                .json(&request)
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            assert!(text.contains("recovered"), "{path}/{failure}: {text}");
            assert!(!text.contains("response.failed"), "{text}");
            assert_eq!(
                calls.load(Ordering::SeqCst),
                if failure == "http_then_disconnect" {
                    3
                } else {
                    2
                }
            );
            if path == "/v1/responses" {
                let events = NativeSse::default().feed(text.as_bytes(), true).unwrap();
                assert_eq!(
                    events
                        .iter()
                        .filter(|e| e["type"] == "response.created")
                        .count(),
                    1
                );
                assert_eq!(events.last().unwrap()["type"], "response.completed");
                for pair in events.windows(2) {
                    assert!(
                        pair[0]["sequence_number"].as_u64() < pair[1]["sequence_number"].as_u64()
                    );
                }
                let logs = reqwest::get(format!("{url}/logs/api"))
                    .await
                    .unwrap()
                    .json::<Value>()
                    .await
                    .unwrap();
                assert_eq!(
                    logs["entries"][0]["retries"],
                    calls.load(Ordering::SeqCst) - 1
                );
                assert_eq!(logs["entries"][0]["state"], "succeeded");
            }
            proxy.abort();
            upstream.abort();
        }
    }
}

#[tokio::test]
async fn stream_bootstrap_respects_shared_budget_and_output_boundary() {
    for (scenario, retries, expected_calls) in [
        ("empty", 0, 1),
        ("empty", 2, 3),
        ("http_then_disconnect", 2, 3),
        ("text", 2, 1),
        ("thought", 2, 1),
        ("tool", 2, 1),
        ("permanent", 2, 1),
        ("permanent_after_retry", 2, 2),
        ("rejected_after_retry", 2, 2),
        ("non_sse_after_retry", 2, 2),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let (upstream_url, upstream) = serve(Router::new().fallback(move |axum::Json(request): axum::Json<Value>| {
            let n = count.fetch_add(1, Ordering::SeqCst);
            async move {
                if scenario == "http_then_disconnect" && n == 0 {
                    return (StatusCode::SERVICE_UNAVAILABLE, "{}").into_response();
                }
                if scenario == "rejected_after_retry" && n > 0 {
                    return (StatusCode::BAD_REQUEST, axum::Json(json!({"error":{"code":400,"status":"INVALID_ARGUMENT","message":"invalid fixture"}}))).into_response();
                }
                if scenario == "non_sse_after_retry" && n > 0 { return "not SSE".into_response(); }
                let body = Body::from_stream(async_stream::stream! {
                    yield Ok::<Bytes, std::io::Error>(Bytes::from_static(b": accepted\n\n"));
                    let native = match scenario {
                        "text" => Some(json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]})),
                        "thought" => Some(json!({"candidates":[{"content":{"parts":[{"text":"partial","thought":true}]}}]})),
                        "tool" => Some(json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":request["tools"][0]["functionDeclarations"][0]["name"],"args":{}}}]}}]})),
                        "permanent" => Some(json!({"error":{"code":400,"status":"INVALID_ARGUMENT","message":"invalid fixture"}})),
                        "permanent_after_retry" if n > 0 => Some(json!({"error":{"code":400,"status":"INVALID_ARGUMENT","message":"invalid fixture"}})),
                        _ => None,
                    };
                    if let Some(native) = native { yield Ok(Bytes::from(format!("data: {native}\n\n"))); }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    if scenario != "empty" { yield Err(std::io::Error::other("synthetic disconnect")); }
                });
                ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
            }
        })).await;
        let mut config = Config {
            gemini: Some(provider(&upstream_url, "bearer")),
            ..Config::test_fixture()
        };
        config.retry.max_retries = retries;
        config.retry.recovery_timeout_ms = 0;
        config.retry.initial_delay_ms = 0;
        config.retry.max_delay_ms = 0;
        let (url, proxy) = serve(super::super::router(config).unwrap()).await;
        let text = reqwest::Client::new().post(format!("{url}/v1/responses"))
            .json(&json!({"model":"gemini/test","input":"fixture","stream":true,"tools":[{"type":"function","name":"run","parameters":{"type":"object"}}]}))
            .send().await.unwrap().text().await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            expected_calls,
            "{scenario}: {text}"
        );
        let events = NativeSse::default().feed(text.as_bytes(), true).unwrap();
        assert_eq!(
            events.last().unwrap()["type"],
            "response.failed",
            "{scenario}"
        );
        assert!(!text.contains("response.function_call_arguments.done"));
        assert!(!text.contains("response.completed"));
        if scenario.starts_with("permanent") || scenario == "rejected_after_retry" {
            assert_eq!(
                events.last().unwrap()["response"]["error"]["code"],
                "invalid_prompt"
            );
        }
        if matches!(scenario, "rejected_after_retry" | "non_sse_after_retry") {
            let logs = reqwest::get(format!("{url}/logs/api"))
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap();
            assert_eq!(logs["entries"][0]["state"], "failed");
            assert_eq!(
                logs["entries"][0]["error_code"],
                events.last().unwrap()["response"]["error"]["code"]
            );
        }
        proxy.abort();
        upstream.abort();
    }
}

#[tokio::test]
async fn stream_bootstrap_heartbeats_and_cancellation_cover_backoff() {
    use futures_util::StreamExt;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let (upstream_url, upstream) = serve(Router::new().fallback(move || {
        count.fetch_add(1, Ordering::SeqCst);
        async {
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                ": accepted\n\n",
            )
        }
    }))
    .await;
    let mut config = Config {
        gemini: Some(provider(&upstream_url, "bearer")),
        ..Config::test_fixture()
    };
    config.retry.max_retries = 2;
    config.retry.recovery_timeout_ms = 0;
    config.retry.initial_delay_ms = 30_000;
    config.retry.max_delay_ms = 30_000;
    let proxy = Arc::new(super::super::local_snapshot(config, None).unwrap());
    let (parts, _) = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .body(Body::empty())
        .unwrap()
        .into_parts();
    let response = forward(
        proxy,
        parts,
        json!({"model":"gemini/test","input":"fixture","stream":true}),
    )
    .await;
    let mut body = response.into_body().into_data_stream();
    let chunk = tokio::time::timeout(HEARTBEAT_INTERVAL + Duration::from_secs(3), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(chunk, ": keepalive\n\n");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(body);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    upstream.abort();
}

#[tokio::test]
async fn stream_bootstrap_honors_deadline_fallback_owner_and_final_sse_frame() {
    for scenario in ["deadline", "fallback", "valid_eof"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let (upstream_url, upstream) = serve(Router::new().fallback(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async move {
                let body = if scenario == "valid_eof" {
                    format!("data: {}", json!({"candidates":[{"content":{"parts":[{"text":"complete"}]},"finishReason":"STOP"}]}))
                } else { ": accepted\n\n".to_owned() };
                ([(header::CONTENT_TYPE, "text/event-stream")], body)
            }
        })).await;
        let mut config = Config {
            gemini: Some(provider(&upstream_url, "bearer")),
            ..Config::test_fixture()
        };
        config.retry.max_retries = 20;
        config.retry.recovery_timeout_ms = 100;
        config.retry.initial_delay_ms = 10;
        config.retry.max_delay_ms = 10;
        let mut proxy = super::super::local_snapshot(config, None).unwrap();
        proxy.fallback_attempt = scenario == "fallback";
        let (parts, _) = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .body(Body::empty())
            .unwrap()
            .into_parts();
        let response = forward(
            Arc::new(proxy),
            parts,
            json!({"model":"gemini/test","input":"fixture","stream":true}),
        )
        .await;
        let bytes = tokio::time::timeout(
            Duration::from_secs(2),
            axum::body::to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        let events = NativeSse::default().feed(&bytes, true).unwrap();
        assert_eq!(
            events.last().unwrap()["type"],
            if scenario == "valid_eof" {
                "response.completed"
            } else {
                "response.failed"
            }
        );
        let calls = calls.load(Ordering::SeqCst);
        if scenario == "deadline" {
            assert!((2..=11).contains(&calls), "{calls}");
        } else {
            assert_eq!(calls, 1);
        }
        upstream.abort();
    }
}
