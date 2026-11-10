//! Transport adapter; conversion lives in the reusable hey_proxy library.
use super::*;
#[cfg(test)]
use hey_proxy::gemini::convert_response;
use hey_proxy::gemini::{
    Auth, ConvertedRequest, HostedSearch, ProviderConfig, ReasoningCodec, ResponseError,
    ResponseStream, convert_request,
};
use std::path::Path;
pub(super) mod continuation;
use continuation::Continuation;

#[cfg(test)]
mod bootstrap_tests;
#[cfg(test)]
mod continuation_tests;

pub(super) struct GeminiState {
    pub codec: ReasoningCodec,
}
impl GeminiState {
    pub fn new(source: Option<&Path>) -> Result<Self> {
        let key = if let Some(source) = source {
            let path = source.with_extension("gemini-reasoning-key");
            // Publish a complete private key atomically. Concurrent startup must
            // never observe a newly created but still empty key file.
            let mut file = tempfile::NamedTempFile::new_in(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?;
            use std::io::Write;
            let key: [u8; 32] = rand::random();
            file.write_all(&key)?;
            file.as_file().sync_all()?;
            match file.persist_noclobber(&path) {
                Ok(_) => key,
                Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let bytes = std::fs::read(&path)?;
                    bytes
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("Invalid Gemini reasoning key file"))?
                }
                Err(e) => return Err(e.error.into()),
            }
        } else {
            rand::random()
        };
        Ok(Self {
            codec: ReasoningCodec::new(&key),
        })
    }
}

fn read_failure(error: &reqwest::Error) -> &'static str {
    use std::{error::Error, io::ErrorKind};
    if error.is_timeout() {
        return "timeout";
    }
    let mut source = error.source();
    while let Some(cause) = source {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            match io.kind() {
                ErrorKind::ConnectionReset => return "connection_reset",
                ErrorKind::UnexpectedEof => return "incomplete_body",
                _ => {}
            }
        }
        if cause
            .downcast_ref::<hyper::Error>()
            .is_some_and(|e| e.is_incomplete_message())
        {
            return "incomplete_body";
        }
        source = cause.source();
    }
    "other_read_error"
}

fn sse_bytes(event: &Value) -> Bytes {
    Bytes::from(format!(
        "event: {}\ndata: {}\n\n",
        event["type"].as_str().unwrap_or("error"),
        event
    ))
}
#[cfg(test)]
use super::sse::MAX_FRAME as MAX_NATIVE_FRAME;
use super::sse::SseDecoder as NativeSse;
const MAX_NATIVE_RESPONSE: usize = 64 * 1024 * 1024;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

fn transport_error(message: &str, proxy_code: &str) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        axum::Json(json!({"error":{
            "type":"server_error","code":"server_error","message":message,
            "param":null,"proxy_code":proxy_code
        }})),
    )
        .into_response()
}
async fn bounded_body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            chunk.len() <= limit - bytes.len(),
            "Gemini response exceeds size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Shared authenticated/retrying native transport. No wire-format conversion.
pub(super) async fn send_native(
    proxy: &Arc<Proxy>,
    config: &ProviderConfig,
    url: &str,
    body: &Value,
) -> std::result::Result<reqwest::Response, Box<Response>> {
    send_native_with_recovery(proxy, config, url, body, &mut Recovery::for_request(proxy)).await
}

async fn send_native_with_recovery(
    proxy: &Arc<Proxy>,
    config: &ProviderConfig,
    url: &str,
    body: &Value,
    recovery: &mut Recovery,
) -> std::result::Result<reqwest::Response, Box<Response>> {
    proxy.record_accounting_route("gemini", "default");
    let mut adc_refreshed = false;
    loop {
        let credentials = match credential_headers(proxy, config).await {
            Ok(v) => v,
            Err(e) => {
                proxy.service.logs.observe(
                    proxy.log_id,
                    &json!({"error":{"code":"gemini_credential_error"}}),
                );
                return Err(Box::new(transport_error(
                    &e.to_string(),
                    "gemini_credential_error",
                )));
            }
        };
        let mut attempt =
            logs::Attempt::new(proxy.service.logs.clone(), proxy.log_id, "gemini_http");
        proxy.service.logs.retries(proxy.log_id, recovery.retries);
        let response = recovery
            .run(
                proxy
                    .client
                    .post(url)
                    .headers(credentials)
                    .json(body)
                    .send(),
            )
            .await;
        let response = match response {
            Ok(Ok(r)) => r,
            Err(()) => {
                attempt.finish("timeout", None, Some("proxy_recovery_timeout"));
                return Err(Box::new(recovery::timeout_response()));
            }
            Ok(Err(e)) => {
                attempt.finish("connection_error", None, Some("gemini_connect_failed"));
                if proxy.fallback_attempt {
                    let mut response =
                        transport_error("Could not reach Gemini upstream", "gemini_connect_failed");
                    if recovery::connection_failure(&e) || e.is_timeout() {
                        response
                            .extensions_mut()
                            .insert(super::fallback::EligibleFailure);
                    }
                    return Err(Box::new(response));
                }
                if recovery::connection_failure(&e)
                    && recovery.retry(&proxy.config, &HeaderMap::new()).await
                {
                    continue;
                }
                return Err(Box::new(transport_error(
                    "Could not reach Gemini upstream; retry the request",
                    "gemini_connect_failed",
                )));
            }
        };
        if !response.status().is_success() {
            let status = response.status();
            let headers = response.headers().clone();
            // Native status/code, rather than arbitrary error message matching.
            let bytes = match recovery.run(bounded_body(response, 1024 * 1024)).await {
                Ok(Ok(b)) => b,
                _ => {
                    return Err(Box::new(transport_error(
                        "Could not read Gemini error; retry the request",
                        "gemini_error_read_failed",
                    )));
                }
            };
            let native: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let code = native
                .pointer("/error/status")
                .and_then(Value::as_str)
                .unwrap_or("gemini_http_error");
            let normalized = ResponseError::from_native(&native, Some(status.as_u16()));
            attempt.finish("http_error", Some(status.as_u16()), Some(code));
            if status == StatusCode::UNAUTHORIZED && config.auth == Auth::Adc && !adc_refreshed {
                adc_refreshed = true;
                proxy.service.credentials.invalidate_adc().await;
                continue;
            }
            if normalized.retryable && recovery.retry(&proxy.config, &headers).await {
                continue;
            }
            let mut response = (
                status,
                axum::Json(json!({"error":normalized.error,"gemini":native})),
            )
                .into_response();
            if let Some(delay) = headers.get(header::RETRY_AFTER) {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, delay.clone());
            }
            response
                .extensions_mut()
                .insert(super::fallback::Upstream { gemini: true });
            return Err(Box::new(response));
        }
        attempt.finish("accepted", Some(response.status().as_u16()), None);
        return Ok(response);
    }
}

