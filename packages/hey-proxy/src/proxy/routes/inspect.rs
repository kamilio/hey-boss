use super::*;

fn quota_error(value: &Value) -> bool {
    if ["/output", "/response/output", "/choices"].iter().any(|p| {
        value
            .pointer(p)
            .and_then(Value::as_array)
            .is_some_and(|v| !v.is_empty())
    }) || ["/usage/output_tokens", "/response/usage/output_tokens"]
        .iter()
        .any(|p| {
            value
                .pointer(p)
                .and_then(Value::as_u64)
                .is_some_and(|v| v > 0)
        })
    {
        return false;
    }
    let error = value
        .pointer("/response/error")
        .or_else(|| value.get("error"))
        .unwrap_or(&Value::Null);
    let code = error["code"].as_str().or_else(|| error["type"].as_str());
    let forbidden = [
        "authentication_error",
        "permission_error",
        "invalid_request_error",
        "content_policy_violation",
    ];
    if error["type"]
        .as_str()
        .is_some_and(|t| forbidden.contains(&t))
    {
        return false;
    }
    matches!(
        code,
        Some(
            "usage_limit_reached"
                | "insufficient_quota"
                | "subscription_sharing_usage_limit_exceeded"
        )
    )
}

/// Only a complete typed quota refusal can advance a route. Every output, tool,
/// unknown event, partial error, disconnect, timeout or oversized prelude commits it.
pub(in crate::proxy) async fn response(response: Response) -> (Response, bool) {
    if response.extensions().get::<fallback::Upstream>().is_none()
        || response
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_some_and(|v| v != "identity")
    {
        return (response, false);
    }
    let sse = response.status().is_success()
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/event-stream"));
    if !sse && response.status() != StatusCode::TOO_MANY_REQUESTS {
        return (response, false);
    }
    let (mut parts, body) = response.into_parts();
    let mut stream = body.into_data_stream();
    let mut chunks = Vec::new();
    let mut size = 0usize;
    let mut decoder = sse::SseDecoder::default();
    let mut exhausted = false;
    // Fresh quota gates handle ordinary exhaustion. Only hold the prelude for
    // an immediate refusal; waiting for generation delays headers/heartbeats.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
    loop {
        let next = match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(next) => next,
            Err(_) => break,
        };
        match next {
            Some(Ok(chunk)) => {
                size = size.saturating_add(chunk.len());
                let events = if sse && size <= hey_proxy::fallback::MAX_PRELUDE {
                    decoder.feed(&chunk, false).ok()
                } else {
                    None
                };
                chunks.push(Ok(chunk));
                if size > hey_proxy::fallback::MAX_PRELUDE {
                    break;
                }
                if sse {
                    let Some(events) = events else { break };
                    let mut committed = false;
                    for event in events {
                        if quota_error(&event)
                            && matches!(event["type"].as_str(), Some("response.failed" | "error"))
                        {
                            exhausted = true;
                        } else if hey_proxy::fallback::prelude_event(
                            event.to_string().as_bytes(),
                            false,
                        ) != hey_proxy::fallback::Prelude::Waiting
                        {
                            committed = true;
                        }
                    }
                    if committed {
                        exhausted = false;
                        break;
                    }
                    if exhausted {
                        break;
                    }
                }
            }
            Some(Err(e)) => {
                chunks.push(Err(e));
                break;
            }
            None => {
                if !sse {
                    let bytes: Vec<u8> = chunks
                        .iter()
                        .filter_map(|c| c.as_ref().ok())
                        .flat_map(|c| c.iter().copied())
                        .collect();
                    exhausted = serde_json::from_slice(&bytes).is_ok_and(|v| quota_error(&v));
                }
                break;
            }
        }
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    let body = Body::from_stream(async_stream::stream! {
        for chunk in chunks { yield chunk; }
        while let Some(chunk) = stream.next().await { yield chunk; }
    });
    (Response::from_parts(parts, body), exhausted)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn typed_quota_only_before_output_across_every_chunk_boundary() {
        let refusal = "data: {\"type\":\"response.created\",\"response\":{\"output\":[]}}\n\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_reached\"}}}\n\n";
        for split in 0..refusal.len() {
            let chunks = vec![
                Bytes::copy_from_slice(&refusal.as_bytes()[..split]),
                Bytes::copy_from_slice(&refusal.as_bytes()[split..]),
            ];
            let mut r = Response::new(Body::from_stream(futures_util::stream::iter(
                chunks.into_iter().map(Ok::<_, std::io::Error>),
            )));
            r.headers_mut()
                .insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
            r.extensions_mut()
                .insert(fallback::Upstream { gemini: false });
            let (r, exhausted) = response(r).await;
            assert!(exhausted, "split {split}");
            assert_eq!(
                axum::body::to_bytes(r.into_body(), 65536).await.unwrap(),
                refusal
            );
        }
        for event in [
            "response.output_text.delta",
            "response.output_item.added",
            "response.reasoning.delta",
            "unknown",
        ] {
            let bytes = format!("data: {{\"type\":\"{event}\"}}\n\n{refusal}");
            let mut r = Response::new(Body::from(bytes.clone()));
            r.headers_mut()
                .insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
            r.extensions_mut()
                .insert(fallback::Upstream { gemini: false });
            let (r, exhausted) = response(r).await;
            assert!(!exhausted);
            assert_eq!(
                axum::body::to_bytes(r.into_body(), 65536).await.unwrap(),
                bytes
            );
        }
    }
}
