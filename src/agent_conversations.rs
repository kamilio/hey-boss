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

pub fn conversation(host: &str, run: &str, cursor: u64) -> Result<Value> {
    let status = overview()?;
    let machine = status["machines"]
        .as_array()
        .and_then(|ms| ms.iter().find(|m| m["host"] == host))
        .ok_or_else(|| Error::invalid("This device is no longer available"))?;
    let known = machine["workers"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|w| w["runs"].as_array().into_iter().flatten())
        .any(|r| r["id"] == run);
    if !known {
        return Err(Error::invalid("This agent is no longer available"));
    }
    if host == "local" {
        return local(run, cursor);
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
    transport(
        command,
        &serde_json::to_vec(&json!({"run":run,"cursor":cursor}))?,
        Duration::from_secs(12),
    )
}

pub fn local(run: &str, cursor: u64) -> Result<Value> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
        .ok_or_else(|| Error::invalid("Codex storage is unavailable"))?;
    page(
        &database(&crate::issues::database_path()?)?,
        &home,
        run,
        cursor,
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
fn page(db: &Connection, home: &Path, run: &str, cursor: u64) -> Result<Value> {
    let saved:Option<Option<String>>=db.query_row("SELECT r.session_id FROM worker_runs r JOIN projects p ON p.id=r.project_id WHERE r.id=?1 AND p.hidden_at IS NULL",[run],|r|r.get(0)).optional()?;
    let session =
        saved.ok_or_else(|| Error::invalid("This conversation is no longer available"))?;
    let mut result =
        json!({"ok":true,"messages":[],"cursor":cursor,"has_more":false,"availability":"waiting"});
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
    if cursor > 0 {
        reader.seek(SeekFrom::Start(cursor - 1))?;
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        if byte[0] != b'\n' {
            return Err(Error::invalid("Invalid conversation cursor"));
        }
    }
    reader.seek(SeekFrom::Start(cursor))?;
    let mut messages = Vec::new();
    let mut complete = true;
    while reader.stream_position()? - cursor < PAGE_BYTES {
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
    result["has_more"] = json!(end < size && complete);
    result["availability"] = json!("available");
    Ok(result)
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
            w.as_object_mut().map(|o| o.remove("config"));
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
        } else {
            conversation(
                request["host"].as_str().unwrap_or(""),
                request["run"].as_str().unwrap_or(""),
                request["cursor"].as_u64().unwrap_or(0),
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
            db.execute_batch("CREATE TABLE projects(id TEXT,hidden_at INTEGER); INSERT INTO projects VALUES('Atlas',NULL); CREATE TABLE worker_runs(id TEXT,project_id TEXT,session_id TEXT); INSERT INTO worker_runs VALUES('run','Atlas','aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee');").unwrap();
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
}
