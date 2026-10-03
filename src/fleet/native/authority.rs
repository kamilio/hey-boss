//! Owner-private companion relay over the already authenticated fleet stream.
//! One bounded request at a time; requests are never queued for offline replay.
use super::{
    Result,
    context::{Context, Lock, encode_frame, read_frame, send},
};
use crate::issues::{Error, Request};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufReader, Write},
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const SOCKET: &str = "fleet-authority.sock";
const WAIT: Duration = Duration::from_secs(10);

fn unavailable(detail: impl std::fmt::Display) -> Error {
    Error::new(
        "fleet_unavailable",
        format!(
            "Cannot reach the authoritative fleet through the existing supervisor connection: {detail}. Restore that connection and upgrade hey-boss on the supervisor and companion. No local fallback was used"
        ),
    )
}

pub(in crate::fleet) fn call(
    state: &Path,
    database: &Path,
    request: Value,
) -> crate::issues::Result<Value> {
    let mut stream = UnixStream::connect(state.join(SOCKET)).map_err(unavailable)?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let kind = request["kind"].as_str().unwrap_or("").to_owned();
    let envelope = json!({"database":database,"request":request});
    send(&mut stream, envelope).map_err(unavailable)?;
    // Requests are newline framed. A fast relay can close after replying before
    // we half-close, making shutdown fail on macOS despite a buffered reply.
    let result = read_frame(&mut BufReader::new(stream))
        .map_err(unavailable)?
        .ok_or_else(|| unavailable("connection closed before acknowledgment"))?;
    if result["ok"] == false {
        let mut error: Error = serde_json::from_value(result["error"].clone())?;
        if matches!(kind.as_str(), "capabilities" | "issue_metadata")
            && error.code == "invalid_input"
            && error.message == "Unsupported authority request"
        {
            let operation = if kind == "capabilities" {
                "capability discovery"
            } else {
                "guarded issue metadata"
            };
            error = Error::new(
                "fleet_capability_unsupported",
                format!(
                    "The running companion or supervisor does not support {operation} through the existing fleet tunnel. Run hey-boss upgrade on the supervisor to update the fleet, then reconnect and inspect hey-boss fleet capabilities. Nothing was saved; no SSH hostname or work claim is needed"
                ),
            );
            error.details = Some(
                json!({"route":"supervisor_tunnel","requested_operation":kind,"upgrade_required":true}),
            );
        }
        return Err(error);
    }
    if result["ok"] != true {
        return Err(unavailable("incomplete response"));
    }
    Ok(result)
}

pub(in crate::fleet) fn resource(
    request: &Request,
    database: &Path,
) -> crate::issues::Result<Value> {
    routed(request, database, "resource")
}

pub(in crate::fleet) fn metadata(
    request: &Request,
    database: &Path,
) -> crate::issues::Result<Value> {
    crate::issues::authority::validate(request)?;
    routed(request, database, "issue_metadata")
}

fn routed(request: &Request, database: &Path, kind: &str) -> crate::issues::Result<Value> {
    // Workers inherit HEY_BOSS_ISSUE_DB for the installed store. The relay
    // verifies its canonical database identity before forwarding any request;
    // a genuinely different private store still cannot use the live fleet.
    let socket = crate::fleet::socket_path()?;
    call(
        socket.parent().unwrap(),
        database,
        json!({"kind":kind,"request":request}),
    )
    .map_err(|mut error| {
        if request.operation.writes() && error.code == "fleet_unavailable" {
            error
                .message
                .push_str(". A write may have completed; retry with the same --request-id");
        }
        error
    })
}

pub(in crate::fleet) fn numbers(
    database: &Path,
    project: &crate::issues::Project,
    next: i64,
) -> crate::issues::Result<Value> {
    let socket = crate::fleet::socket_path()?;
    call(
        socket.parent().unwrap(),
        database,
        json!({"kind":"issue_numbers","project":project,"next":next}),
    )
}

