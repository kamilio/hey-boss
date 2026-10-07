//! Production HTTP/2 detail lanes, connected only to a local TLS server.
use super::*;
use axum::body::Body;
use hyper::{Response as HttpResponse, service::service_fn};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use serde_json::json;
use std::convert::Infallible;
use tokio::sync::Notify;
use tokio_rustls::{
    TlsAcceptor,
    rustls::{ServerConfig, pki_types::PrivatePkcs8KeyDer},
};

const PATH: &str = "repos/acme/demo/issues/7/timeline?per_page=100";

struct Data {
    calls: Vec<(String, u64)>,
    links: HashMap<u64, String>,
    pause_second: bool,
    pause_third: bool,
    deny_third: bool,
}

struct Fixture {
    client: Client,
    data: Arc<Mutex<Data>>,
    second_started: Arc<Notify>,
    third_started: Arc<Notify>,
    second_release: Arc<Notify>,
    third_release: Arc<Notify>,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    fn page(&self, n: u64) -> String {
        format!("{}&page={n}", self.client.rest_url(PATH).unwrap())
    }

    async fn new() -> Self {
        Self::configured(Config::default()).await
    }

    async fn configured(config: Config) -> Self {
        let cert = rcgen::generate_simple_self_signed(vec!["api.github.com".into()]).unwrap();
        let trusted = reqwest::Certificate::from_der(cert.cert.der()).unwrap();
        let mut tls = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.cert.der().clone()],
                PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
            )
            .unwrap();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let url = format!("https://api.github.com:{}/", address.port());
        let page = |n| format!("{url}repositories/123/issues/7/timeline?per_page=100&page={n}");
        let data = Arc::new(Mutex::new(Data {
            calls: Vec::new(),
            links: HashMap::from([
                (
                    1,
                    format!("<{}>; rel=\"next\", <{}>; rel=\"last\"", page(2), page(3)),
                ),
                (
                    2,
                    format!("<{}>; rel=\"next\", <{}>; rel=\"last\"", page(3), page(3)),
                ),
            ]),
            pause_second: true,
            pause_third: false,
            deny_third: false,
        }));
        let second_started = Arc::new(Notify::new());
        let third_started = Arc::new(Notify::new());
        let second_release = Arc::new(Notify::new());
        let third_release = Arc::new(Notify::new());
        let server =
            tokio::spawn({
                let (data, second_started, third_started, second_release, third_release) = (
                    data.clone(),
                    second_started.clone(),
                    third_started.clone(),
                    second_release.clone(),
                    third_release.clone(),
                );
                async move {
                    let mut tasks = tokio::task::JoinSet::new();
                    loop {
                        let (socket, _) = listener.accept().await.unwrap();
                        let (
                            acceptor,
                            data,
                            second_started,
                            third_started,
                            second_release,
                            third_release,
                        ) = (
                            acceptor.clone(),
                            data.clone(),
                            second_started.clone(),
                            third_started.clone(),
                            second_release.clone(),
                            third_release.clone(),
                        );
                        tasks.spawn(async move {
                            let stream = acceptor.accept(socket).await.unwrap();
                            let service = service_fn(
                                move |request: hyper::Request<hyper::body::Incoming>| {
                                    let (
                                        data,
                                        second_started,
                                        third_started,
                                        second_release,
                                        third_release,
                                    ) = (
                                        data.clone(),
                                        second_started.clone(),
                                        third_started.clone(),
                                        second_release.clone(),
                                        third_release.clone(),
                                    );
                                    async move {
                                        assert_eq!(
                                            request.headers()["authorization"],
                                            "Bearer synthetic-token"
                                        );
                                        let url = Url::parse(&format!(
                                            "https://api.github.com{}",
                                            request.uri().path_and_query().unwrap()
                                        ))
                                        .unwrap();
                                        let page = url
                                            .query_pairs()
                                            .find(|(k, _)| k == "page")
                                            .map(|(_, v)| v.parse::<u64>().unwrap())
                                            .unwrap_or(1);
                                        let (link, pause, deny) = {
                                            let mut data = data.lock().unwrap();
                                            data.calls.push((url.path().into(), page));
                                            (
                                                data.links.get(&page).cloned(),
                                                if page == 2 {
                                                    data.pause_second
                                                } else {
                                                    page == 3 && data.pause_third
                                                },
                                                page == 3 && data.deny_third,
                                            )
                                        };
                                        if page == 2 {
                                            second_started.notify_one();
                                            if pause {
                                                second_release.notified().await;
                                            }
                                        }
                                        if page == 3 {
                                            third_started.notify_one();
                                            if pause {
                                                third_release.notified().await;
                                            }
                                        }
                                        let mut response = HttpResponse::builder()
                                            .status(if deny { 403 } else { 200 });
                                        if let Some(link) = link {
                                            response = response.header("link", link);
                                        }
                                        Ok::<_, Infallible>(
                                            response
                                                .body(Body::from(
                                                    if deny {
                                                        json!({"message":"denied"})
                                                    } else {
                                                        json!([page])
                                                    }
                                                    .to_string(),
                                                ))
                                                .unwrap(),
                                        )
                                    }
                                },
                            );
                            let _ = Builder::new(TokioExecutor::new())
                                .serve_connection(TokioIo::new(stream), service)
                                .await;
                        });
                    }
                }
            });
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_http(
            Config {
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                min_spacing: Duration::ZERO,
                request_timeout: Duration::from_secs(5),
                max_attempts: 1,
                ..config
            },
            "synthetic-token".into(),
            reqwest::Client::builder()
                .no_proxy()
                .resolve("api.github.com", address)
                .add_root_certificate(trusted),
        )
        .unwrap();
        Self {
            client,
            data,
            second_started,
            third_started,
            second_release,
            third_release,
            server,
            _dir: dir,
        }
    }

    fn read(&self) -> tokio::task::JoinHandle<Result<Vec<Value>>> {
        let client = self.client.clone();
        tokio::spawn(async move { client.pages(PATH, None, Freshness::Revalidate).await })
    }
}