pub(super) async fn forward(
    proxy: Arc<Proxy>,
    parts: axum::http::request::Parts,
    request: Value,
) -> Response {
    if parts.method != axum::http::Method::POST
        || parts.uri.path().trim_end_matches('/') != "/v1/responses"
        || parts.uri.query().is_some()
    {
        return error(
            StatusCode::BAD_REQUEST,
            "Gemini routes require POST /v1/responses without query parameters",
        );
    }
    let Some(base_config) = &proxy.config.gemini else {
        return error(
            StatusCode::BAD_REQUEST,
            "Configure gemini before using a gemini/model_name override",
        );
    };
    let mut config = base_config.clone();
    if config.max_output_tokens.is_none()
        && let Some(model) = request.get("model").and_then(Value::as_str)
        && let Some(max_tokens) = proxy
            .config
            .model_registry
            .as_ref()
            .and_then(|r| r.configured_max_tokens(&proxy.config, model))
    {
        config.max_output_tokens = Some(max_tokens);
    }
    let converted = match convert_request(&request, &config, &proxy.service.gemini.codec) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    proxy.service.logs.update(
        proxy.log_id,
        "provider_selected",
        json!({"provider":"gemini"}),
        |entry| {
            entry.routed_model = Some(converted.response_model.clone());
            entry.project = Some("gemini".into());
            entry.streaming = converted.stream;
            true
        },
    );
    if converted.has_hosted_search() {
        return forward_hosted(proxy.clone(), config, request, converted).await;
    }
    let url = match config.endpoint(&converted.model, converted.stream) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let mut recovery = Recovery::for_request(&proxy);
    let mut upstream = match send_native_with_recovery(
        &proxy,
        &config,
        &url,
        &converted.body,
        &mut recovery,
    )
    .await
    {
        Ok(response) => response,
        Err(response) => return *response,
    };
    let id = format!("{:032x}", rand::random::<u128>());
    if !converted.stream {
        return match unary_turn(&proxy, &config, &url, &converted, &id, upstream).await {
            Ok((response, _)) => {
                if !proxy.fallback_attempt {
                    proxy.service.logs.observe(proxy.log_id, &response);
                }
                axum::Json(response).into_response()
            }
            Err(response) => *response,
        };
    }

    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.contains("text/event-stream") {
        return error(
            StatusCode::BAD_GATEWAY,
            "Gemini streaming endpoint did not return SSE",
        );
    }
    let body = Body::from_stream(async_stream::stream! {
        let budget = converted.response_fields.get("max_output_tokens").and_then(Value::as_u64);
        let mut continuation = Continuation::new(converted.body.clone(), budget);
        let mut parser=NativeSse::default();let mut converter=ResponseStream::new(converted,id);
        let mut usage=logs::UsageReader::new(true);
        let mut received = 0usize;
        let mut started = false;
        let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            // Keep the same read future alive across heartbeats. They must not
            // reset the upstream idle deadline or conceal a stalled Google stream.
            let chunk = {
                let read = upstream.chunk();
                tokio::pin!(read);
                loop {
                    tokio::select! {
                        result = &mut read => break result,
                        _ = heartbeat.tick() => yield Ok::<Bytes,std::io::Error>(Bytes::from_static(b": keepalive\n\n")),
                    }
                }
            };
            let (bytes,eof,interrupted)=match chunk {
                Ok(Some(bytes))=>(bytes,false,false),Ok(None)=>(Bytes::new(),true,false),Err(err)=>{
                    let cause = read_failure(&err);
                    // Service logs remain available when request history is disabled.
                    eprintln!("Gemini stream read failed: cause={cause} started={started} received_bytes={received}");
                    proxy.service.logs.update(proxy.log_id,"gemini_stream_read_failed",json!({"cause":cause,"started":started,"received_bytes":received}), |_| true);
                    (Bytes::new(),false,true)
                },
            };
            let frame_events=(||->Result<Vec<Value>> {
                received = received.saturating_add(bytes.len());
                anyhow::ensure!(received <= MAX_NATIVE_RESPONSE, "Gemini stream exceeds 64 MiB");
                let mut events=Vec::new();for mut native in parser.feed(&bytes,eof)?{
                    continuation.feed(&mut native)?;
                    events.extend(converter.feed(&native)?);
                    if converter.is_finished() { break; }
                }
                Ok(events)
            })();
            // Comments and partial SSE frames do not commit a model response.
            // Share the opening HTTP retry budget; never replay emitted output.
            if !started && (interrupted || eof && frame_events.as_ref().is_ok_and(Vec::is_empty)) {
                let retried = {
                    let retry = async {
                        if !recovery.retry(&proxy.config, upstream.headers()).await { return None; }
                        Some(send_native_with_recovery(&proxy, &config, &url, continuation.request(), &mut recovery).await)
                    };
                    tokio::pin!(retry);
                    loop {
                        tokio::select! {
                            response = &mut retry => break response,
                            _ = heartbeat.tick() => yield Ok::<Bytes,std::io::Error>(Bytes::from_static(b": keepalive\n\n")),
                        }
                    }
                };
                match retried {
                    Some(Ok(response)) => {
                        if !response.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("text/event-stream")) {
                            let event = converter.fail("server_error", "Gemini streaming endpoint did not return SSE");
                            if !proxy.fallback_attempt { usage.observe_value(&event, &proxy.service.logs, proxy.log_id); }
                            yield Ok(sse_bytes(&event));
                            break;
                        }
                        upstream = response;
                        parser = NativeSse::default();
                        received = 0;
                        continue;
                    }
                    Some(Err(response)) => {
                        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.ok();
                        let body = bytes.and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
                        let mut event = converter.fail("server_error", "Gemini stream recovery failed; retry the request");
                        if let Some(error) = body.and_then(|body| body.get("error").cloned()) {
                            event["response"]["error"] = error;
                        }
                        if !proxy.fallback_attempt { usage.observe_value(&event, &proxy.service.logs, proxy.log_id); }
                        yield Ok(sse_bytes(&event));
                        break;
                    }
                    None => {},
                }
            }
            if interrupted {
                proxy.service.logs.complete(proxy.log_id,"failed","transport",Some("gemini_stream_error"),0);
                yield Ok(sse_bytes(&converter.fail("server_error", "Gemini stream interrupted; retry the request")));break;
            }
            match frame_events {
                Ok(events)=>for event in events {started=true;let bytes=sse_bytes(&event);if !proxy.fallback_attempt { usage.observe_value(&event,&proxy.service.logs,proxy.log_id); }yield Ok(bytes);},
                Err(cause)=>{
                    // Conversion errors contain structural diagnostics, never
                    // response bodies. Omit model-provided tool names as well.
                    let cause = cause.to_string();
                    let cause = if cause.starts_with("Gemini called undeclared tool ") {
                        "Gemini called an undeclared tool"
                    } else { &cause };
                    proxy.service.logs.update(proxy.log_id,"gemini_conversion_failed",json!({"reason":cause}), |_| true);
                    proxy.service.logs.complete(proxy.log_id,"failed","gemini_conversion",Some("gemini_conversion_error"),0);
                    yield Ok(sse_bytes(&converter.fail("server_error", &format!("Gemini stream conversion failed: {cause}; retry the request"))));
                    break;
                }
            }
            if converter.is_finished() {
                if !proxy.fallback_attempt { usage.finish(&proxy.service.logs,proxy.log_id); }
                break;
            }
            if eof {
                let next = match continuation.next() {
                    Ok(next) => next,
                    Err(e) => {
                        let event = converter.fail("server_error", &e.to_string());
                        if !proxy.fallback_attempt { usage.observe_value(&event, &proxy.service.logs, proxy.log_id); }
                        yield Ok(sse_bytes(&event)); break;
                    }
                };
                let next = if next.is_some() { next } else {
                    match converter.feed(&continuation.terminal()) {
                        Ok(events) => for event in events { yield Ok(sse_bytes(&event)); },
                        Err(e) => { yield Ok(sse_bytes(&converter.fail("server_error", &e.to_string()))); break; }
                    }
                    let retry = converter.prepare_tool_recovery();
                    if let Some(body) = &retry {
                        proxy.service.logs.update(proxy.log_id, "gemini_tool_call_recovery", json!({"attempt": converter.recovery_attempts()}), |_| true);
                        continuation = Continuation::new(body.clone(), budget);
                    }
                    retry
                };
                if let Some(retry_body) = next {
                    let send = send_native(&proxy, &config, &url, &retry_body);
                    tokio::pin!(send);
                    let next_upstream = loop {
                        tokio::select! {
                            result = &mut send => break result,
                            _ = heartbeat.tick() => yield Ok::<Bytes,std::io::Error>(Bytes::from_static(b": keepalive\n\n")),
                        }
                    };
                    match next_upstream {
                        Ok(response) => {
                            let ok_sse = response
                                .headers()
                                .get(header::CONTENT_TYPE)
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .contains("text/event-stream");
                            if !ok_sse {
                                proxy.service.logs.complete(proxy.log_id,"failed","transport",Some("gemini_stream_error"),0);
                                yield Ok(sse_bytes(&converter.fail("server_error", "Gemini streaming endpoint did not return SSE")));
                                break;
                            }
                            upstream = response;
                            parser = NativeSse::default();
                            continue;
                        }
                        Err(_) => {
                            proxy.service.logs.complete(proxy.log_id,"failed","transport",Some("gemini_stream_error"),0);
                            yield Ok(sse_bytes(&converter.fail("server_error", "Gemini follow-up request failed; retry the request")));
                            break;
                        }
                    }
                }
                match converter.finish(&proxy.service.gemini.codec) {
                    Ok(events) => for event in events {
                        let bytes = sse_bytes(&event);
                        if !proxy.fallback_attempt {
                            usage.observe_value(&event, &proxy.service.logs, proxy.log_id);
                        }
                        yield Ok(bytes);
                    },
                    Err(cause) => {
                        let cause = cause.to_string();
                        let cause = if cause.starts_with("Gemini called undeclared tool ") {
                            "Gemini called an undeclared tool"
                        } else { &cause };
                        proxy.service.logs.update(proxy.log_id,"gemini_conversion_failed",json!({"reason":cause}), |_| true);
                        proxy.service.logs.complete(proxy.log_id,"failed","gemini_conversion",Some("gemini_conversion_error"),0);
                        yield Ok(sse_bytes(&converter.fail("server_error", &format!("Gemini stream conversion failed: {cause}; retry the request"))));
                        break;
                    }
                }
                if !proxy.fallback_attempt { usage.finish(&proxy.service.logs,proxy.log_id); }
                break;
            }
        }
    });
    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
        .extensions_mut()
        .insert(super::fallback::Upstream { gemini: true });
    response
}

