//! Read-only process/session discovery. Historical files alone are never agents.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Agent {
    pub id: String,
    pub pid: u32,
    pub kind: String,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    pub task: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    pub activity: Option<String>,
    pub state: String,
    pub updated_at: Option<u64>,
    pub evidence: String,
    #[serde(default)]
    pub update: Option<String>,
    #[serde(default)]
    pub activity_at: Option<u64>,
    #[serde(default)]
    pub git: Option<GitInfo>,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GitInfo {
    pub repository_root: String,
    pub common_dir: String,
    pub worktree: String,
    pub branch: Option<String>,
    pub repository_id: String,
    pub origin: Option<String>,
}
fn git_output(cwd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", cwd])
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
fn normalize_origin(origin: &str) -> Option<String> {
    // Query/fragment strings can carry credentials and are never repository identity.
    let value = origin.trim().split(['?', '#']).next()?;
    let (authority, path, scheme) = if let Some((scheme, rest)) = value.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        (authority, path, scheme)
    } else if let Some((host, path)) = value.split_once(':') {
        (host, path.trim_start_matches('/'), "ssh")
    } else {
        return None;
    };
    let authority = authority.rsplit('@').next()?.to_ascii_lowercase();
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "https" => ":443",
        "http" => ":80",
        "ssh" => ":22",
        _ => "",
    };
    let host = if default_port.is_empty() {
        authority.as_str()
    } else {
        authority.strip_suffix(default_port).unwrap_or(&authority)
    };
    let path = path.trim_matches('/').trim_end_matches(".git");
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}"))
}
pub fn git_info(cwd: &str) -> Option<GitInfo> {
    let locations = git_output(
        cwd,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
        ],
    )?;
    let mut lines = locations.lines();
    let worktree = lines.next()?.to_owned();
    let common_dir = lines.next()?.to_owned();
    // Linked worktrees share the main checkout's .git directory. Enumerating
    // hundreds of worktrees for every identity/status read is unnecessary.
    let root = Path::new(&common_dir)
        .file_name()
        .filter(|name| *name == ".git")
        .and_then(|_| Path::new(&common_dir).parent())
        .map(|path| path.to_string_lossy().into_owned())
        .or_else(|| {
            git_output(cwd, &["worktree", "list", "--porcelain"]).and_then(|list| {
                list.lines()
                    .find_map(|line| line.strip_prefix("worktree "))
                    .map(str::to_owned)
            })
        })
        .unwrap_or_else(|| worktree.clone());
    let origin = git_output(cwd, &["config", "--get", "remote.origin.url"])
        .and_then(|url| normalize_origin(&url));
    let branch = git_output(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]).or_else(|| {
        git_output(cwd, &["rev-parse", "--short", "HEAD"])
            .map(|revision| format!("Detached · {revision}"))
    });
    Some(GitInfo {
        repository_root: root,
        common_dir: common_dir.clone(),
        worktree,
        branch,
        repository_id: origin.clone().unwrap_or(common_dir),
        origin,
    })
}
fn set_task(agent: &mut Agent, candidate: Option<String>) {
    if let Some(task) = candidate
        && (task.chars().count() >= 40 || agent.task.is_none())
    {
        agent.task = Some(task);
    }
}
fn readable_tool(name: &str, arguments: &str) -> String {
    let name = name.to_lowercase();
    let label = if name.contains("search_openai") || name.contains("fetch_openai") {
        "Reading official documentation"
    } else if name.contains("cua") || name.contains("playwright") {
        "Inspecting the app interface"
    } else if name.contains("view_image") {
        "Reviewing a screenshot"
    } else if name.contains("apply_patch") || name == "edit" || name == "write" {
        "Editing code"
    } else if name == "read" || name == "read_file" {
        "Reading files"
    } else if name.contains("search") || name == "grep" || name == "glob" {
        "Searching code"
    } else if name.contains("exec") || name.contains("bash") || name.contains("shell") {
        if [
            "cargo test",
            "pytest",
            "npm test",
            "pnpm test",
            "hey-boss-test",
        ]
        .iter()
        .any(|s| arguments.contains(s))
        {
            "Running tests"
        } else if ["cargo build", "swiftc", "npm run build"]
            .iter()
            .any(|s| arguments.contains(s))
        {
            "Building the app"
        } else if arguments.contains("git diff") || arguments.contains("git status") {
            "Reviewing changes"
        } else if arguments.contains("git commit") {
            "Committing changes"
        } else if arguments.contains("git push") {
            "Publishing commits"
        } else if arguments.contains("companion install") {
            "Installing the server companion"
        } else if arguments.contains("ps -") || arguments.contains("lsof") {
            "Inspecting running agents"
        } else if arguments.contains("rg ") {
            "Searching code"
        } else if arguments.contains("cat ") || arguments.contains("sed ") {
            "Reading files"
        } else {
            "Running a command"
        }
    } else {
        "Using a tool"
    };
    label.into()
}
fn event_time(event: &Value) -> Option<u64> {
    if let Some(ms) = event["payload"]["completed_at_ms"]
        .as_u64()
        .or(event["payload"]["started_at_ms"].as_u64())
    {
        return Some(ms / 1000);
    }
    let timestamp = event["timestamp"].as_str()?;
    if !timestamp.ends_with('Z') {
        return None;
    }
    let input = std::ffi::CString::new(timestamp).ok()?;
    let format = c"%Y-%m-%dT%H:%M:%S";
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    if unsafe { libc::strptime(input.as_ptr(), format.as_ptr(), &mut tm) }.is_null() {
        return None;
    }
    let seconds = unsafe { libc::timegm(&mut tm) };
    u64::try_from(seconds).ok()
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Snapshot {
    pub host: String,
    pub observed_at: u64,
    pub agents: Vec<Agent>,
    pub warnings: Vec<String>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn kind(executable: &str) -> Option<&'static str> {
    if executable
        .to_ascii_lowercase()
        .ends_with("/claude.app/contents/macos/claude")
    {
        return None;
    }
    match Path::new(executable)
        .file_name()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "codex" => Some("Codex"),
        "claude" => Some("Claude"),
        _ => None,
    }
}
fn short(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty()
        || text.starts_with('<')
        || text.starts_with("# AGENTS.md")
        || text.starts_with("The following is the Codex agent history")
    {
        return None;
    }
    Some(
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(220)
            .collect(),
    )
}
fn text(content: &Value) -> Option<String> {
    if let Some(s) = content.as_str() {
        return short(s);
    }
    content
        .as_array()?
        .iter()
        .filter(|c| {
            matches!(
                c["type"].as_str(),
                None | Some("text" | "Text" | "output_text" | "input_text")
            )
        })
        .filter_map(|c| c.get("text").and_then(Value::as_str).and_then(short))
        .next_back()
}
fn public_assistant_message(message: &Value) -> bool {
    ["phase", "channel"].into_iter().all(|key| {
        !message[key].as_str().is_some_and(|channel| {
            ["analysis", "reasoning", "thinking"]
                .iter()
                .any(|private| channel.eq_ignore_ascii_case(private))
        })
    })
}
fn saved_codex_git(agent: &mut Agent, git: &Value) {
    if agent.kind == "Codex"
        && let (Some(origin), Some(cwd)) = (
            git["repository_url"].as_str().and_then(normalize_origin),
            agent.cwd.as_ref(),
        )
    {
        agent.git = Some(GitInfo {
            repository_root: cwd.clone(),
            common_dir: String::new(),
            worktree: cwd.clone(),
            branch: git["branch"].as_str().map(str::to_owned),
            repository_id: origin.clone(),
            origin: Some(origin),
        });
    }
}
fn apply_event(agent: &mut Agent, event: &Value) {
    let payload = event.get("payload").unwrap_or(event);
    let category = event["type"].as_str().unwrap_or("");
    let ty = payload["type"].as_str().unwrap_or("");
    if category == "custom-title" && agent.kind == "Claude" {
        agent.title = event["customTitle"]
            .as_str()
            .and_then(short)
            .or(agent.title.take());
    }
    let public_envelope = public_assistant_message(event) && public_assistant_message(payload);
    if category == "session_meta" {
        agent.session_id = payload["id"]
            .as_str()
            .or(payload["session_id"].as_str())
            .map(str::to_owned);
        agent.cwd = payload["cwd"]
            .as_str()
            .map(str::to_owned)
            .or(agent.cwd.take());
        saved_codex_git(agent, &payload["git"]);
    }
    if ty == "item_completed" || ty == "item_started" {
        let item = &payload["item"];
        match item["type"].as_str() {
            Some("UserMessage" | "userMessage") => {
                set_task(agent, text(&item["content"]));
                agent.state = "Working".into();
            }
            Some("AgentMessage" | "agentMessage")
                if public_envelope && public_assistant_message(item) =>
            {
                if item["phase"] == "final_answer" {
                    agent.state = "Idle".into();
                    agent.activity = Some("Turn completed".into());
                    agent.activity_at = event_time(event);
                    agent.update = text(&item["content"]).or(agent.update.take());
                } else if let Some(message) = text(&item["content"]) {
                    agent.update = Some(message);
                }
            }
            Some("CommandExecution" | "commandExecution") => {
                agent.activity = Some(readable_tool("exec", &item["command"].to_string()));
                agent.activity_at = event_time(event);
                if let Some(parsed) = item["parsed_cmd"]
                    .as_array()
                    .and_then(|items| items.first())
                {
                    let verb = match parsed["type"].as_str() {
                        Some("read") => "Reading",
                        Some("write") => "Editing",
                        Some("search") => "Searching",
                        _ => "",
                    };
                    if !verb.is_empty() {
                        agent.activity = parsed["path"]
                            .as_str()
                            .and_then(|path| Path::new(path).file_name())
                            .map(|file| format!("{verb} {}", file.to_string_lossy()))
                            .or_else(|| Some(format!("{verb} files")));
                    }
                }
            }
            Some("FileChange" | "fileChange") => {
                agent.activity = Some("Editing files".into());
            }
            Some("McpToolCall" | "mcpToolCall") => {
                agent.activity = item["tool"].as_str().map(|tool| readable_tool(tool, ""));
                agent.activity_at = event_time(event);
            }
            _ => {}
        }
    }
    if category == "response_item"
        && ty == "message"
        && payload["role"] == "assistant"
        && public_envelope
        && payload["phase"] == "final_answer"
    {
        agent.state = "Idle".into();
        agent.activity = Some("Turn completed".into());
        agent.activity_at = event_time(event);
        agent.update = text(&payload["content"]).or(agent.update.take());
    }
    match ty {
        "task_started" => {
            agent.update = None;
            agent.activity_at = event_time(event);
            agent.state = "Working".into();
            agent.activity = Some("Turn in progress".into());
        }
        "task_complete" | "turn_aborted" => {
            agent.activity_at = event_time(event);
            agent.state = "Idle".into();
            agent.activity = Some(
                if ty == "task_complete" {
                    "Turn completed"
                } else {
                    "Turn interrupted"
                }
                .into(),
            );
        }
        "user_message" => {
            set_task(agent, payload["message"].as_str().and_then(short));
        }
        "function_call" | "custom_tool_call" => {
            agent.activity = payload["name"].as_str().map(|name| {
                readable_tool(
                    name,
                    payload["arguments"]
                        .as_str()
                        .or(payload["input"].as_str())
                        .unwrap_or(""),
                )
            });
            agent.activity_at = event_time(event);
        }
        _ => {}
    }
    if category == "response_item"
        && ty == "message"
        && payload["role"] == "assistant"
        && public_envelope
        && payload["phase"] != "final_answer"
    {
        agent.update = text(&payload["content"]).or(agent.update.take());
    }
    if category == "response_item" && ty == "message" && payload["role"] == "user" {
        set_task(agent, text(&payload["content"]));
    }
    if category == "user" && event["isMeta"] != true && event["isCompactSummary"] != true {
        set_task(agent, text(&event["message"]["content"]));
    }
    if category == "assistant" {
        if let Some(content) = event["message"]["content"].as_array() {
            for item in content {
                if item["type"] == "tool_use" {
                    agent.activity = item["name"]
                        .as_str()
                        .map(|name| readable_tool(name, &item["input"].to_string()));
                    agent.activity_at = event_time(event);
                }
            }
        }
        if public_envelope && public_assistant_message(&event["message"]) {
            agent.update = text(&event["message"]["content"]).or(agent.update.take());
        }
        // Claude transcripts do not prove live execution or an approval state.
        agent.state = "Session open".into();
    }
    if agent.kind == "Claude" {
        agent.session_id = event["sessionId"]
            .as_str()
            .map(str::to_owned)
            .or(agent.session_id.take());
        agent.cwd = event["cwd"]
            .as_str()
            .map(str::to_owned)
            .or(agent.cwd.take());
    }
}
#[derive(Serialize, Deserialize)]
struct CachedSession {
    inode: u64,
    modified: u128,
    offset: u64,
    summary: Agent,
    #[serde(default)]
    internal: bool,
    #[serde(default)]
    process_identity: Option<String>,
}
const CACHE_VERSION: u32 = 7;
#[derive(Serialize, Deserialize, Default)]
struct Cache {
    #[serde(default)]
    version: u32,
    entries: BTreeMap<PathBuf, CachedSession>,
}
fn internal_source(source: &Value) -> bool {
    matches!(source.as_str(), Some("review" | "compact"))
        || matches!(source["subagent"].as_str(), Some("review" | "compact"))
        || source["subagent"]["other"] == "guardian"
}
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;

