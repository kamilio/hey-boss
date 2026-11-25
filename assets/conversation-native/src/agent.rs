use crate::{
    context, emit, field,
    plugins::{Plugins, schemas},
    provider,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

pub async fn run(state: &Path, url: &str, request: &Value, tools: &mut Plugins<'_>) -> Result<()> {
    let (_tx, mut input) = tokio::sync::mpsc::unbounded_channel();
    run_with_input(state, url, request, tools, &mut input).await
}

pub fn run_job<'a>(
    state: &'a Path,
    url: &'a str,
    mut request: Value,
    tools: &'a mut Plugins<'_>,
    completions: &'a mut tokio::sync::mpsc::UnboundedReceiver<Value>,
) -> futures_util::future::BoxFuture<'a, Result<()>> {
    Box::pin(async move {
        let mut queued = std::collections::VecDeque::new();
        loop {
            request["job"] = json!(true);
            let (follow, mut pending) = tokio::sync::mpsc::unbounded_channel();
            {
                let run = run_with_input(state, url, &request, tools, &mut pending);
                tokio::pin!(run);
                loop {
                    tokio::select! {
                        result = &mut run => { result?; break; }
                        item = completions.recv() => {
                            follow.send(crate::delegate::follow_up(item.context("Delegate delivery closed")?))?;
                        }
                    }
                }
            }
            queued.extend(std::iter::from_fn(|| pending.try_recv().ok()));
            if let Some(next) = queued.pop_front() {
                request = next;
            } else if tools.delegates.is_some_and(|tasks| tasks.pending()) {
                request = crate::delegate::follow_up(
                    completions
                        .recv()
                        .await
                        .context("Delegate delivery closed")?,
                );
            } else {
                return Ok(());
            }
        }
    })
}

pub async fn run_with_input(
    state: &Path,
    url: &str,
    request: &Value,
    tools: &mut Plugins<'_>,
    input: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
) -> Result<()> {
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
    if request.get("delegate_result").is_some() {
        if !accept_follow_up(&mut book, request, tools)? {
            return Ok(());
        }
    } else {
        book.push(json!({"role":"user","content":if let Some(link) = attached {
        json!({"attached_thread":link,"automatic_update":automatic,"initial_catch_up":request["initial_catch_up"] == true,"text":turn}).to_string()
    } else if job { turn.to_owned() } else { format!("Observed pre-focus desktop context: {observed}\n\n{turn}") }}))?;
    }
    emit(json!({"AgentAccepted":{"root":tools.cwd,"message":request["message"]}}))?;
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
        let completion = provider::complete(
            tools.http,
            url,
            key,
            &prompt,
            &book.messages,
            &schemas,
            attached.is_none() && !job,
        );
        // A newer input invalidates inference that has not committed any tool calls.
        let (msg, reported) = tokio::select! {
            biased;
            Some(request) = input.recv() => {
                accept_input(&mut book, &request, tools)?;
                while let Ok(request) = input.try_recv() { accept_input(&mut book, &request, tools)?; }
                continue;
            }
            result = completion => result?,
        };
        add_usage(&reported);
        let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
        let text = msg["content"].as_str().unwrap_or("").to_owned();
        book.push(msg)?;
        if !text.is_empty() && (attached.is_none() || calls.is_empty()) {
            emit(json!({"Message":{"text":text}}))?;
        }
        let mut follow_up = input.try_recv().ok();
        for call in &calls {
            let name = field(&call["function"], "name")?;
            let id = field(call, "id")?;
            if follow_up.is_some() {
                book.push(json!({"role":"tool","name":name,"toolCallId":id,"content":"Skipped because the user submitted a follow-up."}))?;
                continue;
            }
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
                    return tools
                        .delegates
                        .context("Delegation requires an owned session")?
                        .start(tools, url, &args, &book.messages)
                        .map(|receipt| json!(receipt.to_string()));
                }
                tools.call(name, &args).await
            }
            .await;
            let content = content.unwrap_or_else(|e| json!(e.to_string()));
            let record = json!({"role":"tool","name":name,"toolCallId":id,"content":content});
            book.push(record)?;
            follow_up = input.try_recv().ok();
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
        follow_up = follow_up.or_else(|| input.try_recv().ok());
        if follow_up.is_some() {
            let mut accepted = false;
            while let Some(request) = follow_up {
                accepted |= accept_input(&mut book, &request, tools)?;
                follow_up = input.try_recv().ok();
            }
            if accepted {
                continue;
            }
        }
        if calls.is_empty() {
            if !job || !tools.delegates.is_some_and(|tasks| tasks.pending()) {
                emit(json!({"TurnCompleted":{"status":"Completed","output":text,"usage":usage}}))?;
            }
            return Ok(());
        }
    }
}
fn accept_input(
    book: &mut crate::notebook::Notebook,
    request: &Value,
    tools: &Plugins<'_>,
) -> Result<bool> {
    if request.get("delegate_result").is_some() {
        return accept_follow_up(book, request, tools);
    }
    let text = field(request, "turn")?;
    book.push(json!({"role":"user","content":text}))?;
    emit(json!({"InputAccepted":{"text":text,"message":request["message"]}}))?;
    Ok(true)
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

fn accept_follow_up(
    book: &mut crate::notebook::Notebook,
    request: &Value,
    tools: &Plugins<'_>,
) -> Result<bool> {
    let item = &request["delegate_result"];
    let id = field(item, "id")?;
    let fresh = !book.delegations.contains(id);
    if fresh {
        book.push(json!({"role":"user","name":"delegate_result","delegation":id,"content":field(request,"turn")?}))?;
        emit(json!({"ToolCompleted":{"id":id,"output":item,"failed":item["failed"]}}))?;
        emit(json!({"InputAccepted":{"text":field(request,"turn")?,"delegate_result":item}}))?;
    }
    if let Some(tasks) = tools.delegates {
        tasks.accept(id)?;
    }
    Ok(fresh)
}
