use super::*;
use crate::shared_read::{self, Identity, Reply, Request, Response, wire};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::net::UnixListener;

fn identity(shared: bool) -> Identity {
    Identity {
        hostname: "github.com".into(),
        user_id: 42,
        instance: if shared { "b" } else { "a" }.repeat(32),
    }
}
struct Local {
    client: ApiClient,
    seen: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Local {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Local {
    async fn new(handler: impl Fn(&str) -> (u16, Value) + Send + Sync + 'static) -> Self {
        Self::delayed(handler, Duration::ZERO).await
    }
    async fn delayed(
        handler: impl Fn(&str) -> (u16, Value) + Send + Sync + 'static,
        delay: Duration,
    ) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = seen.clone();
        let handler = Arc::new(handler);
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let captured = captured.clone();
            let handler = handler.clone();
            async move {
                let url = request.uri().to_string();
                captured.lock().unwrap().push(url.clone());
                if url != "/v1/identity" {
                    tokio::time::sleep(delay).await;
                }
                let (status, body) = handler(&url);
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    axum::Json(body),
                )
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = ApiClient::new(
            format!("http://{}/", listener.local_addr().unwrap())
                .parse()
                .unwrap(),
        )
        .unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { client, seen, task }
    }
    async fn normal() -> Self {
        Self::new(|url| {
            (
                200,
                if url == "/v1/identity" {
                    serde_json::to_value(identity(false)).unwrap()
                } else {
                    json!({"from":"local","cursor":"native-local"})
                },
            )
        })
        .await
    }
}
struct Relay {
    _directory: tempfile::TempDir,
    path: std::path::PathBuf,
    seen: Arc<Mutex<Vec<Request>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Relay {
    fn new(handler: impl Fn(Request) -> Option<Response> + Send + Sync + 'static) -> Self {
        Self::asynchronous(move |request| std::future::ready(handler(request)))
    }
    fn asynchronous<F: std::future::Future<Output = Option<Response>> + Send + 'static>(
        handler: impl Fn(Request) -> F + Send + Sync + 'static,
    ) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("socket");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = seen.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let Ok(request) =
                    wire::read::<Request>(&mut socket, shared_read::MAX_REQUEST_BYTES).await
                else {
                    continue;
                };
                captured.lock().unwrap().push(request.clone());
                if let Some(response) = handler(request).await {
                    wire::write(&mut socket, &response, shared_read::MAX_RESPONSE_BYTES)
                        .await
                        .unwrap();
                }
            }
        });
        Self {
            _directory: directory,
            path,
            seen,
            task,
        }
    }
    fn replying(reply: Reply) -> Self {
        Self::new(move |request| {
            Some(match request {
                Request::Probe { .. } => Response::Identity {
                    identity: identity(true),
                },
                Request::Read { .. } => Response::Reply {
                    reply: reply.clone(),
                },
            })
        })
    }
}
fn reply(body: Value) -> Reply {
    Reply {
        status: 200,
        retry_after_seconds: None,
        body,
    }
}
async fn read(client: &ApiClient, path: &str) -> Result<Value> {
    client.read(client.http.get(client.url(path))).await
}

#[tokio::test]
async fn forwards_query_budget_and_fences_every_exposed_cursor() {
    let mut local = Local::normal().await;
    let relay = Relay::replying(reply(
        json!({"from":"primary","cursor":"native-primary","changes":[{"cursor":"change"}]}),
    ));
    local.client.relay_socket = Some(relay.path.clone());
    let client = local
        .client
        .clone()
        .with_read_deadline(tokio::time::Instant::now() + Duration::from_secs(3));
    let path = "v1/pr-status?cached_only=true&fields=number%2Cstate&repository=poe-internal%2Fpoe2&limit=7&wait_seconds=1&background=true&capture_only=true";
    let body = read(&client, path).await.unwrap();
    assert_eq!(body["from"], "primary");
    let source = cursor::Source::new(true, &identity(true));
    assert_eq!(
        source
            .unwrap(body["cursor"].as_str().unwrap(), true)
            .unwrap(),
        "native-primary"
    );
    assert_eq!(
        source
            .unwrap(body["changes"][0]["cursor"].as_str().unwrap(), true)
            .unwrap(),
        "change"
    );
    let seen = relay.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    let Request::Read { read } = &seen[1] else {
        panic!("read")
    };
    assert_eq!(read.path, "/v1/pr-status");
    assert_eq!(read.query.as_deref(), path.split_once('?').map(|(_, q)| q));
    assert!((1..=3000).contains(&read.timeout_ms));
    assert_eq!(read.identity, identity(true));
    assert_eq!(
        *local.seen.lock().unwrap(),
        vec!["/v1/identity", "/v1/identity"]
    );
}

