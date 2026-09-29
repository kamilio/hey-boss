use super::*;
use axum::routing::any;

fn input() -> Value {
    json!({"model":"friendly","max_tokens":128,"messages":[{"role":"user","content":"hello"}]})
}
fn text(value: &str) -> Value {
    json!({"type":"message","id":"out1","role":"assistant","content":[{"type":"output_text","text":value}]})
}
fn reply(output: Value) -> Value {
    json!({"id":"resp_test","created_at":123,"status":"completed","output":output,
        "usage":{"input_tokens":20,"output_tokens":6,"total_tokens":26,"input_tokens_details":{"cached_tokens":4,"cache_write_tokens":2},"output_tokens_details":{"reasoning_tokens":1}}})
}
async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (
        url,
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }),
    )
}
fn sse(events: &[Value]) -> Response {
    let bytes: Vec<_> = events
        .iter()
        .flat_map(|v| format!("data: {v}\r\n\r\n").into_bytes())
        .collect();
    // Exercise byte, UTF-8, JSON and CRLF partition boundaries.
    let chunks = bytes
        .into_iter()
        .map(|b| Ok::<_, std::io::Error>(Bytes::from(vec![b])));
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(futures_util::stream::iter(chunks)),
    )
        .into_response()
}
fn merge(events: &[Value]) -> Value {
    let mut message = events
        .iter()
        .find(|v| v["type"] == "message_start")
        .unwrap()["message"]
        .clone();
    let mut args = BTreeMap::<usize, String>::new();
    for event in events {
        let index = event["index"].as_u64().unwrap_or(0) as usize;
        match event["type"].as_str().unwrap() {
            "content_block_start" => message["content"]
                .as_array_mut()
                .unwrap()
                .push(event["content_block"].clone()),
            "content_block_delta" => {
                let delta = &event["delta"];
                match delta["type"].as_str().unwrap() {
                    "input_json_delta" => {
                        args.entry(index)
                            .or_default()
                            .push_str(delta["partial_json"].as_str().unwrap());
                    }
                    kind => {
                        let field = match kind {
                            "text_delta" => "text",
                            "thinking_delta" => "thinking",
                            "signature_delta" => "signature",
                            _ => panic!("unknown delta"),
                        };
                        let old = message["content"][index][field].as_str().unwrap_or("");
                        message["content"][index][field] =
                            json!(old.to_owned() + delta[field].as_str().unwrap());
                    }
                }
            }
            "message_delta" => {
                message["stop_reason"] = event["delta"]["stop_reason"].clone();
                message["usage"] = event["usage"].clone();
            }
            _ => {}
        }
    }
    for (index, json) in args {
        message["content"][index]["input"] = serde_json::from_str(&json).unwrap();
    }
    message
}
use std::collections::BTreeMap;

#[test]
fn messages_request_maps_system_images_tools_results_and_options() {
    let mut value = input();
    value["system"] =
        json!([{"type":"text","text":"Be useful","cache_control":{"type":"ephemeral"}}]);
    value["messages"] = json!([
        {"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]},
        {"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"lookup","input":{"q":"Paris"}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","is_error":true,"content":"not found"},{"type":"text","text":"Try again"}]}
    ]);
    value["tools"] = json!([{"name":"lookup","input_schema":{"type":"object","properties":{"q":{"type":"string"}}}}]);
    value["tool_choice"] = json!({"type":"tool","name":"lookup","disable_parallel_tool_use":true});
    value["output_config"] = json!({"effort":"high"});
    let body = request::convert(&value).unwrap();
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(
        body["messages"][1]["content"][0]["image_url"]["url"],
        "data:image/png;base64,AA=="
    );
    assert_eq!(body["messages"][2]["tool_calls"][0]["id"], "call_1");
    assert_eq!(body["messages"][3]["role"], "tool");
    assert_eq!(
        body["messages"][3]["content"][0]["text"],
        "Tool returned an error:"
    );
    assert_eq!(body["messages"][4]["content"][0]["text"], "Try again");
    assert_eq!(body["tools"][0]["function"]["strict"], false);
    assert_eq!(body["tool_choice"]["function"]["name"], "lookup");
    assert_eq!(body["parallel_tool_calls"], false);
    assert_eq!(body["reasoning"]["effort"], "high");
}

