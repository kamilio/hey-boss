use super::*;
use hey_gh::shared_read::{Identity, Read};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

fn identity() -> Identity {
    Identity {
        hostname: "github.com".into(),
        user_id: 42,
        instance: "a".repeat(32),
    }
}
fn read() -> Request {
    Request::Read {
        read: Read {
            identity: identity(),
            path: "/v1/prs/o/r/1/metadata".into(),
            query: Some("cached_only=true".into()),
            timeout_ms: 5000,
        },
    }
}
fn backend(client: ApiClient) -> Backend {
    let mut backend = Backend::new(client).unwrap();
    backend.slots = Arc::new(tokio::sync::Semaphore::new(8));
    backend
}
struct Fixture {
    client: ApiClient,
    received: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let client =
            ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
        let (send, received) = mpsc::channel();
        let (release, ready) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut held: Vec<tiny_http::Request> = Vec::new();
            while !stopped.load(Ordering::Acquire) {
                if ready.try_recv().is_ok() {
                    for request in held.drain(..) {
                        let _ = request.respond(tiny_http::Response::from_string(
                            json!({"data":{"number":1},"validated_at_ms":123}).to_string(),
                        ));
                    }
                }
                let Some(request) = server.recv_timeout(Duration::from_millis(5)).unwrap() else {
                    continue;
                };
                if request.url() == "/v1/identity" {
                    let _ = request.respond(tiny_http::Response::from_string(
                        serde_json::to_string(&identity()).unwrap(),
                    ));
                } else {
                    assert_eq!(request.url(), "/v1/prs/o/r/1/metadata?cached_only=true");
                    held.push(request);
                    send.send(()).unwrap();
                }
            }
        });
        Self {
            client,
            received,
            release,
            stop,
            thread: Some(thread),
        }
    }
    fn admitted(&self, count: usize) {
        for _ in 0..count {
            self.received
                .recv_timeout(Duration::from_secs(5))
                .expect("shared data request");
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn replies(backend: &Backend, count: usize) -> Vec<(String, Response)> {
    let end = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    while output.len() < count && Instant::now() < end {
        output.extend(
            backend
                .drain()
                .into_iter()
                .map(|done| (done.id, done.response)),
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(output.len(), count);
    output
}

#[test]
fn slow_github_reads_overlap_without_blocking_control_and_keep_bounded_slots() {
    let f = Fixture::new();
    let backend = backend(f.client.clone());
    for id in ["1", "2", "3", "4"] {
        assert!(backend.submit(id, read()).unwrap().is_none());
    }
    f.admitted(4);
    let start = Instant::now();
    assert!(matches!(
        backend.submit("5", read()).unwrap(),
        Some(Response::Unavailable)
    ));
    assert!(backend.drain().is_empty());
    // The connection thread can continue writing control frames while all
    // GitHub HTTP requests are held. No backend call waits on those requests.
    let mut wire = Vec::new();
    super::super::context::send(&mut wire, json!({"kind":"ping"})).unwrap();
    assert!(start.elapsed() < Duration::from_millis(100));
    assert!(String::from_utf8(wire).unwrap().contains("ping"));
    f.release.send(()).unwrap();
    let output = replies(&backend, 4);
    assert!(output.iter().all(|(_,r)|matches!(r,Response::Reply{reply} if reply.status==200 && reply.body["validated_at_ms"]==123)));
    assert!(
        backend
            .submit(
                "6",
                Request::Probe {
                    identity: identity()
                }
            )
            .unwrap()
            .is_none()
    );
    assert!(
        matches!(&replies(&backend,1)[0].1,Response::Identity{identity:found} if found==&identity())
    );
}

#[test]
fn completed_read_wakes_the_owner_without_an_incoming_peer_frame() {
    let f = Fixture::new();
    let backend = backend(f.client.clone());
    assert!(backend.submit("1", read()).unwrap().is_none());
    f.admitted(1);
    let started = Instant::now();
    f.release.send(()).unwrap();
    // The connection thread has no further peer input while GitHub completes.
    // A wake before park must also be retained, avoiding a lost-wakeup race.
    std::thread::park_timeout(Duration::from_secs(2));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "completed response waited for the idle receive timeout"
    );
    let completed = backend.drain();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].id, "1");
}

#[test]
fn cancelling_one_request_keeps_peers_and_disconnect_drops_pending_work() {
    let f = Fixture::new();
    let backend = backend(f.client.clone());
    assert!(backend.submit("1", read()).unwrap().is_none());
    assert!(backend.submit("2", read()).unwrap().is_none());
    f.admitted(2);
    assert!(
        backend.submit("2", read()).is_err(),
        "duplicate IDs cannot steal a reply"
    );
    backend.cancel("1");
    f.release.send(()).unwrap();
    let output = replies(&backend, 1);
    assert_eq!(output[0].0, "2");
    assert!(backend.submit("3", read()).unwrap().is_none());
    f.admitted(1);
    let start = Instant::now();
    drop(backend);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "disconnect must not wait for GitHub"
    );
}

