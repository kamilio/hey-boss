//! Foreground transcript. Command output never goes into the fleet activity log.
use serde_json::Value;
use std::io::{self, IsTerminal, Write};

pub(super) fn section(label: &str, text: &str) {
    let mut out = io::stdout().lock();
    let label = printable(label);
    if out.is_terminal() && std::env::var_os("NO_COLOR").is_none() {
        let _ = writeln!(out, "\n\x1b[1;36m{label}\x1b[0m");
    } else {
        let _ = writeln!(out, "\n{label}");
    }
    if !text.is_empty() {
        let _ = writeln!(out, "{}", printable(text));
    }
}

fn printable(text: &str) -> String {
    text.chars()
        .filter(|c| matches!(c, '\n' | '\t') || !c.is_control())
        .collect()
}

pub(super) fn event(method: &str, params: &Value) {
    let item = &params["item"];
    let kind = item["type"].as_str().unwrap_or("Tool");
    match (method, kind) {
        ("item/started", "commandExecution") => {
            section(&format!("$ {}", item["command"].as_str().unwrap_or("")), "")
        }
        ("item/completed", "commandExecution") => {
            let exit = item["exitCode"]
                .as_i64()
                .map(|n| format!("exit {n}"))
                .unwrap_or_else(|| item["status"].as_str().unwrap_or("finished").to_owned());
            let command = item["command"].as_str().unwrap_or("");
            section(
                &format!("Output · {exit}  {command}"),
                item["aggregatedOutput"].as_str().unwrap_or(""),
            );
        }
        ("item/completed", "agentMessage") => section("Agent", item["text"].as_str().unwrap_or("")),
        (_, "userMessage" | "reasoning" | "agentMessage") => (),
        ("item/started", _) => section(
            item["tool"].as_str().unwrap_or(kind),
            &item
                .get("arguments")
                .map(Value::to_string)
                .unwrap_or_default(),
        ),
        ("item/completed", _) => section(
            kind,
            &serde_json::to_string_pretty(item).unwrap_or_default(),
        ),
        _ => (),
    }
}