async fn signal(notify: &Notify) {
    tokio::time::timeout(Duration::from_secs(2), notify.notified())
        .await
        .unwrap();
}

#[tokio::test]
async fn shrinking_chain_drops_unused_page_without_waiting_or_recording_validation() {
    let f = Fixture::new().await;
    {
        let mut data = f.data.lock().unwrap();
        data.links.remove(&2);
        data.pause_third = true;
    }
    let client = f.client.clone();
    let task = tokio::spawn(async move {
        crate::report::VALIDATIONS
            .scope(std::cell::RefCell::new(Vec::new()), async {
                let result = client.pages(PATH, None, Freshness::Revalidate).await;
                (result, crate::report::VALIDATIONS.with(|v| v.take()))
            })
            .await
    });
    signal(&f.third_started).await;
    f.second_release.notify_one();
    let (result, validations) = tokio::time::timeout(Duration::from_millis(500), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap(), vec![json!(1), json!(2)]);
    assert_eq!(validations.len(), 2);
    assert!(validations.iter().all(|v| !v.resource.contains("page=3")));
    f.third_release.notify_one();
}

#[tokio::test]
async fn confirmed_page_error_is_preserved_but_unused_error_is_discarded() {
    for required in [true, false] {
        let f = Fixture::new().await;
        {
            let mut data = f.data.lock().unwrap();
            data.deny_third = true;
            if !required {
                data.links.remove(&2);
            }
        }
        let task = f.read();
        signal(&f.third_started).await;
        f.second_release.notify_one();
        let result = task.await.unwrap();
        if required {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), vec![json!(1), json!(2)]);
        }
    }
}

#[tokio::test]
async fn changed_chain_follows_actual_next_page() {
    let f = Fixture::new().await;
    f.data
        .lock()
        .unwrap()
        .links
        .insert(2, format!("<{}>; rel=\"next\"", f.page(4)));
    let task = f.read();
    signal(&f.third_started).await;
    f.second_release.notify_one();
    assert_eq!(
        task.await.unwrap().unwrap(),
        vec![json!(1), json!(2), json!(4)]
    );
}

#[tokio::test]
async fn speculative_queue_rejection_retries_when_page_becomes_required() {
    let f = Fixture::new().await;
    let leave = f.client.interactive_reserved_slots() + 1;
    let _held = f
        .client
        .0
        .permits
        .acquire_many((f.client.0.config.queue_capacity - leave) as u32)
        .await
        .unwrap();
    let task = f.read();
    signal(&f.second_started).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.client.status().queue_full_rejections == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.second_release.notify_one();
    assert_eq!(
        task.await.unwrap().unwrap(),
        vec![json!(1), json!(2), json!(3)]
    );
}

#[tokio::test]
async fn tight_collection_budget_stays_sequential_and_preserves_limit_error() {
    let f = Fixture::configured(Config {
        max_collection_bytes: 4,
        max_body_bytes: 16,
        ..Config::default()
    })
    .await;
    let task = f.read();
    signal(&f.second_started).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), f.third_started.notified())
            .await
            .is_err()
    );
    f.second_release.notify_one();
    assert!(
        task.await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("collection byte limit")
    );
    assert_eq!(f.data.lock().unwrap().calls.len(), 2);
}

