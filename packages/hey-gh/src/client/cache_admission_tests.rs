use super::*;
use serde_json::json;
use std::sync::atomic::AtomicUsize;

async fn race(graphql: bool, mode: &'static str) {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let router = axum::Router::new().fallback({
        let calls = calls.clone();
        move || {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                let status = if mode == "failed_peer" && call == 1 {
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                } else {
                    axum::http::StatusCode::OK
                };
                (status, axum::Json(json!({"data":{"value":call}})))
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = Client::with_token(
        Config {
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            cache_path: dir.path().join("cache.sqlite"),
            min_spacing: Duration::ZERO,
            max_attempts: 1,
            ..Config::default()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    let read = move |client: Client, freshness| async move {
        if graphql {
            client
                .graphql(
                    "query($owner: String!, $repo: String!) { repository(owner: $owner, name: $repo) { id } }",
                    json!({"owner":"acme","repo":"demo"}),
                    freshness,
                )
                .await
        } else {
            client
                .get("repos/acme/demo/commits/main/status", freshness)
                .await
        }
    };
    let freshness = match mode {
        "explicit" => Freshness::Revalidate,
        "zero_age" => Freshness::MaxAge(Duration::ZERO),
        "offline" => Freshness::CachedOnly,
        _ => Freshness::default(),
    };
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let delayed = tokio::spawn(CACHE_LOOKUP_GATE.scope(
        std::cell::RefCell::new(Some((entered.clone(), resume.clone()))),
        {
            let client = client.clone();
            async move {
                if mode == "probe" {
                    CACHE_PROBE.scope((), read(client, freshness)).await
                } else if mode == "deadline" {
                    READ_DEADLINE
                        .scope(
                            tokio::time::Instant::now() - Duration::from_secs(1),
                            read(client, freshness),
                        )
                        .await
                } else {
                    read(client, freshness).await
                }
            }
        },
    ));
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let peer = read(client.clone(), Freshness::default()).await;
    if mode == "failed_peer" {
        assert!(matches!(peer, Err(Error::GitHub { status: 500, .. })));
    } else {
        assert!(matches!(peer.unwrap().source, Source::Network));
    }
    let db = rusqlite::Connection::open(dir.path().join("cache.sqlite")).unwrap();
    match mode {
        "generation" => {
            db.execute("INSERT INTO repository_generation(scope,repository,generation) VALUES(?1,'acme/demo',1)",[&client.0.scope]).unwrap();
        }
        "deleted" => {
            assert_eq!(db.execute("DELETE FROM cache", []).unwrap(), 1);
        }
        "expired" => {
            assert_eq!(
                db.execute(
                    "UPDATE cache SET response=json_set(response,'$.validated_at_ms',0)",
                    []
                )
                .unwrap(),
                1
            );
        }
        _ => {}
    }
    let _permits = (mode == "full_queue").then(|| {
        client
            .0
            .permits
            .clone()
            .try_acquire_many_owned(client.0.permits.available_permits() as u32)
            .unwrap()
    });
    resume.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(2), delayed)
        .await
        .unwrap()
        .unwrap();
    match mode {
        "offline" | "probe" => {
            assert!(matches!(result, Err(Error::CacheMiss)));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
        "deadline" => {
            assert!(matches!(result, Err(Error::Deadline)));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
        "matching" | "full_queue" => {
            let result = result.unwrap();
            assert_eq!(result.data["data"]["value"], 1);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "graphql={graphql}: missed the completed peer and its persisted cache"
            );
            assert!(matches!(result.source, Source::Cache));
        }
        _ => {
            let result = result.unwrap();
            assert_eq!(result.data["data"]["value"], 2, "{mode}");
            assert_eq!(calls.load(Ordering::SeqCst), 2, "{mode}");
            assert!(matches!(result.source, Source::Network));
        }
    }
    server.abort();
}

#[tokio::test]
async fn a_peer_finishing_after_cache_lookup_does_not_create_a_duplicate_request() {
    for graphql in [false, true] {
        race(graphql, "matching").await;
        race(graphql, "full_queue").await;
    }
}

#[tokio::test]
async fn late_completion_does_not_change_refresh_offline_or_deadline_semantics() {
    for graphql in [false, true] {
        for mode in ["explicit", "zero_age", "offline", "probe", "deadline"] {
            race(graphql, mode).await;
        }
    }
}

#[tokio::test]
async fn completion_hints_cannot_replace_deleted_expired_or_failed_evidence() {
    for graphql in [false, true] {
        for mode in ["deleted", "expired", "failed_peer", "generation"] {
            race(graphql, mode).await;
        }
    }
}