// Unary and hosted turns use the same incremental converter as SSE, retaining
// signed parts and partial tool calls across native continuation boundaries.
async fn unary_turn(
    proxy: &Arc<Proxy>,
    config: &ProviderConfig,
    url: &str,
    converted: &ConvertedRequest,
    id: &str,
    mut upstream: reqwest::Response,
) -> std::result::Result<(Value, Vec<Value>), Box<Response>> {
    let mut converter = ResponseStream::new(converted.clone(), id);
    let budget = converted
        .response_fields
        .get("max_output_tokens")
        .and_then(Value::as_u64);
    let mut continuation = Continuation::new(converted.body.clone(), budget);
    let mut events = Vec::new();
    let mut received = 0usize;
    loop {
        let result = async {
            let bytes = bounded_body(upstream, MAX_NATIVE_RESPONSE - received).await?;
            received += bytes.len();
            let mut native: Value = serde_json::from_slice(&bytes)?;
            continuation.feed(&mut native)?;
            events.extend(converter.feed(&native)?);
            if converter.is_finished() {
                return Ok(None);
            }
            if let Some(body) = continuation.next()? {
                return Ok(Some(body));
            }
            events.extend(converter.feed(&continuation.terminal())?);
            if let Some(body) = converter.prepare_tool_recovery() {
                proxy.service.logs.update(
                    proxy.log_id,
                    "gemini_tool_call_recovery",
                    json!({"attempt": converter.recovery_attempts()}),
                    |_| true,
                );
                continuation = Continuation::new(body.clone(), budget);
                return Ok(Some(body));
            }
            events.extend(converter.finish(&proxy.service.gemini.codec)?);
            Ok::<_, anyhow::Error>(None)
        }
        .await;
        match result {
            Ok(Some(body)) => {
                upstream = match send_native(proxy, config, url, &body).await {
                    Ok(upstream) => upstream,
                    Err(response) => return Err(response),
                };
            }
            Ok(None) => break,
            Err(_) => {
                return Err(Box::new(transport_error(
                    "Gemini response could not be completed; retry the request",
                    "gemini_conversion_error",
                )));
            }
        }
    }
    let response = events
        .last()
        .and_then(|e| e.get("response"))
        .cloned()
        .ok_or_else(|| {
            Box::new(transport_error(
                "Gemini turn has no terminal response",
                "gemini_conversion_error",
            ))
        })?;
    Ok((response, events))
}

