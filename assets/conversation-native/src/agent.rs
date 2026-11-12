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
    let prompt = context::prompt(tools.home, tools.cwd)?;
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
    let observation = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tools.talk(json!({"command":"spoken_context"})),
    )
    .await
    .ok()
    .and_then(Result::ok);
    tools.focus = observation
        .as_ref()
        .and_then(|v| v["id"].as_str())
        .map(str::to_owned);
    let observed = observation
        .map(|v| v["observed"].clone())
        .unwrap_or(json!("unavailable"));
    book.push(json!({"role":"user","content":format!("Observed pre-focus desktop context: {observed}\n\n{turn}")}))?;
    emit(json!({"AgentAccepted":{"root":tools.cwd}}))?;
    let schemas = schemas();
    loop {
        let msg = provider::complete(
            tools.http,
            url,
            key,
            &prompt,
            &book.messages,
            &schemas,
            true,
        )
        .await?;
        let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
        let text = msg["content"].as_str().unwrap_or("").to_owned();
        book.push(msg)?;
        if !text.is_empty() {
            emit(json!({"Message":{"text":text}}))?;
        }
        for call in &calls {
            let name = field(&call["function"], "name")?;
            let content: Result<Value> = async {
                let args = serde_json::from_str(field(&call["function"], "arguments")?)?;
                tools.call(name, &args).await
            }
            .await;
            book.push(json!({"role":"tool","name":name,"toolCallId":call["id"],"content":content.unwrap_or_else(|e|json!(e.to_string()))}))?;
        }
        if let Some(cut) = compact_at(&book.messages) {
            emit(json!({"AgentStatus":{"text":"Remembering the conversation…"}}))?;
            let msg = provider::complete(
                tools.http,
                url,
                key,
                include_str!("../../conversation-compact.md"),
                &book.messages[..cut],
                &[],
                false,
            )
            .await?;
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
            emit(json!({"TurnCompleted":{"status":"Completed","output":text}}))?;
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
