use super::*;
use anyhow::{anyhow, ensure};
use std::collections::BTreeMap;
type Log = Option<(Arc<logs::Store>, u64)>;

#[derive(Default)]
struct State {
    started: bool,
    blocks: usize,
    text: Option<usize>,
    thinking: Option<usize>,
    thinking_text: String,
    tools: BTreeMap<u64, (usize, String)>,
    details: BTreeMap<u64, Value>,
    finish: Option<String>,
    usage: Value,
    retained: usize,
}
fn frame(value: Value) -> Bytes {
    Bytes::from(format!(
        "event: {}\ndata: {value}\n\n",
        value["type"].as_str().unwrap()
    ))
}
fn failure(message: &str, log: &Log) -> Bytes {
    if let Some((store, id)) = log {
        store.observe(*id, &json!({"error":{"code":"messages_stream_error"}}));
    }
    frame(response::error_value(StatusCode::BAD_GATEWAY, message))
}
impl State {
    fn block(&mut self, block: Value, events: &mut Vec<Value>) -> usize {
        let index = self.blocks;
        self.blocks += 1;
        events.push(json!({"type":"content_block_start","index":index,"content_block":block}));
        index
    }
    fn event(&mut self, chunk: &Value, model: &str) -> Result<Vec<Value>> {
        ensure!(
            chunk.get("error").is_none(),
            "{}",
            chunk
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("Upstream stream failed")
        );
        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(json!({"type":"message_start","message":{
                "id":format!("msg_{}",chunk["id"].as_str().ok_or_else(|| anyhow!("Upstream stream lacks id"))?),
                "type":"message","role":"assistant","model":model,"content":[],"stop_reason":null,"stop_sequence":null,
                "usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}));
        }
        if chunk["usage"].is_object() {
            self.usage = chunk["usage"].clone();
        }
        let choice = &chunk["choices"][0];
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish = Some(response::stop(reason)?.into());
        }
        let delta = &choice["delta"];
        // Count retained arguments/reasoning, not text that is streamed through.
        self.retained = self.retained.saturating_add(delta.to_string().len());
        ensure!(self.retained <= LIMIT, "Messages stream exceeds 64 MiB");
        for detail in delta["reasoning_details"].as_array().into_iter().flatten() {
            let index = detail["index"]
                .as_u64()
                .ok_or_else(|| anyhow!("Reasoning delta lacks index"))?;
            let current = self.details.entry(index).or_insert_with(|| {
                let mut v = detail.clone();
                for key in ["summary", "text", "data"] {
                    if v.get(key).is_some() {
                        v[key] = json!("");
                    }
                }
                v
            });
            for key in ["summary", "text", "data"] {
                if let Some(text) = detail[key].as_str() {
                    let old = current[key].as_str().unwrap_or("");
                    current[key] = json!(old.to_owned() + text);
                }
            }
        }
        if let Some(text) = delta["reasoning"].as_str().filter(|s| !s.is_empty()) {
            let index = match self.thinking {
                Some(index) => index,
                None => {
                    let index = self.block(
                        json!({"type":"thinking","thinking":"","signature":""}),
                        &mut events,
                    );
                    self.thinking = Some(index);
                    index
                }
            };
            self.thinking_text.push_str(text);
            events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":text}}));
        }
        for key in ["content", "refusal"] {
            if let Some(text) = delta[key].as_str().filter(|s| !s.is_empty()) {
                let index = match self.text {
                    Some(index) => index,
                    None => {
                        let index = self.block(json!({"type":"text","text":""}), &mut events);
                        self.text = Some(index);
                        index
                    }
                };
                events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}));
            }
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let key = call["index"]
                .as_u64()
                .ok_or_else(|| anyhow!("Tool delta lacks index"))?;
            if !self.tools.contains_key(&key) {
                ensure!(
                    call["id"].is_string() && call["function"]["name"].is_string(),
                    "Tool arguments arrived without identity"
                );
                let index = self.block(json!({"type":"tool_use","id":call["id"],"name":call["function"]["name"],"input":{}}),&mut events);
                self.tools.insert(key, (index, String::new()));
            }
            let (index, args) = self.tools.get_mut(&key).unwrap();
            if let Some(text) = call["function"]["arguments"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                args.push_str(text);
                events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":text}}));
            }
        }
        Ok(events)
    }
    fn end(&mut self) -> Result<Vec<Value>> {
        let reason = self
            .finish
            .clone()
            .ok_or_else(|| anyhow!("Stream ended without a finish reason"))?;
        ensure!(self.usage.is_object(), "Stream ended without usage");
        for (_, args) in self.tools.values() {
            let parsed = serde_json::from_str::<Value>(args);
            ensure!(
                parsed.is_ok_and(|v| v.is_object()),
                "Upstream tool arguments are not a complete JSON object"
            );
        }
        let mut events = Vec::new();
        let details: Vec<_> = self.details.values().cloned().collect();
        if let Some(index) = self.thinking {
            ensure!(
                response::reasoning_text(&details) == self.thinking_text,
                "Thinking replay data does not match streamed text"
            );
            events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":response::encode_details(&details)}}));
        } else if let Some(block) = response::thinking(&details) {
            self.block(block, &mut events);
        }
        for index in 0..self.blocks {
            events.push(json!({"type":"content_block_stop","index":index}));
        }
        events.push(json!({"type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},"usage":response::usage(&self.usage)}));
        events.push(json!({"type":"message_stop"}));
        Ok(events)
    }
}

pub(super) fn adapt(upstream: Response, model: String, log: Log) -> Response {
    if !upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"))
    {
        return error(StatusCode::BAD_GATEWAY, "Expected an event stream");
    }
    let (mut parts, body) = upstream.into_parts();
    response::clean_headers(&mut parts.headers);
    parts.headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/event-stream"),
    );
    let mut source = body.into_data_stream();
    let mut decoder = sse::SseDecoder::default();
    let mut state = State::default();
    let body = Body::from_stream(async_stream::stream! {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        heartbeat.tick().await;
        'read: loop {
            let chunk = tokio::select! {
                chunk = source.next() => chunk,
                _ = heartbeat.tick() => { yield Ok::<Bytes,std::io::Error>(frame(json!({"type":"ping"}))); continue; }
            };
            let eof = chunk.is_none();
            let decoded = match chunk {
                Some(Ok(bytes)) => decoder.feed(&bytes,false),
                None => decoder.feed(&[],true),
                Some(Err(_)) => { yield Ok(failure("Upstream stream disconnected",&log)); break; }
            };
            match decoded {
                Ok(chunks) => for chunk in chunks {
                    match state.event(&chunk,&model) {
                        Ok(events) => for event in events { yield Ok(frame(event)); },
                        Err(e) => { yield Ok(failure(&e.to_string(),&log)); break 'read; }
                    }
                },
                Err(_) => { yield Ok(failure("Invalid or oversized SSE frame",&log)); break; }
            }
            if eof {
                match state.end() {
                    Ok(events) => for event in events { yield Ok(frame(event)); },
                    Err(e) => yield Ok(failure(&e.to_string(),&log)),
                }
                break;
            }
        }
    });
    Response::from_parts(parts, body)
}
