//! Anthropic Messages facade: direct native Gemini, shared Responses for OpenAI.
mod gemini;
mod gemini_request;
mod gemini_stream;
mod request;
mod response;
mod stream;
#[cfg(test)]
mod tests;
use super::*;
const LIMIT: usize = 64 * 1024 * 1024;

pub(super) fn is_path(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    matches!(
        path,
        "/v1/custom/messages"
            | "/custom/v1/messages"
            | "/v1/custom/messages/count_tokens"
            | "/custom/v1/messages/count_tokens"
    )
}
pub(super) fn error(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(response::error_value(status, message))).into_response()
}

pub(super) async fn forward(proxy: Arc<Proxy>, request: Request) -> Response {
    if request.method() != axum::http::Method::POST {
        let mut reply = error(StatusCode::METHOD_NOT_ALLOWED, "Messages requires POST");
        reply
            .headers_mut()
            .insert(header::ALLOW, header::HeaderValue::from_static("POST"));
        return reply;
    }
    if request.uri().query().is_some_and(|q| q != "beta=true")
        || request.headers().contains_key(header::UPGRADE)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "Messages supports HTTP/SSE with only the optional beta=true query",
        );
    }
    if request
        .headers()
        .get(header::CONTENT_ENCODING)
        .is_some_and(|v| v != "identity")
    {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Messages requires uncompressed JSON",
        );
    }
    let (mut parts, body) = request.into_parts();
    parts.uri = parts.uri.path().parse().unwrap();
    let input = match axum::body::to_bytes(body, LIMIT).await {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(input) => input,
            Err(_) => return error(StatusCode::BAD_REQUEST, "Messages body must be valid JSON"),
        },
        Err(_) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Messages body exceeds 64 MiB or could not be read",
            );
        }
    };
    let Some(requested_model) = input
        .get("model")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
    else {
        return error(StatusCode::BAD_REQUEST, "model must be a nonempty string");
    };
    let incoming = super::requested_effort(parts.uri.path(), &input);
    let alias = proxy.config.alias_for(requested_model, parts.uri.path());
    let route = alias.and_then(|a| incoming.and_then(|e| a.reasoning_routes.get(e)));
    let model = route
        .map(|r| r.to.as_str())
        .or_else(|| alias.and_then(|a| a.to.as_deref()))
        .unwrap_or(requested_model)
        .to_owned();
    let effort = alias
        .and_then(|a| a.reasoning.as_deref())
        .or_else(|| {
            input
                .pointer("/output_config/effort")
                .and_then(Value::as_str)
        })
        .map(str::to_owned);
    let count = parts
        .uri
        .path()
        .trim_end_matches('/')
        .ends_with("/count_tokens");
    if model.starts_with("gemini/") {
        return gemini::forward(proxy, input, model, effort, count).await;
    }
    if count {
        return error(
            StatusCode::BAD_REQUEST,
            "Messages count_tokens currently requires a Gemini target",
        );
    }
    let converted = match request::convert(&input) {
        Ok(body) => body,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let streaming = converted["stream"] == true;
    // Host authentication has already run. Never leak a client Anthropic key or
    // Anthropic feature headers into the OpenAI/Gemini request.
    for name in [
        "x-api-key",
        "anthropic-version",
        "anthropic-beta",
        "anthropic-dangerous-direct-browser-access",
    ] {
        parts.headers.remove(name);
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    // Keep the original Messages path for shape selection inside the shared
    // facade; it changes to /v1/responses only after selecting alias rules.
    let upstream = chat::forward(
        proxy.clone(),
        Request::from_parts(parts, Body::from(converted.to_string())),
    )
    .await;
    if upstream.status().is_success() && streaming {
        return stream::adapt(
            upstream,
            input["model"].as_str().unwrap().into(),
            Some((proxy.service.logs.clone(), proxy.log_id)),
        );
    }
    let (mut parts, body) = upstream.into_parts();
    let value = axum::body::to_bytes(body, LIMIT)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    response::clean_headers(&mut parts.headers);
    if !parts.status.is_success() {
        let message = value
            .as_ref()
            .and_then(|v| v.pointer("/error/message"))
            .and_then(Value::as_str)
            .unwrap_or("Upstream request failed");
        let body = response::error_value(parts.status, message);
        return Response::from_parts(parts, Body::from(body.to_string()));
    }
    match value
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Invalid upstream reply"))
        .and_then(response::convert)
    {
        Ok(body) => Response::from_parts(parts, Body::from(body.to_string())),
        Err(e) => error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}
