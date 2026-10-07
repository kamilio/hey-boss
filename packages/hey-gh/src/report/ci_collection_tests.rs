use crate::{Client, Config, Error, Freshness};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::IntoResponse,
};
use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MERGE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[derive(Default)]
struct Mock {
    live: AtomicBool,
    calls: Mutex<Vec<String>>,
    held: Notify,
    release: Notify,
}

async fn handler(
    State(mock): State<Arc<Mock>>,
    uri: Uri,
    headers: HeaderMap,
) -> axum::response::Response {
    if mock.live.load(Ordering::Relaxed) {
        let count = {
            let mut calls = mock.calls.lock().unwrap();
            calls.push(uri.to_string());
            calls.len()
        };
        if count == 4 {
            mock.held.notify_one();
            mock.release.notified().await;
        }
    }
    if headers.contains_key("if-none-match") {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let mut response =
        axum::Json(json!({"check_runs":[],"statuses":[],"workflow_runs":[],"total_count":0}))
            .into_response();
    response
        .headers_mut()
        .insert("etag", "\"fixture\"".parse().unwrap());
    response
}

#[tokio::test]
async fn bounded_foreground_ci_fills_a_missing_source_before_revalidating_warm_sources() {
    for capacity in [1, 256] {
        for foreground in [true, false] {
            let mock = Arc::new(Mock::default());
            let router = axum::Router::new()
                .fallback(handler)
                .with_state(mock.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            let dir = tempfile::tempdir().unwrap();
            let client = Client::with_token(
                Config {
                    cache_path: dir.path().join("cache.sqlite"),
                    rest_url: url.parse().unwrap(),
                    graphql_url: format!("{url}graphql").parse().unwrap(),
                    queue_capacity: capacity,
                    min_spacing: Duration::ZERO,
                    max_attempts: 1,
                    ..Config::default()
                },
                "synthetic-token".into(),
            )
            .unwrap();
            let cold = format!("repos/acme/demo/commits/{MERGE}/status?per_page=100");
            for sha in [HEAD, MERGE] {
                for path in [
                    format!("repos/acme/demo/commits/{sha}/check-runs?filter=latest&per_page=100"),
                    format!("repos/acme/demo/commits/{sha}/status?per_page=100"),
                    format!("repos/acme/demo/actions/runs?head_sha={sha}&per_page=100"),
                ] {
                    if path != cold {
                        client.get(&path, Freshness::Revalidate).await.unwrap();
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            mock.live.store(true, Ordering::Relaxed);
            let read = tokio::spawn({
                let client = client.clone();
                async move {
                    crate::client::INTERACTIVE_READ
                        .scope(
                            Arc::new(AtomicBool::new(foreground)),
                            client.ci_report(
                                "acme/demo",
                                HEAD,
                                Some(MERGE),
                                Freshness::MaxAge(Duration::from_millis(1)),
                            ),
                        )
                        .await
                }
            });
            tokio::time::timeout(Duration::from_secs(3), mock.held.notified())
                .await
                .unwrap();
            assert!(client.status().outstanding_requests <= if capacity == 1 { 1 } else { 3 });
            read.abort();
            assert!(read.await.unwrap_err().is_cancelled());
            let cached = client.get(&cold, Freshness::CachedOnly).await;
            mock.release.notify_one();
            tokio::time::timeout(Duration::from_secs(1), async {
                while client.status().outstanding_requests != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            server.abort();
            let calls = mock.calls.lock().unwrap();
            assert_eq!(
                calls.len(),
                4,
                "cancellation must stop unobserved queued work"
            );
            assert!(
                calls[..2]
                    .iter()
                    .all(|path| path.contains("/actions/runs?")),
                "job dependencies must still come first: {calls:?}"
            );
            if foreground {
                assert!(
                    cached.is_ok(),
                    "limited foreground collection repeated warm evidence before filling a missing source (capacity={capacity}): {calls:?}"
                );
            } else {
                assert!(
                    matches!(cached, Err(Error::CacheMiss)),
                    "background order must remain unchanged"
                );
            }
        }
    }
}

#[tokio::test]
async fn ci_dependencies_enter_the_queue_before_later_sources_without_waiting_for_http() {
    for coalesced in [false, true] {
        admission_scenario(false, coalesced).await;
    }
}

#[tokio::test]
async fn cancelling_ci_during_dependency_preparation_does_not_start_later_sources() {
    for coalesced in [false, true] {
        admission_scenario(true, coalesced).await;
    }
}

async fn admission_scenario(cancel_preparation: bool, coalesced: bool) {
    struct AdmissionMock {
        calls: Mutex<Vec<String>>,
        changed: Notify,
        release: tokio::sync::Semaphore,
    }
    async fn held_handler(State(mock): State<Arc<AdmissionMock>>, uri: Uri) -> impl IntoResponse {
        mock.calls.lock().unwrap().push(uri.to_string());
        mock.changed.notify_one();
        mock.release.acquire().await.unwrap().forget();
        axum::Json(json!({"check_runs":[],"statuses":[],"workflow_runs":[],"total_count":0}))
    }
    let mock = Arc::new(AdmissionMock {
        calls: Mutex::new(Vec::new()),
        changed: Notify::new(),
        release: tokio::sync::Semaphore::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let router = axum::Router::new()
        .fallback(held_handler)
        .with_state(mock.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(
        Config {
            cache_path: dir.path().join("cache.sqlite"),
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            min_spacing: Duration::ZERO,
            max_attempts: 1,
            ..Config::default()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    let shared = if coalesced {
        let client = client.clone();
        let read = tokio::spawn(async move {
            client
                .get(
                    &format!("repos/acme/demo/actions/runs?head_sha={HEAD}&per_page=100"),
                    Freshness::Revalidate,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), mock.changed.notified())
            .await
            .unwrap();
        Some(read)
    } else {
        None
    };
    let entered = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let read = tokio::spawn({
        let (client, entered, resume) = (client.clone(), entered.clone(), resume.clone());
        async move {
            crate::client::CACHE_LOOKUP_PATH_GATE
                .scope(
                    std::cell::RefCell::new(Some((
                        format!("repos/acme/demo/actions/runs?head_sha={MERGE}&per_page=100"),
                        entered,
                        resume,
                    ))),
                    client.ci_report("acme/demo", HEAD, Some(MERGE), Freshness::Revalidate),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    // The first dependency can already be in HTTP while the second prepares.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if !mock.calls.lock().unwrap().is_empty() {
                break;
            }
            mock.changed.notified().await;
        }
    })
    .await
    .unwrap();
    let overtook = tokio::time::timeout(Duration::from_millis(250), async {
        while client.status().outstanding_requests < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        overtook.is_err(),
        "a commit source overtook the dependency's cache preparation: {:?}",
        mock.calls.lock().unwrap()
    );
    if cancel_preparation {
        read.abort();
        assert!(read.await.unwrap_err().is_cancelled());
        resume.notify_one();
        mock.release.add_permits(6);
        if let Some(shared) = shared {
            assert!(
                shared.await.unwrap().is_ok(),
                "cancellation must preserve the independent reader"
            );
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            while client.status().outstanding_requests != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(mock.calls.lock().unwrap().len(), 1);
        server.abort();
        return;
    }
    resume.notify_one();
    tokio::time::timeout(Duration::from_secs(3), async {
        while client.status().outstanding_requests != 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("queue admission must not serialize HTTP responses");
    assert_eq!(client.status().outstanding_requests, 3);
    mock.release.add_permits(6);
    let report = tokio::time::timeout(Duration::from_secs(3), read)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(report.errors.is_empty());
    if let Some(shared) = shared {
        assert!(shared.await.unwrap().is_ok());
    }
    assert_eq!(mock.calls.lock().unwrap().len(), 6);
    assert!(
        mock.calls.lock().unwrap()[..2]
            .iter()
            .all(|path| path.contains("/actions/runs?"))
    );
    let cached = tokio::time::timeout(
        Duration::from_secs(3),
        client.ci_report("acme/demo", HEAD, Some(MERGE), Freshness::CachedOnly),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(cached.errors.is_empty());
    assert_eq!(mock.calls.lock().unwrap().len(), 6);
    server.abort();
}

#[tokio::test]
async fn shared_workflow_proof_admission_does_not_wait_for_its_response_or_fallback() {
    #[derive(Default)]
    struct ProofMock {
        query: Notify,
        check: Notify,
        release: Notify,
        calls: Mutex<Vec<String>>,
    }
    async fn proof_handler(
        State(mock): State<Arc<ProofMock>>,
        uri: Uri,
    ) -> axum::response::Response {
        mock.calls.lock().unwrap().push(uri.path().into());
        if uri.path() == "/graphql" {
            mock.query.notify_one();
            mock.release.notified().await;
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if uri.path().ends_with("/check-runs") {
            mock.check.notify_one();
        }
        axum::Json(json!({"check_runs":[],"statuses":[],"workflow_runs":[],"total_count":0}))
            .into_response()
    }
    let mock = Arc::new(ProofMock::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let router = axum::Router::new()
        .fallback(proof_handler)
        .with_state(mock.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(
        Config {
            cache_path: dir.path().join("cache.sqlite"),
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            min_spacing: Duration::ZERO,
            max_attempts: 1,
            ..Config::default()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    for sha in [HEAD, MERGE] {
        client
            .get(
                &format!("repos/acme/demo/actions/runs?head_sha={sha}&per_page=100"),
                Freshness::Revalidate,
            )
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    let read = tokio::spawn(async move {
        super::ci_metadata::commit_summaries::scope(
            &client,
            "acme/demo",
            HEAD,
            Some(MERGE),
            false,
            false,
            client.ci_report(
                "acme/demo",
                HEAD,
                Some(MERGE),
                Freshness::MaxAge(Duration::from_millis(1)),
            ),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), mock.query.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), mock.check.notified())
        .await
        .expect("REST checks waited for the shared workflow proof's two-second fallback");
    mock.release.notify_one();
    let report = tokio::time::timeout(Duration::from_secs(3), read)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(report.errors.is_empty());
    assert_eq!(
        mock.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.as_str() == "/graphql")
            .count(),
        1
    );
    server.abort();
}
