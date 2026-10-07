//! Ordered, quota-gated provider routes, independent of recommendation policy.
use super::*;
use crate::config::routes::{BillingMode, ResolvedLeg};
pub(super) mod inspect;
pub(super) mod quota;

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(json!({"error":{"type":"proxy_routing_error","code":code,"message":message}})),
    )
        .into_response()
}

pub(super) async fn forward(proxy: Arc<Proxy>, request: Request) -> Response {
    if proxy.binding.is_some()
        || proxy.config.mode == Mode::Client
        || proxy.config.routes.is_empty()
    {
        return forward_selected(proxy, request).await;
    }
    let path = request.uri().path().trim_end_matches('/');
    if request
        .headers()
        .get(header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"))
    {
        return failure(
            StatusCode::UPGRADE_REQUIRED,
            "websocket_not_supported",
            "Configured provider routes require HTTP/SSE; retry using HTTP",
        );
    }
    if request.method() != axum::http::Method::POST
        || config::ApiShape::from_route_path(path).is_none()
    {
        return forward_selected(proxy, request).await;
    }
    if request
        .headers()
        .get(header::CONTENT_ENCODING)
        .is_some_and(|v| v != "identity")
    {
        return failure(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "route_transport_unsupported",
            "Provider routes require uncompressed JSON",
        );
    }
    let (mut parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, 64 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return failure(
                StatusCode::PAYLOAD_TOO_LARGE,
                "route_request_too_large",
                "Provider route body exceeds 64 MiB or could not be read",
            );
        }
    };
    let input: Value = match serde_json::from_slice(&bytes) {
        Ok(input) => input,
        Err(_) => {
            return forward_selected(proxy, Request::from_parts(parts, Body::from(bytes))).await;
        }
    };
    let Some(plan) = input["model"].as_str().and_then(|model| {
        proxy.config.route_plan(
            model,
            parts.uri.path(),
            requested_effort(parts.uri.path(), &input),
        )
    }) else {
        return forward_selected(proxy, Request::from_parts(parts, Body::from(bytes))).await;
    };
    if let Some(response) = probe::parsed(&proxy.config, &mut parts, &input) {
        return response;
    }
    let replayable = replay_blocker(&input).is_none();
    if plan.len() > 1 && !replayable {
        return failure(
            StatusCode::CONFLICT,
            "route_non_replayable",
            "History or tools require a pinned provider/account; automatic provider switching cannot preserve this request",
        );
    }
    for index in 0..plan.len() {
        let leg = plan.resolve(index).expect("validated route snapshot");
        // Capability failure is not quota exhaustion and must never choose paid billing.
        let mut attempt_parts = parts.clone();
        if let Err(message) = transport(&leg, &mut attempt_parts) {
            return failure(
                StatusCode::BAD_REQUEST,
                "route_transport_unsupported",
                message,
            );
        }
        let mut selected = match tokio::time::timeout(
            Duration::from_secs(25),
            proxy.select(&leg.provider),
        )
        .await
        {
            Ok(Ok(selected)) => selected,
            _ => {
                return failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "route_account_unavailable",
                    "Selected provider credentials or account identity are unavailable",
                );
            }
        };
        selected.fallback_attempt = true;
        if leg.billing_mode == BillingMode::IncludedSubscription {
            match quota::availability(&selected, &leg.upstream_model).await {
                quota::Availability::Included => {}
                quota::Availability::Exhausted => {
                    diagnostic(&proxy, &leg, index, "included_exhausted");
                    continue;
                }
                quota::Availability::Unavailable => {
                    return failure(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "route_quota_unavailable",
                        "Fresh account/model-specific included quota is unavailable; no paid leg was selected",
                    );
                }
            }
        }
        let mut payload = input.clone();
        payload["model"] = json!(leg.upstream_model);
        if let Some(effort) = &leg.reasoning {
            let key = if claude::is_path(parts.uri.path()) || messages::is_path(parts.uri.path()) {
                "output_config"
            } else if parts.uri.path().ends_with("/chat/completions") {
                "reasoning_effort"
            } else {
                "reasoning"
            };
            if key == "reasoning_effort" {
                payload[key] = json!(effort);
            } else {
                if payload.get(key).is_none_or(Value::is_null) {
                    payload[key] = json!({});
                }
                if !payload[key].is_object() {
                    return failure(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error",
                        "Reasoning options must be an object",
                    );
                }
                payload[key]["effort"] = json!(effort);
            }
        }
        for name in [
            "x-hey-proxy-provider",
            "x-hey-proxy-account",
            "chatgpt-account-id",
            "content-length",
        ] {
            attempt_parts.headers.remove(name);
        }
        attempt_parts.headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        if let Some(key) = attempt_parts.headers.get("idempotency-key") {
            use sha2::{Digest, Sha256};
            let mut digest = Sha256::new();
            digest.update(key.as_bytes());
            digest.update([0]);
            digest.update(selected.binding.as_ref().unwrap().reference.as_bytes());
            digest.update([0]);
            digest.update(leg.upstream_model.as_bytes());
            attempt_parts.headers.insert(
                "idempotency-key",
                format!("hey-proxy-route-{:x}", digest.finalize())
                    .parse()
                    .unwrap(),
            );
        }
        diagnostic(&proxy, &leg, index, "selected");
        let selected = Arc::new(selected);
        let response = forward_selected(
            selected.clone(),
            Request::from_parts(attempt_parts, Body::from(payload.to_string())),
        )
        .await;
        let (mut response, exhausted) = inspect::response(response).await;
        if exhausted && leg.billing_mode == BillingMode::IncludedSubscription {
            quota::exhausted(&selected, &leg.upstream_model).await;
            diagnostic(&proxy, &leg, index, "upstream_quota_exhausted");
            if index + 1 < plan.len() && replayable {
                drop(response);
                continue;
            }
        }
        diagnostic(&proxy, &leg, index, "result");
        for (name, value) in [
            ("x-hey-proxy-provider", leg.provider.as_str()),
            ("x-hey-proxy-requested-model", leg.source_model.as_str()),
        ] {
            if let Ok(value) = value.parse() {
                response.headers_mut().insert(name, value);
            }
        }
        response
            .headers_mut()
            .insert("x-hey-proxy-route-attempt", (index + 1).into());
        response.headers_mut().insert(
            "x-hey-proxy-account",
            selected
                .binding
                .as_ref()
                .unwrap()
                .reference
                .parse()
                .unwrap(),
        );
        response.headers_mut().insert(
            "x-hey-proxy-billing",
            header::HeaderValue::from_static(match leg.billing_mode {
                BillingMode::IncludedSubscription => "included_subscription",
                BillingMode::PayPerToken => "pay_per_token",
            }),
        );
        let sse = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/event-stream"));
        let (parts, body) = response.into_parts();
        let mut stream = body.into_data_stream();
        let body = Body::from_stream(async_stream::stream! {
            let mut reader = logs::UsageReader::new(sse);
            while let Some(chunk) = stream.next().await {
                if let Ok(bytes) = &chunk { reader.feed(bytes,&proxy.service.logs,proxy.log_id); }
                yield chunk;
            }
            reader.finish(&proxy.service.logs,proxy.log_id);
        });
        return Response::from_parts(parts, body);
    }
    failure(
        StatusCode::TOO_MANY_REQUESTS,
        "route_included_exhausted",
        "All configured included subscription legs are exhausted and no usable paid leg remains",
    )
}

