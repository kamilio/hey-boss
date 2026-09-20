pub mod agent_control;
pub mod agent_conversations;
pub mod agents;
pub mod artifacts;
pub mod attachments;
pub mod document;
pub mod fleet;
pub mod health;
/// SQLite-backed project issues and durable agent ownership.
pub mod issues;
pub mod markdown;
pub mod mindmap;
pub mod notices;
pub mod syntax;
/// Shared worker terminal library, also available as the standalone worker-tui crate.
pub mod worker_tui;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Request {
    pub command: String,
    pub question: Option<String>,
    pub project: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub options: Option<Vec<String>>,
    pub autoclose: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<notices::IssueReference>,
    pub link_url: Option<String>,
    pub link_label: Option<String>,
    pub task_id: Option<String>,
    pub sync: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub comments_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<document::DocumentAttachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_name: Option<String>,
    pub origin: Option<Origin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_path: Option<PathBuf>,
}

/// A subtle status indicator, independent of the chosen icon.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Neutral,
    Info,
    Success,
    Warning,
    Error,
}

/// Optional appearance for items created by a client. Omitted severity is neutral.
#[derive(Debug, Default, Clone)]
pub struct Appearance {
    pub severity: Option<Severity>,
    /// A curated alias or SF Symbol name. Unknown names use the daemon default.
    pub icon: Option<String>,
    /// A local image; mutually exclusive with `icon`.
    pub icon_path: Option<PathBuf>,
}

/// Check installation metadata before sending commands unsupported by legacy daemons.
pub fn require_protocol(state: &Path) -> std::io::Result<()> {
    if std::fs::read_to_string(state.join("protocol-version"))
        .ok()
        .as_deref()
        != Some("1")
    {
        return Err(std::io::Error::other(
            "upgrade/reinstall the Mac daemon before using companion or overview",
        ));
    }
    let request: Request =
        serde_json::from_value(serde_json::json!({"command":"protocol", "sync":false}))?;
    let reply = Client::new(state.join("daemon.sock")).try_send(&request)?;
    if reply.status.as_deref() != Some("ok") || reply.result.as_deref() != Some("1") {
        return Err(std::io::Error::other("Mac daemon protocol is incompatible"));
    }
    Ok(())
}