#[tokio::test]
async fn cached_only_never_starts_network_and_revalidation_records_consumed_pages() {
    let f = Fixture::new().await;
    assert!(
        f.client
            .pages(PATH, None, Freshness::CachedOnly)
            .await
            .is_err()
    );
    assert!(f.data.lock().unwrap().calls.is_empty());
    f.data.lock().unwrap().pause_second = false;
    let (result, validations) = crate::report::VALIDATIONS
        .scope(std::cell::RefCell::new(Vec::new()), async {
            let result = f.client.pages(PATH, None, Freshness::Revalidate).await;
            (result, crate::report::VALIDATIONS.with(|v| v.take()))
        })
        .await;
    assert_eq!(result.unwrap(), vec![json!(1), json!(2), json!(3)]);
    assert_eq!(validations.len(), 3);
    assert_eq!(
        f.client
            .pages(PATH, None, Freshness::CachedOnly)
            .await
            .unwrap(),
        vec![json!(1), json!(2), json!(3)]
    );
    assert_eq!(f.data.lock().unwrap().calls.len(), 3);
}

#[tokio::test]
async fn cyclic_actual_link_remains_an_error() {
    let f = Fixture::new().await;
    f.data.lock().unwrap().links.insert(
        2,
        format!("<{}>; rel=\"next\"", f.client.rest_url(PATH).unwrap()),
    );
    let task = f.read();
    signal(&f.third_started).await;
    f.second_release.notify_one();
    assert!(
        task.await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("pagination link cycle")
    );
    assert_eq!(
        f.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(_, n)| *n == 1)
            .count(),
        1
    );
}

#[tokio::test]
async fn cancelling_collection_preserves_independent_waiter_on_speculative_page() {
    let f = Fixture::new().await;
    f.data.lock().unwrap().pause_third = true;
    let task = f.read();
    signal(&f.third_started).await;
    let client = f.client.clone();
    let third = f.page(3);
    let waiter = tokio::spawn(async move { client.get(&third, Freshness::Revalidate).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.client.status().coalesced_requests == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    f.third_release.notify_one();
    assert_eq!(waiter.await.unwrap().unwrap().data, json!([3]));
    f.second_release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.client.status().outstanding_requests != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(_, n)| *n == 3)
            .count(),
        1
    );
}

#[tokio::test]
async fn distant_last_page_does_not_expand_lookahead_window() {
    let f = Fixture::new().await;
    f.data.lock().unwrap().links.insert(
        1,
        format!(
            "<{}>; rel=\"next\", <{}>; rel=\"last\"",
            f.page(2),
            f.page(999)
        ),
    );
    let task = f.read();
    signal(&f.third_started).await;
    assert_eq!(f.data.lock().unwrap().calls.len(), 3);
    f.second_release.notify_one();
    assert_eq!(
        task.await.unwrap().unwrap(),
        vec![json!(1), json!(2), json!(3)]
    );
    assert_eq!(f.data.lock().unwrap().calls.len(), 3);
}

#[tokio::test]
async fn untrusted_last_hints_keep_sequential_pagination() {
    let f = Fixture::new().await;
    let first = f.client.rest_url(PATH).unwrap().to_string();
    let next = f.page(2);
    let hints = [
        "https://example.invalid/repos/acme/demo/issues/7/timeline?per_page=100&page=3".to_string(),
        f.page(3).replace("/issues/7/", "/issues/8/"),
        format!("{}&since=changed", f.page(3)),
        format!("{}&page=4", f.page(3)),
        f.page(3).replace("per_page=100", "per_page=99"),
    ];
    for hint in hints {
        assert!(
            f.client
                .detail_page_ahead(&first, &next, &format!("<{hint}>; rel=\"last\""))
                .is_none(),
            "{hint}"
        );
    }
    assert!(f.client.detail_page_ahead(&first, &next, "").is_none());
    let cursor_first = format!("{first}&after=cursor");
    assert!(
        f.client
            .detail_page_ahead(
                &cursor_first,
                &format!("{next}&after=cursor"),
                &format!("<{}&after=cursor>; rel=\"last\"", f.page(3))
            )
            .is_none()
    );
    f.data
        .lock()
        .unwrap()
        .links
        .insert(1, format!("<{next}>; rel=\"next\""));
    let task = f.read();
    signal(&f.second_started).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), f.third_started.notified())
            .await
            .is_err()
    );
    f.second_release.notify_one();
    assert_eq!(
        task.await.unwrap().unwrap(),
        vec![json!(1), json!(2), json!(3)]
    );
}

#[tokio::test]
async fn known_detail_pages_overlap_and_preserve_order_and_repository_scope() {
    let f = Fixture::new().await;
    let task = f.read();
    tokio::time::timeout(Duration::from_secs(2), f.second_started.notified())
        .await
        .unwrap();
    let overlapped =
        tokio::time::timeout(Duration::from_millis(500), f.third_started.notified()).await;
    f.second_release.notify_one();
    assert_eq!(
        task.await.unwrap().unwrap(),
        vec![json!(1), json!(2), json!(3)]
    );
    assert!(
        overlapped.is_ok(),
        "The advertised third page waited for the second page's response"
    );
    let calls = f.data.lock().unwrap();
    assert_eq!(calls.calls.len(), 3);
    assert!(
        calls
            .calls
            .iter()
            .all(|(p, _)| p == "/repos/acme/demo/issues/7/timeline")
    );
}
