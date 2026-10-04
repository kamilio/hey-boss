use crate::{Client, Config, Error, Freshness, collection_budget};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Notify,
    task::JoinHandle,
};

const GOOD: &str = "HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\nETag: \"good\"\r\n\r\n{\"ok\":true}";
const BROKEN: &str =
    "HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\nETag: \"broken\"\r\n\r\n{";

struct Wire {
    origin: String,
    calls: Arc<AtomicUsize>,
    release: Arc<Notify>,
    started: Arc<Notify>,
    task: JoinHandle<()>,
}

impl Drop for Wire {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Wire {
    async fn start(replies: Vec<String>, hold_first: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let started = Arc::new(Notify::new());
        let task = tokio::spawn({
            let (calls, release, started) = (calls.clone(), release.clone(), started.clone());
            async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    loop {
                        let mut bytes = [0; 2048];
                        let count = socket.read(&mut bytes).await.unwrap();
                        assert!(count > 0);
                        request.extend_from_slice(&bytes[..count]);
                        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            let length = String::from_utf8_lossy(&request[..end])
                                .lines()
                                .find_map(|line| {
                                    line.split_once(':').filter(|(name, _)| {
                                        name.eq_ignore_ascii_case("content-length")
                                    })
                                })
                                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                                .unwrap_or(0);
                            if request.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    let index = calls.fetch_add(1, Ordering::Relaxed);
                    let reply = &replies[index.min(replies.len() - 1)];
                    socket.write_all(reply.as_bytes()).await.unwrap();
                    if index == 0 {
                        started.notify_one();
                        if hold_first {
                            release.notified().await;
                        }
                    }
                    let _ = socket.shutdown().await;
                }
            }
        });
        Self {
            origin,
            calls,
            release,
            started,
            task,
        }
    }

    fn config(&self, dir: &tempfile::TempDir) -> Config {
        Config {
            rest_url: self.origin.parse().unwrap(),
            graphql_url: format!("{}graphql", self.origin).parse().unwrap(),
            cache_path: dir.path().join("cache.sqlite"),
            min_spacing: Duration::ZERO,
            request_timeout: Duration::from_secs(2),
            queue_timeout: Duration::from_secs(5),
            max_attempts: 2,
            ..Config::default()
        }
    }

