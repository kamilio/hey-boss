use crate::{
    context, emit, field,
    plugins::{Plugins, schemas},
    provider,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

pub async fn run(state: &Path, url: &str, request: &Value, tools: &mut Plugins<'_>) -> Result<()> {
    let turn = field(request, "turn")?;
    std::fs::create_dir_all(tools.cwd)?;
    let mut prompt = context::prompt(tools.home, tools.cwd)?;
    let attached = request["t3_thread"].as_str();
    let automatic = attached.is_some() && request["automatic"] == true;
    if attached.is_some() {
        prompt.push_str("\n\n");
        prompt.push_str(include_str!("../../conversation-t3.md"));
    }
    let keys: Value = serde_json::from_str(
        &context::optional(&tools.home.join(".hey-boss/proxy/config.access-keys.json"))?
            .unwrap_or_else(|| "{}".into()),
    )?;
    let key = if keys.get("local").is_some() {
        field(&keys, "local")?
    } else {
        "local"
    };
    let mut book = crate::notebook::Notebook::open(state, &request["history"])?;
    let job = request["job"] == true;
    let observation = if job || attached.is_some() {
        None
    } else {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tools.talk(json!({"command":"spoken_context"})),
        )
        .await
        .ok()
        .and_then(Result::ok)
    };
    tools.focus = observation
        .as_ref()
        .and_then(|v| v["id"].as_str())
        .map(str::to_owned);
    let mut schemas = schemas();
    if attached.is_some() {
        schemas.extend(crate::t3::schemas());
    }
    if automatic {
        schemas.clear();
    }
    if job {
        schemas.retain(|t| matches!(t["name"].as_str(), Some("shell" | "batch" | "delegate")));
    }
    if let Some(paste) = observation.as_ref().and_then(|v| v.get("paste_tool")) {
        schemas.push(paste.clone());
    }
    let observed = observation
        .and_then(|v| v.get("observed").cloned())
        .unwrap_or(json!("unavailable"));
    book.push(json!({"role":"user","content":if let Some(link) = attached {
        json!({"attached_thread":link,"automatic_update":automatic,"initial_catch_up":request["initial_catch_up"] == true,"text":turn}).to_string()
    } else if job { turn.to_owned() } else { format!("Observed pre-focus desktop context: {observed}\n\n{turn}") }}))?;
    emit(json!({"AgentAccepted":{"root":tools.cwd}}))?;
    let mut usage = Value::Null;
    let mut add_usage = |reported: &Value| {
        if reported.is_object() {
            for (key, path) in [
                ("input_tokens", "/input_tokens"),
                ("cached_input_tokens", "/input_tokens_details/cached_tokens"),
                ("output_tokens", "/output_tokens"),
                (
                    "reasoning_output_tokens",
                    "/output_tokens_details/reasoning_tokens",
                ),
            ] {
                usage[key] = json!(
                    usage[key].as_u64().unwrap_or(0)
                        + reported.pointer(path).and_then(Value::as_u64).unwrap_or(0)
                );
            }
        }
    };
    loop {
        let (msg, reported) = provider::complete(
            tools.http,
            url,
            key,
            &prompt,
            &book.messages,
            &schemas,
            attached.is_none(),
        )
        .await?;
        add_usage(&reported);
        let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
        let text = msg["content"].as_str().unwrap_or("").to_owned();
        book.push(msg)?;
        if !text.is_empty() && (attached.is_none() || calls.is_empty()) {
            emit(json!({"Message":{"text":text}}))?;
        }
        for call in &calls {
            let name = field(&call["function"], "name")?;
            let id = field(call, "id")?;
            let mut activity = None;
            let content: Result<Value> = async {
                ensure!(
                    !job || schemas.iter().any(|tool| tool["name"] == name),
                    "Tool unavailable in a background job"
                );
                ensure!(!automatic, "Tools unavailable during automatic updates");
                let args: Value = serde_json::from_str(field(&call["function"], "arguments")?)?;
                if matches!(name, "t3_read" | "t3_send" | "t3_answer") {
                    return crate::t3::call(
                        tools,
                        attached.context("No attached T3 thread")?,
                        name,
                        &args,
                    )
                    .await;
                }
                if name == "delegate" {
                    let provider = match args.get("provider") {
                        None => "t3",
                        Some(_) => field(&args, "provider")?,
                    };
                    anyhow::ensure!(
                        matches!(provider, "t3" | "codex" | "claude"),
                        "Unknown delegate provider"
                    );
                    let request = field(&args, "request")?;
                    let description = field(&args, "description")?;
                    let mut item =
                        json!({"provider":provider,"request":request,"description":description});
                    let result = crate::delegate::run(tools, id, &mut item).await;
                    activity = Some(item);
                    return result.map(Value::String);
                }
                tools.call(name, &args).await
            }
            .await;
            let failed = content.is_err();
            let content = content.unwrap_or_else(|e| json!(e.to_string()));
            let mut record = json!({"role":"tool","name":name,"toolCallId":id,"content":content});
            if activity.is_some() {
                record["failed"] = json!(failed);
            }
            book.push(record)?;
            if let Some(mut item) = activity {
                item["aggregatedOutput"] = content;
                emit(json!({"ToolCompleted":{"id":id,"output":item,"failed":failed}}))?;
            }
        }
        if let Some(cut) = compact_at(&book.messages) {
            emit(json!({"AgentStatus":{"text":"Remembering the conversation…"}}))?;
            let (msg, reported) = provider::complete(
                tools.http,
                url,
                key,
                include_str!("../../conversation-compact.md"),
                &book.messages[..cut],
                &[],
                false,
            )
            .await?;
            add_usage(&reported);
            let summary = msg["content"].as_str().context("Missing summary")?.trim();
            ensure!(
                !summary.is_empty(),
                "Could not compact the conversation: empty summary. Your history is saved."
            );
            let formatted = poe_agent_rust::context_plugins::format_compaction_summary(
                &summary.encode_utf16().collect::<Vec<_>>(),
            );
            book.compact(cut,json!({"role":"system","name":"compaction","content":String::from_utf16(&formatted)?}))?;
        }
        if calls.is_empty() {
            emit(json!({"TurnCompleted":{"status":"Completed","output":text,"usage":usage}}))?;
            return Ok(());
        }
    }
}
fn compact_at(messages: &[Value]) -> Option<usize> {
    let size: usize = messages
        .iter()
        .map(|m| match m["content"].as_str() {
            Some(s) => s.encode_utf16().count(),
            None => m["content"].to_string().len(),
        })
        .sum();
    if size <= 160000 {
        return None;
    }
    messages
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, m)| m["role"] == "user")
        .nth(2)
        .map(|(i, _)| i)
        .filter(|i| *i > 0 && messages[..*i].iter().any(|m| m["role"] == "user"))
}
