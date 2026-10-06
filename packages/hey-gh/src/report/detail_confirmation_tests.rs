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
    reviews_release: Notify,
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
        mock.reviews_release.notified().await;
    }
    if path.ends_with("/check-runs") {
        return Json(json!({"total_count":0,"check_runs":[]})).into_response();
    }
    if path.ends_with("/status") {
        return Json(json!({"total_count":0,"statuses":[]})).into_response();
    }
    if path.ends_with("/actions/runs") {
        return Json(json!({"total_count":0,"workflow_runs":[]})).into_response();
    }
    if path.ends_with("/comments") {
        let count = if path.contains("/issues/") {
            "comments"
        } else {
            "review_comments"
        };
        if mock.metadata.lock().unwrap()[count]
            .as_u64()
            .is_some_and(|count| count > 0)
        {
            return Json(json!([{"id":1,"body":"New comment","user":{"login":"reviewer"}}]))
                .into_response();
        }
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
            reviews_release: Notify::new(),
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

async fn pending_full_report(f: &Fixture) -> tokio::task::JoinHandle<Result<Report>> {
    let freshness = Freshness::MaxAge(Duration::from_secs(30));
    assert!(
        f.client
            .ci_for_pr("acme/demo", 7, freshness)
            .await
            .unwrap()
            .complete
    );
    f.mock.pause_reviews.store(true, Ordering::Relaxed);
    f.mock.pause_graph.store(true, Ordering::Relaxed);
    let reader = f.client.clone();
    let task = tokio::spawn(async move { reader.pr_report("acme/demo", 7, freshness).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let calls = f.mock.calls.lock().unwrap().clone();
            if calls.iter().any(|p| p.ends_with("/reviews"))
                && calls.iter().any(|p| p == "/graphql")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Drain the nested CI owner before aging its personal metadata. Two
    // independent conversation sources still prevent the report from finishing.
    assert!(
        f.client
            .ci_for_pr("acme/demo", 7, freshness)
            .await
            .unwrap()
            .complete
    );
    task
}

#[tokio::test]
async fn full_report_confirms_metadata_while_its_last_source_is_pending() {
    for expire in [false, true] {
        let f = Fixture::new().await;
        let task = pending_full_report(&f).await;
        let before = f.metadata_calls();
        let old = f.age_metadata(60_000);
        f.mock.reviews_release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let cached = f
                    .client
                    .pull_request("acme/demo", 7, Freshness::CachedOnly)
                    .await
                    .unwrap();
                if cached.validated_at_ms > old {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Full-report metadata must finish before the blocked final conversation source");
        assert!(!task.is_finished());
        if expire {
            f.age_metadata(16_000);
            f.mock.metadata.lock().unwrap()["title"] = json!("Changed while waiting");
        }
        f.mock.release.notify_one();
        let report = task.await.unwrap().unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        assert_eq!(
            f.metadata_calls(),
            before + if expire { 2 } else { 1 },
            "Final publication must reuse the still-fresh confirmation"
        );
        assert_eq!(
            report.data.pull_request["title"],
            if expire {
                "Changed while waiting"
            } else {
                "Before"
            }
        );
        let final_at = f
            .client
            .pull_request("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap()
            .validated_at_ms;
        assert_eq!(
            report
                .validations
                .iter()
                .filter(|v| v.resource.ends_with("/pulls/7") && v.validated_at_ms == final_at)
                .count(),
            1,
            "Only the final REST observation contributes this confirmation's clock"
        );
    }
}

#[tokio::test]
async fn full_report_overlap_denial_cannot_publish_old_metadata() {
    let f = Fixture::new().await;
    {
        let mut metadata = f.mock.metadata.lock().unwrap();
        metadata["comments"] = json!(0);
        metadata["review_comments"] = json!(0);
    }
    let task = pending_full_report(&f).await;
    let before = f.metadata_calls();
    let old = f.age_metadata(60_000);
    f.mock.deny_metadata.store(true, Ordering::Relaxed);
    f.mock.reviews_release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.metadata_calls() == before {
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
    assert_eq!(
        f.client
            .pull_request("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap()
            .validated_at_ms,
        old
    );
    assert!(
        f.client
            .stored_snapshot(&format!(
                "review_status://{}/acme/demo/7",
                f.client.hostname()
            ))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn cancelling_full_report_overlap_preserves_a_shared_metadata_reader() {
    let f = Fixture::new().await;
    let task = pending_full_report(&f).await;
    let before = f.metadata_calls();
    f.age_metadata(60_000);
    f.mock.pause_metadata.store(true, Ordering::Relaxed);
    f.mock.reviews_release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.metadata_calls() == before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let coalesced = f.client.status().coalesced_requests;
    let reader = f.client.clone();
    let shared = tokio::spawn(async move {
        reader
            .pull_request("acme/demo", 7, Freshness::default())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.client.status().coalesced_requests == coalesced {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    f.mock.metadata_release.notify_one();
    assert!(shared.await.unwrap().is_ok());
    assert_eq!(f.metadata_calls(), before + 1);
    f.mock.release.notify_one();
}

#[tokio::test]
async fn full_report_uses_fresh_personal_zero_counts_for_empty_comment_sources() {
    for warm in [false, true] {
        let f = Fixture::new().await;
        {
            let mut metadata = f.mock.metadata.lock().unwrap();
            metadata["comments"] = json!(0);
            metadata["review_comments"] = json!(0);
        }
        if warm {
            f.warm_metadata().await;
            f.age_metadata(5_000);
        }
        let report = f
            .client
            .pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap();
        assert!(report.complete);
        assert!(report.data.comments.is_empty());
        assert!(report.data.review_comments.is_empty());
        assert!(
            !f.mock
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|path| path.ends_with("/comments")),
            "Fresh zero counts already prove these comment lists empty"
        );
        assert!(
            report
                .validations
                .iter()
                .any(|v| v.resource.ends_with("/pulls/7"))
        );
        let before = f.mock.calls.lock().unwrap().len();
        let old = f.age_metadata(60_000);
        let cached = f
            .client
            .pr_report("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert!(
            cached.complete,
            "Derived empty lists must remain available offline"
        );
        assert!(
            cached.oldest_validation_at_ms <= old,
            "Offline proof must retain its old clock"
        );
        assert_eq!(f.mock.calls.lock().unwrap().len(), before);
    }
}

#[tokio::test]
async fn full_report_unknown_or_stale_comment_counts_keep_source_reads() {
    for case in [
        "missing",
        "positive",
        "negative",
        "string",
        "stale",
        "caller_age",
        "future",
        "cached",
        "revalidate",
    ] {
        let f = Fixture::new().await;
        let value = match case {
            "missing" => Value::Null,
            "positive" | "cached" => json!(1),
            "negative" => json!(-1),
            "string" => json!("0"),
            _ => json!(0),
        };
        {
            let mut metadata = f.mock.metadata.lock().unwrap();
            metadata["comments"] = value.clone();
            metadata["review_comments"] = value;
        }
        f.warm_metadata().await;
        if case == "stale" {
            f.age_metadata(16_000);
        }
        if case == "caller_age" {
            f.age_metadata(5_000);
        }
        if case == "future" {
            rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap().execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/pulls/7'",[now_ms()+60_000]).unwrap();
        }
        let freshness = match case {
            "cached" => Freshness::CachedOnly,
            "revalidate" => Freshness::Revalidate,
            "caller_age" => Freshness::MaxAge(Duration::from_secs(1)),
            _ => Freshness::MaxAge(Duration::from_secs(30)),
        };
        let report = f.client.pr_report("acme/demo", 7, freshness).await.unwrap();
        if case == "cached" {
            assert!(!report.complete, "Missing source caches must stay explicit");
            assert!(report.data.errors.iter().any(|e| e.source == "comments"));
        } else {
            assert!(report.complete, "{case}: {:?}", report.data.errors);
            assert_eq!(
                f.mock
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|path| path.ends_with("/comments"))
                    .count(),
                2,
                "{case}"
            );
        }
    }
}

#[tokio::test]
async fn full_report_zero_counts_cannot_erase_known_comments() {
    let f = Fixture::new().await;
    {
        let mut metadata = f.mock.metadata.lock().unwrap();
        metadata["comments"] = json!(1);
        metadata["review_comments"] = json!(1);
    }
    assert!(
        f.client
            .pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    {
        let mut metadata = f.mock.metadata.lock().unwrap();
        metadata["comments"] = json!(0);
        metadata["review_comments"] = json!(0);
    }
    f.warm_metadata().await;
    f.age_metadata(5_000);
    let report = f
        .client
        .pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    assert!(report.complete);
    assert_eq!(report.data.comments.len(), 1);
    assert_eq!(report.data.review_comments.len(), 1);
}

#[tokio::test]
async fn full_report_recollects_comments_added_after_zero_count_proof() {
    for field in ["comments", "review_comments"] {
        let f = Fixture::new().await;
        {
            let mut metadata = f.mock.metadata.lock().unwrap();
            metadata["comments"] = json!(0);
            metadata["review_comments"] = json!(0);
        }
        let task = pending_full_report(&f).await;
        f.age_metadata(60_000);
        f.mock.metadata.lock().unwrap()[field] = json!(1);
        f.mock.pause_reviews.store(false, Ordering::Relaxed);
        f.mock.pause_graph.store(false, Ordering::Relaxed);
        f.mock.reviews_release.notify_one();
        f.mock.release.notify_one();
        let report = task.await.unwrap().unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        let comments = if field == "comments" {
            &report.data.comments
        } else {
            &report.data.review_comments
        };
        assert_eq!(
            comments.len(),
            1,
            "New {field} must not disappear behind an earlier zero count"
        );
        assert_eq!(comments[0]["body"], "New comment");
    }
}

#[tokio::test]
async fn full_report_retry_reuses_personal_metadata_validated_during_collection() {
    for field in ["head", "merge"] {
        let f = Fixture::new().await;
        f.warm_metadata().await;
        f.age_metadata(60_000);
        {
            let mut metadata = f.mock.metadata.lock().unwrap();
            if field == "head" {
                metadata["head"]["sha"] = json!("c".repeat(40));
            } else {
                metadata["merge_commit_sha"] = json!("d".repeat(40));
            }
            metadata["title"] = json!("Validated new selectors");
        }
        let report = f
            .client
            .pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap();
        assert!(report.complete, "{field}: {:?}", report.data.errors);
        assert_eq!(report.data.pull_request["title"], "Validated new selectors");
        assert_eq!(
            report.data.pull_request["head"]["sha"],
            report.data.ci.head_sha
        );
        assert_eq!(
            report.data.pull_request["merge_commit_sha"].as_str(),
            report.data.ci.merge_sha.as_deref()
        );
        assert_eq!(
            f.metadata_calls(),
            2,
            "{field}: the retry must reuse the personal response already validated during collection"
        );
    }
}

#[tokio::test]
async fn full_report_overlap_rechecks_a_push_after_prefetch_expires() {
    let f = Fixture::new().await;
    let task = pending_full_report(&f).await;
    let old = f.age_metadata(60_000);
    f.mock.reviews_release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f
            .client
            .pull_request("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap()
            .validated_at_ms
            == old
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.age_metadata(16_000);
    let before = f.metadata_calls();
    f.mock.metadata.lock().unwrap()["head"]["sha"] = json!("c".repeat(40));
    f.mock.pause_reviews.store(false, Ordering::Relaxed);
    f.mock.pause_graph.store(false, Ordering::Relaxed);
    f.mock.release.notify_one();
    let report = task.await.unwrap().unwrap();
    assert!(report.complete);
    assert_eq!(report.data.pull_request["head"]["sha"], "c".repeat(40));
    assert_eq!(report.data.ci.head_sha, "c".repeat(40));
    assert_eq!(
        f.metadata_calls(),
        before + 1,
        "Retry must reuse final personal metadata after detecting the push"
    );
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
    f.mock.pause_metadata.store(true, Ordering::Relaxed);
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
    // Hold the response until the sibling and overlap read have joined it.
    // Otherwise the response may legitimately capture the old title before
    // this mock mutation, making the test depend on executor scheduling.
    let coalesced = f.client.status().coalesced_requests;
    let reader = f.client.clone();
    let sibling = tokio::spawn(async move {
        reader
            .pull_request("acme/demo", 7, Freshness::Revalidate)
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.client.status().coalesced_requests == coalesced {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.mock.metadata_release.notify_one();
    sibling.await.unwrap().unwrap();
    f.mock.reviews_release.notify_one();
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
    f.mock.reviews_release.notify_one();
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