#[test]
fn messages_rejects_unsupported_and_malformed_requests_and_foreign_signatures() {
    for (key, value) in [
        ("max_tokens", json!(0)),
        ("max_tokens", json!(1.5)),
        ("messages", json!([])),
        ("messages", json!([false])),
        ("stream", json!("yes")),
        ("top_k", json!(5)),
        ("stop_sequences", json!(["stop"])),
        ("thinking", json!({"type":"enabled","budget_tokens":4000})),
        ("temperature", json!(2)),
        (
            "tools",
            json!([{"type":"web_search_20250305","name":"web_search"}]),
        ),
        (
            "messages",
            json!([{"role":"assistant","content":[{"type":"thinking","thinking":"private","signature":"foreign"}]}]),
        ),
        ("mcp_servers", json!([])),
        ("tool_choice", json!({"type":"tool"})),
    ] {
        let mut request = input();
        request[key] = value;
        assert!(request::convert(&request).is_err(), "{request}");
    }
    for value in [Value::Null, json!(1), json!([])] {
        assert!(request::convert(&value).is_err());
    }
}

#[tokio::test]
async fn messages_route_scopes_aliases_and_strips_anthropic_credentials() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let capture = seen.clone();
    let (upstream, upstream_task) = serve(Router::new().fallback(any(move |req: Request| {
        let capture = capture.clone();
        async move {
            let (parts, body) = req.into_parts();
            let body: Value =
                serde_json::from_slice(&axum::body::to_bytes(body, LIMIT).await.unwrap()).unwrap();
            if parts.uri.path() == "/v1/responses" {
                assert!(!parts.headers.contains_key("x-api-key"));
                assert!(!parts.headers.contains_key("anthropic-version"));
                assert_eq!(
                    parts.headers["authorization"],
                    "Bearer replace-with-primary-api-key"
                );
            }
            capture.lock().unwrap().push((parts.uri.to_string(), body));
            axum::Json(reply(json!([text("héllo 🦀")])))
        }
    })))
    .await;
    let config=Config {upstream_url:upstream,aliases:serde_json::from_value(json!([
        {"from":"friendly","to":"wrong","api_shape":"responses"},
        {"from":"friendly","to":"target","api_shape":"messages","api_key":"primary","reasoning":"high","reasoning_routes":{"low":{"to":"low-target"}}}
    ])).unwrap(),..Config::test_fixture()};
    let (url, task) = serve(router(config).unwrap()).await;
    let client = reqwest::Client::new();
    for path in ["/v1/custom/messages", "/custom/v1/messages/"] {
        let mut body = input();
        body["output_config"] = json!({"effort":"low"});
        let response = client
            .post(format!("{url}{path}"))
            .header("x-api-key", "caller-secret")
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let message: Value = response.json().await.unwrap();
        assert_eq!(message["type"], "message");
        assert_eq!(message["model"], "friendly");
        assert_eq!(message["content"][0]["text"], "héllo 🦀");
        assert_eq!(message["stop_reason"], "end_turn");
        assert_eq!(
            message["usage"],
            json!({"input_tokens":14,"output_tokens":6,"cache_creation_input_tokens":2,"cache_read_input_tokens":4})
        );
    }
    client
        .post(format!("{url}/v1/messages"))
        .json(&input())
        .send()
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    for (_, body) in &seen[..2] {
        assert_eq!(body["model"], "low-target");
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["max_output_tokens"], 128);
    }
    assert_eq!(seen[2].0, "/v1/messages");
    assert_eq!(seen[2].1, input());
    task.abort();
    upstream_task.abort();
}