fn transport(
    leg: &ResolvedLeg,
    parts: &mut axum::http::request::Parts,
) -> Result<(), &'static str> {
    let path = parts.uri.path().trim_end_matches('/');
    let replacement = match leg.implementation {
        "codex"
            if matches!(
                path,
                "/responses" | "/v1/responses" | "/responses/compact" | "/v1/responses/compact"
            ) =>
        {
            None
        }
        "codex" => return Err("Codex subscription requires the Responses API"),
        "claude" if claude::is_path(path) => None,
        "claude" if messages::is_path(path) => Some(if path.ends_with("/count_tokens") {
            "/v1/messages/count_tokens"
        } else {
            "/v1/messages"
        }),
        "claude" => return Err("Claude subscription requires the Messages API"),
        "openai" if claude::is_path(path) => Some(if path.ends_with("/count_tokens") {
            "/v1/custom/messages/count_tokens"
        } else {
            "/v1/custom/messages"
        }),
        "openai" => None,
        _ => return Err("Unsupported provider transport"),
    };
    if let Some(path) = replacement {
        let query = parts
            .uri
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        parts.uri = format!("{path}{query}").parse().unwrap();
    }
    Ok(())
}

fn diagnostic(proxy: &Proxy, leg: &ResolvedLeg, index: usize, outcome: &str) {
    proxy.service.logs.update(
        proxy.log_id,
        "provider_route",
        json!({"route":leg,"attempt":index+1,"outcome":outcome}),
        |entry| {
            entry.requested_model = Some(leg.source_model.clone());
            entry.routed_model = Some(leg.upstream_model.clone());
            true
        },
    );
}

fn replay_blocker(input: &Value) -> Option<&'static str> {
    // Native Messages declares local tools by name/input_schema, without a type.
    if let Some(reason) = hey_proxy::fallback::replay_blocker(input) {
        let native_local_tools = reason == "hosted_tools"
            && input.get("messages").is_some()
            && input["tools"].as_array().is_some_and(|tools| {
                tools.iter().all(|tool| {
                    tool.get("type").is_none()
                        && tool["name"].is_string()
                        && tool["input_schema"].is_object()
                })
            });
        if !native_local_tools {
            return Some(reason);
        }
    }
    fn bound(value: &Value) -> bool {
        match value {
            Value::Object(map) => {
                map.contains_key("signature")
                    || map.contains_key("encrypted_content")
                    || map.contains_key("file_id")
                    || matches!(
                        value["type"].as_str(),
                        Some(
                            "reasoning"
                                | "thinking"
                                | "redacted_thinking"
                                | "item_reference"
                                | "compaction"
                        )
                    )
                    || map.values().any(bound)
            }
            Value::Array(values) => values.iter().any(bound),
            _ => false,
        }
    }
    bound(input).then_some("provider_bound_history")
}

#[cfg(test)]
mod tests;
