//! Durable server-side queue. Delivery is at least once across interrupted acknowledgments.
use hey_boss::{Request, Response};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize)]
struct Entry {
    request: Request,
    upstream: Option<String>,
    #[serde(default)]
    terminal: Option<Response>,
    #[serde(default)]
    pending_hide: bool,
    #[serde(default)]
    latest: Option<Response>,
    #[serde(default)]
    delivery_error: Option<String>,
}
fn save(path: &Path, entry: &Entry) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(entry).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    std::fs::rename(tmp, path).map_err(|e| e.to_string())?;
    std::fs::File::open(path.parent().ok_or("queue path has no parent")?)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
// A record can contain both the bounded original request and its terminal reply.
// Leave headroom for serialization while refusing damaged, unbounded disk input.
const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;
fn load(path: &Path) -> Result<Entry, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_ENTRY_BYTES {
        return Err("queue record too large".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err("queue record too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
fn server_hostname() -> String {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        let mut bytes = [0_u8; 256];
        if unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
            return "Server".into();
        }
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        let name: String = String::from_utf8_lossy(&bytes[..end])
            .chars()
            .filter(|c| !c.is_control())
            .take(80)
            .collect();
        if name.is_empty() {
            "Server".into()
        } else {
            name
        }
    })
    .clone()
}