fn read_event_line(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
) -> std::io::Result<(usize, bool)> {
    line.clear();
    let mut total = 0;
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok((total, false));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |index| index + 1);
        total += count;
        if !oversized {
            if line.len() + count > MAX_EVENT_BYTES {
                oversized = true;
                line.clear();
            } else {
                let needed = line.len() + count;
                if line.capacity() < needed {
                    let capacity = needed.next_power_of_two().min(MAX_EVENT_BYTES);
                    line.reserve_exact(capacity - line.len());
                }
                line.extend_from_slice(&available[..count]);
            }
        }
        reader.consume(count);
        if newline.is_some() {
            return Ok((total, true));
        }
    }
}

fn read_session(agent: &mut Agent, path: &Path, cache: &mut Cache) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(mut file) = std::fs::File::open(path) else {
        return true;
    };
    let Ok(meta) = file.metadata() else {
        return true;
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut offset = 0;
    let mut internal = false;
    if let Some(previous) = cache.entries.get(path)
        && previous.inode == meta.ino()
        && previous.offset <= meta.len()
        && (previous.offset < meta.len() || previous.modified == modified)
    {
        offset = previous.offset;
        internal = previous.internal;
        let id = agent.id.clone();
        let pid = agent.pid;
        *agent = previous.summary.clone();
        agent.id = id;
        agent.pid = pid;
    }
    if offset > 0 && agent.kind == "Codex" && agent.git.is_none() {
        // Older caches lack saved Git metadata. Read just the first record;
        // preserve their offsets, activity, and current working directories.
        let mut header = BufReader::new((&mut file).take(MAX_EVENT_BYTES as u64));
        let mut line = Vec::new();
        if let Ok((_, true)) = read_event_line(&mut header, &mut line)
            && let Ok(event) = serde_json::from_slice::<Value>(&line)
            && event["type"] == "session_meta"
            && event["payload"]["id"].as_str() == agent.session_id.as_deref()
        {
            saved_codex_git(agent, &event["payload"]["git"]);
        }
    }
    let _ = file.seek(SeekFrom::Start(offset));
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    while let Ok((bytes, complete)) = read_event_line(&mut reader, &mut line) {
        if bytes == 0 || !complete {
            break;
        }
        offset += bytes as u64;
        // Large tool outputs are irrelevant to detection. No reasoning or
        // arguments are stored in the cache, just the displayable summary.
        if !line.is_empty()
            && let Ok(event) = serde_json::from_slice::<Value>(&line)
        {
            if event["type"] == "session_meta" {
                internal = internal_source(&event["payload"]["source"]);
            }
            if !internal {
                apply_event(agent, &event);
            }
        }
    }
    agent.updated_at = Some((modified / 1_000_000_000) as u64);
    if agent.state == "Process detected" {
        agent.state = "Session open".into();
    }
    cache.entries.insert(
        path.to_owned(),
        CachedSession {
            inode: meta.ino(),
            modified,
            offset,
            summary: agent.clone(),
            internal,
            process_identity: process_identity(agent.pid),
        },
    );
    !internal
}
fn cache_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(format!(
        ".local/share/hey-boss/agents-cache-v{CACHE_VERSION}.json"
    )))
}
fn save_cache(cache: &Cache) {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let Some(path) = cache_path() else { return };
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
    {
        if serde_json::to_writer(&mut file, cache).is_ok() {
            let _ = std::fs::rename(&temporary, &path);
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        let _ = std::fs::remove_file(temporary);
    }
}
#[derive(Default)]
struct Files {
    cwd: Option<String>,
    sessions: BTreeSet<PathBuf>,
}
fn session_file(path: &Path) -> bool {
    let path = path.to_string_lossy();
    path.ends_with(".jsonl")
        && (path.contains("/.codex/sessions/") || path.contains("/.claude/projects/"))
}
#[cfg(target_os = "macos")]
fn files(pids: &[u32]) -> BTreeMap<u32, Files> {
    let mut result = BTreeMap::<u32, Files>::new();
    if pids.is_empty() {
        return result;
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(output) = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-p", &list, "-F", "pfn"])
        .output()
    else {
        return result;
    };
    let mut pid = 0;
    let mut descriptor = "";
    let output = String::from_utf8_lossy(&output.stdout);
    for line in output.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse().unwrap_or(0);
        }
        if let Some(value) = line.strip_prefix('f') {
            descriptor = value;
        }
        if let Some(value) = line.strip_prefix('n') {
            let entry = result.entry(pid).or_default();
            if descriptor == "cwd" {
                entry.cwd = Some(value.into());
            }
            if session_file(Path::new(value)) {
                entry.sessions.insert(PathBuf::from(value));
            }
        }
    }
    result
}
#[cfg(not(target_os = "macos"))]
fn files(pids: &[u32]) -> BTreeMap<u32, Files> {
    pids.iter()
        .map(|pid| {
            let directory = PathBuf::from(format!("/proc/{pid}"));
            let cwd = std::fs::read_link(directory.join("cwd"))
                .ok()
                .map(|p| p.to_string_lossy().into_owned());
            let sessions = std::fs::read_dir(directory.join("fd"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|p| std::fs::read_link(p.path()).ok())
                .filter(|p| session_file(p))
                .collect();
            (*pid, Files { cwd, sessions })
        })
        .collect()
}
#[cfg(target_os = "macos")]
fn process_started(pid: u32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let count = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    (count == size).then(|| unsafe { info.assume_init().pbi_start_tvsec })
}
#[cfg(not(target_os = "macos"))]
fn process_started(pid: u32) -> Option<u64> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "etimes="])
        .output()
        .ok()?;
    let elapsed = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(now().saturating_sub(elapsed))
}
#[cfg(target_os = "macos")]
pub fn process_identity(pid: u32) -> Option<String> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    if unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    } != size
    {
        return None;
    }
    let info = unsafe { info.assume_init() };
    Some(format!(
        "{}:{}",
        info.pbi_start_tvsec, info.pbi_start_tvusec
    ))
}
#[cfg(not(target_os = "macos"))]
pub fn process_identity(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let start = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?;
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(format!("{}:{start}", boot.trim()))
}
fn cached_process_session(cache: &Cache, pid: u32, kind: &str, identity: &str) -> Option<PathBuf> {
    cache
        .entries
        .iter()
        .filter(|(_, entry)| {
            !entry.internal
                && entry.summary.pid == pid
                && entry.summary.kind == kind
                && entry.process_identity.as_deref() == Some(identity)
        })
        .max_by_key(|(_, entry)| {
            entry
                .summary
                .activity_at
                .or(entry.summary.updated_at)
                .unwrap_or(0)
        })
        .map(|(path, _)| path.clone())
}
#[cfg(target_os = "macos")]
fn process_arguments(pids: &[u32]) -> BTreeMap<u32, Vec<String>> {
    if pids.is_empty() {
        return BTreeMap::new();
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(output) = Command::new("ps")
        .args(["-p", &list, "-o", "pid=,args="])
        .output()
    else {
        return BTreeMap::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid = words.next()?.parse().ok()?;
            Some((pid, words.map(str::to_owned).collect()))
        })
        .collect()
}
#[cfg(not(target_os = "macos"))]
fn process_arguments(pids: &[u32]) -> BTreeMap<u32, Vec<String>> {
    pids.iter()
        .filter_map(|pid| {
            let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
            Some((
                *pid,
                bytes
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .collect(),
            ))
        })
        .collect()
}
fn codex_mode(arguments: &[String]) -> Option<&str> {
    let executable = arguments
        .iter()
        .position(|s| Path::new(s).file_name().and_then(|s| s.to_str()) == Some("codex"))?;
    let mut args = arguments[executable + 1..].iter();
    while let Some(arg) = args.next() {
        if matches!(
            arg.as_str(),
            "-c" | "--config"
                | "-C"
                | "--cd"
                | "-m"
                | "--model"
                | "-p"
                | "--profile"
                | "--enable"
                | "--disable"
                | "-s"
                | "--sandbox"
                | "-a"
                | "--ask-for-approval"
        ) {
            args.next()?;
            continue;
        }
        if arg.starts_with("--") && arg.contains('=') {
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        return Some(arg);
    }
    None
}
fn resume_session_id(arguments: &[String]) -> Option<&str> {
    if codex_mode(arguments) != Some("resume") || arguments.iter().any(|s| s == "--last") {
        return None;
    }
    arguments.iter().find_map(|s| {
        let groups = s.split('-').collect::<Vec<_>>();
        (groups.iter().map(|s| s.len()).eq([8, 4, 4, 4, 12])
            && groups
                .iter()
                .all(|s| s.bytes().all(|b| b.is_ascii_hexdigit())))
        .then_some(s.as_str())
    })
}
fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".codex")))
}

