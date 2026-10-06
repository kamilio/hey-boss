//! Exercise native policy reads with the production HTTP/2 concurrency lanes.
//! The production hostname resolves only to this local TLS fixture.
use super::*;
use axum::body::Body;
use hyper::{Response as HttpResponse, service::service_fn};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use serde_json::json;
use std::convert::Infallible;
use tokio::{net::TcpListener, sync::Notify};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{ServerConfig, pki_types::PrivatePkcs8KeyDer},
};

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const TRUNK: &str = "cccccccccccccccccccccccccccccccccccccccc";
const NEXT: &str = "dddddddddddddddddddddddddddddddddddddddd";

#[derive(Default)]
struct Data {
    calls: Vec<String>,
    wait_for_base: Option<&'static str>,
    change_base: bool,
    deny_base: bool,
    stall_base_confirmation: bool,
    deny_pr_confirmation: bool,
    change_pr_base: bool,
}

struct Fixture {
    client: Client,
    data: Arc<Mutex<Data>>,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(data: Data) -> Self {
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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let url = format!("https://api.github.com:{}/", address.port());
        let data = Arc::new(Mutex::new(data));
        let base_started = Arc::new(Notify::new());
        let server = tokio::spawn({
            let data = data.clone();
            async move {
                let mut tasks = tokio::task::JoinSet::new();
                loop {
                    let (socket, _) = listener.accept().await.unwrap();
                    let (acceptor, data, base_started) =
                        (acceptor.clone(), data.clone(), base_started.clone());
                    tasks.spawn(async move {
                        let stream = acceptor.accept(socket).await.unwrap();
                        let service = service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                            let (data, base_started) = (data.clone(), base_started.clone());
                            async move {
                                let (value, wait, signal, denied, stalled) = {
                                    let mut data = data.lock().unwrap();
                                    let path = request.uri().path();
                                    data.calls.push(path.into());
                                    let count = data.calls.iter().filter(|p| p.as_str() == path).count();
                                    let mut wait = false;
                                    let mut signal = false;
                                    let mut denied = false;
                                    let mut stalled = false;
                                    let value = if path.ends_with("/pulls/7") {
                                        wait = count == 2 && data.wait_for_base == Some("confirmation");
                                        denied = count == 2 && data.deny_pr_confirmation;
                                        let changed = count >= 2 && data.change_pr_base;
                                        json!({"node_id":"PR_demo_7","number":7,"state":"open","merged":false,"mergeable":true,
                                            "head":{"sha":HEAD},"base":{"ref":if changed {"other"} else {"layer"},"sha":if changed {NEXT} else {BASE}},"merge_commit_sha":null,
                                            "stack":{"id":12,"number":4,"position":2,"size":2,"base":{"ref":"main","sha":TRUNK}}})
                                    } else if path.contains("/rules/branches/") {
                                        json!([])
                                    } else if path.ends_with("/branches/main") {
                                        wait = count == 1 && data.wait_for_base == Some("collection");
                                        branch(TRUNK)
                                    } else if path.ends_with("/branches/layer") {
                                        signal = (count == 1 && data.wait_for_base == Some("collection"))
                                            || (count == 2 && data.wait_for_base == Some("confirmation"));
                                        denied = count == 2 && data.deny_base;
                                        stalled = count == 2 && data.stall_base_confirmation;
                                        branch(if count >= 2 && data.change_base { NEXT } else { BASE })
                                    } else if path.ends_with("/branches/other") {
                                        branch(NEXT)
                                    } else {
                                        panic!("unexpected source {path}");
                                    };
                                    (value, wait, signal, denied, stalled)
                                };
                                if signal { base_started.notify_one(); }
                                if wait { base_started.notified().await; }
                                if stalled { std::future::pending::<()>().await; }
                                Ok::<_, Infallible>(HttpResponse::builder().status(if denied {403} else {200})
                                    .body(Body::from(if denied {json!({"message":"Base inaccessible"})} else {value}.to_string())).unwrap())
                            }
                        });
                        let _ = Builder::new(TokioExecutor::new()).serve_connection(TokioIo::new(stream), service).await;
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
                max_attempts: 1,
                report_timeout: Duration::from_secs(5),
                request_timeout: Duration::from_secs(5),
                ..Config::default()
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
            server,
            _dir: dir,
        }
    }

    fn calls(&self, suffix: &str) -> usize {
        self.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|p| p.ends_with(suffix))
            .count()
    }
}

fn branch(sha: &str) -> Value {
    json!({"commit":{"sha":sha},"protected":false,"protection":{"enabled":false,
        "required_status_checks":{"enforcement_level":"off","contexts":[],"checks":[]}}})
}

#[tokio::test]
async fn native_diff_base_collection_overlaps_trunk_policy() {
    assert_overlap("collection").await;
}

#[tokio::test]
async fn native_diff_base_confirmation_overlaps_pr_membership() {
    assert_overlap("confirmation").await;
}

async fn assert_overlap(phase: &'static str) {
    let f = Fixture::new(Data {
        wait_for_base: Some(phase),
        ..Data::default()
    })
    .await;
    let report = tokio::time::timeout(
        Duration::from_secs(1),
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate),
    )
    .await
    .expect("independent native base read was serialized")
    .unwrap();
    assert_eq!(report.state, "not_required");
    assert_eq!(report.base_sha.as_deref(), Some(BASE));
    assert_eq!(report.policy_sha.as_deref(), Some(TRUNK));
    assert_eq!(
        f.calls("/branches/layer"),
        2,
        "both reads must still revalidate"
    );
    assert_eq!(
        f.calls("/pulls/7"),
        2,
        "native membership must still revalidate"
    );
}

#[tokio::test]
async fn concurrent_diff_base_confirmation_retries_changed_tips_and_preserves_denial() {
    for denied in [false, true] {
        let f = Fixture::new(Data {
            wait_for_base: Some("confirmation"),
            change_base: !denied,
            deny_base: denied,
            ..Data::default()
        })
        .await;
        let report = tokio::time::timeout(
            Duration::from_secs(1),
            f.client
                .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate),
        )
        .await
        .expect("base confirmation was serialized")
        .unwrap();
        if denied {
            assert_eq!(report.state, "unknown");
            assert!(
                report
                    .errors
                    .iter()
                    .any(|e| e.source == "base_confirmation")
            );
            assert_eq!(f.calls("/branches/layer"), 2);
        } else {
            assert_eq!(report.state, "not_required");
            assert_eq!(report.base_sha.as_deref(), Some(NEXT));
            assert_eq!(
                f.calls("/branches/layer"),
                4,
                "changed base requires recollection and revalidation"
            );
            assert_eq!(f.calls("/pulls/7"), 4);
        }
    }
}