fn forward(state: &Path, request: &Request) -> Result<Response, String> {
    forward_live(state, request, None)
}
fn forward_live(
    state: &Path,
    request: &Request,
    caller: Option<&UnixStream>,
) -> Result<Response, String> {
    if std::fs::read_to_string(state.join("bridge-protocol"))
        .ok()
        .as_deref()
        != Some("1")
    {
        return Err(if request.command == "action" {
            "Desktop companion is not connected. Connect this machine from the Mac before opening a website; the request was not queued.".into()
        } else {
            "reconnect with an upgraded Mac companion to negotiate the bridge protocol".into()
        });
    }
    let mut stream = UnixStream::connect(state.join("bridge.sock")).map_err(|e| {
        if request.command == "action" {
            format!("Desktop companion is not connected: {e}. Connect this machine from the Mac before opening a website; the request was not queued.")
        } else { e.to_string() }
    })?;
    stream
        .set_read_timeout(Some(if caller.is_some() {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(10)
        }))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut payload = serde_json::to_value(request).map_err(|e| e.to_string())?;
    // Stamp at the server boundary, including replay of older queued records.
    payload["source_host"] = serde_json::Value::String(server_hostname());
    if request.command == "action" {
        let host = std::fs::read_to_string(state.join("bridge-host"))
            .map_err(|_| "reconnect the companion to enable browser actions")?;
        payload["bridge_host"] = host.into();
        payload["bridge_generation"] = std::fs::read_to_string(state.join("bridge-generation"))
            .map_err(|_| "reconnect the companion to negotiate action sessions")?
            .into();
    }
    stream
        .write_all(&serde_json::to_vec(&payload).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    if let Some(caller) = caller {
        let deadline = std::time::Instant::now() + Duration::from_secs(900);
        let mut buffer = [0u8; 8192];
        loop {
            if client_disconnected(caller) || std::time::Instant::now() >= deadline {
                return Err("Secret request cancelled or timed out".into());
            }
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.extend_from_slice(&buffer[..n]);
                    if bytes.len() > 1024 * 1024 {
                        return Err("Secret response too large".into());
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                Err(_) => return Err("Secret transport disconnected".into()),
            }
        }
    } else {
        (&mut stream)
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
    }
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("upstream response too large".into());
    }
    let response: Response = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if response.task_id.is_empty() && response.status.as_deref() != Some("error") {
        return Err("upstream response has no task ID".into());
    }
    Ok(response)
}
fn response(id: &str, status: &str) -> Response {
    Response {
        task_id: id.into(),
        status: Some(status.into()),
        result: None,
        comments: None,
        review_status: None,
        document_name: None,
    }
}
fn handle(
    state: &Path,
    lock: &Mutex<()>,
    mut request: Request,
    client: &UnixStream,
) -> Result<Response, String> {
    // Secrets are synchronous and ephemeral. Never queue, replay, or cache the reply.
    if request.command == "secret" {
        if !request.sync {
            return Err("Secret requests must be synchronous".into());
        }
        return forward_live(state, &request, Some(client)).map_err(|_| {
            "Secret request unavailable, cancelled, or disconnected; no secret was queued".into()
        });
    }
    // Control requests use the established bridge and are never put in the durable queue.
    if matches!(
        request.command.as_str(),
        "overview" | "overview_snapshot" | "inbox" | "inbox_list" | "action"
    ) {
        return forward(state, &request);
    }
    if matches!(request.command.as_str(), "alert" | "update" | "ask") {
        if !request
            .project
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
            || !request
                .title
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
            || request.question.is_none()
        {
            return Err("project, title and message are required".into());
        }
        if request
            .autoclose
            .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 31_536_000.0)
        {
            return Err("autoclose must be positive and at most 31536000 seconds".into());
        }
        if let Some(issue) = &request.issue {
            issue.validate().map_err(|e| e.to_string())?;
        }
        if request.link_url.is_some() != request.link_label.is_some() {
            return Err("link URL and label must be supplied together".into());
        }
        if request.link_url.as_ref().is_some_and(|url| {
            !["https://", "http://", "file://"]
                .iter()
                .any(|scheme| url.to_ascii_lowercase().starts_with(scheme))
        }) {
            return Err("unsupported link URL".into());
        }
        if request.command != "alert" && request.description.is_none() {
            request.description = Some(String::new());
        }
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let id = format!(
            "remote-{:020}-{}-{:020}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos(),
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let sync = request.sync;
        request.sync = false;
        request.task_id = Some(id.clone());
        let path = state.join("queue").join(format!("{id}.json"));
        {
            let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            save(
                &path,
                &Entry {
                    request: request.clone(),
                    upstream: None,
                    terminal: None,
                    pending_hide: false,
                    latest: None,
                    delivery_error: None,
                },
            )?;
        }
        if !sync {
            return Ok(response(&id, "pending"));
        }
        request.command = "wait".into();
        request.task_id = Some(id);
    }
    if !matches!(request.command.as_str(), "status" | "wait" | "hide") {
        return Err("unknown command".into());
    }
    let id = request.task_id.clone().ok_or("task ID required")?;
    if !id.starts_with("remote-") || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("invalid remote task ID".into());
    }
    let path = state.join("queue").join(format!("{id}.json"));
    let waiting = request.command == "wait" || (request.command == "status" && request.sync);
    loop {
        if waiting && client_disconnected(client) {
            return Err("waiting client disconnected".into());
        }
        let entry = {
            let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut entry = load(&path)?;
            if let Some(terminal) = &entry.terminal {
                return Ok(terminal.clone());
            }
            if waiting && entry.request.command != "ask" && !entry.request.comments_enabled {
                return Err("only questions and enabled document reviews can be waited on".into());
            }
            if request.command == "hide" && entry.upstream.is_none() {
                let mut terminal = response(
                    &id,
                    if entry.request.command == "ask" || entry.request.comments_enabled {
                        "cancelled"
                    } else {
                        "ok"
                    },
                );
                if entry.request.comments_enabled {
                    terminal.review_status = Some("cancelled".into());
                    terminal.comments = entry.latest.as_ref().and_then(|r| r.comments.clone());
                }
                entry.terminal = Some(terminal.clone());
                save(&path, &entry)?;
                return Ok(terminal);
            }
            entry
        };
        let result = if let Some(upstream) = &entry.upstream {
            let mut action = request.clone();
            if action.command == "wait" {
                action.command = "status".into();
            }
            action.sync = false; // Poll the established local bridge; do not leak long-lived upstream waiters.
            action.task_id = Some(upstream.clone());
            let result = forward(state, &action).ok().map(|mut r| {
                r.task_id = id.clone();
                r
            });
            let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut current = load(&path)?;
            if let Some(terminal) = current.terminal {
                return Ok(terminal);
            }
            if let Some(r) = &result
                && matches!(r.status.as_deref(), Some("ok" | "cancelled" | "error"))
                && (request.command == "hide"
                    || entry.request.command == "ask"
                    || entry.request.comments_enabled)
            {
                current.terminal = Some(r.clone());
                save(&path, &current)?;
            }
            if let Some(r) = &result
                && entry.request.comments_enabled
                && r.review_status.is_some()
                && current.latest.as_ref() != Some(r)
            {
                current.latest = Some(r.clone());
                save(&path, &current)?;
            }
            result.or(entry.latest.clone())
        } else {
            entry.latest.clone()
        };
        if !waiting {
            return Ok(result.unwrap_or_else(|| response(&id, "pending")));
        }
        if let Some(result) = result
            && (matches!(result.status.as_deref(), Some("ok" | "cancelled" | "error"))
                || (request.command == "status"
                    && entry.request.comments_enabled
                    && result
                        .comments
                        .as_ref()
                        .is_some_and(|comments| !comments.is_empty())))
        {
            return Ok(result);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
fn client_disconnected(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    // A write-half shutdown terminates the request, but only POLLHUP means the
    // caller also closed its response half. Do not confuse the two.
    let mut descriptor = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: libc::POLLOUT,
        revents: 0,
    };
    (unsafe { libc::poll(&mut descriptor, 1, 0) }) > 0
        && descriptor.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
}
const MAX_CLIENTS: usize = 64;
struct ClientLease(Arc<AtomicUsize>);
impl Drop for ClientLease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
fn client_lease(active: &Arc<AtomicUsize>) -> Option<ClientLease> {
    active
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            (count < MAX_CLIENTS).then_some(count + 1)
        })
        .ok()?;
    Some(ClientLease(active.clone()))
}
fn read_request(stream: &mut UnixStream) -> Result<Request, String> {
    read_request_with_timeout(stream, Duration::from_secs(10))
}
fn read_request_with_timeout(
    stream: &mut UnixStream,
    timeout: Duration,
) -> Result<Request, String> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("request timed out")?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        let count = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > 8 * 1024 * 1024 {
            return Err("request too large".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

// Parse queue IDs numerically so older unpadded IDs and current fixed-width IDs
// share chronological order during upgrades.
fn queue_order(path: &Path) -> (u128, u64, u64) {
    let mut parts = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .split('-')
        .skip(1);
    (
        parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
        parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
        parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
    )
}

fn invalid_legacy_creation(request: &Request) -> bool {
    matches!(request.command.as_str(), "alert" | "update" | "ask")
        && (request.project.as_ref().is_none_or(|v| v.trim().is_empty())
            || request.title.as_ref().is_none_or(|v| v.trim().is_empty())
            || request.question.is_none())
}

// FIFO retry with capped exponential delay and jitter; receiving a new bridge
// generation resets the delay immediately. Enqueueing never waits for replay.
fn replay_delay(failures: u32) -> Duration {
    let base = (500_u64.saturating_mul(1_u64 << failures.saturating_sub(1).min(6))).min(30_000);
    let jitter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64
        % (base / 4 + 1);
    Duration::from_millis((base + jitter).min(30_000))
}

pub fn serve(state: PathBuf) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(state.join("queue")).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    let singleton = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("broker.lock"))
        .map_err(|e| e.to_string())?;
    if unsafe { libc::flock(singleton.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("broker already running".into());
    }
    let socket = state.join("daemon.sock");
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).map_err(|e| e.to_string())?;
    let state = Arc::new(state);
    let lock = Arc::new(Mutex::new(()));
    let worker_state = state.clone();
    let worker_lock = lock.clone();
    std::thread::Builder::new()
        .name("broker-replay".into())
        .spawn(move || {
            let mut failures = 0_u32;
            let mut retry_at = Instant::now();
            let mut generation = std::fs::read_to_string(worker_state.join("bridge-generation")).ok();
            loop {
                let connected_generation = std::fs::read_to_string(worker_state.join("bridge-generation")).ok();
                if connected_generation != generation {
                    generation = connected_generation;
                    failures = 0;
                    retry_at = Instant::now();
                }
                if Instant::now() < retry_at {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                if let Ok(paths) = std::fs::read_dir(worker_state.join("queue")) {
                    let mut paths: Vec<_> = paths
                        .flatten()
                        .map(|p| p.path())
                        .filter(|p| p.extension().is_some_and(|e| e == "json"))
                        .collect();
                    paths.sort_by_key(|path| queue_order(path));
                    for path in paths {
                        let request = {
                            let _guard = worker_lock.lock().unwrap_or_else(|p| p.into_inner());
                            let Ok(entry) = load(&path) else { continue };
                            if entry.pending_hide {
                                let Some(id) = entry.upstream else { continue };
                                serde_json::from_value(serde_json::json!({"command":"hide", "task_id":id, "sync":false})).unwrap()
                            } else {
                                if entry.upstream.is_some() || entry.terminal.is_some() { continue; }
                                entry.request
                            }
                        };
                        let result = match forward(&worker_state, &request) {
                            Ok(r) if r.status.as_deref() != Some("error") || invalid_legacy_creation(&request) => r,
                            outcome => {
                                let error = match outcome { Ok(_) => "Receiver rejected delivery; retained for retry".into(), Err(error) => error };
                                let _guard = worker_lock.lock().unwrap_or_else(|p| p.into_inner());
                                if let Ok(mut current) = load(&path) {
                                    current.delivery_error = Some(error);
                                    if let Err(e) = save(&path, &current) { eprintln!("Queue persistence: {e}"); }
                                }
                                failures = failures.saturating_add(1);
                                retry_at = Instant::now() + replay_delay(failures);
                                break;
                            }
                        };
                        failures = 0;
                        retry_at = Instant::now();
                        {
                            let _guard = worker_lock.lock().unwrap_or_else(|p| p.into_inner());
                            let Ok(mut current) = load(&path) else {
                                continue;
                            };
                            current.delivery_error = None;
                            if request.command == "hide" {
                                if result.status.as_deref() != Some("error") {
                                    current.pending_hide = false;
                                    if let Err(e) = save(&path, &current) { eprintln!("Queue persistence: {e}"); }
                                }
                                continue;
                            }
                            let cleanup = current.terminal.is_some()
                                && result.status.as_deref() != Some("error");
                            if result.status.as_deref() == Some("error") {
                                // Preserve rejected malformed legacy records for inspection,
                                // while allowing healthy notifications to proceed.
                                let id = path.file_stem().and_then(|n| n.to_str()).unwrap_or("unknown");
                                current.terminal.get_or_insert_with(|| response(id, "error"));
                                current.delivery_error = Some("Malformed legacy request rejected by receiver".into());
                            } else {
                                current.upstream = Some(result.task_id.clone());
                            }
                            current.pending_hide = cleanup;
                            if let Err(e) = save(&path, &current) {
                                eprintln!("Queue persistence: {e}");
                                break;
                            }
                        }
                        // Persisted cleanup is retried by the next worker pass.
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .map_err(|error| format!("broker replay thread: {error}"))?;
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("Broker accept: {error}");
                continue;
            }
        };
        if let Err(error) = stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .and_then(|_| stream.set_write_timeout(Some(Duration::from_secs(5))))
        {
            eprintln!("Broker client disconnected during setup: {error}");
            continue;
        }
        let Some(lease) = client_lease(&active) else {
            let _ = stream.write_all(br#"{"task_id":"","status":"error"}"#);
            continue;
        };
        let state = state.clone();
        let lock = lock.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("broker-client".into())
            .spawn(move || {
                let _lease = lease;
                let result =
                    read_request(&mut stream).and_then(|r| handle(&state, &lock, r, &stream));
                match result {
                    Ok(r) => {
                        if let Ok(bytes) = serde_json::to_vec(&r) {
                            let _ = stream.write_all(&bytes);
                        }
                    }
                    Err(e) => {
                        eprintln!("Broker request: {e}");
                        if let Ok(bytes) = serde_json::to_vec(
                            &serde_json::json!({"task_id":"", "status":"error", "error":e}),
                        ) {
                            let _ = stream.write_all(&bytes);
                        }
                    }
                }
            })
        {
            eprintln!("Broker client thread: {error}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notice_issue_link_survives_durable_queue_and_legacy_records() {
        let directory =
            std::env::temp_dir().join(format!("hey-boss-inbox-queue-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("notice.json");
        let mut request: Request = serde_json::from_value(
            serde_json::json!({"command":"alert","question":"Ready","sync":false}),
        )
        .unwrap();
        let issue = hey_boss::notices::IssueReference {
            project: "github.com/example/repo".into(),
            number: 7,
            host: Some("devbox".into()),
        };
        request.issue = Some(issue.clone());
        save(
            &path,
            &Entry {
                request,
                upstream: None,
                terminal: None,
                pending_hide: false,
                latest: None,
                delivery_error: None,
            },
        )
        .unwrap();
        assert_eq!(load(&path).unwrap().request.issue, Some(issue));
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        legacy["request"].as_object_mut().unwrap().remove("issue");
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert!(load(&path).unwrap().request.issue.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn fifo_order_is_preserved_across_legacy_and_current_queue_ids() {
        let mut paths = [
            "remote-00000000000000001800-2-00000000000000000000.json",
            "remote-1700-2.json",
            "remote-1800-2-00000000000000000001.json",
        ]
        .map(PathBuf::from);
        paths.sort_by_key(|p| queue_order(p));
        assert_eq!(queue_order(&paths[0]), (1700, 2, 0));
        assert_eq!(queue_order(&paths[1]), (1800, 2, 0));
        assert_eq!(queue_order(&paths[2]), (1800, 2, 1));
    }
    #[test]
    fn replay_backoff_is_exponential_and_bounded() {
        for failures in 1..=100 {
            let delay = replay_delay(failures).as_millis();
            let base = (500_u128 * (1_u128 << (failures - 1).min(6))).min(30_000);
            assert!((base..=(base + base / 4).min(30_000)).contains(&delay));
        }
    }
    #[test]
    fn secrets_bypass_disk_queue_and_cancel_on_disconnect() {
        let root = std::env::temp_dir().join(format!(
            "hb-secret-bridge-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("bridge-protocol"), "1").unwrap();
        let listener = UnixListener::bind(root.join("bridge.sock")).unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            s.read_to_end(&mut request).unwrap();
            s.write_all(b"{\"task_id\":\"secret\",\"status\":\"ok\",\"result\":\"[\\\"synthetic-only\\\"]\"}").unwrap();
        });
        let (caller, peer) = UnixStream::pair().unwrap();
        peer.shutdown(Shutdown::Write).unwrap();
        let request: Request =
            serde_json::from_value(serde_json::json!({"command":"secret","sync":true})).unwrap();
        let response = handle(&root, &Mutex::new(()), request.clone(), &caller).unwrap();
        assert_eq!(response.result.as_deref(), Some("[\"synthetic-only\"]"));
        server.join().unwrap();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_file(root.join("bridge.sock")).unwrap();
        let listener = UnixListener::bind(root.join("bridge.sock")).unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            s.read_to_end(&mut request).unwrap();
            std::thread::sleep(Duration::from_millis(400));
        });
        drop(peer);
        let started = Instant::now();
        assert!(handle(&root, &Mutex::new(()), request, &caller).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn waiting_client_half_close_is_distinct_from_disconnect() {
        let (reader, writer) = UnixStream::pair().unwrap();
        writer.shutdown(Shutdown::Write).unwrap();
        assert!(!client_disconnected(&reader));
        drop(writer);
        assert!(client_disconnected(&reader));
    }
    #[test]
    fn client_limit_releases_capacity_after_failure() {
        let active = Arc::new(AtomicUsize::new(0));
        let leases: Vec<_> = (0..MAX_CLIENTS)
            .map(|_| client_lease(&active).unwrap())
            .collect();
        assert!(client_lease(&active).is_none());
        drop(leases);
        assert_eq!(active.load(Ordering::Relaxed), 0);
        assert!(client_lease(&active).is_some());
    }
    #[test]
    fn trickling_bytes_cannot_extend_the_request_deadline() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        let sender = std::thread::spawn(move || {
            for _ in 0..20 {
                if writer.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let started = Instant::now();
        assert!(read_request_with_timeout(&mut reader, Duration::from_millis(100)).is_err());
        assert!(started.elapsed() < Duration::from_millis(300));
        drop(reader);
        sender.join().unwrap();
    }

    #[test]
    fn oversized_saved_record_is_rejected_without_reading_or_removing_it() {
        let path = std::env::temp_dir().join(format!(
            "hey-boss-queue-cap-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.set_len(MAX_ENTRY_BYTES + 1).unwrap();
        assert_eq!(load(&path).err().as_deref(), Some("queue record too large"));
        assert_eq!(file.metadata().unwrap().len(), MAX_ENTRY_BYTES + 1);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn malformed_request_is_rejected() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"not json").unwrap();
        writer.shutdown(Shutdown::Write).unwrap();
        assert!(read_request(&mut reader).is_err());
    }
}
