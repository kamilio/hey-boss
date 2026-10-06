use super::*;
use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};
use std::{sync::Mutex, time::Duration};
use tokio::sync::Notify;

struct Mock {
    calls: Mutex<Vec<String>>,
    metadata: Mutex<Value>,
    pause_reviews: AtomicBool,
    pause_graph: AtomicBool,
    pause_metadata: AtomicBool,
    deny_metadata: AtomicBool,
    metadata_release: Notify,
    release: Notify,
}
async fn handler(State(mock): State<Arc<Mock>>, uri: Uri) -> Response {
    let path = uri.path();
    mock.calls.lock().unwrap().push(path.into());
    if path.ends_with("/pulls/7") {
        if mock.pause_metadata.load(Ordering::Relaxed) {
            mock.metadata_release.notified().await;
        }
        if mock.deny_metadata.load(Ordering::Relaxed) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"message":"metadata denied"})),
            )
                .into_response();
        }
        return Json(mock.metadata.lock().unwrap().clone()).into_response();
    }
    if path == "/graphql" {
        if mock.pause_graph.load(Ordering::Relaxed) {
            mock.release.notified().await;
        }
        let empty = json!({"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}});
        return Json(
            json!({"data":{"repository":{"pullRequest":{"reviewThreads":empty,"timelineItems":empty}}}}),
        ).into_response();
    }
    if path.ends_with("/reviews") && mock.pause_reviews.load(Ordering::Relaxed) {
        mock.release.notified().await;
    }
    Json(json!([])).into_response()
}
struct Fixture {
    client: Client,
    mock: Arc<Mock>,
    dir: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mock = Arc::new(Mock {
            calls: Mutex::new(Vec::new()),
            metadata: Mutex::new(
                json!({"number":7,"node_id":"PR_7","state":"open","merged":false,"title":"Before","head":{"sha":"a".repeat(40)},"base":{"sha":"b".repeat(40),"ref":"main","repo":{"id":1,"node_id":"R_1","full_name":"acme/demo"}},"merge_commit_sha":null,"mergeable":true,"requested_reviewers":[],"requested_teams":[]}),
            ),
            pause_reviews: AtomicBool::new(false),
            pause_graph: AtomicBool::new(false),
            pause_metadata: AtomicBool::new(false),
            deny_metadata: AtomicBool::new(false),
            metadata_release: Notify::new(),
            release: Notify::new(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let app = Router::new().fallback(handler).with_state(mock.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Client::with_token(
            crate::Config {
                rest_url: url.parse().unwrap(),
                graphql_url: format!("{url}graphql").parse().unwrap(),
                cache_path: dir.path().join("cache.sqlite"),
                min_spacing: Duration::ZERO,
                ..crate::Config::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        Self {
            client,
            mock,
            dir,
            server,
        }
    }
    fn metadata_calls(&self) -> usize {
        self.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.ends_with("/pulls/7"))
            .count()
    }
    async fn warm_metadata(&self) {
        self.client
            .pull_request("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
    }
    fn age_metadata(&self, millis: u64) -> u64 {
        let at = now_ms() - millis;
        rusqlite::Connection::open(self.dir.path().join("cache.sqlite")).unwrap().execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/pulls/7'",[at]).unwrap();
        at
    }
    async fn collect(&self, freshness: Freshness) -> Vec<ResourceValidation> {
        VALIDATIONS
            .scope(std::cell::RefCell::new(Vec::new()), async {
                let errors = self
                    .client
                    .refresh_pr_details("acme/demo", 7, freshness)
                    .await
                    .unwrap();
                assert!(errors.is_empty(), "{errors:?}");
                VALIDATIONS.with(|v| v.borrow().clone())
            })
            .await
    }
}

#[tokio::test]
async fn detail_metadata_overlaps_the_last_sources_and_is_rechecked_at_publication() {
    for expire in [false, true] {
        let f = Fixture::new().await;
        f.warm_metadata().await;
        f.age_metadata(60_000);
        f.mock.pause_graph.store(true, Ordering::Relaxed);
        let reader = f.client.clone();
        let task = tokio::spawn(async move {
            VALIDATIONS
                .scope(std::cell::RefCell::new(Vec::new()), async {
                    let result = reader
                        .refresh_pr_details(
                            "acme/demo",
                            7,
                            Freshness::MaxAge(Duration::from_secs(30)),
                        )
                        .await;
                    (result, VALIDATIONS.with(|records| records.borrow().clone()))
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while f.metadata_calls() < 2 {
                tokio::task::yield_now().await;
            }
            // Wait until the response is cached, not merely dispatched.
            loop {
                let cached = f
                    .client
                    .pull_request("acme/demo", 7, Freshness::CachedOnly)
                    .await
                    .unwrap();
                if now_ms().saturating_sub(cached.validated_at_ms) < 5_000 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect(
            "Metadata must be validated while the final conversation sources are still pending",
        );
        assert!(!task.is_finished());
        if expire {
            f.age_metadata(16_000);
            f.mock.metadata.lock().unwrap()["title"] = json!("Newer at publication");
        }
        f.mock.release.notify_one();
        let (result, validations) = task.await.unwrap();
        assert!(result.unwrap().is_empty());
        assert_eq!(
            validations
                .iter()
                .filter(|v| v.resource.ends_with("/pulls/7"))
                .count(),
            1,
            "Only the final observation supplies report validation evidence"
        );
        assert_eq!(f.metadata_calls(), if expire { 3 } else { 2 });
        let snapshot = f
            .client
            .stored_snapshot(&format!("metadata://{}/acme/demo/7", f.client.hostname()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            snapshot["pull_request"]["title"],
            if expire {
                "Newer at publication"
            } else {
                "Before"
            }
        );
    }
}

#[tokio::test]
async fn detail_confirmation_reuses_recent_personal_metadata_without_refreshing_its_clock() {
    let f = Fixture::new().await;
    f.warm_metadata().await;
    let at = f.age_metadata(5_000);
    let validations = f.collect(Freshness::MaxAge(Duration::from_secs(30))).await;
    assert_eq!(
        f.metadata_calls(),
        1,
        "The recent personal response already satisfies the final bound"
    );
    assert!(validations.iter().any(|v| v.resource.ends_with("/pulls/7")
        && v.validated_at_ms == at
        && matches!(v.source, crate::Source::Cache)));
}

#[tokio::test]
async fn denied_metadata_overlap_cannot_publish_old_success() {
    let f = Fixture::new().await;
    f.warm_metadata().await;
    let old = f.age_metadata(60_000);
    f.mock.pause_graph.store(true, Ordering::Relaxed);
    f.mock.deny_metadata.store(true, Ordering::Relaxed);
    let reader = f.client.clone();
    let task = tokio::spawn(async move {
        reader
            .refresh_pr_details("acme/demo", 7, Freshness::default())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.metadata_calls() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.mock.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(Error::GitHub { status: 403, .. })
    ));
    let metadata = f
        .client
        .pull_request("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(metadata.validated_at_ms, old);
    let snapshot = f
        .client
        .stored_snapshot(&format!(
            "review_status://{}/acme/demo/7",
            f.client.hostname()
        ))
        .await
        .unwrap();
    assert!(
        snapshot.is_none(),
        "Denied confirmation must not publish a successful review rollup"
    );
    let comments = f
        .client
        .stored_snapshot(&format!("comments://{}/acme/demo/7", f.client.hostname()))
        .await
        .unwrap();
    assert!(
        comments.is_some(),
        "Independent successful sources retain their progress"
    );
}

#[tokio::test]
async fn cancelled_detail_overlap_preserves_a_shared_metadata_reader() {
    let f = Fixture::new().await;
    f.warm_metadata().await;
    f.age_metadata(60_000);
    f.mock.pause_graph.store(true, Ordering::Relaxed);
    f.mock.pause_metadata.store(true, Ordering::Relaxed);
    let reader = f.client.clone();
    let task = tokio::spawn(async move {
        reader
            .refresh_pr_details("acme/demo", 7, Freshness::default())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.metadata_calls() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let before = f.client.status().coalesced_requests;
    let sibling = f.client.clone();
    let shared = tokio::spawn(async move {
        sibling
            .pull_request("acme/demo", 7, Freshness::Revalidate)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.client.status().coalesced_requests == before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    f.mock.metadata_release.notify_one();
    assert_eq!(shared.await.unwrap().unwrap().data["node_id"], "PR_7");
    assert_eq!(f.metadata_calls(), 2, "A shared read must not be restarted");
    f.mock.release.notify_one();
}

#[tokio::test]
async fn detail_confirmation_uses_metadata_refreshed_by_a_sibling_during_collection() {
    let f = Fixture::new().await;
    f.warm_metadata().await;
    f.age_metadata(60_000);
    f.mock.pause_reviews.store(true, Ordering::Relaxed);
    let reader = f.client.clone();
    let task = tokio::spawn(async move {
        reader
            .refresh_pr_details("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !f
            .mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.ends_with("/reviews"))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.mock.metadata.lock().unwrap()["title"] = json!("After");
    f.warm_metadata().await;
    f.mock.release.notify_one();
    assert!(task.await.unwrap().unwrap().is_empty());
    assert_eq!(
        f.metadata_calls(),
        2,
        "The sibling already performed the final REST validation"
    );
    let snapshot = f
        .client
        .stored_snapshot(&format!("metadata://{}/acme/demo/7", f.client.hostname()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot["pull_request"]["title"], "After");
}

#[tokio::test]
async fn detail_confirmation_never_reuses_expired_or_explicitly_refreshed_metadata() {
    for (age, freshness, expected) in [
        (16_000, Freshness::MaxAge(Duration::from_secs(300)), 2),
        (5_000, Freshness::MaxAge(Duration::from_secs(1)), 2),
        (0, Freshness::Revalidate, 3),
        (0, Freshness::MaxAge(Duration::ZERO), 3),
    ] {
        let f = Fixture::new().await;
        f.warm_metadata().await;
        f.age_metadata(age);
        f.collect(freshness).await;
        assert_eq!(f.metadata_calls(), expected);
    }
}

#[tokio::test]
async fn detail_confirmation_rejects_a_replaced_identity_during_collection() {
    let f = Fixture::new().await;
    f.warm_metadata().await;
    f.mock.pause_reviews.store(true, Ordering::Relaxed);
    let reader = f.client.clone();
    let task = tokio::spawn(async move {
        reader
            .refresh_pr_details("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !f
            .mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.ends_with("/reviews"))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.mock.metadata.lock().unwrap()["node_id"] = json!("PR_replacement");
    f.warm_metadata().await;
    f.mock.release.notify_one();
    assert!(
        task.await.unwrap().is_err(),
        "Retired detail evidence cannot publish against the replacement PR"
    );
}

#[tokio::test]
async fn offline_detail_confirmation_preserves_old_validation_without_network_io() {
    let f = Fixture::new().await;
    f.collect(Freshness::Revalidate).await;
    let at = f.age_metadata(60_000);
    let before = f.mock.calls.lock().unwrap().len();
    let validations = f.collect(Freshness::CachedOnly).await;
    assert_eq!(f.mock.calls.lock().unwrap().len(), before);
    assert!(
        validations
            .iter()
            .any(|v| v.resource.ends_with("/pulls/7") && v.validated_at_ms == at)
    );
}