pub(super) fn failure(error: Error) -> Value {
    json!({"ok":false,"error":error})
}

pub(super) fn capabilities() -> Value {
    json!({"authority_rpc":true,"issue_numbers":true,"issue_metadata":true,"issue_request_status":true,"issue_pr_attachments":true,"issue_draft":true,"issue_reopen":true,"issue_close":true,"issue_dependencies":true,"issue_ready":true,"issue_ready_keep_draft":true,"issue_assignment":true,"issue_reviewed_github_handoff":true,"issue_github_refresh":true,"issue_archives":true})
}

pub(super) fn capability_report(route: &str, capabilities: Value, build: Value) -> Value {
    json!({"ok":true,"route":route,"capabilities":capabilities,"supervisor_build":build,
        "usage":"Title/body/label edits, blocked-by, Ready, close and reopen use --supervisor. Close requires issue_close support and guards captured by the CLI; conflicting owners or live reservations are refused, and --force is unsupported. PR add/list use --supervisor with issue_pr_attachments support; add preserves existing purposes and ownership. PR classify/remove and commit URLs are not supported on that route. Dependency edits require issue_dependencies support, unassigned, unreserved work and no --force; omit blockers to clear links. Reopen requires issue_reopen support and unassigned, unreserved work. Ordinary issue edit NUMBER --draft uses the supervisor tunnel on companions. No SSH hostname or work claim is needed.",
        "recovery":"If a capability is false, run hey-boss upgrade on the supervisor to update the fleet, then reconnect and inspect hey-boss fleet capabilities again."})
}

fn unsupported(capability: &str, build: &Value) -> Error {
    let mut error = Error::new(
        "fleet_capability_unsupported",
        format!(
            "The connected supervisor has not advertised {capability} (build {}). Run hey-boss upgrade on the supervisor to update the fleet, then reconnect and inspect hey-boss fleet capabilities. Nothing was sent or saved; no SSH hostname or work claim is needed",
            build.as_str().unwrap_or("unknown")
        ),
    );
    error.details = Some(
        json!({"route":"supervisor_tunnel","required_capability":capability,"supervisor_build":build,"sent":false}),
    );
    error
}

