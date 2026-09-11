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
    pub link_url: Option<String>,
    pub link_label: Option<String>,
    pub task_id: Option<String>,
    pub sync: bool,
    pub origin: Option<Origin>,
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
                let contents = std::fs::read_to_string(marker).unwrap();
                root.join(contents.trim().strip_prefix("gitdir: ").unwrap())
            };
            let head = std::fs::read_to_string(directory.join("HEAD")).unwrap();
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
        let cwd = std::env::current_dir().unwrap();
        let git = Self::git_context(&cwd);
        let mut launchers = Vec::new();
        let mut pid = unsafe { libc::getppid() };
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
            assert_eq!(count, size);
            let info = unsafe { info.assume_init() };
            let mut path = [0_u8; 4096];
            let length =
                unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
            assert!(length > 0);
            let executable = std::ffi::CStr::from_bytes_until_nul(&path)
                .unwrap()
                .to_str()
                .unwrap()
                .into();
            launchers.push(Launcher {
                pid: pid as u32,
                executable,
            });
            assert_ne!(pid as u32, info.pbsi_ppid);
            pid = info.pbsi_ppid as i32;
        }
        Self {
            cwd,
            pid: std::process::id(),
            executable: std::env::current_exe().unwrap(),
            git,
            launchers,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

pub struct Client {
    socket: PathBuf,
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
            link_url: None,
            link_label: None,
            sync: false,
            origin: None,
        }
    }
}

impl Client {
    pub fn new(socket: impl AsRef<Path>) -> Self {
        Self {
            socket: socket.as_ref().to_owned(),
        }
    }

    pub fn start(&self, request: &Request) -> PendingResponse {
        let mut request = request.clone();
        if matches!(request.command.as_str(), "alert" | "update" | "ask") {
            assert!(
                request
                    .project
                    .as_ref()
                    .is_some_and(|value| !value.trim().is_empty())
                    && request
                        .title
                        .as_ref()
                        .is_some_and(|value| !value.trim().is_empty()),
                "creation project and title must not be blank"
            );
            request.origin = Some(Origin::capture());
        }
        let mut stream = UnixStream::connect(&self.socket).unwrap();
        stream
            .write_all(&serde_json::to_vec(&request).unwrap())
            .unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        PendingResponse { stream }
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
    pub fn wait(mut self) -> Response {
        let mut bytes = Vec::new();
        self.stream.read_to_end(&mut bytes).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::thread;

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
                    let result = std::panic::catch_unwind(|| {
                        Client::new(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/out/hey-boss-no-listener.sock"
                        ))
                        .start(&request)
                    });
                    let Err(panic) = result else {
                        panic!("invalid metadata accepted")
                    };
                    assert_eq!(
                        panic.downcast_ref::<&str>(),
                        Some(&"creation project and title must not be blank")
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
            ready_tx.send(()).unwrap();
            let answer: String = answer_rx.recv().unwrap();
            let response = Response {
                task_id: "test-question".into(),
                status: Some("ok".into()),
                result: Some(answer),
            };
            let bytes = serde_json::to_vec(&response).unwrap();
            for chunk in bytes.chunks(11) {
                stream.write_all(chunk).unwrap();
            }
        });
        let pending = Client::new(&path).ask(
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
    fn creation_metadata_is_sent_over_the_wire() {
        std::fs::create_dir_all(Path::new(env!("CARGO_MANIFEST_DIR")).join("out")).unwrap();
        for command in ["alert", "update"] {
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
            let client = Client::new(&path);
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
            assert_eq!(wire["command"], command);
            assert_eq!(wire["project"], "Atlas");
            assert_eq!(wire["title"], "Review");
            assert_eq!(
                wire["origin"]["cwd"],
                std::env::current_dir().unwrap().to_str().unwrap()
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
