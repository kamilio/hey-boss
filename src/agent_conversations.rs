//! Read-only, bounded Codex history. Scheduler snapshots are never chat history.
use crate::issues::{Error, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Default, serde::Deserialize, serde::Serialize)]
pub struct Window {
    #[serde(default)]
    pub cursor: u64,
    #[serde(default)]
    pub before: Option<u64>,
    #[serde(default)]
    pub latest: bool,
    #[serde(default)]
    pub at: Option<u64>,
}

const PAGE_BYTES: u64 = 1024 * 1024;
const ENTRY_BYTES: u64 = 8 * 1024 * 1024;
static PATHS: OnceLock<Mutex<HashMap<(PathBuf, String), PathBuf>>> = OnceLock::new();

fn database(path: &Path) -> Result<Connection> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(Duration::from_secs(2))?;
    Ok(db)
}
fn visible(db: &Connection) -> Result<HashSet<String>> {
    Ok(db
        .prepare("SELECT id FROM projects WHERE hidden_at IS NULL")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn overview() -> Result<Value> {
    compact(
        crate::fleet::call(&json!({"kind":"status"}))?,
        &visible(&database(&crate::issues::database_path()?)?)?,
    )
}
fn compact(mut data: Value, projects: &HashSet<String>) -> Result<Value> {
    for machine in data["machines"].as_array_mut().into_iter().flatten() {
        for worker in machine["workers"].as_array_mut().into_iter().flatten() {
            let runs = worker["runs"].as_array().cloned().unwrap_or_default();
            if let Some(object) = worker.as_object_mut() {
                object.retain(|k, _| matches!(k.as_str(), "id" | "pid" | "config"));
            }
            // Retain the compact task list after dropping internal process counters.
            // The config is needed only for the collapsed device controls.
            worker["runs"] = Value::Array(runs_for_project(&runs, projects));
        }
        if let Some(object) = machine.as_object_mut() {
            object.retain(|k, _| {
                matches!(
                    k.as_str(),
                    "host" | "hostname" | "state" | "heartbeat" | "workers"
                )
            });
        }
    }
    data["events"] = json!([]);
    for conflict in data["conflicts"].as_array_mut().into_iter().flatten() {
        if let Some(object) = conflict.as_object_mut() {
            object.remove("saved_change");
        }
    }
    Ok(data)
}
fn runs_for_project(runs: &[Value], projects: &HashSet<String>) -> Vec<Value> {
    runs.iter()
        .filter(|r| projects.contains(r["project_id"].as_str().unwrap_or("")))
        .map(|r| {
            let mut value = json!({});
            for key in [
                "id",
                "project_id",
                "project_name",
                "number",
                "title",
                "session_id",
                "actor_id",
                "state",
                "started_at",
                "finished_at",
            ] {
                value[key] = r[key].clone();
            }
            for key in ["summary", "last_event"] {
                value[key] = json!(
                    r[key]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(1000)
                        .collect::<String>()
                );
            }
            value
        })
        .collect()
}

pub fn conversation(host: &str, run: &str, window: &Window) -> Result<Value> {
    let status = overview()?;
    let machine = status["machines"]
        .as_array()
        .and_then(|ms| {
            ms.iter()
                .find(|m| m["host"] == host || m["hostname"] == host)
        })
        .ok_or_else(|| Error::invalid("This device is no longer available"))?;
    let known = machine["workers"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|w| w["runs"].as_array().into_iter().flatten())
        .any(|r| r["id"] == run);
    let referenced = crate::issues::provenance::referenced(
        &database(&crate::issues::database_path()?)?,
        host,
        run,
    )?
    .is_some();
    if !known && !referenced {
        return Err(Error::invalid("This agent is no longer available"));
    }
    let host = machine["host"].as_str().unwrap_or("");
    if host == "local" {
        let mut result = local_window(run, window)?;
        crate::issues::provenance::enrich_conversation(
            &database(&crate::issues::database_path()?)?,
            run,
            &mut result,
        )?;
        return Ok(result);
    }
    if machine["state"] != "connected" {
        return Err(Error::new(
            "device_disconnected",
            "This device is disconnected. Reconnect to load its conversation.",
        ));
    }
    if !crate::health::remote::valid_host(host) {
        return Err(Error::invalid("Invalid device"));
    }
    let mut command = Command::new("ssh");
    command.args(["-T","-o","BatchMode=yes","-o","ConnectTimeout=5","-o","StrictHostKeyChecking=yes",host,
        "export PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH\"; exec hey-boss fleet conversation"]);
    command
        .env("SFT_NO_BROWSER", "1")
        .env("SSH_ASKPASS_REQUIRE", "never");
    let mut result = transport(
        command,
        &serde_json::to_vec(
            &json!({"run":run,"cursor":window.cursor,"before":window.before,"latest":window.latest,"at":window.at}),
        )?,
        Duration::from_secs(12),
    )?;
    crate::issues::provenance::enrich_conversation(
        &database(&crate::issues::database_path()?)?,
        run,
        &mut result,
    )?;
    Ok(result)
}

pub fn assignment(project: &str, number: i64, agent: &str) -> Result<Value> {
    let run = crate::issues::provenance::assigned_run(
        &database(&crate::issues::database_path()?)?,
        project,
        number,
        agent,
    )?
    .ok_or_else(|| Error::invalid("This assignment no longer has a saved Codex session"))?;
    let status = overview()?;
    let local = run["machine"] == crate::issues::identity::machine()?;
    let machine = status["machines"]
        .as_array()
        .and_then(|ms| {
            ms.iter().find(|m| {
                (local && m["host"] == "local")
                    || (!local && (m["host"] == run["host"] || m["hostname"] == run["host"]))
            })
        })
        .ok_or_else(|| Error::invalid("The assignment's device is no longer available"))?;
    let device = json!({"host":machine["host"],"hostname":machine["hostname"],"state":machine["state"],"heartbeat":machine["heartbeat"]});
    Ok(json!({"ok":true,"machine":device,"run":run,"online":machine["state"] == "connected"}))
}

pub fn local_window(run: &str, window: &Window) -> Result<Value> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
        .ok_or_else(|| Error::invalid("Codex storage is unavailable"))?;
    window_page(
        &database(&crate::issues::database_path()?)?,
        &home,
        run,
        window,
    )
}
fn valid_session(session: &str) -> bool {
    session.len() == 36
        && session.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn find(root: &Path, suffix: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let kind = entry.file_type().ok()?;
        if kind.is_dir() {
            if let Some(path) = find(&entry.path(), suffix) {
                return Some(path);
            }
        } else if kind.is_file() && entry.file_name().to_string_lossy().ends_with(suffix) {
            return Some(entry.path());
        }
    }
    None
}
fn rollout(home: &Path, session: &str) -> Option<PathBuf> {
    let key = (home.to_owned(), session.to_owned());
    if let Some(path) = PATHS
        .get_or_init(Default::default)
        .lock()
        .ok()?
        .get(&key)
        .filter(|p| p.exists())
        .cloned()
    {
        return Some(path);
    }
    let suffix = format!("-{session}.jsonl");
    let path = find(&home.join("sessions"), &suffix)
        .or_else(|| find(&home.join("archived_sessions"), &suffix))?;
    let mut paths = PATHS.get_or_init(Default::default).lock().ok()?;
    if paths.len() >= 256 {
        paths.clear();
    }
    paths.insert(key, path.clone());
    Some(path)
}

/// Best-effort bounded metadata capture on the caller's device, before SSH.
/// Tool text and arguments are deliberately excluded from persisted origins.
pub(crate) fn invocation(session: &str) -> Option<crate::issues::Invocation> {
    if !valid_session(session) {
        return None;
    }
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))?;
    invocation_at(&rollout(&home, session)?)
}
fn invocation_at(path: &Path) -> Option<crate::issues::Invocation> {
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let base = size.saturating_sub(ENTRY_BYTES);
    file.seek(SeekFrom::Start(base)).ok()?;
    let mut reader = BufReader::new(file);
    if base > 0 {
        let mut partial = Vec::new();
        reader.read_until(b'\n', &mut partial).ok()?;
    }
    let mut found = None;
    loop {
        let offset = reader.stream_position().ok()?;
        let mut line = Vec::new();
        if reader.read_until(b'\n', &mut line).ok()? == 0 {
            break;
        }
        if line.last() != Some(&b'\n') {
            break;
        }
        let Ok(record) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if record["type"] == "response_item"
            && matches!(
                record["payload"]["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        {
            found = Some(crate::issues::Invocation {
                offset,
                call_id: record["payload"]["call_id"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 256)
                    .map(str::to_owned),
            });
        }
    }
    found
}
#[cfg(test)]
fn page(db: &Connection, home: &Path, run: &str, cursor: u64) -> Result<Value> {
    window_page(
        db,
        home,
        run,
        &Window {
            cursor,
            ..Window::default()
        },
    )
}
pub(crate) fn window_page(
    db: &Connection,
    home: &Path,
    run: &str,
    window: &Window,
) -> Result<Value> {
    let cursor = window.cursor;
    let metadata = crate::issues::provenance::saved_run(db, run)?;
    let saved: Option<Option<String>> = if run.starts_with("session:") {
        metadata
            .as_ref()
            .map(|r| r["session_id"].as_str().map(str::to_owned))
    } else {
        db.query_row("SELECT r.session_id FROM worker_runs r JOIN projects p ON p.id=r.project_id WHERE r.id=?1 AND p.hidden_at IS NULL",[run],|r|r.get(0)).optional()?
    };
    let session =
        saved.ok_or_else(|| Error::invalid("This conversation is no longer available"))?;
    let mut result =
        json!({"ok":true,"messages":[],"cursor":cursor,"has_more":false,"availability":"waiting"});
    result["run"] = json!(metadata);
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='agent_steering' AND type='table')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        let mut stmt = db.prepare("SELECT request_id,scope,text,state,error FROM agent_steering WHERE run_id=?1 ORDER BY created_at DESC,rowid DESC LIMIT 16")?;
        result["steering"] = json!(stmt.query_map([run], |r| Ok(json!({"request_id":r.get::<_,String>(0)?,"scope":r.get::<_,String>(1)?,"text":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"error":r.get::<_,Option<String>>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    let Some(session) = session else {
        return Ok(result);
    };
    if !valid_session(&session) {
        return Err(Error::invalid("Invalid saved conversation ID"));
    }
    let Some(path) = rollout(home, &session) else {
        return Ok(result);
    };
    if !path.canonicalize()?.starts_with(home.canonicalize()?) {
        return Err(Error::invalid(
            "Saved conversation is outside Codex storage",
        ));
    }
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    if cursor > size {
        return Err(Error::invalid("Conversation changed. Reload its history."));
    }
    let mut reader = BufReader::new(file);
    let historical = window.latest || window.before.is_some() || window.at.is_some();
    let (start, boundary) = if let Some(at) = window.at {
        if at > size {
            return Err(Error::invalid(
                "The saved invocation is outside this conversation",
            ));
        }
        let (start, _) = recent_range(&mut reader, at, None)?;
        (start, size.min(at.saturating_add(PAGE_BYTES / 2)))
    } else if historical {
        recent_range(&mut reader, size, window.before)?
    } else {
        (cursor, size)
    };
    if start > 0 {
        reader.seek(SeekFrom::Start(start - 1))?;
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        if byte[0] != b'\n' {
            return Err(Error::invalid("Invalid conversation cursor"));
        }
    }
    reader.seek(SeekFrom::Start(start))?;
    let mut messages = Vec::new();
    let mut complete = true;
    while reader.stream_position()? < boundary
        && (historical || reader.stream_position()? - start < PAGE_BYTES)
    {
        let offset = reader.stream_position()?;
        let mut line = Vec::new();
        reader
            .by_ref()
            .take(ENTRY_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        if line.is_empty() {
            break;
        }
        if line.len() as u64 > ENTRY_BYTES {
            return Err(Error::invalid("A saved conversation entry exceeds 8 MiB"));
        }
        if line.last() != Some(&b'\n') {
            reader.seek(SeekFrom::Start(offset))?;
            complete = false;
            break;
        }
        let record: Value = serde_json::from_slice(&line)?;
        if let Some(message) = item(&record, offset) {
            messages.push(message);
        }
    }
    let end = reader.stream_position()?;
    result["messages"] = json!(messages);
    result["cursor"] = json!(end);
    result["has_more"] = json!((!historical || window.at.is_some()) && end < size && complete);
    if historical {
        result["older_cursor"] = json!(start);
        result["has_earlier"] = json!(start > 0);
    }
    result["availability"] = json!("available");
    Ok(result)
}
fn recent_range(
    reader: &mut BufReader<std::fs::File>,
    size: u64,
    before: Option<u64>,
) -> Result<(u64, u64)> {
    let boundary = before.unwrap_or(size);
    if boundary > size {
        return Err(Error::invalid("Invalid earlier-history cursor"));
    }
    if boundary == 0 {
        return Ok((0, 0));
    }
    let base = boundary.saturating_sub(PAGE_BYTES + ENTRY_BYTES);
    reader.seek(SeekFrom::Start(base))?;
    let mut bytes = vec![0; (boundary - base) as usize];
    reader.read_exact(&mut bytes)?;
    if before.is_some() && bytes.last() != Some(&b'\n') {
        return Err(Error::invalid("Invalid earlier-history cursor"));
    }
    let end = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let target = end.saturating_sub(PAGE_BYTES as usize);
    let start = bytes[..target]
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    if base > 0 && start == 0 {
        return Err(Error::invalid("A saved conversation entry exceeds 8 MiB"));
    }
    Ok((base + start as u64, base + end as u64))
}

fn item(record: &Value, offset: u64) -> Option<Value> {
    if record["type"] != "response_item" {
        return None;
    }
    let value = &record["payload"];
    let kind = value["type"].as_str()?;
    let (role, label, text) = match kind {
        "message" => {
            let role = value["role"].as_str()?;
            if !matches!(role, "user" | "assistant") {
                return None;
            }
            let text = value["content"]
                .as_array()?
                .iter()
                .filter_map(|p| {
                    p["text"].as_str().map(str::to_owned).or_else(|| {
                        (p["type"] == "input_image").then(|| "[Image attachment]".into())
                    })
                })
                .collect::<Vec<_>>()
                .join("\n");
            (
                role,
                if role == "user" { "You" } else { "Codex" }.to_owned(),
                text,
            )
        }
        "function_call" | "custom_tool_call" => (
            "tool",
            value["name"].as_str().unwrap_or("Tool").to_owned(),
            text(value.get("arguments").or_else(|| value.get("input"))),
        ),
        "function_call_output" | "custom_tool_call_output" => {
            ("tool", "Result".into(), text(value.get("output")))
        }
        "reasoning" => (
            "activity",
            "Thinking".into(),
            value["summary"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        "web_search_call" => ("tool", "Search".into(), text(value.get("action"))),
        _ => {
            let mut public = value.clone();
            if let Some(obj) = public.as_object_mut() {
                obj.remove("encrypted_content");
                obj.remove("content");
            }
            ("tool", kind.into(), public.to_string())
        }
    };
    if text.is_empty() {
        return None;
    }
    let mut message = json!({"id":offset.to_string(),"role":role,"label":label,"text":text,"at":record["timestamp"]});
    if role == "assistant" {
        message["html"] = json!(crate::markdown::render_fragment(&text));
    }
    Some(message)
}
fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    }
}

// Drain pipes concurrently and enforce a wall-clock timeout. Credentials and
// request content use stdin, never command arguments or diagnostic output.
fn transport(mut command: Command, input: &[u8], timeout: Duration) -> Result<Value> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (status, bytes, sent) = std::thread::scope(|scope| -> Result<_> {
        let writer = scope.spawn(move || stdin.write_all(input));
        let reader = scope.spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(crate::issues::WIRE_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(s) = child.try_wait()? {
                break Some(s);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        Ok((status, reader.join().unwrap()?, writer.join().unwrap()))
    })?;
    if !status.is_some_and(|s| s.success()) || sent.is_err() {
        return Err(Error::new(
            "device_unavailable",
            "The device did not respond. Try again when it reconnects.",
        ));
    }
    if bytes.len() > crate::issues::WIRE_LIMIT {
        return Err(Error::invalid("Conversation response exceeds 16 MiB"));
    }
    let value: Value = serde_json::from_slice(&bytes)?;
    if value["ok"] == false {
        return Err(Error::new(
            "conversation_unavailable",
            value["error"]
                .as_str()
                .unwrap_or("Conversation unavailable"),
        ));
    }
    Ok(value)
}

pub fn start_bridge(path: PathBuf) {
    if std::env::var_os("HEY_BOSS_ISSUE_DB").is_some() {
        return;
    }
    std::thread::spawn(move || {
        loop {
            let _ = bridge(&path);
            std::thread::sleep(Duration::from_secs(3));
        }
    });
}
fn bridge(path: &Path) -> Result<()> {
    let config: Value =
        serde_json::from_slice(&std::fs::read(path.with_file_name("mobile.json"))?)?;
    let url = config["url"]
        .as_str()
        .ok_or_else(|| Error::invalid("Mobile service is not configured"))?;
    let uri: tungstenite::http::Uri = url
        .parse()
        .map_err(|_| Error::invalid("Invalid mobile origin"))?;
    if uri.scheme_str() != Some("https")
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
        || uri.query().is_some()
        || url.contains('#')
    {
        return Err(Error::invalid("Invalid mobile origin"));
    }
    let token = config["token"]
        .as_str()
        .filter(|t| !t.contains(['\r', '\n']))
        .ok_or_else(|| Error::invalid("Invalid mobile pairing"))?;
    let projects = visible(&database(path)?)?;
    let mut status = compact(crate::fleet::call(&json!({"kind":"status"}))?, &projects)?;
    for m in status["machines"].as_array_mut().into_iter().flatten() {
        for w in m["workers"].as_array_mut().into_iter().flatten() {
            if let Some(obj) = w.as_object_mut() {
                obj.remove("config");
            }
        }
    }
    status["signals"] = json!([]);
    status["conflicts"] = json!([]);
    mobile_call(url, token, "/api/bridge/agents/status", Some(&status))?;
    let requests = mobile_call(url, token, "/api/bridge/agents", None)?;
    for request in requests["requests"]
        .as_array()
        .into_iter()
        .flatten()
        .take(4)
    {
        let id = request["id"]
            .as_str()
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
            .ok_or_else(|| Error::invalid("Invalid conversation transport ID"))?;
        let result = if !projects.contains(request["project"].as_str().unwrap_or("")) {
            Err(Error::invalid("This project is no longer available"))
        } else if request["action"] == "takeover" || request["action"] == "steer" {
            crate::fleet::call(
                &json!({"kind":request["action"],"host":request["host"],"run":request["run"],"scope":request["scope"],"text":request["text"],"request_id":request["request_id"]}),
            )
        } else if request["action"] == "assignment" {
            assignment(
                request["project"].as_str().unwrap_or(""),
                request["issue"].as_i64().unwrap_or(0),
                request["agent"].as_str().unwrap_or(""),
            )
        } else {
            conversation(
                request["host"].as_str().unwrap_or(""),
                request["run"].as_str().unwrap_or(""),
                &serde_json::from_value::<Window>(request.clone())?,
            )
        };
        let result = result.unwrap_or_else(|e| json!({"ok":false,"error":e.to_string()}));
        mobile_call(
            url,
            token,
            &format!("/api/bridge/agents/{id}/result"),
            Some(&result),
        )?;
    }
    Ok(())
}
fn mobile_call(url: &str, token: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    fn quote(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    }
    let mut config = format!(
        "header = \"Authorization: Bearer {}\"\nheader = \"Content-Type: application/json\"\n",
        quote(token)
    );
    if let Some(body) = body {
        config += &format!("data-binary = \"{}\"\n", quote(&body.to_string()));
    }
    let mut command = Command::new("curl");
    command.args([
        "--silent",
        "--fail",
        "--max-time",
        "10",
        "--max-filesize",
        "16777216",
        "--proto",
        "=https",
        "--config",
        "-",
        "--url",
        &format!("{}{path}", url.trim_end_matches('/')),
    ]);
    transport(command, config.as_bytes(), Duration::from_secs(12))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        root: PathBuf,
        db: Connection,
        path: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "hey-boss-conversation-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let path = root.join(
                "sessions/2026/09/19/rollout-test-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.jsonl",
            );
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let db = Connection::open_in_memory().unwrap();
            db.execute_batch("CREATE TABLE projects(id TEXT,name TEXT,hidden_at INTEGER); INSERT INTO projects VALUES('Atlas','Atlas',NULL); CREATE TABLE worker_runs(id TEXT,project_id TEXT,session_id TEXT,issue_number INTEGER,job TEXT,state TEXT,started_at INTEGER,finished_at INTEGER,actor_id TEXT); INSERT INTO worker_runs VALUES('run','Atlas','aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',1,'{}','completed',1,2,'codex:fixture');").unwrap();
            Self { root, db, path }
        }
        fn page(&self, cursor: u64) -> Result<Value> {
            page(&self.db, &self.root, "run", cursor)
        }
        fn line(role: &str, text: &str) -> String {
            json!({"type":"response_item","payload":{"type":"message","role":role,"content":[{"type":"output_text","text":text}]}}).to_string()+"\n"
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn assigned_standalone_session_reads_saved_messages_without_worker_history() {
        let f = Fixture::new();
        f.db.execute_batch("CREATE TABLE issues(project_id TEXT,number INTEGER,title TEXT,assignee TEXT,deleted_at INTEGER,origin TEXT); CREATE TABLE artifacts(project_id TEXT,title TEXT,origin TEXT); CREATE TABLE agents(id TEXT,metadata TEXT); DELETE FROM worker_runs; INSERT INTO issues VALUES('Atlas',4,'Repair','codex:exact',NULL,NULL);").unwrap();
        let actor = json!({"id":"codex:exact","kind":"codex","session_id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","machine":"remote","host":"mac.local","pid":null,"process_start":null,"cwd":"/work","source":"test"});
        f.db.execute(
            "INSERT INTO agents VALUES('codex:exact',?1)",
            [actor.to_string()],
        )
        .unwrap();
        std::fs::write(
            &f.path,
            Fixture::line("assistant", "Saved repair conversation"),
        )
        .unwrap();
        let page = window_page(
            &f.db,
            &f.root,
            "session:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            &Window {
                latest: true,
                ..Window::default()
            },
        )
        .unwrap();
        assert_eq!(page["messages"][0]["text"], "Saved repair conversation");
        assert_eq!(page["run"]["number"], 4);
        assert_eq!(page["run"]["standalone"], true);
    }
    #[test]
    fn roles_tools_markdown_and_no_duplicate_events() {
        let f = Fixture::new();
        let content=Fixture::line("user","Original request")+&json!({"type":"event_msg","payload":{"type":"agent_message","message":"Reply"}}).to_string()+"\n"+&Fixture::line("assistant","**Reply** <script>alert(1)</script>")+&json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"cargo test"}}).to_string()+"\n"+&json!({"type":"response_item","payload":{"type":"function_call_output","output":"Passed"}}).to_string()+"\n";
        std::fs::write(&f.path, content).unwrap();
        let p = f.page(0).unwrap();
        let messages = p["messages"].as_array().unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|m| m["role"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["user", "assistant", "tool", "tool"]
        );
        assert!(
            messages[1]["html"]
                .as_str()
                .unwrap()
                .contains("<strong>Reply</strong>")
        );
        assert!(!messages[1]["html"].as_str().unwrap().contains("<script>"));
        assert_eq!(
            f.page(p["cursor"].as_u64().unwrap()).unwrap()["messages"],
            json!([])
        );
    }
    #[test]
    fn partial_write_waits_then_arrives_once() {
        let f = Fixture::new();
        let line = Fixture::line("assistant", "Streaming reply");
        std::fs::write(&f.path, &line[..line.len() - 10]).unwrap();
        assert_eq!(f.page(0).unwrap()["cursor"], 0);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&f.path)
            .unwrap()
            .write_all(&line.as_bytes()[line.len() - 10..])
            .unwrap();
        assert_eq!(f.page(0).unwrap()["messages"][0]["text"], "Streaming reply");
    }
    #[test]
    fn latest_window_loads_backwards_without_overlap_and_keeps_live_cursor() {
        let f = Fixture::new();
        let history = (0..150)
            .map(|i| Fixture::line("assistant", &format!("{i}:{}", "x".repeat(20000))))
            .collect::<String>();
        let pending = Fixture::line("assistant", "Live final reply");
        std::fs::write(&f.path, history.clone() + &pending[..pending.len() - 5]).unwrap();
        let mut p = window_page(
            &f.db,
            &f.root,
            "run",
            &Window {
                latest: true,
                ..Window::default()
            },
        )
        .unwrap();
        assert!(
            p["messages"].as_array().unwrap().last().unwrap()["text"]
                .as_str()
                .unwrap()
                .starts_with("149:")
        );
        let live_cursor = p["cursor"].as_u64().unwrap();
        assert_eq!(live_cursor, history.len() as u64);
        let mut ids = std::collections::HashSet::new();
        loop {
            for message in p["messages"].as_array().unwrap() {
                assert!(ids.insert(message["id"].as_str().unwrap().to_owned()));
            }
            if p["has_earlier"] == false {
                break;
            }
            p = window_page(
                &f.db,
                &f.root,
                "run",
                &Window {
                    before: p["older_cursor"].as_u64(),
                    ..Window::default()
                },
            )
            .unwrap();
        }
        assert_eq!(ids.len(), 150);
        assert!(
            window_page(
                &f.db,
                &f.root,
                "run",
                &Window {
                    before: Some(2),
                    ..Window::default()
                }
            )
            .is_err()
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(&f.path)
            .unwrap()
            .write_all(&pending.as_bytes()[pending.len() - 5..])
            .unwrap();
        assert_eq!(
            f.page(live_cursor).unwrap()["messages"][0]["text"],
            "Live final reply"
        );
    }
    #[test]
    fn pages_preserve_all_history_and_reject_invalid_offsets() {
        let f = Fixture::new();
        std::fs::write(
            &f.path,
            (0..150)
                .map(|i| Fixture::line("assistant", &format!("{i}{}", "x".repeat(20000))))
                .collect::<String>(),
        )
        .unwrap();
        let (mut cursor, mut count, mut pages) = (0, 0, 0);
        loop {
            let p = f.page(cursor).unwrap();
            count += p["messages"].as_array().unwrap().len();
            pages += 1;
            cursor = p["cursor"].as_u64().unwrap();
            if p["has_more"] == false {
                break;
            }
        }
        assert_eq!(count, 150);
        assert!(pages > 1);
        assert!(f.page(2).is_err());
        assert!(f.page(cursor + 1).is_err());
    }
    #[test]
    fn missing_hidden_unknown_and_arbitrary_sessions_are_explicit() {
        let f = Fixture::new();
        assert_eq!(f.page(0).unwrap()["availability"], "waiting");
        assert!(page(&f.db, &f.root, "../private", 0).is_err());
        f.db.execute("UPDATE worker_runs SET session_id='../private'", [])
            .unwrap();
        assert!(f.page(0).is_err());
        f.db.execute("UPDATE projects SET hidden_at=1", []).unwrap();
        assert!(f.page(0).is_err());
    }
    #[test]
    fn private_reasoning_and_instructions_are_not_exposed() {
        assert!(item(&json!({"type":"response_item","payload":{"type":"message","role":"developer","content":[{"text":"Private instruction"}]}}),0).is_none());
        let p=item(&json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"Public summary"}],"encrypted_content":"private ciphertext"}}),0).unwrap();
        assert_eq!(p["text"], "Public summary");
        assert!(!p.to_string().contains("ciphertext"));
    }
    #[test]
    fn invocation_metadata_and_anchored_history_use_exact_record_offsets() {
        let f = Fixture::new();
        let before = (0..100)
            .map(|_| Fixture::line("assistant", &"old context ".repeat(3000)))
            .collect::<String>();
        let call=json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"create-call","arguments":"hey-boss issue create"}}).to_string()+"\n";
        let after = (0..100)
            .map(|_| Fixture::line("assistant", &"new context ".repeat(3000)))
            .collect::<String>();
        std::fs::write(&f.path, before.clone() + &call + &after + "{\"type\":").unwrap();
        let invocation = invocation_at(&f.path).unwrap();
        assert_eq!(invocation.offset, before.len() as u64);
        assert_eq!(invocation.call_id.as_deref(), Some("create-call"));
        let page = window_page(
            &f.db,
            &f.root,
            "run",
            &Window {
                at: Some(invocation.offset),
                ..Default::default()
            },
        )
        .unwrap();
        let invocation_id = invocation.offset.to_string();
        assert!(
            page["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["id"] == invocation_id)
        );
        assert!(page["has_earlier"].as_bool().unwrap());
        assert!(page["has_more"].as_bool().unwrap());
        assert!(
            window_page(
                &f.db,
                &f.root,
                "run",
                &Window {
                    at: Some(u64::MAX),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn compact_overview_filters_hidden_projects_without_mutating_input() {
        let original = json!({"machines":[{"host":"local","workers":[{"id":"service","pid":1,"runs":[{"id":"run","project_id":"Atlas","events":["large private event"],"summary":"Done"},{"id":"hidden","project_id":"Hidden"}]}]}],"conflicts":[{"reason":"Conflict","saved_change":"private"}],"events":["private"]});
        let result = compact(original.clone(), &HashSet::from(["Atlas".into()])).unwrap();
        assert_eq!(
            result["machines"][0]["workers"][0]["runs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            result["machines"][0]["workers"][0]["runs"][0]
                .get("events")
                .is_none()
        );
        assert_eq!(
            original["machines"][0]["workers"][0]["runs"][0]["events"][0],
            "large private event"
        );
        assert!(result["conflicts"][0].get("saved_change").is_none());
    }
    #[test]
    fn compact_overview_retains_assignment_identity() {
        let runs = runs_for_project(
            &[
                json!({"id":"run","project_id":"Atlas","actor_id":"worker:run","session_id":"session","expanded_prompt":"private"}),
            ],
            &HashSet::from(["Atlas".into()]),
        );
        assert_eq!(runs[0]["actor_id"], "worker:run");
        assert_eq!(runs[0]["session_id"], "session");
        assert!(runs[0].get("expanded_prompt").is_none());
    }
}