pub(super) struct Relay {
    supported: Arc<AtomicBool>,
    numbers_supported: Arc<AtomicBool>,
    advertised: Arc<Mutex<Value>>,
    stopped: Arc<AtomicBool>,
    replies: mpsc::SyncSender<Value>,
    thread: Option<thread::JoinHandle<()>>,
    socket: std::path::PathBuf,
    _lock: Lock,
}
impl Relay {
    pub fn start<W: Write + Send + 'static>(ctx: &Context, output: Arc<Mutex<W>>) -> Result<Self> {
        let lock = ctx
            .lock("fleet-authority.lock", false)?
            .ok_or_else(|| unavailable("another companion connection owns the relay"))?;
        let socket = ctx.state.join(SOCKET);
        ctx.protect_file(&socket)?;
        match fs::symlink_metadata(&socket) {
            Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(&socket)?,
            Ok(_) => return Err(unavailable("relay path is not a socket").into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let listener = UnixListener::bind(&socket)?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let supported = Arc::new(AtomicBool::new(false));
        let numbers_supported = Arc::new(AtomicBool::new(false));
        let advertised = Arc::new(Mutex::new(json!({})));
        let advertisement = advertised.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let (replies, incoming) = mpsc::sync_channel::<Value>(2);
        let ready = supported.clone();
        let numbers_ready = numbers_supported.clone();
        let stop = stopped.clone();
        let database = ctx.path.canonicalize()?;
        let thread = thread::spawn(move || {
            let mut serial = 0_u64;
            while !stop.load(Ordering::Acquire) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    Err(_) => break,
                };
                let result = (|| -> Result<Value> {
                    stream.set_nonblocking(false)?;
                    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                    let request = read_frame(&mut BufReader::new(stream.try_clone()?))?
                        .ok_or_else(|| unavailable("missing request"))?;
                    let requested = request["database"]
                        .as_str()
                        .map(Path::new)
                        .and_then(|p| p.canonicalize().ok());
                    if requested.as_ref() != Some(&database) {
                        return Err(
                            unavailable("relay belongs to a different issue database").into()
                        );
                    }
                    if request["request"]["kind"] == "capabilities" {
                        let message = advertisement.lock().unwrap();
                        let flags = json!({
                            "authority_rpc": message["capabilities"]["authority_rpc"] == true,
                            "issue_numbers": message["capabilities"]["issue_numbers"] == true,
                            "issue_metadata": message["capabilities"]["issue_metadata"] == true,
                            "issue_pr_attachments": message["capabilities"]["issue_pr_attachments"] == true,
                            "issue_request_status": message["capabilities"]["issue_request_status"] == true,
                            "issue_draft": message["capabilities"]["issue_draft"] == true,
                            "issue_reopen": message["capabilities"]["issue_reopen"] == true,
                            "issue_close": message["capabilities"]["issue_close"] == true,
                            "issue_dependencies": message["capabilities"]["issue_dependencies"] == true,
                            "issue_ready": message["capabilities"]["issue_ready"] == true,
                            "issue_ready_keep_draft": message["capabilities"]["issue_ready_keep_draft"] == true,
                            "issue_assignment": message["capabilities"]["issue_assignment"] == true,
                            "issue_reviewed_github_handoff": message["capabilities"]["issue_reviewed_github_handoff"] == true,
                            "issue_github_refresh": message["capabilities"]["issue_github_refresh"] == true,
                            "issue_archives": message["capabilities"]["issue_archives"] == true,
                        });
                        return Ok(capability_report(
                            "supervisor_tunnel",
                            flags,
                            message["build"].clone(),
                        ));
                    }
                    if request["request"]["kind"] == "issue_metadata" {
                        let metadata: Request =
                            serde_json::from_value(request["request"]["request"].clone())?;
                        crate::issues::authority::validate(&metadata)?;
                        let message = advertisement.lock().unwrap();
                        for capability in ["authority_rpc", "issue_metadata"]
                            .into_iter()
                            .chain(matches!(metadata.operation, crate::issues::Operation::RequestStatus { .. }).then_some("issue_request_status"))
                            .chain(matches!(metadata.operation, crate::issues::Operation::AddPullRequest { .. } | crate::issues::Operation::PullRequests { .. }).then_some("issue_pr_attachments"))
                            .chain(
                                matches!(
                                    metadata.operation,
                                    crate::issues::Operation::Edit {
                                        draft: Some(true),
                                        ..
                                    }
                                )
                                .then_some("issue_draft"),
                            )
                            .chain(
                                matches!(
                                    metadata.operation,
                                    crate::issues::Operation::Reopen { .. }
                                )
                                .then_some("issue_reopen"),
                            )
                            .chain(matches!(metadata.operation, crate::issues::Operation::Close { .. }).then_some("issue_close"))
                            .chain(matches!(metadata.operation, crate::issues::Operation::SetBlockers { .. }).then_some("issue_dependencies"))
                            .chain(
                                matches!(
                                    metadata.operation,
                                    crate::issues::Operation::Ready { .. }
                                )
                                .then_some("issue_ready"),
                            )
                            .chain(
                                matches!(metadata.operation, crate::issues::Operation::Ready { keep_draft: true, .. })
                                    .then_some("issue_ready_keep_draft"),
                            )
                            .chain(matches!(metadata.operation,crate::issues::Operation::Assign{..}).then_some("issue_assignment"))
                            .chain(matches!(metadata.operation,crate::issues::Operation::Assign{reviewed_evidence:Some(_),..}).then_some("issue_reviewed_github_handoff"))
                            .chain(matches!(metadata.operation,crate::issues::Operation::RefreshGithub{..}).then_some("issue_github_refresh"))
                        {
                            if message["capabilities"][capability] != true {
                                return Err(unsupported(capability, &message["build"]).into());
                            }
                        }
                    }
                    if !ready.load(Ordering::Acquire) {
                        return Err(unavailable(
                            "supervisor has not advertised authoritative routing support",
                        )
                        .into());
                    }
                    if !matches!(
                        request["request"]["kind"].as_str(),
                        Some(
                            "resource" | "status" | "overview" | "issue_numbers" | "issue_metadata" | "configuration" | "worker_signal" | "chief_run" | "issue_archive"
                        )
                    ) {
                        return Err(Error::invalid("Unsupported authority request").into());
                    }
                    if request["request"]["kind"]=="issue_archive" {
                        let message=advertisement.lock().unwrap();
                        if message["capabilities"]["issue_archives"]!=true {
                            return Err(unsupported("issue_archives",&message["build"]).into());
                        }
                    }
                    if request["request"]["kind"] == "issue_numbers"
                        && !numbers_ready.load(Ordering::Acquire)
                    {
                        return Err(unavailable(
                            "supervisor needs an upgrade for on-demand issue numbers",
                        )
                        .into());
                    }
                    serial += 1;
                    let id = format!("{}-{serial}", std::process::id());
                    send(
                        &mut *output.lock().unwrap(),
                        json!({"kind":"authority_request","id":id,"request":request["request"]}),
                    )?;
                    let deadline = Instant::now() + WAIT;
                    while Instant::now() < deadline && !stop.load(Ordering::Acquire) {
                        match incoming.recv_timeout(Duration::from_millis(50)) {
                            Ok(reply) if reply["id"] == id => return Ok(reply["result"].clone()),
                            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(_) => break,
                        }
                    }
                    Err(unavailable("supervisor did not acknowledge the request").into())
                })()
                .unwrap_or_else(|e| {
                    failure(
                        e.downcast_ref::<Error>()
                            .cloned()
                            .unwrap_or_else(|| unavailable(e)),
                    )
                });
                // This is an issue result, whose `version` is the map revision,
                // not a fleet protocol frame. Preserve it byte-for-byte.
                if let Ok(Some(bytes)) = encode_frame(&result, crate::issues::WIRE_LIMIT - 1) {
                    let _ = stream
                        .write_all(&bytes)
                        .and_then(|_| stream.write_all(b"\n"));
                }
            }
        });
        Ok(Self {
            supported,
            numbers_supported,
            advertised,
            stopped,
            replies,
            thread: Some(thread),
            socket,
            _lock: lock,
        })
    }
    pub fn configure(&self, message: &Value) {
        *self.advertised.lock().unwrap() =
            json!({"capabilities":message["capabilities"],"build":message["build"]});
        self.numbers_supported.store(
            message["capabilities"]["issue_numbers"] == true,
            Ordering::Release,
        );
        self.supported.store(
            message["capabilities"]["authority_rpc"] == true,
            Ordering::Release,
        );
    }
    pub fn replies(&self) -> mpsc::SyncSender<Value> {
        self.replies.clone()
    }
    #[cfg(test)]
    pub fn receive(&self, message: Value) {
        let _ = self.replies.try_send(message);
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.socket);
    }
}