#[tokio::test]
async fn primary_errors_are_not_hidden_by_local_fallback() {
    for error in [
        Error::Deadline,
        Error::CacheMiss,
        Error::RateLimited {
            retry_after_seconds: 19,
        },
        Error::GitHub {
            status: 403,
            message: "denied".into(),
        },
        Error::Transport("upstream unavailable".into()),
    ] {
        let mut local = Local::normal().await;
        let expected = error.to_string();
        let relay = Relay::replying(Reply::from(error));
        local.client.relay_socket = Some(relay.path.clone());
        assert_eq!(
            read(&local.client, "v1/prs/o/r/7/ci")
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
        assert!(
            local
                .seen
                .lock()
                .unwrap()
                .iter()
                .all(|p| p == "/v1/identity")
        );
    }
}

#[tokio::test]
async fn missing_relay_is_direct_without_identity_overhead() {
    let mut local = Local::normal().await;
    let dir = tempfile::tempdir().unwrap();
    local.client.relay_socket = Some(dir.path().join("missing"));
    assert_eq!(
        read(&local.client, "v1/pr-status?cursor=native")
            .await
            .unwrap()["cursor"],
        "native-local"
    );
    assert_eq!(
        *local.seen.lock().unwrap(),
        vec!["/v1/pr-status?cursor=native"]
    );
}

#[tokio::test]
async fn unavailable_or_disconnected_relay_falls_back_locally() {
    for disconnect in [false, true] {
        let mut local = Local::normal().await;
        let relay = Relay::new(move |request| match request {
            Request::Probe { .. } => Some(Response::Identity {
                identity: identity(true),
            }),
            Request::Read { .. } => {
                if disconnect {
                    None
                } else {
                    Some(Response::Unavailable)
                }
            }
        });
        local.client.relay_socket = Some(relay.path.clone());
        assert_eq!(
            read(&local.client, "v1/prs/o/r/7/ci").await.unwrap()["from"],
            "local"
        );
        assert_eq!(
            local
                .seen
                .lock()
                .unwrap()
                .iter()
                .filter(|p| p.starts_with("/v1/prs/"))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn shared_cursor_cannot_enter_a_local_feed_on_fallback() {
    let mut local = Local::normal().await;
    let relay = Relay::new(|request| {
        Some(match request {
            Request::Probe { .. } => Response::Identity {
                identity: identity(true),
            },
            Request::Read { .. } => Response::Unavailable,
        })
    });
    local.client.relay_socket = Some(relay.path.clone());
    let mut body = json!({"cursor":"native-primary"});
    cursor::Source::new(true, &identity(true))
        .wrap(&mut body)
        .unwrap();
    let path = format!("v1/pr-status?cursor={}", body["cursor"].as_str().unwrap());
    assert!(matches!(
        read(&local.client, &path).await,
        Err(Error::CursorExpired)
    ));
    assert!(
        local
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|p| p == "/v1/identity")
    );
    // Detailed reports can expose source cursors. Unsupported raw source feeds
    // must expire those primary cursors instead of sending them to this daemon.
    let path = path.replace("pr-status", "changes");
    assert!(matches!(
        read(&local.client, &path).await,
        Err(Error::CursorExpired)
    ));
}

#[tokio::test]
async fn local_cursor_continues_locally_even_when_a_primary_feed_is_available() {
    let mut local = Local::normal().await;
    let relay = Relay::replying(reply(json!({"from":"primary"})));
    local.client.relay_socket = Some(relay.path.clone());
    assert_eq!(
        read(&local.client, "v1/pr-status?cursor=native-local")
            .await
            .unwrap()["from"],
        "local"
    );
    assert!(relay.seen.lock().unwrap().is_empty());
    assert_eq!(
        *local.seen.lock().unwrap(),
        vec!["/v1/pr-status?cursor=native-local"]
    );
}

#[tokio::test]
async fn writes_and_local_watch_dependent_reads_never_use_the_relay() {
    let mut local = Local::normal().await;
    let relay = Relay::replying(reply(json!({"from":"primary"})));
    local.client.relay_socket = Some(relay.path.clone());
    for path in [
        "v1/snapshot",
        "v1/changes?cursor=native",
        "v1/repos/o/r",
        "v1/viewer",
    ] {
        assert_eq!(read(&local.client, path).await.unwrap()["from"], "local");
    }
    let body: Value = local
        .client
        .read(
            local
                .client
                .http
                .post(local.client.url("v1/releases/observe"))
                .json(&json!({})),
        )
        .await
        .unwrap();
    assert_eq!(body["from"], "local");
    assert!(relay.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn wrong_account_or_host_cannot_select_the_primary() {
    for primary in [
        Identity {
            user_id: 43,
            ..identity(true)
        },
        Identity {
            hostname: "enterprise.example.com".into(),
            ..identity(true)
        },
    ] {
        let mut local = Local::normal().await;
        let relay = Relay::new(move |_| {
            Some(Response::Identity {
                identity: primary.clone(),
            })
        });
        local.client.relay_socket = Some(relay.path.clone());
        assert_eq!(
            read(&local.client, "v1/pr-status").await.unwrap()["from"],
            "local"
        );
        assert_eq!(relay.seen.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn local_identity_change_discards_a_successful_primary_response() {
    for changed in [
        Identity {
            user_id: 43,
            ..identity(false)
        },
        Identity {
            instance: "c".repeat(32),
            ..identity(false)
        },
    ] {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let mut local = Local::new(move |url| {
            assert_eq!(url, "/v1/identity");
            let id = if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                identity(false)
            } else {
                changed.clone()
            };
            (200, serde_json::to_value(id).unwrap())
        })
        .await;
        let relay = Relay::replying(reply(json!({"from":"primary"})));
        local.client.relay_socket = Some(relay.path.clone());
        assert!(matches!(
            read(&local.client, "v1/pr-status").await,
            Err(Error::CursorExpired)
        ));
    }
}

#[tokio::test]
async fn primary_continuation_is_unwrapped_and_restart_expires_it_before_reading() {
    let mut local = Local::normal().await;
    let relay = Relay::replying(reply(json!({"cursor":"next"})));
    local.client.relay_socket = Some(relay.path.clone());
    let source = cursor::Source::new(true, &identity(true));
    let mut body = json!({"cursor":"native:opaque+/="});
    source.wrap(&mut body).unwrap();
    let mut request = local.client.url("v1/pr-status");
    request
        .query_pairs_mut()
        .append_pair("cursor", body["cursor"].as_str().unwrap())
        .append_pair("fields", "number,state");
    let response: Value = local
        .client
        .read(local.client.http.get(request.clone()))
        .await
        .unwrap();
    assert_eq!(
        source
            .unwrap(response["cursor"].as_str().unwrap(), true)
            .unwrap(),
        "next"
    );
    {
        let seen = relay.seen.lock().unwrap();
        let Request::Read { read } = &seen[1] else {
            panic!("read")
        };
        let query: std::collections::BTreeMap<_, _> =
            url::form_urlencoded::parse(read.query.as_ref().unwrap().as_bytes())
                .into_owned()
                .collect();
        assert_eq!(query["cursor"], "native:opaque+/=");
        assert_eq!(query["fields"], "number,state");
    }
    let restarted = Relay::new(|_| {
        Some(Response::Identity {
            identity: Identity {
                instance: "c".repeat(32),
                ..identity(true)
            },
        })
    });
    local.client.relay_socket = Some(restarted.path.clone());
    let result: Result<Value> = local.client.read(local.client.http.get(request)).await;
    assert!(matches!(result, Err(Error::CursorExpired)));
    assert_eq!(restarted.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn local_wrapped_continuation_validates_the_local_daemon() {
    let local = Local::normal().await;
    let source = cursor::Source::new(false, &identity(false));
    let mut body = json!({"cursor":"previous"});
    source.wrap(&mut body).unwrap();
    let body = read(
        &local.client,
        &format!("v1/pr-status?cursor={}", body["cursor"].as_str().unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(
        source
            .unwrap(body["cursor"].as_str().unwrap(), false)
            .unwrap(),
        "native-local"
    );
    assert_eq!(
        *local.seen.lock().unwrap(),
        vec![
            "/v1/identity",
            "/v1/pr-status?cursor=previous",
            "/v1/identity"
        ]
    );
}

#[tokio::test]
async fn wrapped_local_cursor_does_not_switch_to_an_available_primary() {
    let mut local = Local::normal().await;
    let relay = Relay::replying(reply(json!({"from":"primary"})));
    local.client.relay_socket = Some(relay.path.clone());
    let source = cursor::Source::new(false, &identity(false));
    let mut body = json!({"cursor":"previous"});
    source.wrap(&mut body).unwrap();
    let body = read(
        &local.client,
        &format!("v1/pr-status?cursor={}", body["cursor"].as_str().unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(body["from"], "local");
    assert_eq!(
        source
            .unwrap(body["cursor"].as_str().unwrap(), false)
            .unwrap(),
        "native-local"
    );
    assert!(relay.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_oversized_shared_continuation_can_rebootstrap_and_drain_locally() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let mut local = Local::normal().await;
    let oversized = Arc::new(AtomicBool::new(false));
    let state = oversized.clone();
    let relay = Relay::new(move |request| {
        Some(match request {
            Request::Probe { .. } => {
                if state.load(Ordering::Acquire) {
                    Response::Unavailable
                } else {
                    Response::Identity {
                        identity: identity(true),
                    }
                }
            }
            Request::Read { .. } => {
                state.store(true, Ordering::Release);
                Response::TooLarge
            }
        })
    });
    local.client.relay_socket = Some(relay.path.clone());
    let mut body = json!({"cursor":"previous-primary"});
    cursor::Source::new(true, &identity(true))
        .wrap(&mut body)
        .unwrap();
    assert!(matches!(
        read(
            &local.client,
            &format!("v1/pr-status?cursor={}", body["cursor"].as_str().unwrap())
        )
        .await,
        Err(Error::CursorExpired)
    ));
    // No per-SDK state may be needed for the next CLI process to recover.
    let mut fresh = ApiClient::new(local.client.base.clone()).unwrap();
    fresh.relay_socket = Some(relay.path.clone());
    let body = read(&fresh, "v1/pr-status").await.unwrap();
    assert_eq!(body["from"], "local");
    oversized.store(false, Ordering::Release);
    let count = relay.seen.lock().unwrap().len();
    let next = read(
        &fresh,
        &format!("v1/pr-status?cursor={}", body["cursor"].as_str().unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(next["from"], "local");
    assert_eq!(relay.seen.lock().unwrap().len(), count);
}

#[tokio::test]
async fn fallback_spends_only_the_original_remaining_budget() {
    let mut local = Local::delayed(
        |url| {
            (
                200,
                if url == "/v1/identity" {
                    serde_json::to_value(identity(false)).unwrap()
                } else {
                    json!({"from":"local"})
                },
            )
        },
        Duration::from_millis(200),
    )
    .await;
    let relay = Relay::asynchronous(|request| async move {
        Some(match request {
            Request::Probe { .. } => Response::Identity {
                identity: identity(true),
            },
            Request::Read { .. } => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Response::Unavailable
            }
        })
    });
    local.client.relay_socket = Some(relay.path.clone());
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    let client = local.client.clone().with_read_deadline(deadline);
    assert!(matches!(
        read(&client, "v1/prs/o/r/7/ci").await,
        Err(Error::Deadline)
    ));
    assert!(tokio::time::Instant::now() < deadline + Duration::from_millis(100));
    assert!(
        local
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|url| url == "/v1/prs/o/r/7/ci")
    );
}

#[tokio::test]
async fn a_stalled_probe_is_bounded_and_falls_back() {
    let mut local = Local::normal().await;
    let relay = Relay::asynchronous(|_| async { std::future::pending::<Option<Response>>().await });
    local.client.relay_socket = Some(relay.path.clone());
    let started = tokio::time::Instant::now();
    let client = local
        .client
        .clone()
        .with_read_deadline(started + Duration::from_secs(2));
    assert_eq!(
        read(&client, "v1/pr-status").await.unwrap()["from"],
        "local"
    );
    assert!(started.elapsed() < Duration::from_millis(1600));
}

#[tokio::test]
async fn cancellation_closes_the_active_read_socket() {
    use tokio::io::AsyncReadExt;
    let mut local = Local::normal().await;
    let relay = Relay::replying(reply(json!({})));
    relay.task.abort();
    std::fs::remove_file(&relay.path).unwrap();
    let listener = UnixListener::bind(&relay.path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&relay.path, std::fs::Permissions::from_mode(0o600)).unwrap();
    local.client.relay_socket = Some(relay.path.clone());
    let (accepted, ready) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut probe, _) = listener.accept().await.unwrap();
        let _: Request = wire::read(&mut probe, shared_read::MAX_REQUEST_BYTES)
            .await
            .unwrap();
        wire::write(
            &mut probe,
            &Response::Identity {
                identity: identity(true),
            },
            shared_read::MAX_RESPONSE_BYTES,
        )
        .await
        .unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        let _: Request = wire::read(&mut socket, shared_read::MAX_REQUEST_BYTES)
            .await
            .unwrap();
        accepted.send(()).unwrap();
        let mut byte = [0u8];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), socket.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    });
    let client = local.client.clone();
    let caller = tokio::spawn(async move { read(&client, "v1/pr-status").await });
    ready.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    server.await.unwrap();
}

#[tokio::test]
async fn oversized_relay_frame_falls_back_without_reading_its_body() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncWriteExt;
    let mut local = Local::normal().await;
    let relay = Relay::replying(reply(json!({})));
    relay.task.abort();
    std::fs::remove_file(&relay.path).unwrap();
    let listener = UnixListener::bind(&relay.path).unwrap();
    std::fs::set_permissions(&relay.path, std::fs::Permissions::from_mode(0o600)).unwrap();
    local.client.relay_socket = Some(relay.path.clone());
    let server = tokio::spawn(async move {
        let (mut probe, _) = listener.accept().await.unwrap();
        let _: Request = wire::read(&mut probe, shared_read::MAX_REQUEST_BYTES)
            .await
            .unwrap();
        wire::write(
            &mut probe,
            &Response::Identity {
                identity: identity(true),
            },
            shared_read::MAX_RESPONSE_BYTES,
        )
        .await
        .unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        let _: Request = wire::read(&mut socket, shared_read::MAX_REQUEST_BYTES)
            .await
            .unwrap();
        socket.write_u32(u32::MAX).await.unwrap();
        // Keep the socket open: the client must reject the length immediately.
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let result = tokio::time::timeout(Duration::from_secs(1), read(&local.client, "v1/pr-status"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["from"], "local");
    server.abort();
}

#[tokio::test]
async fn unprotected_socket_is_not_used() {
    use std::os::unix::fs::PermissionsExt;
    let mut local = Local::normal().await;
    let relay = Relay::replying(reply(json!({"from":"primary"})));
    local.client.relay_socket = Some(relay.path.clone());
    std::fs::set_permissions(&relay.path, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(
        read(&local.client, "v1/pr-status").await.unwrap()["from"],
        "local"
    );
    assert!(relay.seen.lock().unwrap().is_empty());
    assert_eq!(*local.seen.lock().unwrap(), vec!["/v1/pr-status"]);
}
