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
    let envelope = json!({"database":database,"request":request});
    send(&mut stream, envelope).map_err(unavailable)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let result = read_frame(&mut BufReader::new(stream))
        .map_err(unavailable)?
        .ok_or_else(|| unavailable("connection closed before acknowledgment"))?;
    if result["ok"] == false {
        return Err(serde_json::from_value(result["error"].clone())?);
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
    json!({"authority_rpc":true,"issue_numbers":true,"issue_metadata":true})
}

pub(super) struct Relay {
    supported: Arc<AtomicBool>,
    numbers_supported: Arc<AtomicBool>,
    metadata_supported: Arc<AtomicBool>,
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
        let metadata_supported = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let (replies, incoming) = mpsc::sync_channel::<Value>(2);
        let ready = supported.clone();
        let numbers_ready = numbers_supported.clone();
        let metadata_ready = metadata_supported.clone();
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
                    if !ready.load(Ordering::Acquire) {
                        return Err(unavailable(
                            "supervisor has not advertised authoritative routing support",
                        )
                        .into());
                    }
                    if !matches!(
                        request["request"]["kind"].as_str(),
                        Some(
                            "resource" | "status" | "overview" | "issue_numbers" | "issue_metadata"
                        )
                    ) {
                        return Err(Error::invalid("Unsupported authority request").into());
                    }
                    if request["request"]["kind"] == "issue_numbers"
                        && !numbers_ready.load(Ordering::Acquire)
                    {
                        return Err(unavailable(
                            "supervisor needs an upgrade for on-demand issue numbers",
                        )
                        .into());
                    }
                    if request["request"]["kind"] == "issue_metadata" {
                        if !metadata_ready.load(Ordering::Acquire) {
                            return Err(unavailable(
                                "supervisor needs an upgrade for guarded issue metadata",
                            )
                            .into());
                        }
                        let metadata: Request =
                            serde_json::from_value(request["request"]["request"].clone())?;
                        crate::issues::authority::validate(&metadata)?;
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
            metadata_supported,
            stopped,
            replies,
            thread: Some(thread),
            socket,
            _lock: lock,
        })
    }
    pub fn configure(&self, message: &Value) {
        self.metadata_supported.store(
            message["capabilities"]["issue_metadata"] == true,
            Ordering::Release,
        );
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
        let error = call(&ctx.state, &ctx.path, json!({"kind":"issue_metadata"})).unwrap_err();
        assert_eq!(error.code, "fleet_unavailable");
        assert!(error.message.contains("needs an upgrade"));
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