#[tokio::test]
async fn messages_stream_handles_unicode_parallel_tools_thinking_and_usage() {
    let details = json!([{"type":"reasoning.summary","summary":"Plan","id":"rs_1","format":"openai-responses-v1","index":0},
        {"type":"reasoning.encrypted","data":"opaque","id":"rs_1","format":"openai-responses-v1","index":2}]);
    let mut chunks = vec![json!({"id":"chat_test","choices":[{"delta":{"role":"assistant"}}]})];
    for delta in [
        json!({"reasoning":"Plan","reasoning_details":[details[0].clone()]}),
        json!({"content":"héllo 🦀"}),
        json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"one","arguments":"{\"x\":"}},{"index":1,"id":"b","function":{"name":"two","arguments":"{"}}]}),
        json!({"tool_calls":[{"index":1,"function":{"arguments":"}"}},{"index":0,"function":{"arguments":"1}"}}]}),
        json!({"reasoning_details":[details[1].clone()]}),
    ] {
        chunks.push(json!({"id":"chat_test","choices":[{"delta":delta}]}));
    }
    chunks.push(json!({"id":"chat_test","choices":[{"delta":{},"finish_reason":"tool_calls"}]}));
    chunks.push(json!({"id":"chat_test","choices":[],"usage":{"prompt_tokens":20,"completion_tokens":6,"prompt_tokens_details":{"cached_tokens":4,"cache_write_tokens":2}}}));
    let result = stream::adapt(sse(&chunks), "friendly".into(), None);
    let bytes = axum::body::to_bytes(result.into_body(), LIMIT)
        .await
        .unwrap();
    assert!(!bytes.windows(6).any(|v| v == b"[DONE]"));
    let events = super::super::sse::SseDecoder::default()
        .feed(&bytes, true)
        .unwrap();
    assert_eq!(events.last().unwrap()["type"], "message_stop");
    let message = merge(&events);
    assert_eq!(message["content"][1]["text"], "héllo 🦀");
    assert_eq!(message["content"][2]["input"], json!({"x":1}));
    assert_eq!(message["content"][3]["input"], json!({}));
    assert_eq!(message["usage"]["input_tokens"], 14);
    assert_eq!(message["stop_reason"], "tool_use");
    let replay=request::convert(&json!({"model":"friendly","max_tokens":128,"messages":[{"role":"assistant","content":message["content"]}]})).unwrap();
    assert_eq!(replay["messages"][0]["reasoning_details"], details);
    let mut altered = message["content"].clone();
    altered[0]["thinking"] = json!("different");
    assert!(request::convert(&json!({"model":"friendly","max_tokens":128,"messages":[{"role":"assistant","content":altered}]})).is_err());
}

#[tokio::test]
async fn messages_stream_failure_never_emits_success_and_is_logged() {
    for chunks in [
        vec![],
        vec![json!({"error":{"message":"failed"}})],
        vec![json!({"id":"c","choices":[{"delta":{"content":"partial"}}]})],
        vec![
            json!({"id":"c","choices":[{"delta":{"tool_calls":[{"index":0,"id":"t","function":{"name":"x","arguments":"{"}}]},"finish_reason":"length"}],"usage":{"completion_tokens":10}}),
        ],
    ] {
        let logs = Arc::new(logs::Store::default());
        let id = logs.begin("POST", "/custom/v1/messages", "HTTP");
        let result = stream::adapt(sse(&chunks), "friendly".into(), Some((logs.clone(), id)));
        let bytes = axum::body::to_bytes(result.into_body(), LIMIT)
            .await
            .unwrap();
        let events = super::super::sse::SseDecoder::default()
            .feed(&bytes, true)
            .unwrap();
        assert_eq!(events.last().unwrap()["type"], "error");
        assert!(!events.iter().any(|e| e["type"] == "message_stop"));
        assert_eq!(
            logs.recent()[0].error_code.as_deref(),
            Some("messages_stream_error")
        );
    }
}

