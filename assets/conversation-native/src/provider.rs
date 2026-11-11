use crate::{emit, field};
use anyhow::{Context, Result, bail, ensure};
use futures_util::TryStreamExt;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::io::StreamReader;

pub fn input(messages: &[Value]) -> Result<Vec<Value>> {
    let mut input = Vec::new();
    for m in messages {
        match field(m, "role")? {
            "tool" => {
                let output = if let Some(parts) = m["content"].as_array() {
                    let mut result = vec![];
                    for p in parts {
                        let part = match field(p, "type")? {
                            "text" => json!({"type": "input_text", "text": p["text"]}),
                            "image" => {
                                let url = format!(
                                    "data:{};base64,{}",
                                    field(p, "mimeType")?,
                                    field(p, "data")?
                                );
                                json!({"type": "input_image", "image_url": url})
                            }
                            kind => bail!("Unknown tool output: {kind}"),
                        };
                        result.push(part);
                    }
                    json!(result)
                } else {
                    m["content"].clone()
                };
                input.push(json!({"type":"function_call_output","call_id":field(m,"toolCallId")?,"output":output}));
            }
            role @ ("system" | "user" | "assistant") => {
                if !m["content"].is_null() && m["content"] != "" {
                    input.push(json!({"role":role,"content":m["content"]}));
                }
                for c in m["tool_calls"].as_array().into_iter().flatten() {
                    input.push(json!({"type":"function_call","name":field(&c["function"],"name")?,"call_id":field(c,"id")?,"arguments":c["function"]["arguments"]}));
                }
            }
            other => bail!("Unknown history role {other}"),
        }
    }
    Ok(input)
}
pub async fn complete(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    prompt: &str,
    messages: &[Value],
    tools: &[Value],
    speak: bool,
) -> Result<Value> {
    let body = json!({"model":"gpt-6.1-sol","instructions":prompt,"input":input(messages)?,"tools":tools,"stream":true,"store":false,"reasoning":{"effort":"low"},"text":{"verbosity":"low"}});
    let res = client.post(url).bearer_auth(key).json(&body).send().await?;
    if !res.status().is_success() {
        bail!(
            "Conversation provider: HTTP {}: {}",
            res.status(),
            res.text().await?
        );
    }
    let stream = StreamReader::new(res.bytes_stream().map_err(std::io::Error::other));
    let mut lines = BufReader::new(stream).lines();
    let mut data = String::new();
    let mut items = vec![];
    let mut delta = String::new();
    loop {
        let line = lines.next_line().await?;
        if let Some(s) = line.as_deref().filter(|s| !s.is_empty()) {
            if let Some(s) = s.strip_prefix("data:") {
                data.push_str(s.strip_prefix(' ').unwrap_or(s));
                data.push('\n');
            }
            continue;
        }
        if !data.trim().is_empty() && data.trim() != "[DONE]" {
            let event: Value = serde_json::from_str(&data)?;
            match event["type"].as_str() {
                Some("response.output_text.delta") => {
                    let s = event["delta"].as_str().context("Invalid text delta")?;
                    delta += s;
                    if speak {
                        emit(json!({"TextDelta":{"text":s}}))?;
                    }
                }
                Some("response.output_item.done") => items.push(event["item"].clone()),
                Some("response.completed") => {
                    if let Some(output) = event["response"]["output"]
                        .as_array()
                        .filter(|x| !x.is_empty())
                    {
                        items = output.clone();
                    }
                    let mut calls = vec![];
                    let mut texts = vec![];
                    for item in items {
                        match item["type"].as_str(){
                        Some("function_call") => calls.push(json!({
                            "id": field(&item,"call_id")?, "type":"function",
                            "function": {"name":field(&item,"name")?, "arguments":item["arguments"]}
                        })),
                        Some("message") => {
                            for part in item["content"].as_array().into_iter().flatten() {
                                if let Some(text) = part["text"].as_str().or_else(|| part["refusal"].as_str()) {
                                    texts.push(text.to_owned());
                                }
                            }
                        }
                        _=>{}
                    }
                    }
                    let mut msg = json!({"role":"assistant","content":if texts.is_empty(){delta}else{texts.join("\n")}});
                    if !calls.is_empty() {
                        msg["tool_calls"] = json!(calls);
                    }
                    return Ok(msg);
                }
                Some("error" | "response.failed" | "response.incomplete") => bail!(
                    "Conversation provider: {}",
                    event
                        .pointer("/response/error/message")
                        .or_else(|| event.get("message"))
                        .unwrap_or(&event)
                ),
                _ => {}
            }
        }
        data.clear();
        ensure!(
            line.is_some(),
            "Conversation stream ended before completion"
        );
    }
}