// Hosted discovery uses bounded unary native turns. The Responses stream emits
// each completed discovery turn and keeps the connection alive while Google is
// generating. Client-executed search retains the normal incremental SSE path.
async fn hosted_turn(
    proxy: &Arc<Proxy>,
    config: &ProviderConfig,
    converted: &ConvertedRequest,
    id: &str,
) -> std::result::Result<(Value, Vec<Value>), Box<Response>> {
    let url = config
        .endpoint(&converted.model, false)
        .map_err(|e| Box::new(transport_error(&e.to_string(), "gemini_conversion_error")))?;
    let upstream = send_native(proxy, config, &url, &converted.body).await?;
    unary_turn(proxy, config, &url, converted, id, upstream).await
}

async fn forward_hosted(
    proxy: Arc<Proxy>,
    config: ProviderConfig,
    request: Value,
    mut converted: ConvertedRequest,
) -> Response {
    let id = format!("{:032x}", rand::random::<u128>());
    let response_id = format!("resp_{id}");
    let mut search = HostedSearch::new(request, &converted);
    if !converted.stream {
        for round in 0..16 {
            let (mut response, _) =
                match hosted_turn(&proxy, &config, &converted, &format!("{id}_{round}")).await {
                    Ok(v) => v,
                    Err(e) => return *e,
                };
            let next = match search.advance(
                &mut response,
                &converted,
                &config,
                &proxy.service.gemini.codec,
            ) {
                Ok(v) => v,
                Err(e) => return transport_error(&e.to_string(), "gemini_tool_search_error"),
            };
            response["id"] = json!(response_id);
            if let Some(next) = next {
                converted = next;
            } else {
                if !proxy.fallback_attempt {
                    proxy.service.logs.observe(proxy.log_id, &response);
                }
                return axum::Json(response).into_response();
            }
        }
        return transport_error(
            "Gemini tool search exceeded turn limit",
            "gemini_tool_search_error",
        );
    }
    let body = Body::from_stream(async_stream::stream! {
        let mut sequence = 0u64;
        let mut offset = 0usize;
        let mut usage = logs::UsageReader::new(true);
        for round in 0..16 {
            let turn_id = format!("{id}_{round}");
            let result = {
            let turn = hosted_turn(&proxy, &config, &converted, &turn_id);
            tokio::pin!(turn);
            let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
            loop {
                tokio::select! {
                    result = &mut turn => break result,
                    _ = heartbeat.tick() => yield Ok::<Bytes,std::io::Error>(Bytes::from_static(b": keepalive\n\n")),
                }
            }
            };
            // Drop the future's borrow before assigning the next converted request.
            let result = match result {
                Ok((mut response, events)) => {
                    let count = response["output"].as_array().map_or(0, Vec::len);
                    match search.advance(&mut response, &converted, &config, &proxy.service.gemini.codec) {
                        Ok(next) => Ok((response, events, count, next)),
                        Err(e) => Err(json!({"code":"gemini_tool_search_error","message":e.to_string()})),
                    }
                }
                Err(response) => {
                    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.ok();
                    let body = bytes.and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
                    Err(body.and_then(|body| body.get("error").cloned()).unwrap_or_else(||
                        json!({"code":"gemini_tool_search_error","message":"Gemini hosted tool search upstream request failed; retry the request"})))
                }
            };
            let (mut response, events, count, next) = match result {
                Ok(v) => v,
                Err(error) => {
                    let event = json!({"type":"response.failed","sequence_number":sequence,"response":search.failed_response(&response_id,error)});
                    let bytes = sse_bytes(&event);
                    if !proxy.fallback_attempt {
                        usage.observe_value(&event, &proxy.service.logs, proxy.log_id);
                        usage.finish(&proxy.service.logs, proxy.log_id);
                    }
                    yield Ok(bytes);
                    break;
                }
            };
            response["id"] = json!(response_id);
            let mut outgoing = Vec::new();
            for mut event in events {
                let kind = event["type"].as_str().unwrap_or("");
                if matches!(kind, "response.completed" | "response.incomplete" | "response.failed") { continue; }
                if round > 0 && matches!(kind, "response.created" | "response.in_progress") { continue; }
                if let Some(index) = event["output_index"].as_u64() { event["output_index"] = json!(index + offset as u64); }
                if let Some(response) = event.get_mut("response") { response["id"] = json!(response_id); }
                outgoing.push(event);
            }
            for (index, item) in response["output"].as_array().unwrap().iter().enumerate().skip(offset + count) {
                outgoing.push(json!({"type":"response.output_item.added","output_index":index,"item":item}));
                outgoing.push(json!({"type":"response.output_item.done","output_index":index,"item":item}));
            }
            offset = response["output"].as_array().unwrap().len();
            if next.is_none() {
                let kind = match response["status"].as_str() {Some("completed")=>"response.completed",Some("incomplete")=>"response.incomplete",_=>"response.failed"};
                outgoing.push(json!({"type":kind,"response":response}));
            }
            for mut event in outgoing {
                event["sequence_number"] = json!(sequence); sequence += 1;
                let bytes = sse_bytes(&event);
                if !proxy.fallback_attempt { usage.observe_value(&event, &proxy.service.logs, proxy.log_id); }
                yield Ok(bytes);
            }
            if let Some(next) = next { converted = next; } else {
                if !proxy.fallback_attempt { usage.finish(&proxy.service.logs, proxy.log_id); }
                break;
            }
        }
    });
    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
        .extensions_mut()
        .insert(super::fallback::Upstream { gemini: true });
    response
}