// Never open Codex's SQLite state, even read-only: observers must not
// participate in its locking or WAL recovery. These are plain session files.
struct SessionRecord {
    title: Option<String>,
    rollout: Option<PathBuf>,
}
struct SessionFiles {
    root: PathBuf,
    titles: BTreeMap<String, String>,
}
impl SessionFiles {
    fn open(root: &Path) -> Option<Self> {
        if !root.is_dir() {
            return None;
        }
        Some(Self {
            root: root.to_owned(),
            titles: Self::titles(root).unwrap_or_default(),
        })
    }
    fn titles(root: &Path) -> std::io::Result<BTreeMap<String, String>> {
        let mut file = std::fs::File::open(root.join("session_index.jsonl"))?;
        let size = file.metadata()?.len();
        let base = size.saturating_sub(4 * 1024 * 1024);
        file.seek(SeekFrom::Start(base))?;
        let mut reader = BufReader::new(file.take(size - base));
        let mut line = Vec::new();
        if base > 0 {
            read_event_line(&mut reader, &mut line)?;
        }
        let mut titles = BTreeMap::new();
        loop {
            let (bytes, complete) = read_event_line(&mut reader, &mut line)?;
            if bytes == 0 || !complete {
                break;
            }
            if let Ok(record) = serde_json::from_slice::<Value>(&line)
                && let Some(id) = record["id"].as_str()
                && let Some(title) = record["thread_name"].as_str().and_then(short)
            {
                titles.insert(id.to_owned(), title);
            }
        }
        Ok(titles)
    }
    fn thread(&self, id: &str) -> Option<SessionRecord> {
        let rollout = rollout(&self.root, id)?;
        Some(SessionRecord {
            title: self.titles.get(id).cloned(),
            rollout: Some(rollout),
        })
    }
}
fn metadata_state(status: &str) -> Option<&'static str> {
    match status {
        "busy" => Some("Working"),
        "idle" => Some("Idle"),
        "waiting" | "waiting_for_input" | "waiting-for-input" => Some("Waiting for input"),
        _ => None,
    }
}
fn claude_metadata(agent: &mut Agent) -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let path = config.join("sessions").join(format!("{}.json", agent.pid));
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return None;
    }
    let value: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if value["pid"].as_u64()? != u64::from(agent.pid) {
        return None;
    }
    let started = value["startedAt"].as_u64()? / 1000;
    let process_start = process_started(agent.pid)?;
    if started.abs_diff(process_start) > 60 {
        return None;
    }
    let session = value["sessionId"].as_str()?;
    if session.is_empty()
        || !session
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return None;
    }
    agent.session_id = Some(session.into());
    agent.cwd = value["cwd"]
        .as_str()
        .map(str::to_owned)
        .or(agent.cwd.take());
    agent.state = value["status"]
        .as_str()
        .and_then(metadata_state)
        .unwrap_or("Session open")
        .into();
    agent.updated_at = value["updatedAt"].as_u64().map(|t| t / 1000);
    agent.evidence = "Live PID-specific Claude session metadata (process start verified)".into();
    // Find this exact session ID; never guess the newest transcript in a folder.
    for directory in std::fs::read_dir(config.join("projects")).ok()?.flatten() {
        let path = directory.path().join(format!("{session}.jsonl"));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

pub fn scan() -> Snapshot {
    let host = Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "Unknown host".into());
    let mut snapshot = Snapshot {
        host,
        observed_at: now(),
        agents: Vec::new(),
        warnings: Vec::new(),
    };
    let output = match Command::new("ps")
        .args(["-axo", "pid=,uid=,comm="])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => {
            snapshot
                .warnings
                .push("Process discovery unavailable".into());
            return snapshot;
        }
    };
    let processes: Vec<_> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim().splitn(3, char::is_whitespace);
            let pid = fields.next()?.parse::<u32>().ok()?;
            let remainder = line.trim().strip_prefix(&pid.to_string())?.trim();
            let (uid, executable) = remainder.split_once(char::is_whitespace)?;
            if uid.parse::<u32>().ok()? != unsafe { libc::getuid() } {
                return None;
            }
            let kind = kind(executable.trim())?;
            Some((pid, kind.to_owned()))
        })
        .collect();
    let pids = processes.iter().map(|(pid, _)| *pid).collect::<Vec<_>>();
    let details = files(&pids);
    let arguments = process_arguments(&pids);
    let session_files = codex_home().and_then(|root| SessionFiles::open(&root));
    let mut cache: Cache = cache_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default();
    if cache.version != CACHE_VERSION {
        cache.entries.clear();
        cache.version = CACHE_VERSION;
    }
    let mut active_paths = BTreeSet::new();
    for (pid, kind) in processes {
        let detail = details.get(&pid);
        let mut base = Agent {
            id: format!("pid-{pid}"),
            pid,
            kind,
            cwd: detail.and_then(|d| d.cwd.clone()),
            session_id: None,
            task: None,
            title: None,
            activity: None,
            state: "Process detected".into(),
            updated_at: None,
            evidence: "Running process; session unavailable".into(),
            update: None,
            activity_at: None,
            git: None,
        };
        if base.kind == "Claude" {
            if let Some(path) = claude_metadata(&mut base) {
                let metadata = (base.state.clone(), base.updated_at, base.evidence.clone());
                read_session(&mut base, &path, &mut cache);
                active_paths.insert(path);
                base.state = metadata.0;
                base.updated_at = metadata.1;
                base.evidence = metadata.2;
            }
            if base.session_id.is_some() {
                snapshot.agents.push(base);
                continue;
            }
        }
        if base.kind == "Codex" && detail.is_none_or(|d| d.sessions.is_empty()) {
            if let Some(args) = arguments.get(&pid) {
                if matches!(codex_mode(args), Some("app-server" | "mcp-server")) {
                    continue;
                }
                if let Some(id) = resume_session_id(args)
                    && let Some(record) = session_files.as_ref().and_then(|s| s.thread(id))
                    && let Some(path) = record.rollout
                {
                    let mut resumed = base.clone();
                    if read_session(&mut resumed, &path, &mut cache)
                        && resumed.session_id.as_deref() == Some(id)
                    {
                        resumed.title = record.title.or(resumed.title);
                        resumed.id = format!("{pid}:{}", path.display());
                        resumed.evidence = "Resume target from live launch command; current loaded session unverified".into();
                        resumed.state = "Resume target".into();
                        resumed.activity =
                            Some("Requested resume target; live turn state unavailable".into());
                        active_paths.insert(path);
                        snapshot.agents.push(resumed);
                        continue;
                    }
                }
            }
            if let Some(identity) = process_identity(pid)
                && let Some(path) = cached_process_session(&cache, pid, &base.kind, &identity)
            {
                let mut known = base.clone();
                if read_session(&mut known, &path, &mut cache) && known.session_id.is_some() {
                    known.id = format!("{pid}:{}", path.display());
                    known.evidence = "Previously observed transcript for this same live process (start identity verified); currently closed".into();
                    known.state = "Session open".into();
                    known.activity = Some("Transcript closed; live turn state unavailable".into());
                    active_paths.insert(path);
                    snapshot.agents.push(known);
                    continue;
                }
            }
            base.evidence =
                "Live terminal process; no transcript held open and no verified resume ID".into();
        }
        if let Some(detail) = detail
            && !detail.sessions.is_empty()
        {
            for path in &detail.sessions {
                let mut agent = base.clone();
                agent.id = format!("{pid}:{}", path.display());
                agent.evidence = "Session file held open by this process".into();
                let visible = read_session(&mut agent, path, &mut cache);
                active_paths.insert(path.clone());
                if visible {
                    snapshot.agents.push(agent);
                }
            }
        } else {
            snapshot.agents.push(base);
        }
    }
    for agent in &mut snapshot.agents {
        if agent.kind == "Codex"
            && let Some(id) = &agent.session_id
            && let Some(title) = session_files.as_ref().and_then(|s| s.titles.get(id))
        {
            agent.title = Some(title.clone());
        }
    }
    let mut repositories = BTreeMap::<String, Option<GitInfo>>::new();
    for agent in &mut snapshot.agents {
        if let Some(cwd) = &agent.cwd {
            let live_git = repositories
                .entry(cwd.clone())
                .or_insert_with(|| git_info(cwd))
                .clone();
            if let Some(git) = live_git {
                agent.git = Some(git);
            } else if agent.git.is_some() {
                agent.evidence +=
                    "; repository/branch from saved session metadata (checkout unavailable)";
            }
        }
    }
    cache.entries.retain(|path, _| active_paths.contains(path));
    save_cache(&cache);
    snapshot.agents.sort_by(|a, b| {
        a.cwd
            .cmp(&b.cwd)
            .then(a.kind.cmp(&b.kind))
            .then(a.id.cmp(&b.id))
    });
    snapshot
}
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
static PATHS: OnceLock<Mutex<HashMap<(PathBuf, String), PathBuf>>> = OnceLock::new();
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
pub fn rollout(home: &Path, session: &str) -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;
    fn agent() -> Agent {
        Agent {
            id: "test".into(),
            pid: 1,
            kind: "Codex".into(),
            cwd: None,
            session_id: None,
            task: None,
            title: None,
            activity: None,
            state: "Process detected".into(),
            updated_at: None,
            evidence: "test".into(),
            update: None,
            activity_at: None,
            git: None,
        }
    }
    #[test]
    fn cached_idle_bindings_reject_reused_pids_and_internal_sessions() {
        let mut cache = Cache::default();
        let mut a = agent();
        a.pid = 42;
        a.session_id = Some("live-thread".into());
        cache.entries.insert(
            PathBuf::from("/known.jsonl"),
            CachedSession {
                inode: 1,
                modified: 0,
                offset: 0,
                summary: a,
                internal: false,
                process_identity: Some("boot:start-ticks".into()),
            },
        );
        assert_eq!(
            cached_process_session(&cache, 42, "Codex", "boot:start-ticks"),
            Some(PathBuf::from("/known.jsonl"))
        );
        assert!(cached_process_session(&cache, 42, "Codex", "boot:reused-pid").is_none());
        assert!(cached_process_session(&cache, 43, "Codex", "boot:start-ticks").is_none());
        cache
            .entries
            .get_mut(Path::new("/known.jsonl"))
            .unwrap()
            .internal = true;
        assert!(cached_process_session(&cache, 42, "Codex", "boot:start-ticks").is_none());
    }
    #[test]
    fn backend_modes_and_exact_resume_identifiers() {
        let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            codex_mode(&args("/bin/codex app-server")),
            Some("app-server")
        );
        assert_eq!(
            codex_mode(&args("/bin/codex mcp-server")),
            Some("mcp-server")
        );
        assert_eq!(
            resume_session_id(&args("codex resume 01a06878-8f0f-7591-8065-877500724753")),
            Some("01a06878-8f0f-7591-8065-877500724753")
        );
        assert!(
            resume_session_id(&args(
                "codex resume --last 01a06878-8f0f-7591-8065-877500724753"
            ))
            .is_none()
        );
        assert!(
            resume_session_id(&args("codex exec 01a06878-8f0f-7591-8065-877500724753")).is_none()
        );
        assert_eq!(kind("/Applications/Claude.app/Contents/MacOS/Claude"), None);
        assert_eq!(kind("/usr/local/bin/claude"), Some("Claude"));
    }
    #[test]
    fn session_metadata_works_while_codex_database_is_exclusively_locked() {
        let root = std::env::temp_dir().join(format!("hb-session-files-{}", std::process::id()));
        std::fs::create_dir_all(root.join("archived_sessions")).unwrap();
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let path = root.join("state_5.sqlite");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY, title TEXT, rollout_path TEXT, name TEXT); BEGIN EXCLUSIVE;").unwrap();
        let rollout = root
            .join("archived_sessions")
            .join(format!("rollout-{id}.jsonl"));
        std::fs::write(&rollout, format!("{}\n", serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":"/deleted/worktree","git":{"repository_url":"git@github.com:poe-internal/poe2.git","branch":"codex/deleted-worktree"}}}))).unwrap();
        std::fs::write(
            root.join("session_index.jsonl"),
            format!(
                "{}\n{}\ninvalid\n{{\"id\":",
                serde_json::json!({"id":id,"thread_name":"Original name"}),
                serde_json::json!({"id":id,"thread_name":"Renamed chat"})
            ),
        )
        .unwrap();
        let store = SessionFiles::open(&root).unwrap();
        let row = store.thread(id).unwrap();
        assert_eq!(row.title.as_deref(), Some("Renamed chat"));
        assert_eq!(row.rollout, Some(rollout.clone()));
        assert!(
            store
                .thread("ffffffff-bbbb-cccc-dddd-eeeeeeeeeeee")
                .is_none()
        );
        let mut a = agent();
        let mut cache = Cache::default();
        assert!(read_session(&mut a, &rollout, &mut cache));
        assert_eq!(
            a.git.as_ref().unwrap().origin.as_deref(),
            Some("github.com/poe-internal/poe2")
        );
        assert_eq!(
            a.git.as_ref().unwrap().branch.as_deref(),
            Some("codex/deleted-worktree")
        );
        // Upgrade old observer caches without rescanning transcript history or
        // replacing the most recent working directory with the session header.
        let saved = &mut cache.entries.get_mut(&rollout).unwrap().summary;
        saved.git = None;
        saved.cwd = Some("/current/worktree".into());
        saved.task = Some("Current task".into());
        assert!(read_session(&mut a, &rollout, &mut cache));
        assert!(a.git.is_some());
        assert_eq!(a.cwd.as_deref(), Some("/current/worktree"));
        assert_eq!(a.task.as_deref(), Some("Current task"));
        db.execute_batch("COMMIT; BEGIN EXCLUSIVE; COMMIT;")
            .unwrap();
        assert!(!root.join("state_5.sqlite-wal").exists());
        assert!(!root.join("state_5.sqlite-shm").exists());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn claude_explicit_custom_title_is_separate_from_latest_task() {
        let mut a = agent();
        a.kind = "Claude".into();
        a.task = Some("Latest request".into());
        apply_event(
            &mut a,
            &serde_json::json!({"type":"custom-title","customTitle":"My chat"}),
        );
        assert_eq!(a.title.as_deref(), Some("My chat"));
        assert_eq!(a.task.as_deref(), Some("Latest request"));
    }
    #[test]
    fn oversized_events_are_skipped_without_losing_the_next_record() {
        let mut bytes = vec![b'x'; MAX_EVENT_BYTES + 123];
        bytes.extend_from_slice(b"\nnext record\npartial");
        let mut reader = BufReader::with_capacity(97, std::io::Cursor::new(bytes));
        let mut line = Vec::new();
        assert_eq!(
            read_event_line(&mut reader, &mut line).unwrap(),
            (MAX_EVENT_BYTES + 124, true)
        );
        assert!(line.is_empty() && line.capacity() <= MAX_EVENT_BYTES);
        assert_eq!(read_event_line(&mut reader, &mut line).unwrap(), (12, true));
        assert_eq!(line, b"next record\n");
        assert_eq!(read_event_line(&mut reader, &mut line).unwrap(), (7, false));
    }

    #[test]
    fn guardian_sessions_stay_hidden_on_initial_and_cached_reads() {
        let path = std::env::temp_dir().join(format!("hb-guardian-{}.jsonl", std::process::id()));
        std::fs::write(&path, concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"source\":{\"subagent\":{\"other\":\"guardian\"}}}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Private approval payload\"}}\n"
        )).unwrap();
        let mut cache = Cache::default();
        for _ in 0..2 {
            cache = serde_json::from_slice(&serde_json::to_vec(&cache).unwrap()).unwrap();
            let mut summary = agent();
            assert!(!read_session(&mut summary, &path, &mut cache));
            assert!(summary.task.is_none() && summary.update.is_none());
        }
        assert!(!internal_source(
            &serde_json::json!({"subagent":{"spawn":{"parent_thread_id":"user-task"}}})
        ));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn explicit_private_channels_never_replace_public_progress() {
        for private in ["analysis", "reasoning", "thinking", "Analysis"] {
            for event in [
                serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","phase":private,"content":[{"type":"text","text":"PRIVATE_CHANNEL_PAYLOAD"}]}}}),
                serde_json::json!({"type":"response_item","payload":{"type":"message","role":"assistant","channel":private,"content":[{"type":"output_text","text":"PRIVATE_CHANNEL_PAYLOAD"}]}}),
                serde_json::json!({"type":"assistant","message":{"channel":private,"content":[{"type":"text","text":"PRIVATE_CHANNEL_PAYLOAD"}]}}),
                serde_json::json!({"type":"event_msg","channel":private,"payload":{"type":"item_completed","item":{"type":"AgentMessage","content":[{"type":"text","text":"PRIVATE_CHANNEL_PAYLOAD"}]}}}),
                serde_json::json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","channel":private,"content":[{"type":"output_text","text":"PRIVATE_CHANNEL_PAYLOAD"}]}}),
            ] {
                let mut summary = agent();
                summary.update = Some("Public update".into());
                apply_event(&mut summary, &event);
                assert_eq!(summary.update.as_deref(), Some("Public update"));
                assert!(
                    !serde_json::to_string(&summary)
                        .unwrap()
                        .contains("PRIVATE_CHANNEL_PAYLOAD")
                );
            }
        }
    }

    #[test]
    fn helpers_and_desktop_renderers_are_not_agents() {
        assert_eq!(kind("/vendor/bin/codex"), Some("Codex"));
        for executable in [
            "codex-code-mode-host",
            "Codex (Renderer)",
            "Claude Helper",
            "chrome-native-host",
        ] {
            assert_eq!(kind(executable), None);
        }
    }
    #[test]
    fn turn_lifecycle_controls_activity_without_exposing_reasoning_or_tool_arguments() {
        let mut agent = agent();
        for event in [
            serde_json::json!({"type":"event_msg","payload":{"type":"task_started"}}),
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"Fix the offline queue"}]}}),
            serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"SECRET"}}),
        ] {
            apply_event(&mut agent, &event);
        }
        assert_eq!(agent.state, "Working");
        assert_eq!(agent.task.as_deref(), Some("Fix the offline queue"));
        assert_eq!(agent.activity.as_deref(), Some("Running a command"));
        apply_event(
            &mut agent,
            &serde_json::json!({"type":"event_msg","payload":{"type":"task_complete"}}),
        );
        assert_eq!(agent.state, "Idle");
        assert!(!serde_json::to_string(&agent).unwrap().contains("SECRET"));
        assert!(short("<environment_context>private context").is_none());
    }
    #[test]
    fn modern_codex_items_supply_tasks_and_final_messages_close_turns() {
        let mut agent = agent();
        apply_event(
            &mut agent,
            &serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"Build the overview"}]}}}),
        );
        assert_eq!(agent.task.as_deref(), Some("Build the overview"));
        assert_eq!(agent.state, "Working");
        apply_event(
            &mut agent,
            &serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","phase":"final_answer","content":[{"type":"Text","text":"Done"}]}}}),
        );
        assert_eq!(agent.state, "Idle");
        assert_eq!(agent.update.as_deref(), Some("Done"));
    }
    #[test]
    fn incremental_reads_keep_task_when_only_tools_are_appended_and_retry_partial_lines() {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!("hb-log-{}.jsonl", std::process::id()));
        std::fs::write(&path,b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Original task\"}}\n").unwrap();
        let mut cache = Cache::default();
        let mut first = agent();
        read_session(&mut first, &path, &mut cache);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}")
            .unwrap();
        let mut second = agent();
        read_session(&mut second, &path, &mut cache);
        assert_eq!(second.task.as_deref(), Some("Original task"));
        file.write_all(b"\n").unwrap();
        read_session(&mut second, &path, &mut cache);
        assert_eq!(second.state, "Idle");
        std::fs::write(&path,b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"New\"}}\n").unwrap();
        let mut third = agent();
        read_session(&mut third, &path, &mut cache);
        assert_eq!(third.task.as_deref(), Some("New"));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn repository_urls_strip_sensitive_suffixes_and_default_ports() {
        for url in [
            "https://user:password@GitHub.com:443/example/atlas.git?token=private#secret",
            "ssh://git@github.com:22/example/atlas.git",
            "git@github.com:example/atlas.git",
        ] {
            assert_eq!(
                normalize_origin(url).as_deref(),
                Some("github.com/example/atlas")
            );
        }
        assert_eq!(
            normalize_origin("https://host:8443/team/repo.git").as_deref(),
            Some("host:8443/team/repo")
        );
        assert_eq!(normalize_origin("https://host/"), None);
    }
    #[test]
    fn private_reasoning_blocks_are_never_progress_text() {
        assert_eq!(
            text(&serde_json::json!([{"type":"reasoning","text":"private"}])),
            None
        );
        assert_eq!(
            text(
                &serde_json::json!([{"type":"thinking","text":"private"}, {"type":"text","text":"Public progress"}])
            ),
            Some("Public progress".into())
        );
    }
    #[test]
    fn repository_identity_groups_worktrees_without_exposing_remote_credentials() {
        assert_eq!(
            normalize_origin("git@github.com:example/atlas.git"),
            Some("github.com/example/atlas".into())
        );
        assert_eq!(
            normalize_origin("https://secret@github.com/example/atlas.git"),
            Some("github.com/example/atlas".into())
        );
        let root = std::env::temp_dir().join(format!("hb-git-{}", std::process::id()));
        let repository = root.join("atlas");
        let worktree = root.join("atlas-feature");
        std::fs::create_dir_all(&repository).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&repository)
                    .args(args)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-b", "main"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "Fixture",
        ]);
        git(&[
            "remote",
            "add",
            "origin",
            "git@github.com:example/atlas.git",
        ]);
        git(&[
            "worktree",
            "add",
            "-b",
            "feature",
            worktree.to_str().unwrap(),
        ]);
        let primary = git_info(repository.to_str().unwrap()).unwrap();
        let linked = git_info(worktree.to_str().unwrap()).unwrap();
        assert_eq!(primary.repository_id, linked.repository_id);
        assert_eq!(primary.common_dir, linked.common_dir);
        assert_eq!(primary.repository_root, linked.repository_root);
        assert_ne!(primary.worktree, linked.worktree);
        assert_eq!(linked.branch.as_deref(), Some("feature"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn progress_is_separate_from_tool_action_and_short_steering_keeps_task_context() {
        let mut agent = agent();
        set_task(
            &mut agent,
            Some("Improve the native overview and add repository grouping".into()),
        );
        set_task(&mut agent, Some("check again".into()));
        assert!(agent.task.unwrap().contains("repository grouping"));
        assert_eq!(
            readable_tool("exec_command", "cargo test --locked"),
            "Running tests"
        );
        assert_eq!(
            event_time(&serde_json::json!({"timestamp":"2026-09-15T03:18:45.123Z"})),
            Some(1789442325)
        );
    }
    #[test]
    fn claude_messages_do_not_claim_live_execution() {
        let mut agent = agent();
        agent.kind = "Claude".into();
        apply_event(
            &mut agent,
            &serde_json::json!({"type":"assistant","sessionId":"abc","cwd":"/repo","message":{"content":[{"type":"tool_use","name":"Read","input":{"secret":"hidden"}}]}}),
        );
        assert_eq!(agent.state, "Session open");
        assert_eq!(agent.activity.as_deref(), Some("Reading files"));
        assert_eq!(agent.session_id.as_deref(), Some("abc"));
    }
}