pub(super) fn response(id: &Value, result: Value) -> Result<Value> {
    let frame = json!({"kind":"authority_reply","id":id,"result":result});
    // Check the complete envelope before writing; an oversized map/status must
    // fail this request without tearing down replication or emitting a prefix.
    if encode_frame(&frame, crate::issues::WIRE_LIMIT - 64)?.is_some() {
        Ok(frame)
    } else {
        Ok(
            json!({"kind":"authority_reply","id":id,"result":failure(unavailable("authoritative response exceeds 16 MiB; narrow the requested map"))}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context() -> (std::path::PathBuf, Context, crate::issues::Store) {
        let root = std::path::PathBuf::from("/tmp").join(format!(
            "hb-authority-{}",
            super::super::context::id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let ctx = Context {
            home: root.clone(),
            state: root.clone(),
            desired: root.join("fleet.json"),
            binary: std::env::current_exe().unwrap(),
            path: root.join("issues.db"),
            node: "authority-test".into(),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let store = crate::issues::Store::open(&ctx.path).unwrap();
        (root, ctx, store)
    }

    #[test]
    fn legacy_relay_rejection_explains_required_fleet_upgrade() {
        let (root, ctx, store) = test_context();
        let listener = UnixListener::bind(ctx.state.join(SOCKET)).unwrap();
        let server = thread::spawn(move || {
            for _ in 0..300 {
                let (mut stream, _) = listener.accept().unwrap();
                read_frame(&mut BufReader::new(stream.try_clone().unwrap())).unwrap();
                send(
                    &mut stream,
                    failure(Error::invalid("Unsupported authority request")),
                )
                .unwrap();
            }
        });
        for kind in ["capabilities", "issue_metadata", "resource"]
            .into_iter()
            .cycle()
            .take(300)
        {
            let error = call(&ctx.state, &ctx.path, json!({"kind":kind})).unwrap_err();
            if kind == "resource" {
                assert_eq!(error.code, "invalid_input");
                assert_eq!(error.message, "Unsupported authority request");
            } else {
                assert_eq!(error.code, "fleet_capability_unsupported", "{error:?}");
                assert!(error.message.contains("hey-boss upgrade"));
                assert!(error.message.contains("companion"));
                assert!(error.message.contains("supervisor"));
                assert_eq!(error.details.unwrap()["route"], "supervisor_tunnel");
            }
        }
        server.join().unwrap();
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_supervisor_and_wrong_database_fail_without_sending_a_request() {
        let (root, ctx, store) = test_context();
        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        let relay = Relay::start(&ctx, output.clone()).unwrap();
        let error = call(&ctx.state, &ctx.path, json!({"kind":"status"})).unwrap_err();
        assert_eq!(error.code, "fleet_unavailable");
        assert!(error.message.contains("has not advertised"));
        relay.configure(&json!({"capabilities":{"authority_rpc":true}}));
        let error = call(&ctx.state, &ctx.path, json!({"kind":"issue_numbers"})).unwrap_err();
        assert_eq!(error.code, "fleet_unavailable");
        assert!(error.message.contains("needs an upgrade"));
        let read = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"operation":{"action":"view","number":1}}});
        let error = call(&ctx.state, &ctx.path, read).unwrap_err();
        assert_eq!(error.code, "fleet_capability_unsupported");
        assert_eq!(
            error.details.unwrap()["required_capability"],
            "issue_metadata"
        );
        assert!(error.message.contains("hey-boss upgrade"));
        let capabilities = call(&ctx.state, &ctx.path, json!({"kind":"capabilities"})).unwrap();
        assert_eq!(capabilities["route"], "supervisor_tunnel");
        assert_eq!(capabilities["capabilities"]["issue_metadata"], false);
        relay.configure(&json!({"build":"old-metadata-build","capabilities":{"authority_rpc":true,"issue_metadata":true}}));
        for operation in [
            json!({"action":"add_pull_request","number":1,"url":"https://github.com/example/repo/pull/123","purpose":"prerequisite"}),
            json!({"action":"pull_requests","number":1}),
        ] {
            let request = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"request_id":"pr-old","operation":operation}});
            let error = call(&ctx.state, &ctx.path, request).unwrap_err();
            assert_eq!(error.code, "fleet_capability_unsupported");
            let details = error.details.unwrap();
            assert_eq!(details["required_capability"], "issue_pr_attachments");
            assert_eq!(details["supervisor_build"], "old-metadata-build");
            assert_eq!(details["sent"], false);
        }
        let refresh = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"request_id":"refresh-old","operation":{"action":"refresh_github","number":1}}});
        let error = call(&ctx.state, &ctx.path, refresh).unwrap_err();
        assert_eq!(error.code, "fleet_capability_unsupported");
        assert_eq!(
            error.details.as_ref().unwrap()["required_capability"],
            "issue_github_refresh"
        );
        assert_eq!(error.details.unwrap()["sent"], false);
        relay.configure(&json!({"build":"old-handoff-build","capabilities":{"authority_rpc":true,"issue_metadata":true,"issue_assignment":true}}));
        let handoff = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"request_id":"handoff-old","operation":{"action":"assign","number":1,"target":"github","if_version":1,"reviewed_evidence":[]}}});
        let error = call(&ctx.state, &ctx.path, handoff).unwrap_err();
        assert_eq!(error.code, "fleet_capability_unsupported");
        assert_eq!(
            error.details.as_ref().unwrap()["required_capability"],
            "issue_reviewed_github_handoff"
        );
        assert_eq!(error.details.unwrap()["sent"], false);
        relay.configure(&json!({"build":"old-metadata-build","capabilities":{"authority_rpc":true,"issue_metadata":true}}));
        let receipt = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"operation":{"action":"request_status","id":"original"}}});
        let error = call(&ctx.state, &ctx.path, receipt).unwrap_err();
        assert_eq!(error.code, "fleet_capability_unsupported");
        assert_eq!(
            error.details.as_ref().unwrap()["required_capability"],
            "issue_request_status"
        );
        assert_eq!(error.details.unwrap()["sent"], false);
        let draft = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"request_id":"draft-old","operation":{"action":"edit","number":1,"draft":true,"if_version":1,"add_labels":[],"remove_labels":[]}}});
        let error = call(&ctx.state, &ctx.path, draft).unwrap_err();
        assert_eq!(error.code, "fleet_capability_unsupported");
        let details = error.details.unwrap();
        assert_eq!(details["required_capability"], "issue_draft");
        assert_eq!(details["supervisor_build"], "old-metadata-build");
        assert_eq!(details["sent"], false);
        let assignment = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"request_id":"assignment-old","operation":{"action":"assign","number":1,"target":"github","if_version":1}}});
        let error = call(&ctx.state, &ctx.path, assignment).unwrap_err();
        assert_eq!(error.code, "fleet_capability_unsupported");
        assert_eq!(
            error.details.as_ref().unwrap()["required_capability"],
            "issue_assignment"
        );
        assert_eq!(error.details.unwrap()["sent"], false);
        let ready = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"request_id":"ready-old","operation":{"action":"ready","number":1,"force":true,"guard":{"if_version":1,"expected_assignee":null,"expected_reservation":"snapshot"}}}});
        let error = call(&ctx.state, &ctx.path, ready).unwrap_err();
        assert_eq!(error.code, "fleet_capability_unsupported");
        assert_eq!(
            error.details.as_ref().unwrap()["required_capability"],
            "issue_ready"
        );
        assert_eq!(error.details.unwrap()["sent"], false);
        relay.configure(&json!({"build":"old-ready-build","capabilities":{"authority_rpc":true,"issue_metadata":true,"issue_ready":true}}));
        let close = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"request_id":"close-old","operation":{"action":"close","number":1,"force":false,"comment":null,"guard":{"if_version":1,"expected_assignee":null,"expected_reservation":"snapshot"}}}});
        let error = call(&ctx.state, &ctx.path, close).unwrap_err();
        assert_eq!(error.code, "fleet_capability_unsupported");
        assert_eq!(
            error.details.as_ref().unwrap()["required_capability"],
            "issue_close"
        );
        assert_eq!(error.details.unwrap()["sent"], false);
        let draft_ready = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"request_id":"draft-ready-old","operation":{"action":"ready","number":1,"force":false,"keep_draft":true,"guard":{"if_version":1,"expected_assignee":null,"expected_reservation":"snapshot"}}}});
        let error = call(&ctx.state, &ctx.path, draft_ready).unwrap_err();
        assert_eq!(error.code, "fleet_capability_unsupported");
        assert_eq!(
            error.details.as_ref().unwrap()["required_capability"],
            "issue_ready_keep_draft"
        );
        assert_eq!(error.details.unwrap()["sent"], false);
        let reopen = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"request_id":"reopen-old","operation":{"action":"reopen","number":1,"if_version":1}}});
        let error = call(&ctx.state, &ctx.path, reopen).unwrap_err();
        assert_eq!(error.code, "fleet_capability_unsupported");
        let details = error.details.unwrap();
        assert_eq!(details["required_capability"], "issue_reopen");
        assert_eq!(details["sent"], false);
        let dependencies = json!({"kind":"issue_metadata","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"request_id":"dependencies-old","operation":{"action":"set_blockers","number":1,"blockers":[],"if_version":1}}});
        let error = call(&ctx.state, &ctx.path, dependencies).unwrap_err();
        assert_eq!(error.code, "fleet_capability_unsupported");
        let details = error.details.unwrap();
        assert_eq!(details["required_capability"], "issue_dependencies");
        assert_eq!(details["sent"], false);
        assert_eq!(
            call(&ctx.state, &ctx.path, json!({"kind":"capabilities"})).unwrap()["capabilities"]["issue_dependencies"],
            false
        );
        let error = call(
            &ctx.state,
            &root.join("unrelated.db"),
            json!({"kind":"status"}),
        )
        .unwrap_err();
        assert!(error.message.contains("different issue database"));
        let error = call(&ctx.state, &ctx.path, json!({"kind":"signal"})).unwrap_err();
        assert_eq!(error.code, "invalid_input");
        assert!(output.lock().unwrap().is_empty());
        let socket = ctx.state.join(SOCKET);
        assert_eq!(
            fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(relay);
        assert!(!socket.exists());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn relay_correlates_replies_and_preserves_the_authoritative_revision() {
        let (root, ctx, store) = test_context();
        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        let relay = Relay::start(&ctx, output.clone()).unwrap();
        relay.configure(&json!({"capabilities":{"authority_rpc":true}}));
        let client_ctx = ctx.clone();
        let client = thread::spawn(move || {
            call(
                &client_ctx.state,
                &client_ctx.path,
                json!({"kind":"resource","request":{"request_id":"stable-id"}}),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let request = loop {
            if let Ok(value) = serde_json::from_slice::<Value>(&output.lock().unwrap()) {
                break value;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(request["request"]["request"]["request_id"], "stable-id");
        relay.receive(json!({"id":"expired-request","result":{"ok":true,"version":1}}));
        let expected = json!({"ok":true,"version":17,"nodes":[]});
        relay.receive(json!({"id":request["id"],"result":expected}));
        assert_eq!(client.join().unwrap().unwrap(), expected);
        drop(relay);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn disconnect_fails_an_inflight_request_and_cleans_up_its_socket() {
        let (root, ctx, store) = test_context();
        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        let relay = Relay::start(&ctx, output.clone()).unwrap();
        relay.configure(&json!({"capabilities":{"authority_rpc":true}}));
        let client_ctx = ctx.clone();
        let client = thread::spawn(move || {
            call(
                &client_ctx.state,
                &client_ctx.path,
                json!({"kind":"status"}),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while output.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        drop(relay);
        let error = client.join().unwrap().unwrap_err();
        assert_eq!(error.code, "fleet_unavailable");
        assert!(error.message.contains("No local fallback"));
        assert!(!ctx.state.join(SOCKET).exists());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_authority_response_is_a_complete_error_frame() {
        let result = response(
            &json!("request-1"),
            json!({"ok":true,"data":"x".repeat(crate::issues::WIRE_LIMIT)}),
        )
        .unwrap();
        assert_eq!(result["id"], "request-1");
        assert_eq!(result["result"]["error"]["code"], "fleet_unavailable");
        let mut bytes = vec![];
        send(&mut bytes, result).unwrap();
        assert!(bytes.len() < 1024);
        assert_eq!(
            read_frame(&mut std::io::Cursor::new(bytes))
                .unwrap()
                .unwrap()["version"],
            1
        );
    }
}