#[tokio::test]
async fn messages_host_auth_errors_and_client_relay_use_the_same_facade() {
    let (upstream, upstream_task) =
        serve(Router::new().fallback(any(|| async { axum::Json(reply(json!([text("OK")]))) })))
            .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("proxy.json");
    let keys = crate::access::ensure(&path, &[]).unwrap();
    let config = Config {
        mode: config::Mode::Host,
        upstream_url: upstream,
        ..Config::test_fixture()
    };
    let (host, host_task) = serve(
        router_with(
            config,
            Options {
                access_config: Some(path),
                ..Options::default()
            },
        )
        .unwrap(),
    )
    .await;
    let http = reqwest::Client::new();
    let denied = http
        .post(format!("{host}/custom/v1/messages"))
        .json(&input())
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
    assert_eq!(
        denied.json::<Value>().await.unwrap()["error"]["type"],
        "authentication_error"
    );
    let allowed = http
        .post(format!("{host}/custom/v1/messages"))
        .header("x-api-key", &keys.local)
        .json(&input())
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);
    let config = Config {
        mode: config::Mode::Client,
        connection: Some(config::ClientConnection {
            url: host,
            api_key: keys.local,
        }),
        ..Config::default()
    };
    let (relay, relay_task) = serve(router(config).unwrap()).await;
    let result = http
        .post(format!("{relay}/custom/v1/messages"))
        .json(&input())
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), 200);
    assert_eq!(
        result.json::<Value>().await.unwrap()["content"][0]["text"],
        "OK"
    );
    for task in [upstream_task, host_task, relay_task] {
        task.abort();
    }
}