async fn credential_headers(proxy: &Proxy, config: &ProviderConfig) -> Result<HeaderMap> {
    if config.auth == Auth::Adc {
        return proxy.service.credentials.adc_headers().await;
    }
    let source = if config.auth == Auth::GcloudAdc {
        "sh://gcloud auth application-default print-access-token"
    } else {
        config.api_key.as_deref().unwrap_or("")
    };
    let value = proxy
        .service
        .credentials
        .resolve(source, Duration::from_secs(config.credential_cache_seconds))
        .await?;
    let mut headers = HeaderMap::new();
    if config.auth == Auth::ApiKey {
        headers.insert("x-goog-api-key", value);
    } else {
        let mut bearer = header::HeaderValue::from_str(&format!("Bearer {}", value.to_str()?))?;
        bearer.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, bearer);
    }
    Ok(headers)
}

pub(super) fn native_path(path: &str) -> bool {
    path == "/v1beta"
        || path.starts_with("/v1beta/")
        || path.starts_with("/v1/projects/")
        || path.starts_with("/v1beta1/")
        || (path.starts_with("/v1/models/") && path.contains(':'))
}

// Gemini CLI trims tool names for execution but can replay the untrimmed model
// call. Repair only a mismatch confirmed by the result with the same call ID.
fn repair_native_tool_names(body: &mut Value) -> bool {
    let Some(contents) = body.get_mut("contents").and_then(Value::as_array_mut) else {
        return false;
    };
    let results: Vec<(String, String)> = contents
        .iter()
        .filter_map(|content| content.get("parts").and_then(Value::as_array))
        .flatten()
        .filter_map(|part| {
            let result = part.get("functionResponse")?;
            Some((
                result.get("id")?.as_str()?.into(),
                result.get("name")?.as_str()?.into(),
            ))
        })
        .collect();
    let mut changed = false;
    for part in contents
        .iter_mut()
        .filter_map(|content| content.get_mut("parts").and_then(Value::as_array_mut))
        .flatten()
    {
        let Some(call) = part.get_mut("functionCall") else {
            continue;
        };
        let (Some(id), Some(name)) = (
            call.get("id").and_then(Value::as_str),
            call.get("name").and_then(Value::as_str),
        ) else {
            continue;
        };
        let trimmed = name.trim();
        if id.is_empty() || trimmed.is_empty() || name == trimmed {
            continue;
        }
        let mut matching = results
            .iter()
            .filter(|(result_id, _)| result_id == id)
            .peekable();
        if matching.peek().is_some() && matching.all(|(_, result_name)| result_name == trimmed) {
            call["name"] = Value::String(trimmed.to_owned());
            changed = true;
        }
    }
    changed
}