/// Resolve a readable local icon up to 4 MiB before sending. AppKit decodes its contents.
pub fn resolve_icon_file(path: impl AsRef<Path>) -> Result<PathBuf, String> {
    let path = path.as_ref();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "tif" | "tiff" | "icns" | "pdf"
    ) {
        return Err("icon file must be PNG, JPEG, TIFF, ICNS, or PDF".into());
    }
    let absolute = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve icon file: {error}"))?;
    if !absolute.is_file() {
        return Err("icon file must be a regular file".into());
    }
    let file = std::fs::File::open(&absolute)
        .map_err(|error| format!("cannot read icon file: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect icon file: {error}"))?;
    if metadata.len() > 4 * 1024 * 1024 {
        return Err("icon file must be at most 4 MiB".into());
    }
    if absolute
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "tif" | "tiff" | "icns" | "pdf"
            )
        })
    {
        return Ok(absolute);
    }
    // Resolve the directory while retaining the supplied filename/extension.
    // This makes validation idempotent for symlinks to extensionless targets.
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent
        .canonicalize()
        .map_err(|e| e.to_string())?
        .join(path.file_name().ok_or("icon filename is missing")?))
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Launcher {
    pub pid: u32,
    pub executable: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GitContext {
    pub root: PathBuf,
    pub branch: Option<String>,
    pub revision: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Origin {
    pub cwd: PathBuf,
    pub pid: u32,
    pub executable: PathBuf,
    pub git: Option<GitContext>,
    pub launchers: Vec<Launcher>,
}

impl Origin {
    fn git_context(cwd: &Path) -> Option<GitContext> {
        for root in cwd.ancestors() {
            let marker = root.join(".git");
            if !marker.exists() {
                continue;
            }
            let directory = if marker.is_dir() {
                marker
            } else {
                let contents = std::fs::read_to_string(marker).ok()?;
                root.join(contents.trim().strip_prefix("gitdir: ")?)
            };
            let head = std::fs::read_to_string(directory.join("HEAD")).ok()?;
            let head = head.trim();
            let (branch, revision) = if let Some(branch) = head.strip_prefix("ref: refs/heads/") {
                (Some(branch.into()), None)
            } else {
                (None, Some(head.into()))
            };
            return Some(GitContext {
                root: root.into(),
                branch,
                revision,
            });
        }
        None
    }

    pub fn capture() -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();
        let git = Self::git_context(&cwd);
        #[allow(unused_mut)]
        let mut launchers = Vec::new();
        #[cfg(target_os = "macos")]
        let mut pid = unsafe { libc::getppid() };
        #[cfg(target_os = "macos")]
        while pid > 1 {
            let mut info = std::mem::MaybeUninit::<libc::proc_bsdshortinfo>::uninit();
            let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as i32;
            let count = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDT_SHORTBSDINFO,
                    0,
                    info.as_mut_ptr().cast(),
                    size,
                )
            };
            if count != size {
                break;
            }
            let info = unsafe { info.assume_init() };
            let mut path = [0_u8; 4096];
            let length =
                unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
            if length <= 0 {
                break;
            }
            let Ok(executable) = std::ffi::CStr::from_bytes_until_nul(&path) else {
                break;
            };
            let executable = PathBuf::from(executable.to_string_lossy().into_owned());
            launchers.push(Launcher {
                pid: pid as u32,
                executable,
            });
            if pid as u32 == info.pbsi_ppid {
                break;
            }
            pid = info.pbsi_ppid as i32;
        }
        #[cfg(target_os = "linux")]
        {
            let mut pid = unsafe { libc::getppid() } as u32;
            while pid > 1 {
                let directory = PathBuf::from(format!("/proc/{pid}"));
                if let Ok(executable) = std::fs::read_link(directory.join("exe")) {
                    launchers.push(Launcher { pid, executable });
                }
                let Ok(status) = std::fs::read_to_string(directory.join("status")) else {
                    break;
                };
                let parent = status
                    .lines()
                    .find_map(|line| line.strip_prefix("PPid:"))
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .unwrap_or(0);
                if parent == pid {
                    break;
                }
                pid = parent;
            }
        }
        Self {
            cwd,
            pid: std::process::id(),
            executable: std::env::current_exe().unwrap_or_default(),
            git,
            launchers,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct DocumentSelection {
    pub line_start: usize,
    pub line_end: usize,
    pub source_text: String,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct DocumentComment {
    pub id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    pub created_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<DocumentSelection>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Response {
    #[serde(default)]
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<DocumentComment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_name: Option<String>,
}

pub struct Client {
    socket: PathBuf,
    appearance: Appearance,
    comments_enabled: bool,
}

pub struct PendingResponse {
    stream: UnixStream,
}

pub struct Notification<'a> {
    pub project: &'a str,
    pub title: &'a str,
    pub message: &'a str,
    pub autoclose: Option<f64>,
    pub link: Option<(&'a str, &'a str)>,
}

pub struct Question<'a> {
    pub project: &'a str,
    pub title: &'a str,
    pub question: &'a str,
    pub description: &'a str,
    pub options: &'a [&'a str],
}

pub struct Update<'a> {
    pub project: &'a str,
    pub title: &'a str,
    pub summary: &'a str,
    pub markdown: &'a str,
}