    fn count(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

#[tokio::test]
async fn truncated_body_retries_once_for_coalesced_readers() {
    let wire = Wire::start(vec![BROKEN.into(), GOOD.into()], true).await;
    let dir = tempfile::tempdir().unwrap();
    let c = Client::with_token(wire.config(&dir), "synthetic-token".into()).unwrap();
    let first = tokio::spawn({
        let c = c.clone();
        async move { c.get("read", Freshness::Revalidate).await }
    });
    wire.started.notified().await;
    let second = tokio::spawn({
        let c = c.clone();
        async move { c.get("read", Freshness::Revalidate).await }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while c.status().coalesced_requests == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    wire.release.notify_one();
    let a = first.await.unwrap().expect("truncated response must retry");
    let b = second.await.unwrap().unwrap();
    assert_eq!(a.data["ok"], true);
    assert_eq!(a.validated_at_ms, b.validated_at_ms);
    assert_eq!(wire.count(), 2, "coalesced readers must share the retry");
    let cached = c.get("read", Freshness::CachedOnly).await.unwrap();
    assert_eq!(cached.data, a.data);
    assert_eq!(cached.etag.as_deref(), Some("\"good\""));
    assert_eq!(c.status().outstanding_requests, 0);
}

#[tokio::test]
async fn background_body_timeout_is_a_local_deadline_without_retry() {
    let wire = Wire::start(vec![BROKEN.into()], true).await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = wire.config(&dir);
    config.request_timeout = Duration::from_millis(150);
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    let result = collection_budget::CURRENT
        .scope(
            collection_budget::Budget::new(),
            c.get("read", Freshness::Revalidate),
        )
        .await;
    assert!(matches!(result, Err(Error::Deadline)), "{result:?}");
    assert_eq!(wire.count(), 1);
    assert!(matches!(
        c.get("read", Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
    assert_eq!(c.status().outstanding_requests, 0);
}

#[tokio::test]
async fn truncated_body_retry_cannot_outlive_the_request_deadline() {
    let wire = Wire::start(vec![BROKEN.into()], false).await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = wire.config(&dir);
    config.queue_timeout = Duration::from_millis(250);
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), c.get("read", Freshness::Revalidate))
        .await
        .unwrap();
    assert!(matches!(result, Err(Error::Deadline)), "{result:?}");
    assert_eq!(wire.count(), 1);
    assert_eq!(c.status().outstanding_requests, 0);
}

#[tokio::test]
async fn exhausted_body_retries_preserve_the_last_good_cache() {
    let wire = Wire::start(vec![GOOD.into(), BROKEN.into()], false).await;
    let dir = tempfile::tempdir().unwrap();
    let c = Client::with_token(wire.config(&dir), "synthetic-token".into()).unwrap();
    let good = c.get("read", Freshness::Revalidate).await.unwrap();
    let failed = c.get("read", Freshness::Revalidate).await;
    assert!(matches!(failed, Err(Error::Transport(_))), "{failed:?}");
    assert_eq!(wire.count(), 3, "one seed and exactly two failed attempts");
    let cached = c.get("read", Freshness::CachedOnly).await.unwrap();
    assert_eq!(cached.data, good.data);
    assert_eq!(cached.etag, good.etag);
    assert_eq!(cached.validated_at_ms, good.validated_at_ms);
    assert_eq!(c.status().outstanding_requests, 0);
}

#[tokio::test]
async fn body_limits_and_permanent_http_failures_do_not_retry() {
    for too_large in [true, false] {
        let response = if too_large {
            GOOD.into()
        } else {
            BROKEN.replace("200 OK", "403 Forbidden")
        };
        let wire = Wire::start(vec![response], false).await;
        let dir = tempfile::tempdir().unwrap();
        let mut config = wire.config(&dir);
        if too_large {
            config.max_body_bytes = 10;
        }
        let c = Client::with_token(config, "synthetic-token".into()).unwrap();
        let result = c.get("read", Freshness::Revalidate).await;
        if too_large {
            assert!(matches!(result, Err(Error::Invalid(_))), "{result:?}");
        } else {
            assert!(matches!(result, Err(Error::Transport(_))), "{result:?}");
        }
        assert_eq!(wire.count(), 1);
        assert!(matches!(
            c.get("read", Freshness::CachedOnly).await,
            Err(Error::CacheMiss)
        ));
    }
}

#[tokio::test]
async fn broken_bodies_preserve_quota_and_retry_after_headers() {
    for kind in ["primary", "secondary", "server"] {
        let reset = crate::now_ms() / 1000 + 60;
        let (status, headers) = match kind {
            "primary" => (
                "200 OK",
                format!(
                    "X-RateLimit-Resource: core\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: {reset}\r\n"
                ),
            ),
            "secondary" => ("429 Too Many Requests", "Retry-After: 60\r\n".into()),
            _ => ("503 Service Unavailable", "Retry-After: 60\r\n".into()),
        };
        let broken = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: 20\r\nConnection: close\r\n\r\n{{"
        );
        let wire = Wire::start(vec![broken, GOOD.into()], false).await;
        let dir = tempfile::tempdir().unwrap();
        let mut config = wire.config(&dir);
        config.queue_timeout = Duration::from_secs(2);
        let c = Client::with_token(config, "synthetic-token".into()).unwrap();
        let result = c.get("read", Freshness::Revalidate).await;
        if kind == "server" {
            assert!(matches!(result, Err(Error::Deadline)), "{result:?}");
        } else {
            assert!(
                matches!(result, Err(Error::RateLimited { .. })),
                "{result:?}"
            );
        }
        assert_eq!(
            wire.count(),
            1,
            "{kind}: retry ignored authoritative headers"
        );
        if kind != "server" {
            let other = c
                .graphql(
                    "query { viewer { login } }",
                    serde_json::json!({}),
                    Freshness::Revalidate,
                )
                .await;
            if kind == "secondary" {
                assert!(matches!(other, Err(Error::RateLimited { .. })));
                assert_eq!(wire.count(), 1);
            } else {
                assert!(
                    other.is_ok(),
                    "core exhaustion must not block GraphQL: {other:?}"
                );
                assert_eq!(wire.count(), 2);
            }
        }
    }
}

#[tokio::test]
async fn foreground_coalescer_can_recover_a_background_body_timeout() {
    let wire = Wire::start(vec![BROKEN.into(), GOOD.into()], true).await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = wire.config(&dir);
    config.request_timeout = Duration::from_millis(150);
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    let background = tokio::spawn({
        let c = c.clone();
        async move {
            collection_budget::CURRENT
                .scope(
                    collection_budget::Budget::new(),
                    c.get("read", Freshness::Revalidate),
                )
                .await
        }
    });
    wire.started.notified().await;
    let foreground = tokio::spawn({
        let c = c.clone();
        async move {
            crate::client::INTERACTIVE_READ
                .scope(
                    crate::client::foreground_priority(),
                    c.get("read", Freshness::Revalidate),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while c.status().coalesced_requests == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Keep the first body open beyond its network timeout, then allow the
    // server to answer the promoted request's retry.
    tokio::time::sleep(Duration::from_millis(200)).await;
    wire.release.notify_one();
    assert_eq!(foreground.await.unwrap().unwrap().data["ok"], true);
    assert_eq!(background.await.unwrap().unwrap().data["ok"], true);
    assert_eq!(wire.count(), 2);
}
