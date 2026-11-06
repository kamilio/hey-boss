use super::*;

#[test]
fn discovered_models_use_the_exact_active_profile_for_each_family() {
    let page = json!({"inferenceProfileSummaries":[
        {"status":"ACTIVE","inferenceProfileId":"us.openai.gpt-future"},
        {"status":"ACTIVE","inferenceProfileId":"global.anthropic.claude-future","models":[
            {"modelArn":"arn:aws:bedrock:region-a::foundation-model/anthropic.claude-future"},
            {"modelArn":"arn:aws:bedrock:region-b::foundation-model/anthropic.claude-future"}]},
        {"status":"INACTIVE","inferenceProfileId":"us.openai.retired"},
        {"status":"ACTIVE","inferenceProfileId":"us.other.future"}]});
    let mut rows = std::collections::BTreeMap::new();
    catalog::collect(&page, &mut rows).unwrap();
    let models = rows.keys().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(
        models,
        ["global.anthropic.claude-future", "us.openai.gpt-future"]
    );
    assert_eq!(
        rows["global.anthropic.claude-future"]
            .iter()
            .collect::<Vec<_>>(),
        ["anthropic.claude-future"]
    );
}

#[tokio::test]
async fn unconfigured_namespace_never_uses_the_default_upstream() {
    let mut config = Config::default();
    config.upstream_url = "http://127.0.0.1:1".into();
    let proxy = Arc::new(local_snapshot(config, None).unwrap());
    let req = Request::post("/bedrock/v1/responses")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        forward_api(proxy, req).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn stale_catalog_is_served_without_waiting_for_discovery() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("config.yaml");
    let socket = temp.path().join("offline.sock");
    let config =
        json!({"listen":"127.0.0.1:8080", "providers":{"bedrock":{"relay_socket":socket}}});
    std::fs::write(&source, config.to_string()).unwrap();
    let list = json!({"object":"list","data":[{"id":"us.openai.example","object":"model"}]});
    let cache =
        json!({"provider":{"relay_socket":socket}, "updated":SystemTime::UNIX_EPOCH,"list":list});
    std::fs::write(
        source.with_extension("bedrock-models.json"),
        cache.to_string(),
    )
    .unwrap();
    let proxy = local_snapshot(serde_json::from_value(config).unwrap(), Some(source)).unwrap();
    catalog::start(&proxy.service);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let res = catalog::reply(&proxy, proxy.config.bedrock.as_ref().unwrap());
            if res.status().is_success() {
                let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
                assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), list);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn relay_preserves_native_bodies_and_drops_client_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("relay.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(|req: Request| async move {
                assert!(!req.headers().contains_key("x-api-key"));
                assert!(!req.headers().contains_key(header::AUTHORIZATION));
                let echo = axum::body::to_bytes(req.into_body(), 4096).await.unwrap();
                ([(header::CONTENT_TYPE, "text/event-stream")], echo)
            }),
        )
        .await
        .unwrap();
    });
    let config = serde_json::from_value(
        json!({"listen":"127.0.0.1:8080","providers":{"bedrock":{"relay_socket":socket}}}),
    )
    .unwrap();
    let proxy = Arc::new(local_snapshot(config, None).unwrap());
    for (path, id) in [
        ("messages", "global.anthropic.example"),
        ("responses", "us.openai.example"),
    ] {
        let body = json!({"model":id,"stream":true}).to_string();
        let req = Request::post(format!("/bedrock/v1/{path}"))
            .header("authorization", "Bearer client-secret")
            .header("x-api-key", "client-secret")
            .body(Body::from(body.clone()))
            .unwrap();
        let response = forward_api(proxy.clone(), req).await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap(),
            body
        );
    }
    let wrong = Request::post("/bedrock/v1/messages")
        .body(Body::from(r#"{"model":"us.openai.example"}"#))
        .unwrap();
    assert_eq!(forward_api(proxy, wrong).await.status(), 400);
    server.abort();
}