#[tokio::test]
async fn messages_gemini_is_native_counts_tokens_and_replays_signed_tools() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let upstream=Router::new().fallback(any(move |request:Request| {
        let captured=captured.clone();
        async move {
            let path=request.uri().path().to_owned();
            assert!(path.starts_with("/models/test:"),"Messages must never use Responses: {path}");
            let body:Value=serde_json::from_slice(&axum::body::to_bytes(request.into_body(),LIMIT).await.unwrap()).unwrap();
            assert!(body.get("input").is_none());
            captured.lock().unwrap().push((path.clone(),body.clone()));
            if path.ends_with(":countTokens"){assert!(body["generateContentRequest"]["contents"].is_array());return axum::Json(json!({"totalTokens":12345})).into_response();}
            let replay=body["contents"].as_array().unwrap().iter().any(|c|c["parts"].as_array().unwrap().iter().any(|p|p.get("functionResponse").is_some()));
            let parts=if replay {
                assert_eq!(body["contents"][1]["parts"][0]["thoughtSignature"],"signed-tool");
                assert_eq!(body["contents"][2]["parts"][0]["functionResponse"]["response"]["result"],"file contents");
                json!([{"text":"File reviewed.","thoughtSignature":"signed-text"}])
            }else{json!([{"functionCall":{"name":body["tools"][0]["functionDeclarations"][0]["name"],"args":{"path":"file.txt"}},"thoughtSignature":"signed-tool"}])};
            let native=json!({"candidates":[{"index":0,"content":{"role":"model","parts":parts},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2000,"cachedContentTokenCount":1000,"candidatesTokenCount":20,"thoughtsTokenCount":30}});
            if path.ends_with(":streamGenerateContent"){sse(&[native])}else{axum::Json(native).into_response()}
        }
    }));
    let (upstream, upstream_task) = serve(upstream).await;
    let config = Config {
        gemini: Some(
            serde_json::from_value(json!({"upstream_url":upstream,"api_key":"synthetic"})).unwrap(),
        ),
        aliases: serde_json::from_value(
            json!([{"from":"friendly","to":"gemini/test","api_shape":"messages"}]),
        )
        .unwrap(),
        ..Config::test_fixture()
    };
    let (url, task) = serve(router(config).unwrap()).await;
    let http = reqwest::Client::new();
    for streaming in [false, true] {
        let mut request = input();
        request["stream"] = json!(streaming);
        request["tools"] = json!([{"name":"Read","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}}]);
        let result = http
            .post(format!("{url}/custom/v1/messages"))
            .json(&request)
            .send()
            .await
            .unwrap();
        let status = result.status();
        let bytes = result.bytes().await.unwrap();
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
        let message = if streaming {
            let events = super::super::sse::SseDecoder::default()
                .feed(&bytes, true)
                .unwrap();
            assert_eq!(events.last().unwrap()["type"], "message_stop");
            merge(&events)
        } else {
            serde_json::from_slice::<Value>(&bytes).unwrap()
        };
        assert_eq!(message["stop_reason"], "tool_use");
        assert_eq!(message["content"][0]["name"], "Read");
        assert_eq!(message["usage"]["output_tokens"], 50);
        assert_eq!(message["usage"]["input_tokens"], 1000);
        request["messages"].as_array_mut().unwrap().extend([json!({"role":"assistant","content":message["content"]}),json!({"role":"user","content":[{"type":"tool_result","tool_use_id":message["content"][0]["id"],"content":"file contents"}]})]);
        request["messages"][1]["content"][0]["cache_control"] = json!({"type":"ephemeral"});
        request["stream"] = json!(false);
        let result = http
            .post(format!("{url}/custom/v1/messages"))
            .json(&request)
            .send()
            .await
            .unwrap();
        let status = result.status();
        let result: Value = result.json().await.unwrap();
        assert_eq!(status, 200, "{result}");
        assert_eq!(result["content"][0]["text"], "File reviewed.");
        let logs: Value = http
            .get(format!("{url}/logs/api?local=true"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let entry = &logs["entries"][0];
        assert_eq!(
            entry["input_tokens"], 2000,
            "Dashboard must include cache reads"
        );
        assert_eq!(entry["cached_input_tokens"], 1000);
        assert_eq!(entry["output_tokens"], 50);

        request["messages"][1]["content"][0]["input"]["path"] = json!("tampered.txt");
        let result = http
            .post(format!("{url}/custom/v1/messages"))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(result.status(), 400);
    }
    let mut count = input();
    count.as_object_mut().unwrap().remove("max_tokens");
    let result = http
        .post(format!("{url}/custom/v1/messages/count_tokens"))
        .json(&count)
        .send()
        .await
        .unwrap();
    let status = result.status();
    let result: Value = result.json().await.unwrap();
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["input_tokens"], 12345);
    assert_eq!(seen.lock().unwrap().len(), 5);
    task.abort();
    upstream_task.abort();
}

#[test]
fn native_messages_rejects_missing_terminal_usage_and_partial_tools() {
    let codec = hey_proxy::gemini::ReasoningCodec::new(&[7; 32]);
    let provider: hey_proxy::gemini::ProviderConfig =
        serde_json::from_value(json!({"api_key":"synthetic"})).unwrap();
    let request =
        gemini_request::convert(&input(), "gemini/test", None, &provider, &codec, false).unwrap();
    for native in [
        json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}],"usageMetadata":{"promptTokenCount":1}}),
        json!({"candidates":[{"finishReason":"STOP"}]}),
        json!({"candidates":[{"content":{"parts":[{"text":"complete"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1}}),
        json!({"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"trafficType":"ON_DEMAND"}}),
        json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"unknown","args":{}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1}}),
    ] {
        let mut stream = gemini_stream::NativeStream::new("friendly");
        assert!(
            stream
                .feed(&native, &request)
                .and_then(|_| stream.end(&request, &codec))
                .is_err()
        );
    }
}

#[test]
fn native_messages_separates_tool_results_from_continuation_text() {
    let codec = hey_proxy::gemini::ReasoningCodec::new(&[7; 32]);
    let provider: hey_proxy::gemini::ProviderConfig =
        serde_json::from_value(json!({"api_key":"synthetic"})).unwrap();
    for system_update in [false, true] {
        let mut value = input();
        value["messages"] = json!([
            {"role":"user","content":"Review files"},
            {"role":"assistant","content":[
                {"type":"tool_use","id":"a","name":"Read","input":{"path":"a"}},
                {"type":"tool_use","id":"b","name":"Read","input":{"path":"b"}}
            ]},
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"a","content":"A"},
                {"type":"tool_result","tool_use_id":"b","content":"B"}
            ]}
        ]);
        if system_update {
            value["messages"]
                .as_array_mut()
                .unwrap()
                .push(json!({"role":"system","content":"Continue after compaction"}));
        } else {
            value["messages"][2]["content"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"text","text":"Continue after compaction"}));
        }
        let request =
            gemini_request::convert(&value, "gemini/test", None, &provider, &codec, false).unwrap();
        let turns = request.body["contents"].as_array().unwrap();
        assert_eq!(turns.len(), 4);
        assert_eq!(turns[2]["role"], "user");
        let results = turns[2]["parts"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|p| p.get("functionResponse").is_some()));
        assert_eq!(turns[3]["role"], "user");
        assert!(turns[3]["parts"][0]["text"].is_string());
        if system_update {
            assert!(
                request.body["systemInstruction"]
                    .to_string()
                    .contains("Continue after compaction")
            );
        }
    }
    let mut value = input();
    value["stop_sequences"] = json!(["stop"]);
    assert!(
        gemini_request::convert(&value, "gemini/test", None, &provider, &codec, false).is_err()
    );
}

