//! Native Gemini chunks -> Messages events, with authenticated native replay.
use super::*;
use anyhow::{anyhow, bail, ensure};
use gemini_request::NativeRequest;
use hey_proxy::gemini::{PartialCalls, ReasoningCodec};

pub(super) struct NativeStream {
    id: String,
    pub message: Value,
    native: Vec<Value>,
    active: Option<(usize, bool)>,
    partial: PartialCalls,
    started: bool,
    reason: Option<String>,
    usage: Option<Value>,
    bytes: usize,
}
pub(super) fn usage(value: &Value) -> Result<Value> {
    let input = value["promptTokenCount"]
        .as_u64()
        .ok_or_else(|| anyhow!("Gemini response lacks prompt token usage"))?;
    let cache = value["cachedContentTokenCount"].as_u64().unwrap_or(0);
    let output = value["candidatesTokenCount"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(value["thoughtsTokenCount"].as_u64().unwrap_or(0));
    Ok(
        json!({"input_tokens":input.saturating_sub(cache),"output_tokens":output,"cache_read_input_tokens":cache,"cache_creation_input_tokens":0}),
    )
}
impl NativeStream {
    pub fn new(model: &str) -> Self {
        let id = format!("msg_{:032x}", rand::random::<u128>());
        Self {
            message: json!({"id":id,"type":"message","role":"assistant","model":model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}),
            id,
            native: Vec::new(),
            active: None,
            partial: PartialCalls::default(),
            started: false,
            reason: None,
            usage: None,
            bytes: 0,
        }
    }
    fn block(&mut self, block: Value, events: &mut Vec<Value>) -> usize {
        let blocks = self.message["content"].as_array_mut().unwrap();
        let index = blocks.len();
        blocks.push(block.clone());
        events.push(json!({"type":"content_block_start","index":index,"content_block":block}));
        index
    }
    pub fn feed(&mut self, chunk: &Value, request: &NativeRequest) -> Result<Vec<Value>> {
        self.bytes = self.bytes.saturating_add(chunk.to_string().len());
        ensure!(self.bytes <= LIMIT, "Gemini response exceeds 64 MiB");
        ensure!(
            chunk.get("error").is_none(),
            "Gemini stream returned an error"
        );
        if let Some(value) = chunk.get("usageMetadata") {
            // Native streaming metadata may contain only trafficType or partial
            // counts until the terminal chunk. Accumulate fields; require real
            // prompt usage at completion, not on the first metadata envelope.
            let fields = value
                .as_object()
                .ok_or_else(|| anyhow!("Invalid Gemini usage metadata"))?;
            let accumulated = self.usage.get_or_insert_with(|| json!({}));
            accumulated.as_object_mut().unwrap().extend(fields.clone());
            if accumulated["promptTokenCount"].is_u64() {
                self.message["usage"] = usage(accumulated)?;
            }
        }
        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(json!({"type":"message_start","message":self.message.clone()}));
        }
        if let Some(blocked) = chunk
            .pointer("/promptFeedback/blockReason")
            .and_then(Value::as_str)
        {
            ensure!(!blocked.is_empty(), "Invalid block reason");
            self.reason = Some("SAFETY".into());
        }
        if let Some(candidates) = chunk.get("candidates") {
            let candidates = candidates
                .as_array()
                .ok_or_else(|| anyhow!("Gemini candidates must be an array"))?;
            ensure!(
                candidates.len() <= 1,
                "Cannot discard multiple Gemini candidates"
            );
            if let Some(candidate) = candidates.first() {
                ensure!(
                    candidate.get("index").is_none_or(|i| i == 0),
                    "Unexpected Gemini candidate index"
                );
                let parts = candidate
                    .pointer("/content/parts")
                    .map(|p| {
                        p.as_array()
                            .ok_or_else(|| anyhow!("Invalid Gemini content parts"))
                    })
                    .transpose()?;
                for part in parts.into_iter().flatten() {
                    ensure!(
                        self.reason.is_none(),
                        "Gemini emitted content after its finish reason"
                    );
                    for part in self.partial.part(part)? {
                        let object = part
                            .as_object()
                            .ok_or_else(|| anyhow!("Invalid Gemini part"))?;
                        ensure!(
                            object.keys().all(|k| [
                                "text",
                                "thought",
                                "thoughtSignature",
                                "functionCall"
                            ]
                            .contains(&k.as_str())),
                            "Unsupported Gemini output part"
                        );
                        if let Some(text) = part.get("text") {
                            let text = text
                                .as_str()
                                .ok_or_else(|| anyhow!("Gemini text must be a string"))?;
                            let thought = part["thought"] == true;
                            if !text.is_empty() && (!thought || request.display_thinking) {
                                let index = match self.active.filter(|(_, t)| *t == thought) {
                                    Some((index, _)) => index,
                                    None => {
                                        let index=self.block(if thought{json!({"type":"thinking","thinking":"","signature":""})}else{json!({"type":"text","text":""})},&mut events);
                                        self.active = Some((index, thought));
                                        index
                                    }
                                };
                                let field = if thought { "thinking" } else { "text" };
                                let previous =
                                    self.message["content"][index][field].as_str().unwrap();
                                self.message["content"][index][field] =
                                    json!(previous.to_owned() + text);
                                events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":if thought{"thinking_delta"}else{"text_delta"},field:text}}));
                            }
                        } else if let Some(call) = part.get("functionCall") {
                            let native = call["name"]
                                .as_str()
                                .ok_or_else(|| anyhow!("Function call lacks name"))?;
                            let name = request
                                .tools
                                .get(native)
                                .or_else(|| request.tools.values().find(|v| v.as_str() == native))
                                .ok_or_else(|| anyhow!("Gemini called undeclared tool {native}"))?;
                            let args = call.get("args").cloned().unwrap_or(json!({}));
                            ensure!(args.is_object(), "Tool input must be an object");
                            let index = self.message["content"].as_array().unwrap().len();
                            let id = call["id"]
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| format!("toolu_{}_{index}", self.id));
                            ensure!(
                                !self.message["content"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .any(|b| b["type"] == "tool_use" && b["id"] == id),
                                "Duplicate Gemini tool id"
                            );
                            let index = self.block(
                                json!({"type":"tool_use","id":id,"name":name,"input":{}}),
                                &mut events,
                            );
                            self.message["content"][index]["input"] = args.clone();
                            self.active = None;
                            events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":args.to_string()}}));
                        } else {
                            ensure!(
                                object.len() == 1 && object.contains_key("thoughtSignature"),
                                "Unsupported empty Gemini part"
                            );
                        }
                        // Native chunks can split signed text. Coalesce only unsigned
                        // adjacent text of the same kind; a signed part is a boundary.
                        let previous = self.native.last_mut();
                        if let Some(previous) = previous.filter(|p| {
                            p.get("text").is_some()
                                && part.get("text").is_some()
                                && p["thought"] == part["thought"]
                                && p.get("thoughtSignature").is_none()
                        }) {
                            previous["text"] = json!(
                                previous["text"].as_str().unwrap().to_owned()
                                    + part["text"].as_str().unwrap()
                            );
                            if let Some(signature) = part.get("thoughtSignature") {
                                previous["thoughtSignature"] = signature.clone();
                            }
                        } else {
                            self.native.push(part);
                        }
                    }
                }
                if let Some(reason) = candidate.get("finishReason") {
                    self.reason = Some(
                        reason
                            .as_str()
                            .ok_or_else(|| anyhow!("Invalid Gemini finish reason"))?
                            .into(),
                    );
                }
            }
        }
        Ok(events)
    }
    pub fn end(&mut self, request: &NativeRequest, codec: &ReasoningCodec) -> Result<Vec<Value>> {
        ensure!(
            !self.partial.pending(),
            "Gemini stream ended with incomplete function arguments"
        );
        self.message["usage"] = usage(self.usage.as_ref().ok_or_else(|| {
            anyhow!("Gemini response lacks token usage; cannot track compaction safely")
        })?)?;
        if self.native.iter().any(|part| {
            part.get("functionCall").is_some()
                || (part["thought"] != true && part["text"].as_str().is_some_and(|s| !s.is_empty()))
        }) {
            ensure!(
                self.usage.as_ref().unwrap()["candidatesTokenCount"].is_u64(),
                "Gemini response lacks generated token usage"
            );
        }
        let calls = self.message["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .count();
        ensure!(
            request.parallel || calls <= 1,
            "Gemini violated disable_parallel_tool_use"
        );
        let reason = match self.reason.as_deref() {
            Some("STOP") => {
                if calls > 0 {
                    "tool_use"
                } else {
                    "end_turn"
                }
            }
            Some("MAX_TOKENS") => "max_tokens",
            Some(
                "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII"
                | "IMAGE_SAFETY",
            ) => "refusal",
            Some(reason) => bail!("Unsupported Gemini finish reason {reason}"),
            None => bail!("Gemini response ended without finishReason"),
        };
        if let Some(schema) = request.body.pointer("/generationConfig/responseJsonSchema") {
            let text: String = self.message["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect();
            let value: Value = serde_json::from_str(&text)
                .map_err(|_| anyhow!("Gemini structured output is not JSON"))?;
            ensure!(
                jsonschema::validator_for(schema)?.is_valid(&value),
                "Gemini structured output violates the requested schema"
            );
        }
        let mut events = Vec::new();
        let content = gemini_request::visible(self.message["content"].as_array().unwrap());
        let carrier = codec.seal_messages(&request.model, &json!(self.native), &json!(content))?;
        let thinking: Vec<_> = self.message["content"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, b)| b["type"] == "thinking")
            .map(|(i, _)| i)
            .collect();
        if thinking.is_empty() {
            self.block(
                json!({"type":"redacted_thinking","data":carrier}),
                &mut events,
            );
        } else {
            for index in thinking {
                self.message["content"][index]["signature"] = json!(carrier);
                events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":carrier}}));
            }
        }
        for index in 0..self.message["content"].as_array().unwrap().len() {
            events.push(json!({"type":"content_block_stop","index":index}));
        }
        self.message["stop_reason"] = json!(reason);
        events.push(json!({"type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},"usage":self.message["usage"]}));
        events.push(json!({"type":"message_stop"}));
        Ok(events)
    }
    pub fn log_usage(&self, proxy: &Proxy) {
        if let Some(u) = self
            .usage
            .as_ref()
            .filter(|u| u["promptTokenCount"].is_u64())
        {
            proxy.service.logs.usage(proxy.log_id,&json!({"usage":{"input_tokens":u["promptTokenCount"],"input_tokens_details":{"cached_tokens":u["cachedContentTokenCount"].as_u64().unwrap_or(0)},"output_tokens":self.message["usage"]["output_tokens"],"output_tokens_details":{"reasoning_tokens":u["thoughtsTokenCount"].as_u64().unwrap_or(0)}}}));
        }
    }
}
