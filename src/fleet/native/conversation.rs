//! Read bounded public conversation pages; never expose encrypted reasoning.
use super::{
    Result,
    context::Context,
    replica::{self, invalid},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
static PATHS: OnceLock<Mutex<BTreeMap<(PathBuf, String), PathBuf>>> = OnceLock::new();
fn item(record: &Value, offset: u64) -> Option<Value> {
    if record["type"] != "response_item" {
        return None;
    }
    let item = &record["payload"];
    let kind = item["type"].as_str().unwrap_or("Activity");
    let mut role = "tool";
    let label;
    let text;
    match kind {
        "message" => {
            role = item["role"].as_str().unwrap_or("assistant");
            if !matches!(role, "user" | "assistant") {
                return None;
            }
            label = if role == "user" {
                "You".into()
            } else {
                "Codex".into()
            };
            text = Value::String(
                item["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| {
                        p["text"].as_str().map(str::to_owned).or_else(|| {
                            matches!(p["type"].as_str(), Some("input_image" | "image"))
                                .then(|| "[Image attachment]".into())
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        "function_call" | "custom_tool_call" => {
            label = item["name"].as_str().unwrap_or("Tool").into();
            text = item
                .get("arguments")
                .or_else(|| item.get("input"))
                .cloned()
                .unwrap_or(json!(""));
        }
        "function_call_output" | "custom_tool_call_output" => {
            label = "Result".into();
            text = item.get("output").cloned().unwrap_or(json!(""));
        }
        "reasoning" => {
            role = "activity";
            label = "Thinking".into();
            text = json!(
                item["summary"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        "web_search_call" => {
            label = "Search".into();
            text = json!(item.get("action").cloned().unwrap_or(json!({})).to_string());
        }
        _ => {
            label = kind.to_owned();
            let mut public = item.clone();
            if let Some(m) = public.as_object_mut() {
                m.remove("encrypted_content");
                m.remove("content");
            }
            text = json!(public.to_string());
        }
    }
    let text = text
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| text.to_string());
    if text.is_empty() {
        return None;
    }
    Some(
        json!({"id":offset.to_string(),"role":role,"label":label,"text":text,"at":record["timestamp"]}),
    )
}
fn valid_session(session: &str) -> bool {
    session.len() == 36
        && session.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}
fn find(folder: &Path, suffix: &str) -> Result<Option<PathBuf>> {
    match fs::read_dir(folder) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    if let Some(path) = find(&entry.path(), suffix)? {
                        return Ok(Some(path));
                    }
                } else if kind.is_file() && entry.file_name().to_string_lossy().ends_with(suffix) {
                    return Ok(Some(entry.path()));
                }
            }
            Ok(None)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub(super) fn page(ctx: &Context, run_id: &str, cursor: &Value) -> Result<Value> {
    let cursor = cursor
        .as_u64()
        .ok_or_else(|| invalid("Invalid conversation cursor"))?;
    let run = replica::rows(
        &ctx.db()?,
        "SELECT session_id FROM worker_runs WHERE id=?",
        &[json!(run_id)],
    )?;
    let run = run
        .first()
        .ok_or_else(|| invalid("This agent is no longer available"))?;
    let result =
        json!({"ok":true,"messages":[],"cursor":cursor,"has_more":false,"availability":"waiting"});
    let Some(session) = run["session_id"].as_str() else {
        return Ok(result);
    };
    if !valid_session(session) {
        return Err(invalid("Invalid saved conversation ID"));
    }
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| ctx.home.join(".codex"));
    if !home.exists() {
        return Ok(result);
    }
    let home = home.canonicalize()?;
    let key = (home.clone(), session.to_owned());
    let cache = PATHS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let cached = cache
        .lock()
        .unwrap()
        .get(&key)
        .filter(|p| p.exists())
        .cloned();
    let path = match cached {
        Some(path) => Some(path),
        None => {
            let suffix = format!("-{session}.jsonl");
            find(&home.join("sessions"), &suffix)?
                .or(find(&home.join("archived_sessions"), &suffix)?)
        }
    };
    let Some(path) = path else {
        return Ok(result);
    };
    if !path.canonicalize()?.starts_with(&home) {
        return Err(invalid("Saved conversation is outside Codex storage"));
    }
    {
        let mut cache = cache.lock().unwrap();
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert(key, path.clone());
    }
    read_page(&path, cursor)
}
fn read_page(path: &Path, cursor: u64) -> Result<Value> {
    let mut reader = BufReader::new(File::open(path)?);
    let size = reader.get_ref().metadata()?.len();
    if cursor > size {
        return Err(invalid("Conversation changed; reload its history"));
    }
    if cursor > 0 {
        reader.seek(SeekFrom::Start(cursor - 1))?;
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        if byte[0] != b'\n' {
            return Err(invalid("Invalid conversation cursor"));
        }
    }
    reader.seek(SeekFrom::Start(cursor))?;
    let mut messages = vec![];
    let mut complete = false;
    while reader.stream_position()?.saturating_sub(cursor) < 1024 * 1024 {
        let offset = reader.stream_position()?;
        let mut line = vec![];
        if reader
            .by_ref()
            .take(8 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)?
            == 0
        {
            break;
        }
        if line.len() > 8 * 1024 * 1024 {
            return Err(invalid("A saved conversation entry exceeds 8 MiB"));
        }
        complete = line.ends_with(b"\n");
        if !complete {
            reader.seek(SeekFrom::Start(offset))?;
            break;
        }
        let record: Value = serde_json::from_slice(&line)
            .map_err(|_| invalid("A saved conversation entry could not be read"))?;
        if let Some(message) = item(&record, offset) {
            messages.push(message);
        }
    }
    let position = reader.stream_position()?;
    Ok(
        json!({"ok":true,"messages":messages,"cursor":position,"has_more":position<size && complete,"availability":"available"}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_reasoning_and_system_instructions_are_never_conversation_messages() {
        assert!(item(&json!({"type":"response_item","payload":{"type":"message","role":"system","content":[{"text":"Private instructions"}]}}),0).is_none());
        assert!(item(&json!({"type":"response_item","payload":{"type":"reasoning","encrypted_content":"Private"}}),0).is_none());
        let public=item(&json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"Public summary"}],"encrypted_content":"Private"}}),10).unwrap();
        assert_eq!(public["text"], "Public summary");
        assert!(!public.to_string().contains("Private"));
    }
    #[test]
    fn incomplete_lines_do_not_advance_cursor_and_invalid_offsets_are_rejected() {
        let root = std::env::temp_dir().join(format!(
            "hb-conversation-{}",
            super::super::context::id().unwrap()
        ));
        let record=json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"Hello"}]}}).to_string()+"\n";
        fs::write(&root, record.clone() + "{\"type\":").unwrap();
        let page = read_page(&root, 0).unwrap();
        assert_eq!(page["messages"].as_array().unwrap().len(), 1);
        assert_eq!(page["cursor"], record.len());
        assert_eq!(page["has_more"], false);
        assert!(read_page(&root, 1).is_err());
        assert!(read_page(&root, u64::MAX).is_err());
        let _ = fs::remove_file(root);
    }
}
