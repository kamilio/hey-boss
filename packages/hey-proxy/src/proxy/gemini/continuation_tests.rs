use super::tests::{provider, serve};
use super::*;
use std::sync::Mutex;

#[tokio::test]
async fn continuation_resumes_one_response_in_all_client_shapes() {
    for path in [
        "/v1/responses",
        "/v1/custom/chat/completions",
        "/v1/custom/messages",
    ] {
        for streaming in [false, true] {
            let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
            let captured = requests.clone();
            let (upstream_url, upstream_task) = serve(Router::new().fallback(
                move |axum::Json(body): axum::Json<Value>| {
                    let mut requests = captured.lock().unwrap();
                    let n = requests.len();
                    requests.push(body);
                    async move {
                        let mut candidate = json!({"index":0,"content":{"role":"model","parts":[{"text":(["first ","second ","last"][n])}]},"finishReason":"STOP"});
                        if n < 2 {
                            candidate["finishReason"] = json!("CONTINUATION");
                            candidate["continuationToken"] = json!(["token-one", "token-two"][n]);
                        }
                        let mut native = json!({"candidates":[candidate]});
                        let usage = json!({"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2,"thoughtsTokenCount":1,"totalTokenCount":13}});
                        if streaming {
                            ([(header::CONTENT_TYPE, "text/event-stream")], format!("data: {native}\n\ndata: {usage}\n\n")).into_response()
                        } else {
                            native["usageMetadata"] = usage["usageMetadata"].clone();
                            axum::Json(native).into_response()
                        }
                    }
                },
            )).await;
            let config = Config {
                gemini: Some(provider(&upstream_url, "api_key")),
                ..Config::test_fixture()
            };
            let (url, proxy_task) = serve(super::super::router(config).unwrap()).await;
            let mut request = json!({"model":"gemini/test","stream":streaming});
            if path == "/v1/responses" {
                request["input"] = json!("test");
            } else {
                request["messages"] = json!([{"role":"user","content":"test"}]);
                request["max_tokens"] = json!(100);
            }
            let response = reqwest::Client::new()
                .post(format!("{url}{path}"))
                .json(&request)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let bytes = response.bytes().await.unwrap();
            assert_eq!(
                status,
                200,
                "{path} streaming={streaming}: {}",
                String::from_utf8_lossy(&bytes)
            );
            let requests = requests.lock().unwrap();
            assert_eq!(
                requests.len(),
                3,
                "{path} streaming={streaming}: {}",
                String::from_utf8_lossy(&bytes)
            );
            for n in 1..3 {
                let mut expected = requests[0].clone();
                expected["continuationToken"] = json!(["token-one", "token-two"][n - 1]);
                assert_eq!(requests[n], expected);
            }
            assert!(!String::from_utf8_lossy(&bytes).contains("token-one"));
            if path == "/v1/responses" {
                let body = if streaming {
                    let events = NativeSse::default().feed(&bytes, true).unwrap();
                    for (n, event) in events.iter().enumerate() {
                        assert_eq!(event["sequence_number"], n);
                    }
                    assert_eq!(
                        events
                            .iter()
                            .filter(|e| e["type"] == "response.created")
                            .count(),
                        1
                    );
                    assert_eq!(
                        events
                            .iter()
                            .filter(|e| matches!(
                                e["type"].as_str(),
                                Some(
                                    "response.completed"
                                        | "response.failed"
                                        | "response.incomplete"
                                )
                            ))
                            .count(),
                        1
                    );
                    events.last().unwrap()["response"].clone()
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                assert_eq!(body["status"], "completed");
                assert_eq!(body["output"][1]["content"][0]["text"], "first second last");
                assert_eq!(body["usage"]["total_tokens"], 39);
            } else if !streaming {
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                if path == "/v1/custom/messages" {
                    assert_eq!(body["content"][0]["text"], "first second last");
                    assert_eq!(body["usage"]["output_tokens"], 9);
                } else {
                    assert_eq!(
                        body["choices"][0]["message"]["content"],
                        "first second last"
                    );
                }
            }
            upstream_task.abort();
            proxy_task.abort();
        }
    }
}

#[tokio::test]
async fn continuation_preserves_partial_signed_tools_and_replays_completed_turn() {
    for streaming in [false, true] {
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = requests.clone();
        let (upstream_url, upstream_task) = serve(Router::new().fallback(move |axum::Json(body): axum::Json<Value>| {
            let mut requests = captured.lock().unwrap();
            let n = requests.len(); requests.push(body.clone());
            async move {
                let name = body["tools"][0]["functionDeclarations"][0]["name"].clone();
                let mut candidate = json!({"index":0,"content":{"role":"model","parts":[]},"finishReason":"CONTINUATION","continuationToken":format!("token-{n}")});
                match n {
                    0 => candidate["content"]["parts"] = json!([{"text":"working ","thought":true},{"functionCall":{"id":"call-one","name":name,"partialArgs":[{"jsonPath":"$.text","stringValue":"first ","willContinue":true}],"willContinue":true},"thoughtSignature":"signed-call"}]),
                    1 => {}, // Empty continuation must retain pending tool assembly.
                    2 => {
                        candidate["content"]["parts"] = json!([{"functionCall":{"id":"call-one","partialArgs":[{"jsonPath":"$.text","stringValue":"last"}]}}]);
                        candidate["finishReason"] = json!("STOP"); candidate.as_object_mut().unwrap().remove("continuationToken");
                    }
                    _ => {
                        assert!(body["contents"].as_array().unwrap().iter().any(|c| c["parts"].as_array().unwrap().iter().any(|p| p["thoughtSignature"] == "signed-call" && p["functionCall"]["args"]["text"] == "first last")));
                        candidate = json!({"content":{"parts":[{"text":"replayed"}]},"finishReason":"STOP"});
                    }
                }
                let native = json!({"candidates":[candidate],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2,"totalTokenCount":12}});
                if streaming { ([(header::CONTENT_TYPE,"text/event-stream")],format!("data: {native}\n\n")).into_response() } else { axum::Json(native).into_response() }
            }
        })).await;
        let config = Config {
            gemini: Some(provider(&upstream_url, "api_key")),
            ..Config::test_fixture()
        };
        let (url, proxy_task) = serve(super::super::router(config).unwrap()).await;
        let mut request = json!({"model":"gemini/test","stream":streaming,"input":"test","tools":[{"type":"function","name":"write","parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]});
        let client = reqwest::Client::new();
        for turn in 0..2 {
            let bytes = client
                .post(format!("{url}/v1/responses"))
                .json(&request)
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            let response = if streaming {
                let events = NativeSse::default().feed(&bytes, true).unwrap();
                for (n, e) in events.iter().enumerate() {
                    assert_eq!(e["sequence_number"], n);
                }
                assert_eq!(
                    events
                        .iter()
                        .filter(|e| e["type"] == "response.completed")
                        .count(),
                    1
                );
                if turn == 0 {
                    assert_eq!(
                        events
                            .iter()
                            .filter(|e| e["type"] == "response.output_item.done"
                                && e["item"]["type"] == "function_call")
                            .count(),
                        1
                    );
                }
                events.last().unwrap()["response"].clone()
            } else {
                serde_json::from_slice(&bytes).unwrap()
            };
            assert_eq!(response["status"], "completed", "{response}");
            if turn == 0 {
                assert_eq!(
                    response["output"][1]["arguments"],
                    r#"{"text":"first last"}"#
                );
                assert_eq!(response["usage"]["total_tokens"], 36);
                let mut input = vec![json!({"role":"user","content":"test"})];
                input.extend(response["output"].as_array().unwrap().clone());
                input.push(
                    json!({"type":"function_call_output","call_id":"call-one","output":"ok"}),
                );
                request["input"] = json!(input);
            }
        }
        assert_eq!(requests.lock().unwrap().len(), 4);
        upstream_task.abort();
        proxy_task.abort();
    }
}

#[tokio::test]
async fn continuation_retry_keeps_token_and_failure_never_commits_output() {
    for failure in [false, true] {
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = requests.clone();
        let (upstream_url, upstream_task) = serve(Router::new().fallback(move |axum::Json(body): axum::Json<Value>| {
            let mut requests = captured.lock().unwrap(); let n=requests.len(); requests.push(body);
            async move {
                if n==1 { return (if failure {StatusCode::BAD_REQUEST} else {StatusCode::SERVICE_UNAVAILABLE},axum::Json(json!({"error":{"status":if failure {"INVALID_ARGUMENT"} else {"UNAVAILABLE"}}}))).into_response(); }
                let native = if n==0 { json!({"candidates":[{"content":{"parts":[{"text":"visible"}]},"finishReason":"CONTINUATION","continuationToken":"private-token"}]}) } else { json!({"candidates":[{"content":{"parts":[{"text":" end"}]},"finishReason":"STOP"}]}) };
                ([(header::CONTENT_TYPE,"text/event-stream")],format!("data: {native}\n\n")).into_response()
            }
        })).await;
        let mut config = Config {
            gemini: Some(provider(&upstream_url, "api_key")),
            ..Config::test_fixture()
        };
        config.retry.initial_delay_ms = 0;
        config.retry.max_delay_ms = 0;
        let (url, proxy_task) = serve(super::super::router(config).unwrap()).await;
        let bytes = reqwest::Client::new()
            .post(format!("{url}/v1/responses"))
            .json(&json!({"model":"gemini/test","input":"test","stream":true}))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let events = NativeSse::default().feed(&bytes, true).unwrap();
        assert!(
            events
                .iter()
                .any(|e| e["type"] == "response.output_text.delta" && e["delta"] == "visible")
        );
        assert_eq!(
            events.last().unwrap()["type"],
            if failure {
                "response.failed"
            } else {
                "response.completed"
            }
        );
        for (n, e) in events.iter().enumerate() {
            assert_eq!(e["sequence_number"], n);
        }
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), if failure { 2 } else { 3 });
        if failure {
            assert!(
                !events
                    .iter()
                    .any(|e| e["type"] == "response.output_item.done")
            );
            assert_eq!(events.last().unwrap()["response"]["output"], json!([]));
        } else {
            assert_eq!(requests[1], requests[2]);
            assert_eq!(requests[1]["continuationToken"], "private-token");
        }
        assert!(!String::from_utf8_lossy(&bytes).contains("private-token"));
        upstream_task.abort();
        proxy_task.abort();
    }
}

#[tokio::test]
async fn continuation_timeout_and_cancellation_drop_upstream_without_retry() {
    use futures_util::StreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    for cancel in [false, true] {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let active_copy = active.clone();
        let (url,task)=serve(Router::new().fallback(move || {
            let n=calls.fetch_add(1,Ordering::SeqCst); let active=active_copy.clone();
            async move {
                let body=Body::from_stream(async_stream::stream! {
                    active.fetch_add(1,Ordering::SeqCst); let _guard=Active(active);
                    let native=if n==0 {json!({"candidates":[{"finishReason":"CONTINUATION","continuationToken":"one"}]})} else {json!({"candidates":[{"content":{"parts":[{"text":"pending"}]}}]})};
                    yield Ok::<_,std::io::Error>(Bytes::from(format!("data: {native}\n\n")));
                    if n>0 { std::future::pending::<()>().await; }
                });
                ([(header::CONTENT_TYPE,"text/event-stream")],body).into_response()
            }
        })).await;
        let config = Config {
            gemini: Some(provider(&url, "api_key")),
            ..Config::test_fixture()
        };
        let mut proxy = super::super::local_snapshot(config, None).unwrap();
        proxy.client = reqwest::Client::builder()
            .read_timeout(Duration::from_millis(80))
            .build()
            .unwrap();
        let (parts, _) = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .body(Body::empty())
            .unwrap()
            .into_parts();
        let response = forward(
            Arc::new(proxy),
            parts,
            json!({"model":"gemini/test","input":"test","stream":true}),
        )
        .await;
        let mut body = response.into_body().into_data_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = body.next().await {
            bytes.extend_from_slice(&chunk.unwrap());
            if cancel && String::from_utf8_lossy(&bytes).contains("pending") {
                break;
            }
        }
        drop(body);
        if !cancel {
            let events = NativeSse::default().feed(&bytes, true).unwrap();
            assert_eq!(events.last().unwrap()["type"], "response.failed");
            assert!(
                !events
                    .iter()
                    .any(|e| e["type"] == "response.output_item.done")
            );
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while active.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 2);
        task.abort();
    }
}

#[tokio::test]
async fn continuation_retains_structured_output_and_terminal_semantics() {
    for (finish, budget, expected) in [
        ("STOP", None, "completed"),
        ("MAX_TOKENS", None, "incomplete"),
        ("SAFETY", None, "failed"),
        ("STOP", Some(2), "incomplete"),
    ] {
        for streaming in [false, true] {
            let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let calls = count.clone();
            let (upstream,task)=serve(Router::new().fallback(move || {
                let n=calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
                async move {
                    let candidate=if n==0 {json!({"content":{"parts":[{"text":"{\"answer\":"}]},"finishReason":"CONTINUATION","continuationToken":"one"})} else {json!({"content":{"parts":[{"text":"42}"}]},"finishReason":finish})};
                    let native=json!({"candidates":[candidate],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2,"totalTokenCount":12}});
                    if streaming {( [(header::CONTENT_TYPE,"text/event-stream")],format!("data: {native}\n\n")).into_response()} else {axum::Json(native).into_response()}
                }
            })).await;
            let config = Config {
                gemini: Some(provider(&upstream, "api_key")),
                ..Config::test_fixture()
            };
            let (url, proxy) = serve(super::super::router(config).unwrap()).await;
            let mut request = json!({"model":"gemini/test","stream":streaming,"input":"test","text":{"format":{"type":"json_schema","strict":true,"name":"result","schema":{"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"],"additionalProperties":false}}}});
            if let Some(budget) = budget {
                request["max_output_tokens"] = json!(budget);
            }
            let bytes = reqwest::Client::new()
                .post(format!("{url}/v1/responses"))
                .json(&request)
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            let response = if streaming {
                let events = NativeSse::default().feed(&bytes, true).unwrap();
                for (n, event) in events.iter().enumerate() {
                    assert_eq!(event["sequence_number"], n);
                }
                if expected != "completed" {
                    assert!(
                        !events
                            .iter()
                            .any(|e| e["type"] == "response.output_item.done"
                                || e["type"] == "response.output_text.delta")
                    );
                }
                events.last().unwrap()["response"].clone()
            } else {
                serde_json::from_slice(&bytes).unwrap()
            };
            assert_eq!(response["status"], expected, "{response}");
            assert_eq!(
                count.load(std::sync::atomic::Ordering::SeqCst),
                if budget.is_some() { 1 } else { 2 }
            );
            if expected == "completed" {
                assert_eq!(
                    response["output"][1]["content"][0]["text"],
                    r#"{"answer":42}"#
                );
            }
            task.abort();
            proxy.abort();
        }
    }
}