#[test]
fn native_messages_partial_tools_and_omitted_thinking_preserve_signed_replay() {
    let codec = hey_proxy::gemini::ReasoningCodec::new(&[7; 32]);
    let provider: hey_proxy::gemini::ProviderConfig =
        serde_json::from_value(json!({"api_key":"synthetic"})).unwrap();
    for omitted in [false, true] {
        let mut value = input();
        value["thinking"] =
            json!({"type":"adaptive","display":if omitted {"omitted"} else {"summarized"}});
        value["tools"] = json!([{"name":"Read","input_schema":{"type":"object"}}]);
        let request =
            gemini_request::convert(&value, "gemini/test", None, &provider, &codec, false).unwrap();
        assert_eq!(
            request.body["generationConfig"]["thinkingConfig"]["includeThoughts"],
            !omitted
        );
        let name = &request.body["tools"][0]["functionDeclarations"][0]["name"];
        let mut state = gemini_stream::NativeStream::new("friendly");
        let mut events = Vec::new();
        for chunk in [
            json!({"candidates":[{"content":{"parts":[{"text":"Plan","thought":true}]}}],"usageMetadata":{"trafficType":"ON_DEMAND"}}),
            json!({"candidates":[{"content":{"parts":[{"functionCall":{"id":"a","name":name,"partialArgs":[{"jsonPath":"$.path","stringValue":"file","willContinue":true}],"willContinue":true},"thoughtSignature":"signed"}]}}]}),
            json!({"candidates":[{"content":{"parts":[{"functionCall":{"id":"a","partialArgs":[{"jsonPath":"$.path","stringValue":".txt"}]}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":40,"candidatesTokenCount":5,"thoughtsTokenCount":20}}),
        ] {
            events.extend(state.feed(&chunk, &request).unwrap());
        }
        events.extend(state.end(&request, &codec).unwrap());
        let message = merge(&events);
        assert_eq!(message, state.message);
        assert_eq!(message["usage"]["input_tokens"], 60);
        assert_eq!(message["usage"]["output_tokens"], 25);
        let blocks = message["content"].as_array().unwrap();
        assert_eq!(blocks.iter().any(|b| b["type"] == "thinking"), !omitted);
        let tool = blocks.iter().find(|b| b["type"] == "tool_use").unwrap();
        assert_eq!(tool["input"], json!({"path":"file.txt"}));
        let carrier = blocks
            .iter()
            .find_map(|b| b["signature"].as_str().or_else(|| b["data"].as_str()))
            .unwrap();
        assert!(codec.open_messages("different", carrier).is_err());
        assert!(codec.replay_items("test", carrier).is_err());
        value["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":blocks}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":tool["id"],"content":"done"}]}),
            json!({"role":"system","content":"Post-compaction reminder"}),
        ]);
        let replay =
            gemini_request::convert(&value, "gemini/test", None, &provider, &codec, false).unwrap();
        assert_eq!(
            replay.body["contents"][1]["parts"][1]["thoughtSignature"],
            "signed"
        );
        assert_eq!(replay.body["contents"].as_array().unwrap().len(), 4);
    }
}