/// Native responses and non-generation requests remain byte-for-byte native.
pub(super) async fn forward_native(
    proxy: Arc<Proxy>,
    parts: axum::http::request::Parts,
    body: Body,
) -> Response {
    let Some(config) = &proxy.config.gemini else {
        return error(
            StatusCode::BAD_REQUEST,
            "Configure providers.gemini to proxy native Gemini requests",
        );
    };
    let credentials = match credential_headers(&proxy, config).await {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_GATEWAY, &e.to_string()),
    };
    proxy.record_accounting_route("gemini", "default");
    let incoming = parts.uri.path();
    let body = if parts.method == axum::http::Method::POST
        && [":generateContent", ":streamGenerateContent"]
            .iter()
            .any(|suffix| incoming.ends_with(suffix))
    {
        let bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "Could not read native Gemini request",
                );
            }
        };
        if let Ok(mut value) = serde_json::from_slice::<Value>(&bytes)
            && {
                let mut changed = repair_native_tool_names(&mut value);
                // -1 delegates the budget to Gemini. Level-based models express the
                // same choice by leaving the level unset and reject a numeric budget.
                if matches!(config.thinking, hey_proxy::gemini::Thinking::Level)
                    && let Some(thinking) = value
                        .pointer_mut("/generationConfig/thinkingConfig")
                        .and_then(Value::as_object_mut)
                    && thinking.get("thinkingBudget").and_then(Value::as_i64) == Some(-1)
                {
                    thinking.remove("thinkingBudget");
                    changed = true;
                }
                changed
            }
        {
            Body::from(value.to_string())
        } else {
            Body::from(bytes)
        }
    } else {
        body
    };
    // Config can name either the Developer API version root or a complete
    // Vertex publisher root. Incoming full Vertex resource paths remain valid.
    let root = match reqwest::Url::parse(&config.upstream_url) {
        Ok(v) => v,
        Err(_) => return error(StatusCode::BAD_REQUEST, "Invalid Gemini root"),
    };
    let base = root.path().trim_end_matches('/');
    let relative = if let Some(rest) = incoming
        .strip_prefix(base)
        .filter(|r| !base.is_empty() && r.starts_with('/'))
    {
        rest.to_owned()
    } else if let Some((_, rest)) = incoming.split_once("/models/") {
        format!("/models/{rest}")
    } else if let Some(rest) = incoming.strip_prefix("/v1beta") {
        rest.to_owned()
    } else {
        return error(
            StatusCode::BAD_REQUEST,
            "Native Gemini path does not match the configured provider root",
        );
    };
    let mut path = relative;
    if let Some(resource) = path.strip_prefix("/models/") {
        let (model, suffix) = resource
            .split_once(':')
            .map(|(m, s)| (m, format!(":{s}")))
            .unwrap_or((resource, String::new()));
        let model = model.to_owned();
        let alias = proxy.config.aliases.iter().find(|a| {
            a.api_shape.is_none() && (a.from == model || a.from == format!("models/{model}"))
        });
        if let Some(target) = alias.and_then(|a| a.to.as_deref()) {
            let Some(target) = target.strip_prefix("gemini/") else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "Native Gemini cross-provider rewrites require a gemini/model target",
                );
            };
            let target = target.strip_prefix("models/").unwrap_or(target);
            if config.endpoint(target, false).is_err() {
                return error(
                    StatusCode::BAD_REQUEST,
                    "Invalid native Gemini alias target",
                );
            }
            path = format!("/models/{target}{suffix}");
        }
        proxy.service.logs.route(
            proxy.log_id,
            Some(format!("gemini/{model}")),
            Some(format!(
                "gemini/{}",
                path.trim_start_matches("/models/")
                    .split(':')
                    .next()
                    .unwrap_or(&model)
            )),
            "gemini",
        );
    }
    let mut url = format!("{}{}", config.upstream_url.trim_end_matches('/'), path);
    if let Some(query) = parts.uri.query() {
        let retained: Vec<_> = url::form_urlencoded::parse(query.as_bytes())
            .filter(|(k, _)| k != "key" && k != "access_token")
            .collect();
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(retained)
            .finish();
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query);
        }
    }
    let mut headers = parts.headers;
    clean_headers(&mut headers);
    for name in [
        "host",
        "authorization",
        "x-goog-api-key",
        "x-api-key",
        "api-key",
        "cookie",
        "content-length",
        "openai-project",
        "openai-organization",
    ] {
        headers.remove(name);
    }
    headers.extend(credentials);
    let upstream = match proxy
        .client
        .request(parts.method, &url)
        .headers(headers)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await
    {
        Ok(v) => v,
        Err(_) => {
            return error(
                StatusCode::BAD_GATEWAY,
                "Could not reach native Gemini upstream",
            );
        }
    };
    let status = upstream.status();
    if status == StatusCode::UNAUTHORIZED && config.auth == Auth::Adc {
        proxy.service.credentials.invalidate_adc().await;
    }
    let mut headers = upstream.headers().clone();
    clean_headers(&mut headers);
    let mut response = Response::new(Body::from_stream(upstream.bytes_stream()));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, routing::post};
    use std::sync::atomic::{AtomicUsize, Ordering};
    pub(super) async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        (
            url,
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
        )
    }
    pub(super) fn provider(url: &str, auth: &str) -> ProviderConfig {
        serde_json::from_value(json!({"upstream_url":url,"auth":auth,"api_key":"synthetic-gemini"}))
            .unwrap()
    }
    #[test]
    fn native_sse_handles_every_byte_boundary_unicode_crlf_and_multiline() {
        let bytes="event: chunk\r\ndata: {\"candidates\":\r\ndata: [{\"content\":{\"parts\":[{\"text\":\"🌍\"}]}}]}\r\n\r\ndata: {\"usageMetadata\":{\"promptTokenCount\":2}}\n\n".as_bytes();
        let mut parser = NativeSse::default();
        let mut events = Vec::new();
        for byte in bytes {
            events.extend(parser.feed(&[*byte], false).unwrap());
        }
        events.extend(parser.feed(&[], true).unwrap());
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0]["candidates"][0]["content"]["parts"][0]["text"],
            "🌍"
        );
        assert!(
            NativeSse::default()
                .feed(b"data: invalid\n\n", false)
                .is_err()
        );
    }
    #[test]
    fn codec_key_survives_restart_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.json");
        let first = GeminiState::new(Some(&config)).unwrap();
        let provider = provider("http://localhost", "bearer");
        let request = convert_request(
            &json!({"model":"gemini/test","input":"hi"}),
            &provider,
            &first.codec,
        )
        .unwrap();
        let response=convert_response(&json!({"candidates":[{"content":{"role":"model","parts":[{"text":"hello","thoughtSignature":"synthetic"}]},"finishReason":"STOP"}]}),&request,&first.codec,"restart").unwrap();
        let second = GeminiState::new(Some(&config)).unwrap();
        assert!(
            convert_request(
                &json!({"model":"gemini/test","input":response["output"]}),
                &provider,
                &second.codec
            )
            .is_ok()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(config.with_extension("gemini-reasoning-key"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[tokio::test]
    async fn converts_aliases_strips_client_credentials_and_retries_capacity() {
        async fn upstream(
            State(count): State<Arc<AtomicUsize>>,
            headers: HeaderMap,
            axum::Json(body): axum::Json<Value>,
        ) -> Response {
            assert_eq!(headers["authorization"], "Bearer synthetic-gemini");
            assert!(!headers.contains_key("cookie"));
            assert!(!headers.contains_key("x-goog-api-key"));
            assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    axum::Json(
                        json!({"error":{"status":"UNAVAILABLE","message":"synthetic capacity"}}),
                    ),
                )
                    .into_response();
            }
            axum::Json(json!({"candidates":[{"content":{"role":"model","parts":[{"text":"hello","thoughtSignature":"signed-text"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3,"thoughtsTokenCount":4,"totalTokenCount":9}})).into_response()
        }
        let count = Arc::new(AtomicUsize::new(0));
        let (url, task) = serve(
            Router::new()
                .route(
                    "/models/gemini-3.1-pro-preview:generateContent",
                    post(upstream),
                )
                .with_state(count.clone()),
        )
        .await;
        let mut config = Config::test_fixture();
        config.gemini = Some(provider(&url, "bearer"));
        config.aliases = vec![crate::config::Alias {
            api_shape: None,
            from: "test-alias".into(),
            to: Some("gemini/models/gemini-3.1-pro-preview".into()),
            reasoning: None,
            api_key: None,
            reasoning_routes: Default::default(),
        }];
        config.retry.initial_delay_ms = 0;
        config.retry.max_delay_ms = 0;
        let (url, proxy) = serve(super::super::router(config).unwrap()).await;
        let response = reqwest::Client::new()
            .post(format!("{url}/v1/responses"))
            .bearer_auth("client-secret")
            .header("x-goog-api-key", "client-key")
            .header("cookie", "client-cookie")
            .json(&json!({"model":"test-alias","input":"hi"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["model"], "gemini/models/gemini-3.1-pro-preview");
        assert_eq!(body["usage"]["output_tokens"], 7);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert!(
            body["output"][0]["encrypted_content"]
                .as_str()
                .unwrap()
                .starts_with("hey_gemini_v1.")
        );
        task.abort();
        proxy.abort();
    }
    #[tokio::test]
    async fn hosted_tool_search_http_and_sse_preserve_discovery_and_replay() {
        async fn upstream(axum::Json(body): axum::Json<Value>) -> Response {
            let declarations = body["tools"][0]["functionDeclarations"].as_array().unwrap();
            let loaded = declarations
                .iter()
                .find(|d| d["name"] == hey_proxy::gemini::native_tool_name("lookup"));
            let parts = if let Some(tool) = loaded {
                assert!(body["contents"].as_array().unwrap().iter().any(|c| {
                    c["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|p| p.get("functionResponse").is_some())
                }));
                json!([{"functionCall":{"name":tool["name"],"args":{"id":"C42"},"id":"lookup-id"},"thoughtSignature":"lookup-signature"}])
            } else {
                assert_eq!(declarations.len(), 1);
                json!([{"functionCall":{"name":declarations[0]["name"],"args":{"paths":["lookup"]},"id":"search-id"},"thoughtSignature":"search-signature"}])
            };
            axum::Json(json!({"candidates":[{"content":{"role":"model","parts":parts},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2,"totalTokenCount":12}})).into_response()
        }
        let (upstream_url, upstream_task) =
            serve(Router::new().route("/models/test:generateContent", post(upstream))).await;
        let config = Config {
            gemini: Some(provider(&upstream_url, "api_key")),
            ..Config::test_fixture()
        };
        let (url, proxy_task) = serve(super::super::router(config).unwrap()).await;
        for streaming in [false, true] {
            let request = json!({"model":"gemini/test","stream":streaming,"input":"Find C42","tools":[
                {"type":"tool_search"},
                {"type":"function","name":"lookup","defer_loading":true,"parameters":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}}
            ]});
            let response = reqwest::Client::new()
                .post(format!("{url}/v1/responses"))
                .json(&request)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let body = if streaming {
                assert_eq!(
                    response.headers()[header::CONTENT_TYPE],
                    "text/event-stream"
                );
                let bytes = response.bytes().await.unwrap();
                let events = NativeSse::default().feed(&bytes, true).unwrap();
                for (index, event) in events.iter().enumerate() {
                    assert_eq!(event["sequence_number"], index);
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
                        .filter(|e| e["type"] == "response.completed")
                        .count(),
                    1
                );
                let completed = events.last().unwrap()["response"].clone();
                let done: Vec<_> = events
                    .iter()
                    .filter(|e| e["type"] == "response.output_item.done")
                    .map(|e| e["item"].clone())
                    .collect();
                assert_eq!(json!(done), completed["output"]);
                for event in &events {
                    if event["type"] == "response.output_item.done" {
                        assert_eq!(
                            event["item"],
                            completed["output"][event["output_index"].as_u64().unwrap() as usize]
                        );
                    }
                    if let Some(id) = event.pointer("/response/id") {
                        assert_eq!(id, &completed["id"]);
                    }
                }
                completed
            } else {
                response.json::<Value>().await.unwrap()
            };
            assert_eq!(body["status"], "completed");
            assert_eq!(body["usage"]["total_tokens"], 24);
            assert_eq!(body["output"][1]["type"], "tool_search_call");
            assert_eq!(body["output"][2]["type"], "tool_search_output");
            assert_eq!(body["output"][4]["name"], "lookup");
            // Replay the exact combined public output through the proxy once more.
            let mut next = request.clone();
            let mut input = vec![json!({"role":"user","content":"Find C42"})];
            input.extend(body["output"].as_array().unwrap().clone());
            input.push(
                json!({"type":"function_call_output","call_id":"lookup-id","output":"found"}),
            );
            next["input"] = json!(input);
            next["stream"] = json!(false);
            let replay = reqwest::Client::new()
                .post(format!("{url}/v1/responses"))
                .json(&next)
                .send()
                .await
                .unwrap();
            assert_eq!(replay.status(), 200);
            assert_eq!(replay.json::<Value>().await.unwrap()["status"], "completed");
        }
        upstream_task.abort();
        proxy_task.abort();
    }

    #[tokio::test]
    async fn native_gemini_repairs_confirmed_tool_name_mismatch_without_changing_history() {
        let input = json!({
            "contents": [
                {"role":"user","parts":[{"text":"Original task"}]},
                {"role":"model","parts":[
                    {"functionCall":{"id":"read-1","name":"read_file","args":{"name":" invoke_agent"}},"thoughtSignature":"opaque-first-signature"},
                    {"functionCall":{"id":"agent-2","name":" invoke_agent","args":{"task":"Investigate"}}}
                ]},
                {"role":"user","parts":[
                    {"functionResponse":{"id":"read-1","name":"read_file","response":{"text":"untouched"}}},
                    {"functionResponse":{"id":"agent-2","name":"invoke_agent","response":{"name":" result name ","result":"Done"}}}
                ]}
            ],
            "tools":[{"functionDeclarations":[{"name":"invoke_agent","parameters":{"type":"object"}}]}],
            "generationConfig":{"thinkingConfig":{"thinkingLevel":"HIGH"}}
        });
        let mut expected = input.clone();
        expected["contents"][1]["parts"][1]["functionCall"]["name"] = json!("invoke_agent");
        let original = serde_json::to_vec_pretty(&input).unwrap();
        async fn upstream(
            State((expected, original)): State<(Value, Vec<u8>)>,
            request: Request,
        ) -> Response {
            let untouched = request.uri().path().ends_with(":countTokens");
            let bytes = axum::body::to_bytes(request.into_body(), 10000)
                .await
                .unwrap();
            if untouched {
                assert_eq!(bytes.as_ref(), original);
            } else {
                assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), expected);
            }
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                "data: {\"thoughtSignature\":\"unchanged-response\"}\n\n",
            )
                .into_response()
        }
        let (upstream_url, task) = serve(
            Router::new()
                .fallback(upstream)
                .with_state((expected, original.clone())),
        )
        .await;
        let config = Config {
            gemini: Some(provider(&upstream_url, "api_key")),
            ..Config::test_fixture()
        };
        let (url, proxy) = serve(super::super::router(config).unwrap()).await;
        for operation in ["generateContent", "streamGenerateContent", "countTokens"] {
            let response = reqwest::Client::new()
                .post(format!("{url}/v1beta/models/gemini-test:{operation}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(original.clone())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(
                response.text().await.unwrap(),
                "data: {\"thoughtSignature\":\"unchanged-response\"}\n\n"
            );
        }
        task.abort();
        proxy.abort();
    }

    #[test]
    fn native_gemini_leaves_unconfirmed_or_conflicting_tool_names_unchanged() {
        for (id, name, result_ids, result_names) in [
            ("a", "invoke_agent", vec!["a"], vec!["invoke_agent"]),
            ("", " invoke_agent", vec![""], vec!["invoke_agent"]),
            ("a", " invoke_agent", vec!["b"], vec!["invoke_agent"]),
            ("a", " invoke_agent", vec!["a"], vec!["other_tool"]),
            ("a", "invoke agent", vec!["a"], vec!["invoke_agent"]),
            (
                "a",
                " invoke_agent",
                vec!["a", "a"],
                vec!["invoke_agent", "other_tool"],
            ),
            ("a", "  ", vec!["a"], vec![""]),
        ] {
            let mut body = json!({"contents":[
                {"parts":[{"functionCall":{"id":id,"name":name,"args":{}}}]},
                {"parts":result_ids.iter().zip(result_names).map(|(id,name)|json!({"functionResponse":{"id":id,"name":name,"response":{}}})).collect::<Vec<_>>()}
            ]});
            let original = body.clone();
            assert!(!repair_native_tool_names(&mut body));
            assert_eq!(body, original);
        }
        for mut body in [
            Value::Null,
            json!({"contents":"invalid"}),
            json!({"contents":[{"parts":[{"functionCall":{}}, {"functionResponse":{}}]}]}),
        ] {
            let original = body.clone();
            assert!(!repair_native_tool_names(&mut body));
            assert_eq!(body, original);
        }
    }

    #[tokio::test]
    async fn native_level_provider_accepts_automatic_thinking() {
        let (upstream_url, upstream) = serve(
            Router::new()
                .fallback(|axum::Json(body): axum::Json<Value>| async move { axum::Json(body) }),
        )
        .await;
        let mut gemini = provider(&upstream_url, "api_key");
        gemini.thinking = hey_proxy::gemini::Thinking::Level;
        let (url, proxy) = serve(
            super::super::router(Config {
                gemini: Some(gemini),
                ..Config::test_fixture()
            })
            .unwrap(),
        )
        .await;
        let client = reqwest::Client::new();
        for (budget, expected) in [
            (-1, json!({"includeThoughts": true})),
            (
                1024,
                json!({"includeThoughts": true, "thinkingBudget": 1024}),
            ),
        ] {
            let response = client
                .post(format!("{url}/v1beta/models/example:generateContent"))
                .json(&json!({"generationConfig":{"thinkingConfig":{"includeThoughts":true,"thinkingBudget":budget}}}))
                .send().await.unwrap().json::<Value>().await.unwrap();
            assert_eq!(response["generationConfig"]["thinkingConfig"], expected);
        }
        upstream.abort();
        proxy.abort();
    }

    #[tokio::test]
    async fn native_gemini_passthrough_preserves_body_and_strips_query_keys() {
        async fn upstream(request: Request) -> Response {
            assert_eq!(
                request.uri().path(),
                "/models/gemini-3.1-pro-preview:generateContent"
            );
            assert_eq!(request.uri().query(), Some("alt=sse"));
            assert_eq!(request.headers()["x-goog-api-key"], "synthetic-gemini");
            assert!(!request.headers().contains_key("authorization"));
            let bytes = axum::body::to_bytes(request.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(bytes, b"{ \"contents\": [] }"[..]);
            (
                [(header::CONTENT_TYPE, "application/json")],
                "{ \"native\": true }",
            )
                .into_response()
        }
        let (upstream_url, task) = serve(Router::new().fallback(upstream)).await;
        let config = Config {
            gemini: Some(provider(&upstream_url, "api_key")),
            ..Config::test_fixture()
        };
        let (url, proxy) = serve(super::super::router(config).unwrap()).await;
        let response=reqwest::Client::new().post(format!("{url}/v1beta/models/gemini-3.1-pro-preview:generateContent?key=CLIENT_SECRET&alt=sse&access_token=CLIENT_TOKEN")).header("authorization","Bearer client").body("{ \"contents\": [] }").send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "{ \"native\": true }");
        task.abort();
        proxy.abort();
    }
    #[tokio::test]
    async fn openai_stays_default_with_both_providers_configured() {
        async fn upstream(headers: HeaderMap, axum::Json(body): axum::Json<Value>) -> Response {
            assert_eq!(headers["authorization"], "Bearer something");
            assert_eq!(body["model"], "ordinary-model");
            axum::Json(json!({"id":"openai-response","output":[]})).into_response()
        }
        let (upstream_url, task) =
            serve(Router::new().route("/v1/responses", post(upstream))).await;
        let config = Config {
            upstream_url,
            gemini: Some(provider("http://127.0.0.1:1", "bearer")),
            ..Config::test_fixture()
        };
        let (url, proxy) = serve(super::super::router(config).unwrap()).await;
        for model in ["ordinary-model", "openai/ordinary-model"] {
            let response = reqwest::Client::new()
                .post(format!("{url}/v1/responses"))
                .json(&json!({"model":model,"input":"hi"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(
                response.json::<Value>().await.unwrap()["id"],
                "openai-response"
            );
        }
        task.abort();
        proxy.abort();
    }
    #[tokio::test]
    async fn stream_is_incremental_and_records_terminal_usage() {
        async fn upstream() -> Response {
            let body = Body::from_stream(async_stream::stream! {
                yield Ok::<_,std::io::Error>(Bytes::from_static(b"data: {\"candidates\":[{\"index\":0,\"content\":{\"parts\":[{\"text\":\"early\"}]}}]}\n\n"));
                tokio::time::sleep(Duration::from_millis(150)).await;
                yield Ok(Bytes::from_static(b"data: {\"candidates\":[{\"index\":0,\"finishReason\":\"STOP\"}]}\n\ndata: {\"usageMetadata\":{\"promptTokenCount\":1,\"candidatesTokenCount\":2,\"thoughtsTokenCount\":3,\"totalTokenCount\":6}}\n\n"));
            });
            let mut response = Response::new(body);
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
            response
        }
        let (upstream_url, task) = serve(Router::new().fallback(upstream)).await;
        let config = Config {
            gemini: Some(provider(&upstream_url, "bearer")),
            ..Config::test_fixture()
        };
        let (url, proxy) = serve(super::super::router(config).unwrap()).await;
        let mut response = reqwest::Client::new()
            .post(format!("{url}/v1/responses"))
            .json(&json!({"model":"gemini/test","input":"hi","stream":true}))
            .send()
            .await
            .unwrap();
        let early = tokio::time::timeout(Duration::from_millis(100), response.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let mut text = String::from_utf8(early.to_vec()).unwrap();
        assert!(text.contains("response.created"));
        while let Some(bytes) = response.chunk().await.unwrap() {
            text.push_str(std::str::from_utf8(&bytes).unwrap());
        }
        assert!(text.contains("response.output_text.delta"));
        assert!(text.contains("response.completed"));
        assert!(text.contains("\"output_tokens\":5"));
        let logs = reqwest::get(format!("{url}/logs/api"))
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(logs["entries"][0]["output_tokens"], 5);
        task.abort();
        proxy.abort();
    }
}

#[cfg(test)]
mod hardening_tests;
