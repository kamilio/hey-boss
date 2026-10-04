use flate2::{Compression, write::GzEncoder};
use hey_gh::{Client, Config, Error, Freshness, Source};
use serde_json::json;
use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

struct Reply {
    headers: String,
    body: Vec<u8>,
    delay: Duration,
}

impl Reply {
    fn new(status: &str, extra: &str, body: Vec<u8>) -> Self {
        Self {
            headers: format!(
                "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n",
                body.len()
            ),
            body,
            delay: Duration::ZERO,
        }
    }
}

struct Wire {
    origin: String,
    requests: Arc<Mutex<Vec<String>>>,
    sent: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Wire {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Wire {
    async fn start(handler: impl Fn(&str, usize) -> Reply + Send + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let sent = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn({
            let requests = requests.clone();
            let sent = sent.clone();
            async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    loop {
                        let mut buffer = [0; 2048];
                        let count = socket.read(&mut buffer).await.unwrap();
                        assert!(count > 0);
                        request.extend_from_slice(&buffer[..count]);
                        if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                        {
                            let length = String::from_utf8_lossy(&request[..end])
                                .lines()
                                .find_map(|line| {
                                    line.split_once(':').filter(|(name, _)| {
                                        name.eq_ignore_ascii_case("content-length")
                                    })
                                })
                                .map_or(0, |(_, value)| value.trim().parse::<usize>().unwrap());
                            if request.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    let request = String::from_utf8(request).unwrap();
                    let index = {
                        let mut requests = requests.lock().unwrap();
                        let index = requests.len();
                        requests.push(request.clone());
                        index
                    };
                    let reply = handler(&request, index);
                    if socket.write_all(reply.headers.as_bytes()).await.is_ok() {
                        for chunk in reply.body.chunks(8192) {
                            tokio::time::sleep(reply.delay).await;
                            if socket.write_all(chunk).await.is_err() {
                                break;
                            }
                            sent.fetch_add(chunk.len(), Ordering::Relaxed);
                        }
                    }
                    let _ = socket.shutdown().await;
                }
            }
        });
        Self {
            origin,
            requests,
            sent,
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
        self.requests.lock().unwrap().len()
    }
}

fn accepts_gzip(request: &str) -> bool {
    request
        .lines()
        .filter_map(|line| line.split_once(':'))
        .any(|(name, value)| {
            name.eq_ignore_ascii_case("accept-encoding")
                && value.split(',').any(|encoding| encoding.trim() == "gzip")
        })
}

#[tokio::test]
async fn compressed_collection_fits_the_same_bandwidth_and_deadline() {
    let value = json!({"comments": (0..512).map(|id| json!({"id":id,"body":"Review line with repeated JSON fields. ".repeat(16)})).collect::<Vec<_>>()});
    let bytes = serde_json::to_vec(&value).unwrap();
    let original_size = bytes.len();
    let wire = Wire::start(move |request, _| {
        let compressed = accepts_gzip(request);
        let mut reply = Reply::new(
            "200 OK",
            if compressed {
                "Content-Encoding: gzip\r\n"
            } else {
                ""
            },
            if compressed {
                gzip(&bytes)
            } else {
                bytes.clone()
            },
        );
        // A plain response takes several seconds at this bandwidth. Gzip must
        // fit the existing budget without changing the returned JSON.
        reply.delay = Duration::from_millis(100);
        reply
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = wire.config(&dir);
    config.request_timeout = Duration::from_millis(500);
    config.queue_timeout = Duration::from_secs(1);
    config.max_attempts = 1;
    let client = Client::with_token(config, "synthetic-token".into()).unwrap();
    let response = client
        .get("comments", Freshness::Revalidate)
        .await
        .expect("compressed collection should finish inside the unchanged request budget");
    assert_eq!(response.data, value);
    assert_eq!(wire.count(), 1);
    assert!(wire.sent.load(Ordering::Relaxed) < original_size / 8);
}

#[tokio::test]
async fn gzip_negotiation_supports_rest_graphql_and_uncompressed_fallback() {
    let wire = Wire::start(|request, index| {
        let body = if request.starts_with("POST /graphql ") {
            br#"{"data":{"viewer":{"login":"me"}}}"#.to_vec()
        } else {
            br#"{"ok":true}"#.to_vec()
        };
        if index == 1 {
            Reply::new("200 OK", "", body)
        } else {
            Reply::new("200 OK", "Content-Encoding: gzip\r\n", gzip(&body))
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(wire.config(&dir), "synthetic-token".into()).unwrap();
    assert_eq!(
        client
            .get("compressed", Freshness::Revalidate)
            .await
            .unwrap()
            .data["ok"],
        true
    );
    assert_eq!(
        client
            .get("plain", Freshness::Revalidate)
            .await
            .unwrap()
            .data["ok"],
        true
    );
    assert_eq!(
        client
            .graphql(
                "query { viewer { login } }",
                json!({}),
                Freshness::Revalidate
            )
            .await
            .unwrap()
            .data["data"]["viewer"]["login"],
        "me"
    );
    assert!(
        wire.requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| accepts_gzip(request))
    );
}

#[tokio::test]
async fn gzip_304_reuses_cache_and_preserves_quota_headers() {
    let reset = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 60;
    let wire = Wire::start(move |_, index| {
        if index == 0 {
            Reply::new("200 OK", "Content-Encoding: gzip\r\nETag: \"good\"\r\n", gzip(br#"{"ok":true}"#))
        } else {
            Reply::new("304 Not Modified", &format!("Content-Encoding: gzip\r\nETag: \"good\"\r\nX-RateLimit-Resource: core\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: {reset}\r\n"), Vec::new())
        }
    }).await;
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(wire.config(&dir), "synthetic-token".into()).unwrap();
    let first = client.get("read", Freshness::Revalidate).await.unwrap();
    let second = client.get("read", Freshness::Revalidate).await.unwrap();
    assert!(matches!(second.source, Source::Revalidated));
    assert_eq!(second.data, first.data);
    assert!(
        wire.requests.lock().unwrap()[1]
            .to_ascii_lowercase()
            .contains("if-none-match: \"good\"")
    );
    assert!(matches!(
        client.get("different", Freshness::Revalidate).await,
        Err(Error::RateLimited { .. })
    ));
    assert_eq!(wire.count(), 2);
}

#[tokio::test]
async fn decompressed_body_limit_preserves_cache_without_retrying() {
    let oversized = gzip(
        serde_json::to_string(&json!({"body":"x".repeat(8192)}))
            .unwrap()
            .as_bytes(),
    );
    assert!(oversized.len() < 128);
    let wire = Wire::start(move |_, index| {
        Reply::new(
            "200 OK",
            "Content-Encoding: gzip\r\nETag: \"candidate\"\r\n",
            if index == 0 {
                gzip(br#"{"ok":true}"#)
            } else {
                oversized.clone()
            },
        )
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = wire.config(&dir);
    config.max_body_bytes = 128;
    let client = Client::with_token(config, "synthetic-token".into()).unwrap();
    let first = client.get("read", Freshness::Revalidate).await.unwrap();
    assert!(matches!(
        client.get("read", Freshness::Revalidate).await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(wire.count(), 2);
    let cached = client.get("read", Freshness::CachedOnly).await.unwrap();
    assert_eq!(cached.data, first.data);
    assert_eq!(cached.validated_at_ms, first.validated_at_ms);
}

#[tokio::test]
async fn damaged_gzip_retries_within_attempt_limit_and_keeps_valid_cache() {
    for recovers in [true, false] {
        let wire = Wire::start(move |_, index| {
            let mut body = gzip(br#"{"ok":true}"#);
            if index > 0 && !(recovers && index == 2) {
                body.truncate(body.len() - 8);
            }
            Reply::new(
                "200 OK",
                "Content-Encoding: gzip\r\nETag: \"good\"\r\n",
                body,
            )
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(wire.config(&dir), "synthetic-token".into()).unwrap();
        let first = client.get("read", Freshness::Revalidate).await.unwrap();
        let result = client.get("read", Freshness::Revalidate).await;
        if recovers {
            assert_eq!(result.unwrap().data, first.data);
        } else {
            assert!(matches!(result, Err(Error::Transport(_))), "{result:?}");
        }
        assert_eq!(wire.count(), 3);
        let cached = client.get("read", Freshness::CachedOnly).await.unwrap();
        assert_eq!(cached.data, first.data);
        if !recovers {
            assert_eq!(cached.validated_at_ms, first.validated_at_ms);
        }
    }
}
