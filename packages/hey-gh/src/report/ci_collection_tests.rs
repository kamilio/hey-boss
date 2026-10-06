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
