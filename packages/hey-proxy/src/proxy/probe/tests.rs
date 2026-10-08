use super::*;
use axum::body::to_bytes;

#[test]
fn probe_matches_only_the_complete_latest_user_message() {
    for (shape, field, kind) in [
        (Shape::Responses, "input", "input_text"),
        (Shape::Chat, "messages", "text"),
        (Shape::Messages, "messages", "text"),
    ] {
        for content in [json!(PROMPT), json!([{"type":kind,"text":PROMPT}])] {
            let input = json!({field:[{"role":"system","content":"instructions"},{"role":"assistant","content":"earlier reply"},{"role":"user","content":content}]});
            assert!(matches(shape, &input));
        }
        for content in [
            json!(" hello-hey-proxy"),
            json!("hello-hey-proxy\n"),
            json!("HELLO-HEY-PROXY"),
            json!("Please say hello-hey-proxy"),
            json!("`hello-hey-proxy`"),
            json!([{"type":kind,"text":PROMPT},{"type":kind,"text":""}]),
            json!([{"type":kind,"text":PROMPT},{"type":"image_url","image_url":{"url":"image"}}]),
            json!([{"type":"tool_result","content":PROMPT}]),
        ] {
            assert!(!matches(
                shape,
                &json!({field:[{"role":"user","content":content}]})
            ));
        }
        for role in ["assistant", "system", "developer", "tool"] {
            assert!(!matches(
                shape,
                &json!({field:[{"role":role,"content":PROMPT}]})
            ));
        }
        assert!(!matches(
            shape,
            &json!({field:[{"role":"user","content":PROMPT},{"role":"assistant","content":"next"}]})
        ));
        assert!(!matches(
            shape,
            &json!({field:[{"type":"function_call_output","output":PROMPT}]})
        ));
        assert!(!matches(
            shape,
            &json!({field:[{"role":"user","content":PROMPT,"tool_calls":[]}]})
        ));
    }
    assert!(matches(Shape::Responses, &json!({"input":PROMPT})));
    assert!(!matches(
        Shape::Responses,
        &json!({"input":"prefix hello-hey-proxy"})
    ));
    assert!(matches(
        Shape::Gemini,
        &json!({"contents":[{"role":"user","parts":[{"text":PROMPT}]}]})
    ));
    for message in [
        json!({"role":"model","parts":[{"text":PROMPT}]}),
        json!({"role":"user","parts":[{"text":PROMPT},{"inlineData":{}}]}),
        json!({"role":"user","parts":[{"text":PROMPT,"functionResponse":{}}]}),
    ] {
        assert!(!matches(Shape::Gemini, &json!({"contents":[message]})));
    }
}

async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, task)
}

