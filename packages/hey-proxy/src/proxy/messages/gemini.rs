//! Direct Messages transport for Gemini; no Responses conversion is involved.
use super::super::gemini::continuation::Continuation;
use super::*;
use gemini_stream::NativeStream;

fn frame(event: &Value) -> Bytes {
    Bytes::from(format!(
        "event: {}\ndata: {event}\n\n",
        event["type"].as_str().unwrap_or("error")
    ))
}
fn failed(proxy: &Proxy, message: &str) -> Bytes {
    proxy.service.logs.observe(
        proxy.log_id,
        &json!({"error":{"code":"messages_gemini_stream_error"}}),
    );
    frame(&response::error_value(StatusCode::BAD_GATEWAY, message))
}
async fn body(mut response: reqwest::Response) -> Result<Value> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            chunk.len() <= LIMIT - bytes.len(),
            "Gemini body exceeds 64 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}
pub(super) async fn forward(
    proxy: Arc<Proxy>,
    input: Value,
    model: String,
    effort: Option<String>,
    count: bool,
) -> Response {
    let Some(config) = &proxy.config.gemini else {
        return error(StatusCode::BAD_REQUEST, "Configure providers.gemini first");
    };
    let converted = match gemini_request::convert(
        &input,
        &model,
        effort.as_deref(),
        config,
        &proxy.service.gemini.codec,
        count,
    ) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let mut url = match config.endpoint(&converted.model, converted.stream && !count) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let mut request = converted.body.clone();
    if count {
        request.as_object_mut().unwrap().remove("generationConfig");
        request.as_object_mut().unwrap().remove("toolConfig");
        url = url.replace(":generateContent", ":countTokens");
        if config.upstream_url.contains("/publishers/") {
            request.as_object_mut().unwrap().remove("toolConfig");
        } else {
            request["model"] = json!(format!("models/{}", converted.model));
            request = json!({"generateContentRequest":request});
        }
    }
    proxy.service.logs.route(
        proxy.log_id,
        input["model"].as_str().map(str::to_owned),
        Some(model.clone()),
        "gemini",
    );
    proxy.service.logs.update(proxy.log_id,"provider_selected",json!({"provider":"gemini","adapter":"messages_gemini_direct","operation":if count{"countTokens"}else{"generateContent"}}),|entry|{entry.streaming=converted.stream&&!count;true});
    let mut upstream = match super::super::gemini::send_native(&proxy, config, &url, &request).await
    {
        Ok(r) => r,
        Err(r) => {
            let status = r.status();
            let delay = r.headers().get(header::RETRY_AFTER).cloned();
            let bytes = axum::body::to_bytes((*r).into_body(), 1024 * 1024)
                .await
                .unwrap_or_default();
            let value = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
            let mut out = error(
                status,
                value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Gemini request failed"),
            );
            if let Some(delay) = delay {
                out.headers_mut().insert(header::RETRY_AFTER, delay);
            }
            return out;
        }
    };
    if count {
        return match body(upstream).await {
            Ok(v) if v["totalTokens"].as_u64().is_some() => {
                axum::Json(json!({"input_tokens":v["totalTokens"]})).into_response()
            }
            _ => error(
                StatusCode::BAD_GATEWAY,
                "Gemini countTokens returned no token count",
            ),
        };
    }
    let mut continuation = Continuation::new(request, input["max_tokens"].as_u64());
    let mut state = NativeStream::new(input["model"].as_str().unwrap_or(&model));
    if !converted.stream {
        loop {
            let result = async {
                let mut native = body(upstream).await?;
                continuation.feed(&mut native)?;
                state.feed(&native, &converted)?;
                if let Some(next) = continuation.next()? {
                    return Ok(Some(next));
                }
                state.feed(&continuation.terminal(), &converted)?;
                state.end(&converted, &proxy.service.gemini.codec)?;
                Ok::<_, anyhow::Error>(None)
            }
            .await;
            match result {
                Ok(Some(next)) => {
                    upstream = match super::super::gemini::send_native(&proxy, config, &url, &next)
                        .await
                    {
                        Ok(response) => response,
                        Err(_) => {
                            return error(
                                StatusCode::BAD_GATEWAY,
                                "Gemini follow-up request failed",
                            );
                        }
                    };
                }
                Ok(None) => {
                    state.log_usage(&proxy);
                    proxy.service.logs.first_output(proxy.log_id);
                    return axum::Json(state.message).into_response();
                }
                Err(e) => return error(StatusCode::BAD_GATEWAY, &e.to_string()),
            }
        }
    }

    if !upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"))
    {
        return error(StatusCode::BAD_GATEWAY, "Expected native Gemini SSE");
    }
    let config = config.clone();
    let body = Body::from_stream(async_stream::stream! {
        let mut decoder=sse::SseDecoder::default();
        let mut heartbeat=tokio::time::interval(Duration::from_secs(15));heartbeat.tick().await;
        'read: loop {
            // Keep the pending native read alive across heartbeat ticks.
            let chunk = {
                let read=upstream.chunk();tokio::pin!(read);
                loop {tokio::select!{chunk=&mut read=>break chunk,_=heartbeat.tick()=>{yield Ok::<Bytes,std::io::Error>(frame(&json!({"type":"ping"})));}}}
            };
            let eof=matches!(&chunk,Ok(None));
            let events=match chunk{Ok(Some(bytes))=>decoder.feed(&bytes,false),Ok(None)=>decoder.feed(&[],true),Err(_)=>{yield Ok(failed(&proxy,"Native Gemini stream disconnected"));break;}};
            match events {
                Ok(events)=>for mut event in events {
                    if let Err(e) = continuation.feed(&mut event) { yield Ok(failed(&proxy, &e.to_string())); break 'read; }
                    match state.feed(&event,&converted){Ok(events)=>for event in events{
                        if event["type"] == "content_block_delta" { proxy.service.logs.first_output(proxy.log_id); }
                        yield Ok(frame(&event));
                    },Err(e)=>{yield Ok(failed(&proxy,&e.to_string()));break 'read;}}
                    state.log_usage(&proxy);
                },
                Err(_)=>{yield Ok(failed(&proxy,"Invalid or oversized native Gemini SSE"));break;}
            }
            if eof {
                match continuation.next() {
                    Ok(Some(next)) => {
                        let send = super::super::gemini::send_native(&proxy, &config, &url, &next);
                        tokio::pin!(send);
                        let response = loop { tokio::select! {
                            result = &mut send => break result,
                            _ = heartbeat.tick() => yield Ok(frame(&json!({"type":"ping"}))),
                        }};
                        match response {
                            Ok(response) if response.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("text/event-stream")) => {
                                upstream = response; decoder = sse::SseDecoder::default(); continue;
                            }
                            _ => { yield Ok(failed(&proxy, "Gemini follow-up request failed")); break; }
                        }
                    }
                    Ok(None) => {},
                    Err(e) => { yield Ok(failed(&proxy, &e.to_string())); break; }
                }
                if let Err(e) = state.feed(&continuation.terminal(), &converted) { yield Ok(failed(&proxy, &e.to_string())); break; }
                match state.end(&converted,&proxy.service.gemini.codec){Ok(events)=>for event in events{yield Ok(frame(&event));},Err(e)=>yield Ok(failed(&proxy,&e.to_string()))}
                state.log_usage(&proxy);break;
            }
        }
    });
    (
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        body,
    )
        .into_response()
}
