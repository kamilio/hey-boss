//! ChatGPT OAuth uses the Codex Responses backend, never an API-key connection.
use super::*;

pub(in crate::proxy) async fn forward(proxy: Arc<Proxy>, request: Request) -> Response {
    let path = request.uri().path().trim_end_matches('/');
    if request.method() != axum::http::Method::POST
        || !matches!(
            path,
            "/responses" | "/v1/responses" | "/responses/compact" | "/v1/responses/compact"
        )
        || request.headers().contains_key(header::UPGRADE)
        || request
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_some_and(|v| v != "identity")
    {
        return error(
            StatusCode::BAD_REQUEST,
            "Codex subscription requires uncompressed Responses JSON over HTTP/SSE",
        );
    }
    let compact = path.ends_with("/compact");
    let (mut parts, body) = request.into_parts();
    let mut input: Value = match axum::body::to_bytes(body, 64 * 1024 * 1024)
        .await
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(Value::is_object)
    {
        Some(input) => input,
        None => {
            return error(
                StatusCode::BAD_REQUEST,
                "Codex Responses body must be a JSON object",
            );
        }
    };
    // Do not silently drop unsupported state or change execution semantics.
    if input["store"] == true || input["background"] == true {
        return error(
            StatusCode::BAD_REQUEST,
            "Codex subscription does not support stored/background Responses requests",
        );
    }
    let streaming = input["stream"] == true;
    if !compact {
        input["store"] = json!(false);
        input["stream"] = json!(true);
        if input.get("instructions").is_none() {
            input["instructions"] = json!("");
        }
        if let Some(text) = input["input"].as_str() {
            input["input"] = json!([{"role":"user","content":[{"type":"input_text","text":text}]}]);
        }
    }
    let query = parts
        .uri
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    parts.uri = format!("/responses{}{query}", if compact { "/compact" } else { "" })
        .parse()
        .unwrap();
    parts.headers.remove("chatgpt-account-id");
    parts.headers.remove(header::CONTENT_LENGTH);
    let binding = proxy.binding.as_ref().expect("pinned Codex transport");
    let Some(account) = binding.account_id.as_ref().and_then(|id| id.parse().ok()) else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Codex subscription account identity is unavailable",
        );
    };
    parts.headers.insert("chatgpt-account-id", account);
    parts
        .headers
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    let response = forward_request(
        proxy.clone(),
        Request::from_parts(parts, Body::from(input.to_string())),
    )
    .await;
    if streaming
        || compact
        || !response.status().is_success()
        || !response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/event-stream"))
    {
        return response;
    }
    // Nonstreaming clients receive the terminal Responses object. No retry is
    // possible once we consume generation events, even if no text was returned.
    let (response, exhausted) = super::super::routes::inspect::response(response).await;
    let mut stream = response.into_body().into_data_stream();
    let mut decoder = sse::SseDecoder::default();
    while let Some(chunk) = stream.next().await {
        let events = match chunk.ok().and_then(|c| decoder.feed(&c, false).ok()) {
            Some(events) => events,
            None => {
                return error(
                    StatusCode::BAD_GATEWAY,
                    "Codex response stream interrupted or invalid",
                );
            }
        };
        for event in events {
            match event["type"].as_str() {
                Some("response.completed" | "response.incomplete")
                    if event["response"].is_object() =>
                {
                    return axum::Json(event["response"].clone()).into_response();
                }
                Some("response.failed" | "error") if exhausted => {
                    let error = event
                        .pointer("/response/error")
                        .or_else(|| event.get("error"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let mut response = (
                        StatusCode::TOO_MANY_REQUESTS,
                        axum::Json(json!({"error":error})),
                    )
                        .into_response();
                    response
                        .extensions_mut()
                        .insert(fallback::Upstream { gemini: false });
                    return response;
                }
                Some("response.failed" | "error") => {
                    return error(
                        StatusCode::BAD_GATEWAY,
                        "Codex generation failed; the request was not replayed",
                    );
                }
                _ => {}
            }
        }
    }
    error(
        StatusCode::BAD_GATEWAY,
        "Codex response stream ended before completion",
    )
}
