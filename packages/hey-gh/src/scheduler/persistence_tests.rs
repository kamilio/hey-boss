use crate::{Client, Config, Error, Freshness};
use axum::{Router, http::StatusCode, response::IntoResponse};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

async fn writer_contention(unchanged: bool, fail_write: bool) {
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new().fallback({
        let calls = calls.clone();
        move |uri: axum::http::Uri| {
            let calls = calls.clone();
            async move {
                if uri.path() == "/graphql" {
                    return (
                        StatusCode::FORBIDDEN,
                        axum::Json(serde_json::json!({"message":"denied"})),
                    )
                        .into_response();
                }
                let call = calls.fetch_add(1, Ordering::SeqCst);
                if unchanged && call > 0 {
                    StatusCode::NOT_MODIFIED.into_response()
                } else {
                    (
                        [("etag", "\"v1\"")],
                        axum::Json(serde_json::json!({"ok":true})),
                    )
                        .into_response()
                }
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let client = Client::with_token(
        Config {
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            cache_path: path.clone(),
            min_spacing: Duration::ZERO,
            queue_timeout: Duration::from_secs(10),
            ..Config::default()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    // Real WAL contention from another SQLite writer, as can happen with an
    // independent SDK process. Cached evidence remains readable throughout.
    let seed = client.get("first", Freshness::Revalidate).await.unwrap();
    let writer = rusqlite::Connection::open(path).unwrap();
    if fail_write {
        writer.execute_batch("CREATE TRIGGER fail_cache_update BEFORE UPDATE ON cache BEGIN SELECT RAISE(FAIL, 'test write failure'); END;").unwrap();
    }
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let first = tokio::spawn({
        let client = client.clone();
        async move { client.get("first", Freshness::Revalidate).await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Let the completed response reach persistence before admitting a peer.
    // The external writer remains held, so the first read cannot finish.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!first.is_finished());
    let coalesced = tokio::spawn({
        let client = client.clone();
        async move { client.get("first", Freshness::Revalidate).await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while client.status().coalesced_requests == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let peer = tokio::time::timeout(
        Duration::from_millis(500),
        client.graphql(
            "query { viewer { login } }",
            serde_json::json!({}),
            Freshness::Revalidate,
        ),
    )
    .await;
    writer.execute_batch("ROLLBACK").unwrap();
    let first = first.await.unwrap();
    let coalesced = coalesced.await.unwrap();
    let cached = client.get("first", Freshness::CachedOnly).await.unwrap();
    if fail_write {
        assert!(matches!(first, Err(Error::Storage(_))));
        assert!(matches!(coalesced, Err(Error::Storage(_))));
        assert_eq!(cached.validated_at_ms, seed.validated_at_ms);
    } else {
        let response = first.unwrap();
        assert_eq!(response.data["ok"], true);
        assert_eq!(cached.validated_at_ms, response.validated_at_ms);
        assert_eq!(coalesced.unwrap().validated_at_ms, response.validated_at_ms);
        assert_eq!(
            matches!(response.source, crate::Source::Revalidated),
            unchanged
        );
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "persistence must retain coalescing ownership"
    );
    server.abort();
    assert!(matches!(
        peer.expect("cache persistence blocked another quota's response"),
        Err(Error::GitHub { status: 403, .. })
    ));
}

#[tokio::test]
async fn response_persistence_does_not_block_another_quota() {
    writer_contention(false, false).await;
}

#[tokio::test]
async fn revalidation_persistence_does_not_block_another_quota() {
    writer_contention(true, false).await;
}

#[tokio::test]
async fn failed_persistence_reaches_every_waiter_without_validating_old_cache() {
    writer_contention(false, true).await;
    writer_contention(true, true).await;
}

#[tokio::test]
async fn throttle_headers_take_effect_while_an_unrelated_write_waits() {
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new().fallback({
        let calls = calls.clone();
        move |uri: axum::http::Uri| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if uri.path() == "/graphql" {
                    (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "60")], "{}").into_response()
                } else {
                    axum::Json(serde_json::json!({"ok":true})).into_response()
                }
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let client = Client::with_token(
        Config {
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            cache_path: path.clone(),
            min_spacing: Duration::ZERO,
            queue_timeout: Duration::from_secs(5),
            max_attempts: 1,
            ..Config::default()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    let writer = rusqlite::Connection::open(path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let first = tokio::spawn({
        let client = client.clone();
        async move { client.get("first", Freshness::Revalidate).await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let peer = tokio::time::timeout(
        Duration::from_millis(500),
        client.graphql(
            "query { viewer { login } }",
            serde_json::json!({}),
            Freshness::Revalidate,
        ),
    )
    .await;
    // This endpoint uses a free detail lane, but the shared cooldown must
    // reject it before another socket is opened, even with the writer held.
    let details = tokio::time::timeout(
        Duration::from_millis(500),
        client.get("repos/acme/demo/issues/7/comments", Freshness::Revalidate),
    )
    .await;
    let count = calls.load(Ordering::SeqCst);
    writer.execute_batch("ROLLBACK").unwrap();
    first.await.unwrap().unwrap();
    server.abort();
    assert!(matches!(peer.unwrap(), Err(Error::RateLimited { .. })));
    assert!(matches!(details.unwrap(), Err(Error::RateLimited { .. })));
    assert_eq!(count, 2);
}