#[test]
fn cancelled_buffered_replies_keep_their_slots_and_ids_until_drained() {
    let f = Fixture::new();
    let backend = backend(f.client.clone());
    for id in ["1", "2", "3", "4"] {
        assert!(
            backend
                .submit(
                    id,
                    Request::Probe {
                        identity: identity()
                    }
                )
                .unwrap()
                .is_none()
        );
    }
    let end = Instant::now() + Duration::from_secs(5);
    while !backend
        .pending
        .lock()
        .unwrap()
        .values()
        .all(|task| task.is_finished())
    {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    for id in ["1", "2", "3", "4"] {
        backend.cancel(id);
    }
    assert!(
        backend.submit("1", read()).is_err(),
        "a canceled ID must never alias a new request"
    );
    assert!(
        matches!(
            backend.submit("5", read()).unwrap(),
            Some(Response::Unavailable)
        ),
        "buffered bodies still occupy slots"
    );
    assert!(
        backend.drain().is_empty(),
        "canceled replies stay suppressed"
    );
    assert!(
        backend
            .submit(
                "6",
                Request::Probe {
                    identity: identity()
                }
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(replies(&backend, 1)[0].0, "6");
}

#[test]
fn reply_memory_and_active_work_share_one_global_budget_across_connections() {
    let f = Fixture::new();
    let first = backend(f.client.clone());
    let mut second = backend(f.client.clone());
    second.slots = first.slots.clone();
    let mut third = backend(f.client.clone());
    third.slots = first.slots.clone();
    for id in ["1", "2", "3", "4"] {
        assert!(first.submit(id, read()).unwrap().is_none());
        assert!(second.submit(id, read()).unwrap().is_none());
    }
    f.admitted(8);
    assert!(matches!(
        third.submit("1", read()).unwrap(),
        Some(Response::Unavailable)
    ));
    f.release.send(()).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    let mut buffered = Vec::new();
    while buffered.len() < 8 {
        assert!(Instant::now() < end);
        buffered.extend(first.drain());
        buffered.extend(second.drain());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        matches!(
            third.submit("2", read()).unwrap(),
            Some(Response::Unavailable)
        ),
        "replies waiting for the wire still own permits"
    );
    drop(buffered);
    assert!(
        third
            .submit(
                "3",
                Request::Probe {
                    identity: identity()
                }
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(replies(&third, 1)[0].0, "3");
}

#[test]
fn fleet_envelopes_are_bounded_and_invalid_requests_never_reach_the_daemon() {
    let f = Fixture::new();
    let backend = backend(f.client.clone());
    let bad = json!({"kind":"github_read","id":"1","request":{"kind":"read","read":{
        "identity":identity(),"path":"/v1/releases/observe","query":null,"timeout_ms":5000
    }}});
    let reply = backend.receive(&bad).unwrap().unwrap();
    assert_eq!(reply["kind"], "github_reply");
    assert_eq!(reply["id"], "1");
    assert_eq!(reply["response"]["reply"]["body"]["code"], "invalid");
    assert!(f.received.try_recv().is_err());
    let huge = Response::Reply {
        reply: hey_gh::shared_read::Reply {
            status: 200,
            retry_after_seconds: None,
            body: json!("x".repeat(hey_gh::shared_read::MAX_RESPONSE_BYTES)),
        },
    };
    assert_eq!(frame("2", &huge).unwrap()["response"]["kind"], "too_large");
}
