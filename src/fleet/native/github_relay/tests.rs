use super::*;
use hey_gh::shared_read::{
    Identity, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, Read, Request, Response, wire,
};
use serde_json::json;
use std::{sync::mpsc, time::Duration};
use tokio::{net::UnixStream, runtime::Runtime};

struct Recording {
    bytes: Vec<u8>,
    frames: mpsc::Sender<Value>,
    hold: Option<mpsc::Receiver<()>>,
}
impl Write for Recording {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let message = serde_json::from_slice(&self.bytes)?;
        self.bytes.clear();
        self.frames.send(message).map_err(std::io::Error::other)?;
        if let Some(hold) = self.hold.take() {
            hold.recv().map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}
struct Fixture {
    relay: Option<Relay>,
    replies: Replies,
    frames: mpsc::Receiver<Value>,
    root: PathBuf,
    runtime: Runtime,
    output: Arc<Mutex<Recording>>,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(format!(
            "/tmp/hgr-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (frames, received) = mpsc::channel();
        let output = Arc::new(Mutex::new(Recording {
            bytes: vec![],
            frames,
            hold: None,
        }));
        let relay = Relay::start_at(root.join("socket"), output.clone()).unwrap();
        let replies = relay.replies();
        Self {
            relay: Some(relay),
            replies,
            frames: received,
            root,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
            output,
        }
    }
    fn ready(&self) {
        self.replies
            .configure(&json!({"capabilities":{"github_reads_v1":true}}));
    }
    fn request(&self) -> UnixStream {
        self.request_with(&read())
    }
    fn request_with(&self, request: &Request) -> UnixStream {
        self.runtime.block_on(async {
            let mut socket = UnixStream::connect(self.root.join("socket")).await.unwrap();
            wire::write(&mut socket, request, MAX_REQUEST_BYTES)
                .await
                .unwrap();
            socket
        })
    }
    fn frame(&self) -> Value {
        self.frames
            .recv_timeout(Duration::from_secs(3))
            .expect("fleet request/cancel")
    }
    fn answer(&self, id: &str, value: Value) {
        self.replies.receive(&json!({"kind":"github_reply","id":id,"response":{"kind":"reply","reply":{"status":200,"retry_after_seconds":null,"body":value}}})).unwrap();
    }
    fn response(&self, socket: &mut UnixStream) -> Response {
        self.runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(3),
                wire::read(socket, MAX_RESPONSE_BYTES),
            )
            .await
            .unwrap()
            .unwrap()
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.relay.take());
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn read() -> Request {
    Request::Read {
        read: Read {
            identity: Identity {
                hostname: "github.com".into(),
                user_id: 42,
                instance: "a".repeat(32),
            },
            path: "/v1/pr-status".into(),
            query: Some("cached_only=true&limit=1".into()),
            timeout_ms: 5000,
        },
    }
}
fn body(response: Response) -> Value {
    match response {
        Response::Reply { reply } => reply.decode().unwrap(),
        _ => panic!("expected API reply"),
    }
}

#[test]
fn independent_replies_do_not_require_a_control_handler_and_preserve_request_identity() {
    let f = Fixture::new();
    let mut unsupported = f.request();
    assert!(matches!(
        f.response(&mut unsupported),
        Response::Unavailable
    ));
    assert!(f.frames.try_recv().is_err());
    f.ready();
    let mut one = f.request();
    let first = f.frame();
    let mut two = f.request();
    let second = f.frame();
    assert_eq!(first["kind"], "github_read");
    assert_eq!(first["id"], "1");
    assert_eq!(second["id"], "2");
    assert_eq!(
        first["request"]["read"]["query"],
        "cached_only=true&limit=1"
    );
    f.answer("2", json!("second"));
    assert_eq!(body(f.response(&mut two)), "second");
    f.answer("1", json!("first"));
    assert_eq!(body(f.response(&mut one)), "first");
}

#[test]
fn cancel_releases_capacity_and_late_replies_cannot_alias_new_requests() {
    let f = Fixture::new();
    f.ready();
    let mut held = Vec::new();
    for n in 1..=4 {
        held.push(f.request());
        assert_eq!(f.frame()["id"], n.to_string());
    }
    let mut excess = f.request();
    assert!(matches!(f.response(&mut excess), Response::Unavailable));
    assert!(f.frames.try_recv().is_err());
    drop(held.remove(0));
    let cancelled = f.frame();
    assert_eq!(cancelled["kind"], "github_cancel");
    assert_eq!(cancelled["id"], "1");
    f.answer("1", json!("late"));
    let mut next = f.request();
    let sent = f.frame();
    assert_eq!(sent["id"], "5");
    f.answer("5", json!("current"));
    assert_eq!(body(f.response(&mut next)), "current");
}

#[test]
fn disconnect_releases_waiters_and_stops_new_forwarding() {
    let f = Fixture::new();
    f.ready();
    let mut waiting = f.request();
    f.frame();
    f.replies.close();
    assert!(matches!(f.response(&mut waiting), Response::Unavailable));
    let mut next = f.request();
    assert!(matches!(f.response(&mut next), Response::Unavailable));
    assert!(f.frames.try_recv().is_err());
}

#[test]
fn socket_is_private_and_a_second_owner_cannot_replace_it() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let mut f = Fixture::new();
    let path = f.root.join("socket");
    let before = std::fs::metadata(&path).unwrap();
    assert_eq!(before.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        std::fs::metadata(&f.root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(Relay::start_at(path.clone(), Arc::new(Mutex::new(Vec::<u8>::new()))).is_err());
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), before.ino());
    drop(f.relay.take());
    assert!(!path.exists());
}

#[test]
fn delivered_remote_replies_keep_capacity_until_the_socket_writer_finishes() {
    let f = Fixture::new();
    f.ready();
    let mut held = Vec::new();
    for n in 1..=4 {
        held.push(f.request());
        assert_eq!(f.frame()["id"], n.to_string());
    }
    // Each reply exceeds the socket send buffer; no SDK reader is draining yet.
    for n in 1..=4 {
        f.answer(&n.to_string(), json!("x".repeat(1024 * 1024)));
    }
    let mut excess = f.request();
    assert!(matches!(f.response(&mut excess), Response::Unavailable));
    assert!(f.frames.try_recv().is_err());
    assert_eq!(
        body(f.response(&mut held[0])).as_str().unwrap().len(),
        1024 * 1024
    );
}

#[test]
fn expired_requests_cancel_remote_work_and_keep_the_deadline_error() {
    let f = Fixture::new();
    f.ready();
    let mut request = read();
    let Request::Read { read } = &mut request else {
        unreachable!()
    };
    read.timeout_ms = 150;
    let mut socket = f.request_with(&request);
    let sent = f.frame();
    assert!((1..=150).contains(&sent["request"]["read"]["timeout_ms"].as_u64().unwrap()));
    let Response::Reply { reply } = f.response(&mut socket) else {
        panic!("deadline reply")
    };
    assert!(matches!(
        reply.decode::<Value>(),
        Err(hey_gh::Error::Deadline)
    ));
    let cancel = f.frame();
    assert_eq!(cancel["kind"], "github_cancel");
    assert_eq!(cancel["id"], sent["id"]);
}

#[test]
fn unsupported_local_source_feeds_never_reach_the_supervisor() {
    let f = Fixture::new();
    f.ready();
    let mut request = read();
    let Request::Read { read } = &mut request else {
        unreachable!()
    };
    read.path = "/v1/snapshot".into();
    let mut socket = f.request_with(&request);
    let Response::Reply { reply } = f.response(&mut socket) else {
        panic!("invalid route reply")
    };
    assert!(matches!(
        reply.decode::<Value>(),
        Err(hey_gh::Error::Invalid(_))
    ));
    assert!(f.frames.try_recv().is_err());
}

#[test]
fn busy_control_output_does_not_block_availability_or_cancellation() {
    let f = Fixture::new();
    f.ready();
    {
        let _busy = f.output.lock().unwrap();
        let mut socket = f.request();
        assert!(matches!(f.response(&mut socket), Response::Unavailable));
    }
    let socket = f.request();
    let sent = f.frame();
    {
        let _busy = f.output.lock().unwrap();
        drop(socket);
        let mut next = f.request();
        assert!(matches!(f.response(&mut next), Response::Unavailable));
    }
    let canceled = f.frame();
    assert_eq!(canceled["kind"], "github_cancel");
    assert_eq!(canceled["id"], sent["id"]);
    assert!(f.frames.try_recv().is_err());
}

#[test]
fn blocked_pipe_write_cannot_hold_up_other_clients_or_the_original_deadline() {
    let f = Fixture::new();
    f.ready();
    let (release, hold) = mpsc::channel();
    f.output.lock().unwrap().hold = Some(hold);
    let mut request = read();
    let Request::Read { read } = &mut request else {
        unreachable!()
    };
    read.timeout_ms = 150;
    let mut socket = f.request_with(&request);
    let sent = f.frame(); // The writer now blocks inside flush, holding its mutex.
    let mut other = f.request();
    assert!(matches!(f.response(&mut other), Response::Unavailable));
    let Response::Reply { reply } = f.response(&mut socket) else {
        panic!("deadline reply")
    };
    assert!(matches!(
        reply.decode::<Value>(),
        Err(hey_gh::Error::Deadline)
    ));
    release.send(()).unwrap();
    let canceled = f.frame();
    assert_eq!(canceled["kind"], "github_cancel");
    assert_eq!(canceled["id"], sent["id"]);
}
