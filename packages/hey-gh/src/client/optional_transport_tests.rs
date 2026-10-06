use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Notify,
};

struct Fixture {
    client: Client,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(stall_body: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let server = tokio::spawn({
            let entered = entered.clone();
            let release = release.clone();
            async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|s| s == b"\r\n\r\n") {
                    let mut bytes = [0; 2048];
                    let count = socket.read(&mut bytes).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&bytes[..count]);
                }
                let headers = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\nETag: \"v1\"\r\n\r\n";
                if stall_body {
                    socket.write_all(headers).await.unwrap();
                }
                entered.notify_one();
                release.notified().await;
                if !stall_body {
                    let _ = socket.write_all(headers).await;
                }
                let _ = socket.write_all(b"{\"ok\":true}").await;
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            Config {
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                min_spacing: Duration::ZERO,
                queue_timeout: Duration::from_secs(5),
                request_timeout: Duration::from_secs(5),
                max_attempts: 1,
                ..Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        Self {
            client,
            entered,
            release,
            server,
            _dir: dir,
        }
    }

    async fn optional(&self) -> tokio::task::JoinHandle<Result<Response>> {
        let client = self.client.clone();
        let task = tokio::spawn(async move {
            optional_selector_read(client.get(
                "repos/acme/demo/git/ref/pull/7/merge",
                Freshness::Revalidate,
            ))
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), self.entered.notified())
            .await
            .unwrap();
        task
    }
}

#[tokio::test]
async fn optional_rest_deadline_releases_stalled_headers_and_bodies_without_caching() {
    for stall_body in [false, true] {
        let f = Fixture::new(stall_body).await;
        let optional = f.optional().await;
        assert!(matches!(optional.await.unwrap(), Err(Error::Deadline)));
        tokio::time::timeout(Duration::from_millis(500), async {
            while f.client.status().outstanding_requests != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("expired transport still occupies a request lane");
        assert!(matches!(
            f.client
                .get(
                    "repos/acme/demo/git/ref/pull/7/merge",
                    Freshness::CachedOnly
                )
                .await,
            Err(Error::CacheMiss)
        ));
        assert_eq!(f.client.status().network_requests, 1);
    }
}

#[tokio::test]
async fn required_coalescer_extends_inflight_optional_rest_headers_and_bodies() {
    for stall_body in [false, true] {
        let f = Fixture::new(stall_body).await;
        let optional = f.optional().await;
        let client = f.client.clone();
        let required = tokio::spawn(async move {
            client
                .get(
                    "repos/acme/demo/git/ref/pull/7/merge",
                    Freshness::Revalidate,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while f.client.status().coalesced_requests == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(optional.await.unwrap(), Err(Error::Deadline)));
        assert!(
            !required.is_finished(),
            "optional deadline stopped a required coalescer"
        );
        f.release.notify_one();
        let response = tokio::time::timeout(Duration::from_secs(1), required)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(response.data["ok"], true);
        assert_eq!(f.client.status().network_requests, 1);
        assert_eq!(
            f.client
                .get(
                    "repos/acme/demo/git/ref/pull/7/merge",
                    Freshness::CachedOnly
                )
                .await
                .unwrap()
                .data,
            response.data
        );
    }
}
