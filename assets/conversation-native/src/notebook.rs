//! One durable JSON line per agent message. Compaction changes model context,
//! not the notebook: replay the summary in place of the summarized prefix.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, ErrorKind, Write},
    os::unix::fs::DirBuilderExt,
    path::Path,
};

pub struct Notebook {
    file: File,
    pub messages: Vec<Value>,
}

impl Notebook {
    pub fn open(state: &Path, fallback: &Value) -> Result<Self> {
        let path = state.with_extension("jsonl");
        let dir = path.parent().context("Missing notebook directory")?;
        let open = || OpenOptions::new().read(true).append(true).open(&path);
        let file = match open() {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(dir)?;
                let seed = if let Some(raw) = crate::context::optional(state)? {
                    serde_json::from_str::<Value>(&raw)?
                        .get_mut("messages")
                        .context("Invalid conversation history")?
                        .take()
                } else if fallback.is_null() {
                    json!([])
                } else {
                    fallback.clone()
                };
                let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
                for msg in seed.as_array().context("Invalid conversation history")? {
                    write_line(tmp.as_file_mut(), msg)?;
                }
                tmp.as_file().sync_all()?;
                match tmp.persist_noclobber(&path) {
                    Ok(_) => File::open(dir)?.sync_all()?,
                    Err(e) if e.error.kind() == ErrorKind::AlreadyExists => (),
                    Err(e) => return Err(e.into()),
                }
                open()?
            }
            Err(e) => return Err(e.into()),
        };
        file.try_lock().context("Conversation is already in use")?;
        let mut messages = Vec::new();
        let mut reader = BufReader::new(&file);
        let mut line = Vec::new();
        let mut end = 0;
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            if line.last() != Some(&b'\n') {
                file.set_len(end)?;
                file.sync_data()?;
                break;
            }
            let mut entry: Value =
                serde_json::from_slice(&line).context("Invalid notebook entry")?;
            if let Some(cut) = entry.get("compact").and_then(Value::as_u64) {
                ensure!(
                    cut > 0 && cut <= messages.len() as u64,
                    "Invalid notebook compaction"
                );
                let summary = entry.get_mut("summary").context("Missing summary")?.take();
                messages.splice(..cut as usize, [summary]);
            } else {
                ensure!(entry["role"].is_string(), "Invalid notebook message");
                messages.push(entry);
            }
            end += line.len() as u64;
        }
        let mut book = Self { file, messages };
        // At most the last batch can still be executing at a crash. Never replay it.
        let last = book.messages.iter().rposition(|m| m["role"] != "tool");
        if let Some(i) = last
            && let Some(calls) = book.messages[i]["tool_calls"].as_array()
        {
            let missing: Vec<_> = calls.iter().filter(|c| !book.messages[i+1..].iter().any(|r| r["toolCallId"] == c["id"]))
                .map(|c| json!({"role":"tool","toolCallId":c["id"],"name":c["function"]["name"],"content":"Interrupted before the result was saved. The command may have run."})).collect();
            for result in missing {
                book.push(result)?;
            }
        }
        Ok(book)
    }
    pub fn push(&mut self, msg: Value) -> Result<()> {
        write_line(&mut self.file, &msg)?;
        self.file.sync_data()?;
        self.messages.push(msg);
        Ok(())
    }
    pub fn compact(&mut self, cut: usize, summary: Value) -> Result<()> {
        ensure!(
            cut > 0 && cut <= self.messages.len(),
            "Invalid notebook compaction"
        );
        write_line(&mut self.file, &json!({"compact":cut,"summary":summary}))?;
        self.file.sync_data()?;
        self.messages.splice(..cut, [summary]);
        Ok(())
    }
}

fn write_line(file: &mut File, value: &Value) -> Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    file.write_all(&line)?;
    Ok(())
}
