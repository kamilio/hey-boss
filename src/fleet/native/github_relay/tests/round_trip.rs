use super::*;
use hey_gh::ApiClient;
use std::sync::atomic::AtomicBool;

#[test]
fn companion_and_supervisor_round_trip_preserves_repo_scope_and_api_errors() {
    let mut f = Fixture::new();
    f.ready();
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client =
        ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
    let identity = Identity {
        hostname: "github.com".into(),
        user_id: 42,
        instance: "a".repeat(32),
    };
    let primary_identity = identity.clone();
    let primary = std::thread::spawn(move || {
        let mut reads = Vec::new();
        // One probe and two data reads, each fenced by before/after identities.
        for _ in 0..7 {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .expect("primary API request");
            if request.url() == "/v1/identity" {
                request
                    .respond(tiny_http::Response::from_string(
                        serde_json::to_string(&primary_identity).unwrap(),
                    ))
                    .unwrap();
            } else {
                reads.push(request.url().to_owned());
                let budget = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("x-hey-gh-read-timeout-ms"))
                    .unwrap()
                    .value
                    .as_str()
                    .parse::<u64>()
                    .unwrap();
                assert!((1..=5000).contains(&budget));
                if request.url().contains("poe-internal/poe2") {
                    request.respond(tiny_http::Response::from_string(json!({"data":{"number":7,"state":"open"},"source":"cache","validated_at_ms":123}).to_string())).unwrap();
                } else {
                    request
                        .respond(
                            tiny_http::Response::from_string(
                                json!({"code":"rate_limited","error":"synthetic quota"})
                                    .to_string(),
                            )
                            .with_status_code(503)
                            .with_header(
                                tiny_http::Header::from_bytes("Retry-After", "19").unwrap(),
                            ),
                        )
                        .unwrap();
                }
            }
        }
        reads
    });
    let (_, empty) = mpsc::channel();
    let frames = std::mem::replace(&mut f.frames, empty);
    let replies = f.replies.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let supervisor = std::thread::spawn(move || {
        let backend = super::super::super::github_reads::Backend::new(client).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while !done.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            if let Ok(frame) = frames.recv_timeout(Duration::from_millis(2))
                && let Some(reply) = backend.receive(&frame).unwrap()
            {
                replies.receive(&reply).unwrap();
            }
            for completed in backend.drain() {
                replies
                    .receive(
                        &super::super::super::github_reads::frame(
                            &completed.id,
                            &completed.response,
                        )
                        .unwrap(),
                    )
                    .unwrap();
            }
        }
    });
    let mut socket = f.request_with(&Request::Probe {
        identity: identity.clone(),
    });
    assert!(
        matches!(f.response(&mut socket),Response::Identity {identity:found} if found==identity)
    );
    for (repository, success) in [
        ("poe-internal/poe2", true),
        ("poe-platform/poe-code", false),
    ] {
        let mut socket = f.request_with(&Request::Read {
            read: Read {
                identity: identity.clone(),
                path: format!("/v1/prs/{repository}/7/metadata"),
                query: Some("cached_only=true&background=true".into()),
                timeout_ms: 5000,
            },
        });
        let Response::Reply { reply } = f.response(&mut socket) else {
            panic!("primary reply")
        };
        if success {
            assert_eq!(reply.decode::<Value>().unwrap()["data"]["number"], 7);
        } else {
            assert!(matches!(
                reply.decode::<Value>(),
                Err(hey_gh::Error::RateLimited {
                    retry_after_seconds: 19
                })
            ));
        }
    }
    stop.store(true, Ordering::Release);
    supervisor.join().unwrap();
    assert_eq!(
        primary.join().unwrap(),
        vec![
            "/v1/prs/poe-internal/poe2/7/metadata?cached_only=true&background=true",
            "/v1/prs/poe-platform/poe-code/7/metadata?cached_only=true&background=true"
        ]
    );
}