impl Request {
    fn action(command: &str, task_id: Option<&str>) -> Self {
        Self {
            command: command.into(),
            task_id: task_id.map(str::to_owned),
            question: None,
            project: None,
            title: None,
            description: None,
            options: None,
            autoclose: None,
            issue: None,
            link_url: None,
            link_label: None,
            sync: false,
            comments_enabled: false,
            attachment: None,
            document_name: None,
            origin: None,
            severity: None,
            icon: None,
            icon_path: None,
        }
    }
}

impl Client {
    /// Versioned desktop action over the established local/SSH bridge.
    /// The JSON envelope is returned in Response.result; actions are not queued offline.
    pub fn try_action(
        &self,
        id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> std::io::Result<Response> {
        let mut request = Request::action("action", None);
        request.question = Some(
            serde_json::json!({"version":1,"id":id,"method":method,"params":params}).to_string(),
        );
        self.try_send(&request)
    }
    pub fn new(socket: impl AsRef<Path>) -> Self {
        Self {
            socket: socket.as_ref().to_owned(),
            appearance: Appearance::default(),
            comments_enabled: false,
        }
    }

    /// Enable review comments for subsequent updates; status returns them automatically.
    pub fn with_comments(mut self, enabled: bool) -> Self {
        self.comments_enabled = enabled;
        self
    }

    /// Snapshot a Markdown, code/text, or image file for review.
    pub fn try_review_file(
        &self,
        project: &str,
        title: &str,
        summary: &str,
        path: &Path,
        sync: bool,
    ) -> std::io::Result<PendingResponse> {
        let (content, attachment) = document::read_file(path)?;
        let mut request = Request::action("update", None);
        request.project = Some(project.into());
        request.title = Some(title.into());
        request.description = Some(summary.into());
        request.question = Some(content);
        request.attachment = attachment;
        request.document_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        request.comments_enabled = true;
        request.sync = sync;
        self.try_start(&request)
    }

    /// Configure appearance for subsequent alerts, updates, and questions.
    pub fn with_appearance(mut self, appearance: Appearance) -> Self {
        assert!(
            appearance.icon.is_none() || appearance.icon_path.is_none(),
            "icon and icon_path are mutually exclusive"
        );
        self.appearance = appearance;
        self
    }

    pub fn start(&self, request: &Request) -> PendingResponse {
        self.try_start(request)
            .expect("Cannot contact hey-boss daemon")
    }

    pub fn try_start(&self, request: &Request) -> std::io::Result<PendingResponse> {
        let mut request = request.clone();
        if request.command == "update" && self.comments_enabled {
            request.comments_enabled = true;
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
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "creation project and title must not be blank",
                ));
            }
            if request
                .autoclose
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "autoclose must be positive and finite",
                ));
            }
            if let Some(issue) = &request.issue {
                issue
                    .validate()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            }
            request.severity = request.severity.or(self.appearance.severity);
            if request.icon.is_none() && request.icon_path.is_none() {
                request.icon.clone_from(&self.appearance.icon);
                request.icon_path.clone_from(&self.appearance.icon_path);
            }
            if request.icon.is_some() && request.icon_path.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "icon and icon_path are mutually exclusive",
                ));
            }
            if let Some(path) = &request.icon_path {
                request.icon_path = Some(resolve_icon_file(path).map_err(|error| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, error)
                })?);
            }
            request.origin = Some(Origin::capture());
        }
        let mut stream = UnixStream::connect(&self.socket)?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        if !request.sync && request.command != "wait" {
            stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        }
        let mut payload = serde_json::to_value(&request)?;
        payload["source_host"] = serde_json::Value::String("This Mac".into());
        stream.write_all(&serde_json::to_vec(&payload)?)?;
        stream.shutdown(Shutdown::Write)?;
        Ok(PendingResponse { stream })
    }

    pub fn try_send(&self, request: &Request) -> std::io::Result<Response> {
        self.try_start(request)?
            .try_wait_response(request.command == "action")
    }

    pub fn send(&self, request: &Request) -> Response {
        self.start(request).wait()
    }

    pub fn alert(&self, notification: Notification<'_>) -> Response {
        let mut request = Request::action("alert", None);
        request.project = Some(notification.project.into());
        request.title = Some(notification.title.into());
        request.question = Some(notification.message.into());
        request.autoclose = notification.autoclose;
        if let Some(seconds) = request.autoclose {
            assert!(seconds.is_finite() && seconds > 0.0);
        }
        if let Some((url, label)) = notification.link {
            assert!(
                ["https://", "http://", "file://"]
                    .iter()
                    .any(|prefix| url.starts_with(prefix))
            );
            assert!(!label.trim().is_empty());
            request.link_url = Some(url.into());
            request.link_label = Some(label.into());
        }
        self.send(&request)
    }

    pub fn update(&self, update: Update<'_>) -> Response {
        let mut request = Request::action("update", None);
        request.project = Some(update.project.into());
        request.title = Some(update.title.into());
        request.description = Some(update.summary.into());
        request.question = Some(update.markdown.into());
        self.send(&request)
    }

    pub fn ask(&self, question: Question<'_>, sync: bool) -> PendingResponse {
        let mut request = Request::action("ask", None);
        request.project = Some(question.project.into());
        request.title = Some(question.title.into());
        request.question = Some(question.question.into());
        request.description = Some(question.description.into());
        request.options = Some(
            question
                .options
                .iter()
                .map(|value| (*value).into())
                .collect(),
        );
        request.sync = sync;
        self.start(&request)
    }

    /// Wait until review comments are available, an answer arrives, or the item closes.
    /// Existing comments return immediately; no extra comments retrieval is needed.
    pub fn try_wait_for_feedback(&self, task_id: &str) -> std::io::Result<Response> {
        let mut request = Request::action("status", Some(task_id));
        request.sync = true;
        self.try_send(&request)
    }

    pub fn status(&self, task_id: &str) -> Response {
        self.send(&Request::action("status", Some(task_id)))
    }

    pub fn hide(&self, task_id: &str) -> Response {
        self.send(&Request::action("hide", Some(task_id)))
    }

    pub fn wait(&self, task_id: &str) -> PendingResponse {
        self.start(&Request::action("wait", Some(task_id)))
    }
}