#[tokio::test]
async fn probe_returns_native_json_and_complete_streams_without_credentials() {
    let config = Config {
        skip_blocked_security_work: true,
        ..Config::default()
    };
    let (url, task) = serve(router(config).unwrap()).await;
    let client = reqwest::Client::new();
    for (path, body, pointer, terminal) in [
        (
            "/v1/responses",
            json!({"model":"anything","input":PROMPT}),
            "/output/0/content/0/text",
            "response.completed",
        ),
        (
            "/v1/chat/completions",
            json!({"model":"anything","messages":[{"role":"user","content":PROMPT}]}),
            "/choices/0/message/content",
            "",
        ),
        (
            "/v1/custom/chat/completions",
            json!({"model":"anything","messages":[{"role":"user","content":[{"type":"text","text":PROMPT}]}]}),
            "/choices/0/message/content",
            "",
        ),
        (
            "/v1/messages",
            json!({"model":"anything","messages":[{"role":"user","content":PROMPT}]}),
            "/content/0/text",
            "message_stop",
        ),
        (
            "/v1/custom/messages",
            json!({"model":"anything","messages":[{"role":"user","content":PROMPT}]}),
            "/content/0/text",
            "message_stop",
        ),
        (
            "/custom/v1/messages?beta=true",
            json!({"model":"anything","messages":[{"role":"user","content":PROMPT}]}),
            "/content/0/text",
            "message_stop",
        ),
        (
            "/v1beta/models/gemini:generateContent",
            json!({"contents":[{"role":"user","parts":[{"text":PROMPT}]}]}),
            "/candidates/0/content/parts/0/text",
            "",
        ),
    ] {
        let response = client
            .post(format!("{url}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()["x-hey-proxy-probe"], "true");
        let value: Value = response.json().await.unwrap();
        assert_eq!(value.pointer(pointer), Some(&json!(REPLY)));
        let mut body = body;
        body["stream"] = json!(true);
        body["stream_options"] = json!({"include_usage":true});
        let path = path.replace(":generateContent", ":streamGenerateContent?alt=sse");
        let response = client
            .post(format!("{url}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let text = response.text().await.unwrap();
        let mut decoder = sse::SseDecoder::default();
        let events = decoder.feed(text.as_bytes(), true).unwrap();
        assert!(text.contains(REPLY));
        if !terminal.is_empty() {
            assert_eq!(events.last().unwrap()["type"], terminal);
        } else if path.contains("chat/completions") {
            assert!(text.ends_with("data: [DONE]\n\n"));
            assert_eq!(events[1]["choices"][0]["finish_reason"], "stop");
            assert_eq!(events[2]["usage"]["total_tokens"], 0);
        }
        if path == "/v1/responses" {
            for (index, event) in events.iter().enumerate() {
                assert_eq!(event["sequence_number"], index);
            }
            assert_eq!(
                events.last().unwrap()["response"]["output"][0]["content"][0]["text"],
                REPLY
            );
        }
    }
    task.abort();
}

#[tokio::test]
async fn non_probes_preserve_bytes_and_normal_upstream_routing() {
    let (upstream_url, upstream) = serve(Router::new().fallback(|request: Request| async move {
        to_bytes(request.into_body(), usize::MAX).await.unwrap()
    }))
    .await;
    let (url, task) = serve(
        router(Config {
            upstream_url,
            ..Config::test_fixture()
        })
        .unwrap(),
    )
    .await;
    let client = reqwest::Client::new();
    let bytes = r#"{ "model": "gpt-5", "input": "Explain hello-hey-proxy" }"#;
    let response = client
        .post(format!("{url}/v1/responses"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("x-hey-proxy-probe"));
    assert_eq!(response.text().await.unwrap(), bytes);
    for (method, path, content_type, encoding) in [
        (
            "POST",
            "/v1/responses/compact",
            "application/json",
            "identity",
        ),
        (
            "POST",
            "/v1/messages/count_tokens",
            "application/json",
            "identity",
        ),
        (
            "POST",
            "/v1/images/generations",
            "application/json",
            "identity",
        ),
        ("GET", "/v1/responses", "application/json", "identity"),
        (
            "POST",
            "/v1/responses",
            "application/octet-stream",
            "identity",
        ),
        ("POST", "/v1/responses", "application/json", "gzip"),
    ] {
        let bytes =
            json!({"input":PROMPT,"messages":[{"role":"user","content":PROMPT}]}).to_string();
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_ENCODING, encoding)
            .body(Body::from(bytes.clone()))
            .unwrap();
        let request = intercept(&Config::default(), request).await.unwrap();
        assert_eq!(
            to_bytes(request.into_body(), usize::MAX).await.unwrap(),
            bytes
        );
    }
    task.abort();
    upstream.abort();
}

#[tokio::test]
async fn probe_requires_host_authentication() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let keys = crate::access::ensure(&path, &[]).unwrap();
    let app = router_with(
        Config {
            mode: Mode::Host,
            ..Config::default()
        },
        Options {
            access_config: Some(path),
            ..Options::default()
        },
    )
    .unwrap();
    let (url, task) = serve(app).await;
    let client = reqwest::Client::new();
    let request = client
        .post(format!("{url}/v1/responses"))
        .json(&json!({"model":"probe","input":PROMPT}));
    assert_eq!(
        request.try_clone().unwrap().send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let response = request.bearer_auth(keys.local).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-hey-proxy-probe"], "true");
    task.abort();
}

#[tokio::test]
async fn probe_inspection_preserves_large_bodies_and_trailers() {
    let large = vec![b' '; INSPECTION_LIMIT + 1];
    let request = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(large.clone()))
        .unwrap();
    let request = intercept(&Config::default(), request).await.unwrap();
    assert_eq!(request.body().size_hint().exact(), Some(large.len() as u64));
    assert_eq!(
        to_bytes(request.into_body(), usize::MAX)
            .await
            .unwrap()
            .as_ref(),
        large
    );

    let mut trailers = HeaderMap::new();
    trailers.insert("x-checksum", "preserved".parse().unwrap());
    let frames = VecDeque::from([
        Frame::data(Bytes::from_static(br#"{"input":"ordinary message"}"#)),
        Frame::trailers(trailers.clone()),
    ]);
    let request = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::new(InspectedBody {
            frames,
            inner: Body::empty(),
        }))
        .unwrap();
    let mut body = intercept(&Config::default(), request)
        .await
        .unwrap()
        .into_body();
    let data = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
        .await
        .unwrap()
        .unwrap();
    assert!(data.is_data());
    let frame = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frame.trailers_ref(), Some(&trailers));
}

#[tokio::test]
async fn route_probe_reports_alias_and_effort_without_upstream_credentials() {
    let config = Config {
        default: crate::config::DefaultRoute { api_key: "fast-key".into() },
        api_keys: std::collections::BTreeMap::from([("fast-key".into(), "sh://exit 1".into())]),
        aliases: vec![serde_json::from_value(json!({"from":"requested","to":"normal","reasoning":"high","reasoning_routes":{"low":{"to":"openai/fast","api_key":"fast-key"}}})).unwrap()],
        ..Config::default()
    };
    let (url, task) = serve(router(config).unwrap()).await;
    let response = reqwest::Client::new().post(format!("{url}/v1/responses"))
        .json(&json!({"model":"requested","reasoning":{"effort":"low"},"input":"hello-hey-proxy-model"})).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = response.json().await.unwrap();
    let text = value["output"][0]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("requested -> openai/fast"), "{text}");
    assert!(text.contains("API key alias: fast-key"), "{text}");
    assert!(text.contains("Reasoning: low -> high"), "{text}");
    assert!(text.contains("No upstream request"), "{text}");
    task.abort();
}

#[test]
fn model_probe_has_the_same_exact_match_rules() {
    for (shape, field, kind) in [
        (Shape::Responses, "input", "input_text"),
        (Shape::Chat, "messages", "text"),
        (Shape::Messages, "messages", "text"),
    ] {
        assert!(matches_prompt(
            shape,
            &json!({field:[{"role":"user","content":MODEL_PROMPT}]}),
            MODEL_PROMPT
        ));
        assert!(matches_prompt(
            shape,
            &json!({field:[{"role":"user","content":[{"type":kind,"text":MODEL_PROMPT}]}]}),
            MODEL_PROMPT
        ));
        for content in [
            json!(format!("{MODEL_PROMPT}\n")),
            json!(format!("prefix {MODEL_PROMPT}")),
            json!([{"type":kind,"text":MODEL_PROMPT},{"type":"image","source":{}}]),
        ] {
            assert!(!matches_prompt(
                shape,
                &json!({field:[{"role":"user","content":content}]}),
                MODEL_PROMPT
            ));
        }
        assert!(!matches_prompt(
            shape,
            &json!({field:[{"role":"assistant","content":MODEL_PROMPT}]}),
            MODEL_PROMPT
        ));
        assert!(!matches_prompt(
            shape,
            &json!({field:[{"role":"user","content":MODEL_PROMPT},{"role":"tool","content":"next"}]}),
            MODEL_PROMPT
        ));
    }
}

#[test]
fn route_probe_respects_api_shapes_native_routes_and_client_relays() {
    let mut config = Config {
        aliases: vec![
            serde_json::from_value(
                json!({"from":"coding","to":"gemini/fast","api_shape":"responses"}),
            )
            .unwrap(),
            serde_json::from_value(
                json!({"from":"coding","to":"openai/other","api_shape":"chat_completions"}),
            )
            .unwrap(),
            serde_json::from_value(json!({"from":"models/native","to":"gemini/native-fast"}))
                .unwrap(),
        ],
        ..Config::default()
    };
    let input = json!({"model":"coding"});
    assert!(routing::describe(&config, "/v1/responses", &input).contains("coding -> gemini/fast"));
    assert!(
        routing::describe(&config, "/v1/custom/chat/completions", &input)
            .contains("coding -> openai/other")
    );
    assert!(
        routing::describe(&config, "/v1beta/models/native:generateContent", &json!({}))
            .contains("native -> gemini/native-fast")
    );
    config.claude = Some(Default::default());
    let text = routing::describe(&config, "/v1/messages", &input);
    assert!(text.contains("coding -> claude/coding"), "{text}");
    config.mode = Mode::Client;
    config.upstream_url = "https://host.example".into();
    let text = routing::describe(&config, "/v1/responses", &input);
    assert!(
        text.contains("client relay -> https://host.example/v1/responses"),
        "{text}"
    );
    assert!(text.contains("resolved by the host"));
    assert!(!text.contains("gemini/fast"));
}

#[test]
fn route_probe_never_exposes_credentials_or_claims_fallbacks_were_used() {
    let mut config = Config {
        upstream_url: "https://user:secret@upstream.example/root?key=query-secret#fragment-secret"
            .into(),
        ..Config::default()
    };
    config
        .api_keys
        .insert("default".into(), "sh://credential-secret".into());
    config.fallbacks.insert(
        "primary".into(),
        vec!["backup".into(), "gemini/last".into()],
    );
    let text = routing::describe(&config, "/v1/responses", &json!({"model":"primary"}));
    assert!(
        text.contains("https://upstream.example/root/v1/responses"),
        "{text}"
    );
    assert!(
        text.contains("(not attempted): backup, gemini/last"),
        "{text}"
    );
    assert!(!text.contains("secret"), "{text}");
    assert!(!text.contains("user:"), "{text}");
}

#[tokio::test]
async fn route_probe_streams_all_protocols_locally() {
    let (url, task) = serve(router(Config::default()).unwrap()).await;
    for (path, body) in [
        (
            "/v1/responses",
            json!({"model":"coding","input":MODEL_PROMPT,"stream":true}),
        ),
        (
            "/v1/chat/completions",
            json!({"model":"coding","messages":[{"role":"user","content":MODEL_PROMPT}],"stream":true}),
        ),
        (
            "/v1/custom/chat/completions",
            json!({"model":"coding","messages":[{"role":"user","content":MODEL_PROMPT}],"stream":true}),
        ),
        (
            "/custom/v1/messages",
            json!({"model":"coding","messages":[{"role":"user","content":MODEL_PROMPT}],"stream":true}),
        ),
        (
            "/v1/messages",
            json!({"model":"coding","messages":[{"role":"user","content":MODEL_PROMPT}],"stream":true}),
        ),
        (
            "/v1beta/models/coding:streamGenerateContent?alt=sse",
            json!({"contents":[{"role":"user","parts":[{"text":MODEL_PROMPT}]}]}),
        ),
    ] {
        let response = reqwest::Client::new()
            .post(format!("{url}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()["x-hey-proxy-probe"], "true");
        let text = response.text().await.unwrap();
        let events = sse::SseDecoder::default()
            .feed(text.as_bytes(), true)
            .unwrap();
        assert!(!events.is_empty());
        assert!(text.contains("Route: coding ->"), "{path}: {text}");
        assert!(text.contains("No upstream request"));
    }
    task.abort();
}

#[tokio::test]
async fn document_text_never_becomes_a_probe_after_adapter_conversion() {
    let (url, task) = serve(router(Config::default()).unwrap()).await;
    let response = reqwest::Client::new().post(format!("{url}/custom/v1/messages"))
        .json(&json!({"model":"coding","max_tokens":100,"messages":[{"role":"user","content":[{"type":"document","source":{"type":"text","media_type":"text/plain","data":MODEL_PROMPT}}]}]}))
        .send().await.unwrap();
    assert!(!response.headers().contains_key("x-hey-proxy-probe"));
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    task.abort();
}

#[test]
fn model_probe_supports_bound_codex_responses_path() {
    let config = Config {
        codex: Some(Default::default()),
        upstream_url: "https://chatgpt.com/backend-api/codex".into(),
        ..Config::default()
    };
    let input = json!({"model":"gpt-6-astra","input":MODEL_PROMPT});
    let text = routing::describe(&config, "/responses", &input);
    assert!(text.contains("gpt-6-astra -> codex/gpt-6-astra"), "{text}");
    assert!(
        text.contains("https://chatgpt.com/backend-api/codex/responses"),
        "{text}"
    );
    assert!(response(&config, "/responses", &input).is_some());
}

#[tokio::test]
async fn model_probe_reports_unavailable_credentials_with_the_configured_plan() {
    let config: Config = serde_json::from_value(json!({
        "listen":"127.0.0.1:8080", "account_schema_version":1,
        "default":{"api_key":"alpha"},
        "api_keys":{"alpha":"sh://exit 1"},
        "accounts":{
            "codex-subscription":{"implementation":"codex","auth":"subscription","credentials_file":"/missing/probe-oauth.json"},
            "openai-api":{"implementation":"openai","auth":"api","endpoint":"https://api.openai.com","credential":"file:///missing/probe-api.key"}
        },
        "routes":[{"model":"gpt-6.1-sol","api_shape":"responses","legs":[
            {"provider":"codex-subscription"}, {"provider":"openai-api","override":"fallback"}
        ]}],
        "overrides":{"openai-api":{"fallback":{"from":"gpt-6.1-sol","reasoning":"high",
            "reasoning_routes":{"medium":{"to":"fallback-model"}}
        }}}
    })).unwrap();
    let input = json!({"model":"gpt-6.1-sol","reasoning":{"effort":"medium"},"input":MODEL_PROMPT});
    let unmatched = routing::describe(&config, "/v1/chat/completions", &input);
    assert!(unmatched.contains("Rule: passthrough"), "{unmatched}");
    let (url, task) = serve(router(config).unwrap()).await;
    let client = reqwest::Client::new();
    for stream in [false, true] {
        let mut input = input.clone();
        input["stream"] = json!(stream);
        let response = client
            .post(format!("{url}/v1/responses"))
            .json(&input)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-hey-proxy-probe"], "true");
        let body = response.text().await.unwrap();
        let text = if stream {
            body
        } else {
            serde_json::from_str::<Value>(&body).unwrap()["output"][0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let first = text
            .find("1. codex-subscription -> codex/gpt-6.1-sol (included subscription)")
            .expect(&text);
        let second = text
            .find("2. openai-api -> openai/fallback-model (pay per token)")
            .expect(&text);
        assert!(first < second, "{text}");
        for expected in [
            "Rule: ordered provider route",
            "Reasoning: medium -> medium",
            "Reasoning: medium -> high",
            "Using: none",
            "credentials or account identity are unavailable",
            "No model request was made.",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        for hidden in [
            "alpha",
            "passthrough",
            "/missing",
            "probe-oauth",
            "probe-api",
        ] {
            assert!(!text.contains(hidden), "{text}");
        }
    }
    task.abort();
}
