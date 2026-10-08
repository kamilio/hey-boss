//! Ordered, quota-gated provider routes, independent of recommendation policy.
use super::*;
use crate::config::routes::{BillingMode, ResolvedLeg};
mod diagnostic;
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
    let is_probe = probe::is_model(parts.uri.path(), &input);
    let mut report =
        is_probe.then(|| diagnostic::Probe::new(&proxy.config, parts.uri.path(), &input));
    // Probes run exactly the same selection as forwarding, but stop before a
    // model request. Only probes have this short total deadline.
    let routing = async {
        let replayable = replay_blocker(&input).is_none();
        // Bound history prevents switching, not the initial attempt. Keep it intact
        // and use only the first configured provider unless the caller pins another.
        let attempts = if replayable { plan.len() } else { 1 };
        for index in 0..attempts {
            let leg = plan.resolve(index).expect("validated route snapshot");
            // Capability failure is not quota exhaustion and must never choose paid billing.
            let mut attempt_parts = parts.clone();
            if let Err(message) = transport(&leg, &mut attempt_parts) {
                return diagnostic::failure(
                    report.as_ref(),
                    StatusCode::BAD_REQUEST,
                    "route_transport_unsupported",
                    message,
                );
            }
            let mut selected =
                match tokio::time::timeout(Duration::from_secs(25), proxy.select(&leg.provider))
                    .await
                {
                    Ok(Ok(selected)) => selected,
                    _ => {
                        return diagnostic::failure(
                            report.as_ref(),
                            StatusCode::SERVICE_UNAVAILABLE,
                            "route_account_unavailable",
                            "Selected provider credentials or account identity are unavailable",
                        );
                    }
                };
            selected.fallback_attempt = true;
            if leg.billing_mode == BillingMode::IncludedSubscription {
                let (availability, reading) = quota::check(&selected, &leg.upstream_model).await;
                if let Some(report) = &mut report {
                    report.quota(&leg, &availability, reading.as_ref());
                }
                match availability {
                    quota::Availability::Included => {}
                    quota::Availability::Exhausted => {
                        diagnostic(&proxy, &leg, index, "included_exhausted");
                        continue;
                    }
                    quota::Availability::Unavailable => {
                        return diagnostic::failure(
                            report.as_ref(),
                            StatusCode::SERVICE_UNAVAILABLE,
                            "route_quota_unavailable",
                            "Fresh account/model-specific included quota is unavailable; no paid leg was selected",
                        );
                    }
                }
            }
            if let Some(report) = &report {
                return report.reply(Some(&leg), None);
            }
            let mut payload = input.clone();
            // Full visible history and local tool pairs are portable. Opaque
            // reasoning belongs to its originating provider and is optional on
            // another leg; never strip compaction or server-side references.
            let stripped = if index > 0 {
                strip_reasoning(&mut payload)
            } else {
                0
            };
            payload["model"] = json!(leg.upstream_model);
            if let Some(effort) = &leg.reasoning {
                let key =
                    if claude::is_path(parts.uri.path()) || messages::is_path(parts.uri.path()) {
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
            // Buffering the stream prelude only serves fallback. Return it directly
            // when no further attempt is permitted, including stateful requests.
            let can_switch = index + 1 < attempts;
            let (mut response, exhausted) = if can_switch {
                inspect::response(response).await
            } else {
                (response, false)
            };
            if exhausted && leg.billing_mode == BillingMode::IncludedSubscription {
                quota::exhausted(&selected, &leg.upstream_model).await;
                diagnostic(&proxy, &leg, index, "upstream_quota_exhausted");
                if can_switch {
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
            if stripped > 0 {
                response.headers_mut().insert(
                    "x-hey-proxy-reasoning-stripped",
                    stripped.to_string().parse().unwrap(),
                );
            }
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
        diagnostic::failure(
            report.as_ref(),
            StatusCode::TOO_MANY_REQUESTS,
            "route_included_exhausted",
            if !replayable && plan.len() > 1 {
                "The first provider's included quota is exhausted; history or tools prevent automatic switching. Pin the original provider/account or start a new conversation"
            } else {
                "All configured included subscription legs are exhausted and no usable paid leg remains"
            },
        )
    };
    if is_probe {
        match tokio::time::timeout(Duration::from_secs(2), routing).await {
            Ok(response) => response,
            Err(_) => report.as_ref().unwrap().reply(
                None,
                Some("selection could not be verified within 2s; quota or credentials unavailable"),
            ),
        }
    } else {
        routing.await
    }
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
        // Declaring web search is not executing it. The response inspection
        // commits the provider as soon as a tool/output event is observed.
        fn portable_tool(tool: &Value) -> bool {
            match tool["type"].as_str() {
                Some("function" | "custom" | "web_search" | "web_search_preview") => true,
                Some("namespace") => tool["tools"]
                    .as_array()
                    .is_some_and(|tools| tools.iter().all(portable_tool)),
                _ => false,
            }
        }
        let portable_tools = reason == "hosted_tools"
            && input["tools"]
                .as_array()
                .is_some_and(|tools| tools.iter().all(portable_tool));
        if !native_local_tools && !portable_tools {
            return Some(reason);
        }
    }
    fn bound(value: &Value) -> bool {
        match value {
            Value::Object(map) => {
                if reasoning_item(value) {
                    return false;
                }
                map.contains_key("signature")
                    || map.contains_key("encrypted_content")
                    || map.contains_key("file_id")
                    || map.contains_key("container_id")
                    || map.contains_key("vector_store_id")
                    || matches!(
                        value["type"].as_str(),
                        Some(
                            "item_reference"
                                | "compaction"
                                | "compaction_summary"
                                | "code_interpreter_call"
                                | "file_search_call"
                                | "computer_call"
                                | "computer_call_output"
                        )
                    )
                    || map.values().any(bound)
            }
            Value::Array(values) => values.iter().any(bound),
            _ => false,
        }
    }
    input
        .get("input")
        .into_iter()
        .chain(input.get("messages"))
        .any(bound)
        .then_some("provider_bound_history")
}

fn reasoning_item(value: &Value) -> bool {
    matches!(
        value["type"].as_str(),
        Some("reasoning" | "thinking" | "redacted_thinking")
    )
}

/// Only known history slots are changed, never tool arguments/results or text.
fn strip_reasoning(input: &mut Value) -> usize {
    let mut stripped = 0;
    if let Some(items) = input.get_mut("input").and_then(Value::as_array_mut) {
        items.retain(|item| {
            let remove = reasoning_item(item);
            stripped += usize::from(remove);
            !remove
        });
    }
    if let Some(messages) = input.get_mut("messages").and_then(Value::as_array_mut) {
        messages.retain_mut(|message| {
            if message["role"] != "assistant" {
                return true;
            }
            let Some(parts) = message["content"].as_array_mut() else {
                return true;
            };
            let before = parts.len();
            parts.retain(|part| !reasoning_item(part));
            stripped += before - parts.len();
            before == parts.len() || !parts.is_empty()
        });
    }
    stripped
}

#[cfg(test)]
mod tests;