#[tokio::test]
async fn unusable_pr_confirmation_does_not_wait_for_its_obsolete_diff_base() {
    for denied in [false, true] {
        let f = Fixture::new(Data {
            stall_base_confirmation: true,
            deny_pr_confirmation: denied,
            change_pr_base: !denied,
            ..Data::default()
        })
        .await;
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            f.client
                .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate),
        )
        .await
        .expect("unusable PR confirmation waited for an obsolete base");
        if denied {
            assert!(matches!(result, Err(Error::GitHub { status: 403, .. })));
        } else {
            let report = result.unwrap();
            assert_eq!(report.state, "not_required");
            assert_eq!(report.base_branch, "other");
            assert_eq!(report.base_sha.as_deref(), Some(NEXT));
            assert_eq!(f.calls("/pulls/7"), 3);
            assert_eq!(f.calls("/branches/other"), 2);
        }
    }
}

#[tokio::test]
async fn unchanged_selectors_still_wait_for_diff_base_confirmation_before_publication() {
    let f = Fixture::new(Data {
        stall_base_confirmation: true,
        ..Data::default()
    })
    .await;
    let client = f.client.clone();
    let reader = tokio::spawn(async move {
        client
            .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.calls("/branches/layer") < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !reader.is_finished(),
        "an unfinished base confirmation became a policy result"
    );
    assert!(
        !f.client
            .bootstrap()
            .await
            .unwrap()
            .snapshots
            .iter()
            .any(|s| s.resource.starts_with("required_checks://"))
    );
    reader.abort();
    assert!(reader.await.unwrap_err().is_cancelled());
}