impl PendingResponse {
    pub fn wait(self) -> Response {
        self.try_wait().expect("Cannot read hey-boss response")
    }

    pub fn try_wait(self) -> std::io::Result<Response> {
        self.try_wait_response(false)
    }

    fn try_wait_response(mut self, accept_action_error: bool) -> std::io::Result<Response> {
        let mut bytes = Vec::new();
        (&mut self.stream)
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Response is too large",
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        if value["status"] == "error" && !(accept_action_error && value["result"].is_string()) {
            return Err(std::io::Error::other(
                value["error"]
                    .as_str()
                    .unwrap_or("Daemon rejected the request")
                    .to_owned(),
            ));
        }
        let response: Response = serde_json::from_value(value)?;
        if response.task_id.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Daemon response has no task ID",
            ));
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn malformed_rejected_and_stalled_responses_are_recoverable() {
        for bytes in [
            b"not json".as_slice(),
            b"",
            br#"{"status":"error","error":"Unknown task ID"}"#,
        ] {
            let (stream, mut server) = UnixStream::pair().unwrap();
            server.write_all(bytes).unwrap();
            drop(server);
            assert!(PendingResponse { stream }.try_wait().is_err());
        }
        let (stream, _server) = UnixStream::pair().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(20)))
            .unwrap();
        let error = PendingResponse { stream }.try_wait().unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
    }

    #[test]
    fn legacy_installation_is_rejected_without_sending_a_probe() {
        let root = std::env::temp_dir().join(format!("hb-protocol-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let listener = UnixListener::bind(root.join("daemon.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        assert!(require_protocol(&root).is_err());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        std::fs::write(root.join("protocol-version"), "1").unwrap();
        listener.set_nonblocking(false).unwrap();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_nonblocking(false).unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["command"],
                "protocol"
            );
            stream
                .write_all(br#"{"task_id":"protocol","status":"ok","result":"1"}"#)
                .unwrap();
        });
        require_protocol(&root).unwrap();
        peer.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stopped_daemon_is_a_recoverable_transport_error() {
        let root =
            std::env::temp_dir().join(format!("hey-boss-unavailable-{}", std::process::id()));
        let client = Client::new(root.join("absent.sock"));
        assert!(
            client
                .try_send(&Request::action("status", Some("missing")))
                .is_err()
        );
    }

    #[test]
    fn invalid_creation_metadata_is_rejected_before_connecting() {
        for command in ["alert", "update", "ask"] {
            for field in ["project", "title"] {
                for value in [None, Some(""), Some(" \t\n")] {
                    let mut request = Request::action(command, None);
                    request.project = Some("Atlas".into());
                    request.title = Some("Review".into());
                    if field == "project" {
                        request.project = value.map(str::to_owned);
                    } else {
                        request.title = value.map(str::to_owned);
                    }
                    let error = Client::new(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/out/hey-boss-no-listener.sock"
                    ))
                    .try_start(&request)
                    .err()
                    .expect("invalid metadata accepted");
                    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
                    assert_eq!(
                        error.to_string(),
                        "creation project and title must not be blank"
                    );
                }
            }
        }
    }

    #[test]
    fn question_can_wait_without_blocking_the_caller() {
        let path = PathBuf::from(format!(
            "{}/out/hey-boss-library-{}.sock",
            env!("CARGO_MANIFEST_DIR"),
            std::process::id()
        ));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&path).unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (answer_tx, answer_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            let request: Request = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(request.project.as_deref(), Some("Atlas"));
            assert_eq!(request.title.as_deref(), Some("Review"));
            assert_eq!(request.question.as_deref(), Some("Title?"));
            assert!(request.sync);
            assert_eq!(request.severity, Some(Severity::Warning));
            assert_eq!(request.icon.as_deref(), Some("question"));
            assert!(request.icon_path.is_none());
            ready_tx.send(()).unwrap();
            let answer: String = answer_rx.recv().unwrap();
            let response = Response {
                task_id: "test-question".into(),
                status: Some("ok".into()),
                comments: None,
                review_status: None,
                document_name: None,
                result: Some(answer),
            };
            let bytes = serde_json::to_vec(&response).unwrap();
            for chunk in bytes.chunks(11) {
                stream.write_all(chunk).unwrap();
            }
        });
        let pending = Client::new(&path)
            .with_appearance(Appearance {
                severity: Some(Severity::Warning),
                icon: Some("question".into()),
                icon_path: None,
            })
            .ask(
                Question {
                    project: "Atlas",
                    title: "Review",
                    question: "Title?",
                    description: "",
                    options: &[],
                },
                true,
            );
        ready_rx.recv().unwrap();
        answer_tx.send("Native answer α".into()).unwrap();
        assert_eq!(pending.wait().result.as_deref(), Some("Native answer α"));
        worker.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn question_cancellation_is_returned_without_an_answer() {
        for command in ["ask", "wait"] {
            let path = PathBuf::from(format!(
                "{}/out/hey-boss-cancel-{}-{command}.sock",
                env!("CARGO_MANIFEST_DIR"),
                std::process::id()
            ));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let listener = UnixListener::bind(&path).unwrap();
            let worker = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).unwrap();
                let request: Request = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(request.command, command);
                stream
                    .write_all(br#"{"task_id":"cancelled-question","status":"cancelled"}"#)
                    .unwrap();
            });
            let client = Client::new(&path);
            let pending = if command == "ask" {
                client.ask(
                    Question {
                        project: "Atlas",
                        title: "Review",
                        question: "Approve?",
                        description: "",
                        options: &["Yes", "No"],
                    },
                    true,
                )
            } else {
                client.wait("cancelled-question")
            };
            let response = pending.wait();
            assert_eq!(response.status.as_deref(), Some("cancelled"));
            assert!(response.result.is_none());
            worker.join().unwrap();
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn creation_metadata_is_sent_over_the_wire() {
        std::fs::create_dir_all(Path::new(env!("CARGO_MANIFEST_DIR")).join("out")).unwrap();
        for (command, custom) in [
            ("alert", false),
            ("update", false),
            ("alert", true),
            ("update", true),
        ] {
            let path = PathBuf::from(format!(
                "{}/out/hey-boss-{}-{}.sock",
                env!("CARGO_MANIFEST_DIR"),
                command,
                std::process::id()
            ));
            let listener = UnixListener::bind(&path).unwrap();
            let worker = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).unwrap();
                let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                stream.write_all(br#"{"task_id":"created"}"#).unwrap();
                wire
            });
            let icon_file = path.with_extension("png");
            let client = if custom {
                std::fs::write(&icon_file, b"image decoding belongs to AppKit").unwrap();
                Client::new(&path).with_appearance(Appearance {
                    severity: Some(Severity::Success),
                    icon: None,
                    icon_path: Some(icon_file.clone()),
                })
            } else {
                Client::new(&path)
            };
            if command == "alert" {
                client.alert(Notification {
                    project: "Atlas",
                    title: "Review",
                    message: "Ready",
                    autoclose: None,
                    link: None,
                });
            } else {
                client.update(Update {
                    project: "Atlas",
                    title: "Review",
                    summary: "Ready",
                    markdown: "**Done**",
                });
            }
            let wire = worker.join().unwrap();
            if custom {
                assert_eq!(wire["severity"], "success");
                assert_eq!(
                    wire["icon_path"],
                    icon_file.canonicalize().unwrap().to_str().unwrap()
                );
                assert!(wire.get("icon").is_none());
                std::fs::remove_file(icon_file).unwrap();
            } else {
                for field in ["severity", "icon", "icon_path"] {
                    assert!(wire.get(field).is_none());
                }
                let legacy: Request = serde_json::from_value(wire.clone()).unwrap();
                assert!(legacy.severity.is_none());
            }
            assert_eq!(wire["command"], command);
            assert_eq!(wire["project"], "Atlas");
            assert_eq!(wire["title"], "Review");
            assert_eq!(
                wire["origin"]["cwd"],
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_str()
                    .unwrap()
            );
            assert_eq!(wire["origin"]["pid"], std::process::id());
            assert!(!wire["origin"]["launchers"].as_array().unwrap().is_empty());
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn git_context_handles_branches_worktrees_and_detached_heads() {
        std::fs::create_dir_all(Path::new(env!("CARGO_MANIFEST_DIR")).join("out")).unwrap();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("out")
            .canonicalize()
            .unwrap()
            .join(format!("hey-boss-origin-{}", std::process::id()));
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("repo/nested")).unwrap();
        std::fs::write(
            root.join("repo/.git/HEAD"),
            "ref: refs/heads/feature/launch-details\n",
        )
        .unwrap();
        let context = Origin::git_context(&root.join("repo/nested")).unwrap();
        assert_eq!(context.root, root.join("repo"));
        assert_eq!(context.branch.as_deref(), Some("feature/launch-details"));
        assert!(context.revision.is_none());
        std::fs::create_dir_all(root.join("worktree")).unwrap();
        std::fs::create_dir_all(root.join("gitdir")).unwrap();
        std::fs::write(root.join("worktree/.git"), "gitdir: ../gitdir\n").unwrap();
        std::fs::write(
            root.join("gitdir/HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();
        let context = Origin::git_context(&root.join("worktree")).unwrap();
        assert_eq!(context.root, root.join("worktree"));
        assert!(context.branch.is_none());
        assert_eq!(
            context.revision.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert!(Origin::git_context(Path::new("/")).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
