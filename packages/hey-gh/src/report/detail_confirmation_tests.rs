use super::*;
use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};
use std::{sync::Mutex, time::Duration};
use tokio::sync::Notify;
#[path = "review_report_tests.rs"]
mod review_report_tests;

struct Mock {
    calls: Mutex<Vec<String>>,
    metadata: Mutex<Value>,
    pause_reviews: AtomicBool,
    pause_graph: AtomicBool,
    pause_metadata: AtomicBool,
    deny_metadata: AtomicBool,
    deny_timeline: AtomicBool,
    deny_path: Mutex<Option<String>>,
    pause_timeline: AtomicBool,
    timeline_release: Notify,
    metadata_release: Notify,
    reviews_release: Notify,
    release: Notify,
    core_release: Notify,
    ordinary_release: Notify,
    detail_release: Notify,
    ordinary_detail_release: Notify,
}
async fn handler(State(mock): State<Arc<Mock>>, uri: Uri) -> Response {
    let path = uri.path();
    mock.calls.lock().unwrap().push(path.into());
    if mock.deny_path.lock().unwrap().as_deref() == Some(path) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"message":"source denied"})),
        )
            .into_response();
    }
    if path.ends_with("/timeline") && mock.pause_timeline.load(Ordering::Relaxed) {
        mock.timeline_release.notified().await;
    }
    if path.ends_with("/timeline") && mock.deny_timeline.load(Ordering::Relaxed) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"message":"timeline denied"})),
        )
            .into_response();
    }
    if path.ends_with("/access_tokens") {
        return (StatusCode::CREATED, Json(json!({"token":"synthetic-app-token", "expires_at":
            chrono::DateTime::from_timestamp((now_ms()/1000 + 3600) as i64, 0).unwrap().to_rfc3339()}))).into_response();
    }
    if path.ends_with("/pulls/8") {
        mock.core_release.notified().await;
    }
    if path.ends_with("/pulls/9") {
        mock.ordinary_release.notified().await;
    }
    if path.ends_with("/issues/8/comments") {
        mock.detail_release.notified().await;
    }
    if path.ends_with("/issues/9/comments") {
        mock.ordinary_detail_release.notified().await;
    }
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
        Self::with_app(false).await
    }
    async fn with_app(installation: bool) -> Self {
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
            deny_timeline: AtomicBool::new(false),
            deny_path: Mutex::new(None),
            pause_timeline: AtomicBool::new(false),
            timeline_release: Notify::new(),
            metadata_release: Notify::new(),
            reviews_release: Notify::new(),
            release: Notify::new(),
            core_release: Notify::new(),
            ordinary_release: Notify::new(),
            detail_release: Notify::new(),
            ordinary_detail_release: Notify::new(),
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
                installation: installation.then(|| {
                    crate::AppInstallation::new(
                        "synthetic-client".into(),
                        42,
                        vec!["acme/demo".into()],
                        include_str!("../../tests/fixtures/github-app-test-key.pem"),
                    )
                    .unwrap()
                }),
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
async fn full_report_admits_timeline_before_other_rest_details_without_waiting_for_http() {
    for shared in [false, true] {
        timeline_admission_scenario(shared, false).await;
    }
}

#[tokio::test]
async fn cancelling_timeline_preparation_does_not_start_waiting_details() {
    timeline_admission_scenario(false, true).await;
}

#[tokio::test]
async fn invalid_timeline_before_admission_keeps_other_sources_and_its_error() {
    let f = Fixture::new().await;
    assert!(
        f.client
            .pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    // A cached malformed next link fails before any request is admitted.
    // Independent detail errors must still be collected, without deadlock.
    rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap().execute(
        "UPDATE cache SET response=json_set(response,'$.link','<https://foreign.invalid/page>; rel=\"next\"') WHERE key LIKE '%/timeline?%'", [],
    ).unwrap();
    rusqlite::Connection::open(f.dir.path().join("cache.sqlite"))
        .unwrap()
        .execute("DELETE FROM cache WHERE key LIKE '%/reviews?%'", [])
        .unwrap();
    f.mock
        .deny_path
        .lock()
        .unwrap()
        .replace("/repos/acme/demo/pulls/7/reviews".into());
    let report = tokio::time::timeout(
        Duration::from_secs(3),
        f.client
            .pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30))),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!report.complete);
    assert!(
        report
            .data
            .errors
            .iter()
            .any(|error| error.source == "timeline")
    );
    assert!(
        report
            .data
            .errors
            .iter()
            .any(|error| error.source == "reviews")
    );
}

async fn timeline_admission_scenario(shared: bool, cancel: bool) {
    let f = Fixture::new().await;
    assert!(
        f.client
            .pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    // Keep CI, GraphQL, and metadata warm so only the four REST detail
    // collections can contribute outstanding requests below.
    rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap().execute(
        "DELETE FROM cache WHERE key LIKE '%/timeline?%' OR key LIKE '%/comments?%' OR key LIKE '%/reviews?%'", [],
    ).unwrap();
    f.mock.calls.lock().unwrap().clear();
    f.mock.pause_timeline.store(true, Ordering::Relaxed);
    let path = "repos/acme/demo/issues/7/timeline?per_page=100";
    let peer = shared.then(|| {
        let client = f.client.clone();
        tokio::spawn(async move { client.get(path, Freshness::Revalidate).await })
    });
    if shared {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !f
                .mock
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.ends_with("/timeline"))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    let entered = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let read = tokio::spawn({
        let (client, entered, resume) = (f.client.clone(), entered.clone(), resume.clone());
        async move {
            crate::client::CACHE_LOOKUP_PATH_GATE
                .scope(
                    std::cell::RefCell::new(Some((path.into(), entered, resume))),
                    client.pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30))),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    let is_detail = |p: &str| p.ends_with("/comments") || p.ends_with("/reviews");
    let overtook = tokio::time::timeout(Duration::from_millis(250), async {
        while !f.mock.calls.lock().unwrap().iter().any(|p| is_detail(p)) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        overtook.is_err(),
        "REST details overtook timeline preparation: {:?}",
        f.mock.calls.lock().unwrap()
    );
    if cancel {
        read.abort();
        assert!(read.await.unwrap_err().is_cancelled());
        resume.notify_one();
        assert!(!f.mock.calls.lock().unwrap().iter().any(|p| is_detail(p)));
        return;
    }
    resume.notify_one();
    tokio::time::timeout(Duration::from_secs(3), async {
        // The HTTP/1 fixture serializes detail dispatch. All siblings must
        // nevertheless enter the queue while the timeline response is held.
        while f.client.status().outstanding_requests < 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("details must overlap the admitted timeline's HTTP response");
    assert!(!read.is_finished());
    f.mock.pause_timeline.store(false, Ordering::Relaxed);
    f.mock.timeline_release.notify_one();
    let report = tokio::time::timeout(Duration::from_secs(3), read)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(report.complete, "{:?}", report.data.errors);
    if let Some(peer) = peer {
        assert!(peer.await.unwrap().is_ok());
    }
    let before = f.mock.calls.lock().unwrap().len();
    assert_eq!(
        f.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.ends_with("/timeline"))
            .count(),
        1
    );
    let cached = tokio::time::timeout(
        Duration::from_secs(3),
        f.client.pr_report("acme/demo", 7, Freshness::CachedOnly),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(cached.complete);
    assert_eq!(f.mock.calls.lock().unwrap().len(), before);
}

#[tokio::test]
async fn review_evidence_does_not_require_or_fetch_the_rest_timeline() {
    let f = Fixture::new().await;
    f.mock.deny_timeline.store(true, Ordering::Relaxed);
    let report = f
        .client
        .pr_review_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(
        report.complete,
        "Review evidence must not depend on unused timeline access"
    );
    assert!(
        !f.mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.ends_with("/timeline"))
    );
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
async fn short_http_report_overlaps_stale_metadata_with_all_pending_sources() {
    let f = Fixture::new().await;
    f.warm_metadata().await;
    f.age_metadata(60_000);
    f.mock.pause_reviews.store(true, Ordering::Relaxed);
    f.mock.pause_graph.store(true, Ordering::Relaxed);
    // CI cannot supply an incidental metadata refresh for this regression.
    let lock = f.client.report_lock("acme/demo#7:ci");
    let guard = lock.lock().await;
    let before = f.metadata_calls();
    let api = crate::api::Api::new(f.client.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, api.router()).await.unwrap() });
    let reader = crate::ApiClient::new(url.parse().unwrap())
        .unwrap()
        .with_read_deadline(tokio::time::Instant::now() + Duration::from_secs(15));
    let task = tokio::spawn(async move {
        reader
            .pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
    });
    let overlapped = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if f.metadata_calls() > before {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let pending = !task.is_finished();
    drop(guard);
    f.mock.pause_reviews.store(false, Ordering::Relaxed);
    f.mock.pause_graph.store(false, Ordering::Relaxed);
    f.mock.reviews_release.notify_one();
    f.mock.release.notify_one();
    let report = task.await.unwrap().unwrap();
    server.abort();
    assert!(
        overlapped.is_ok(),
        "Stale metadata must start before CI and conversation sources finish"
    );
    assert!(pending);
    assert!(report.complete, "{:?}", report.data.errors);
    assert_eq!(
        f.metadata_calls(),
        before + 1,
        "Early confirmation must be reused, not fetched twice"
    );
}

#[tokio::test]
async fn fresh_or_long_full_reports_do_not_start_early_metadata_work() {
    for (seconds, metadata_age) in [(15, 0), (60, 60_000)] {
        let f = Fixture::new().await;
        f.warm_metadata().await;
        f.age_metadata(metadata_age);
        f.mock.pause_reviews.store(true, Ordering::Relaxed);
        f.mock.pause_graph.store(true, Ordering::Relaxed);
        let lock = f.client.report_lock("acme/demo#7:ci");
        let guard = lock.lock().await;
        let before = f.metadata_calls();
        let reader = f.client.clone();
        let task = tokio::spawn(async move {
            crate::client::READ_DEADLINE
                .scope(
                    tokio::time::Instant::now() + Duration::from_secs(seconds),
                    reader.pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30))),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !f
                .mock
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|path| path == "/graphql")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let early = tokio::time::timeout(Duration::from_millis(100), async {
            while f.metadata_calls() == before {
                tokio::task::yield_now().await;
            }
        })
        .await;
        drop(guard);
        f.mock.pause_reviews.store(false, Ordering::Relaxed);
        f.mock.pause_graph.store(false, Ordering::Relaxed);
        f.mock.reviews_release.notify_one();
        f.mock.release.notify_one();
        assert!(task.await.unwrap().unwrap().complete);
        assert!(
            early.is_err(),
            "deadline={seconds}, metadata_age={metadata_age}"
        );
        assert_eq!(f.metadata_calls(), before + usize::from(metadata_age > 0));
    }
}

#[tokio::test]
async fn short_report_refreshes_expiring_metadata_while_sources_are_pending() {
    let f = Fixture::new().await;
    f.warm_metadata().await;
    f.age_metadata(14_000);
    f.mock.pause_reviews.store(true, Ordering::Relaxed);
    f.mock.pause_graph.store(true, Ordering::Relaxed);
    // Prevent nested CI from refreshing the personal metadata incidentally.
    let lock = f.client.report_lock("acme/demo#7:ci");
    let guard = lock.lock().await;
    let before = f.metadata_calls();
    let reader = f.client.clone();
    let task = tokio::spawn(async move {
        crate::client::READ_DEADLINE
            .scope(
                tokio::time::Instant::now() + Duration::from_secs(15),
                reader.pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30))),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        f.metadata_calls(),
        before,
        "Still-fresh metadata must not trigger an eager network request"
    );
    let refreshed = tokio::time::timeout(Duration::from_secs(3), async {
        while f.metadata_calls() == before {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let pending = !task.is_finished();
    drop(guard);
    f.mock.pause_reviews.store(false, Ordering::Relaxed);
    f.mock.pause_graph.store(false, Ordering::Relaxed);
    f.mock.reviews_release.notify_one();
    f.mock.release.notify_one();
    let report = task.await.unwrap().unwrap();
    assert!(
        refreshed.is_ok(),
        "Expired metadata waited for unrelated pending sources before refreshing"
    );
    assert!(pending);
    assert!(report.complete, "{:?}", report.data.errors);
    assert_eq!(f.metadata_calls(), before + 1);
}

#[tokio::test]
async fn short_report_does_not_wait_for_metadata_expiry_after_sources_finish() {
    let f = Fixture::new().await;
    f.warm_metadata().await;
    f.age_metadata(10_000);
    let before = f.metadata_calls();
    let report = tokio::time::timeout(
        Duration::from_secs(2),
        crate::client::READ_DEADLINE.scope(
            tokio::time::Instant::now() + Duration::from_secs(15),
            f.client
                .pr_report("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30))),
        ),
    )
    .await
    .expect("A completed report must not wait for still-fresh metadata to expire")
    .unwrap();
    assert!(report.complete);
    assert_eq!(f.metadata_calls(), before);
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
async fn full_report_tail_confirmation_takes_a_reserved_turn_before_unrelated_slow_reads() {
    let f = Fixture::new().await;
    let report = pending_full_report(&f).await;
    let read = |number| {
        let client = f.client.clone();
        tokio::spawn(async move {
            crate::client::INTERACTIVE_READ
                .scope(
                    Arc::new(AtomicBool::new(true)),
                    client.get(
                        &format!("repos/acme/demo/pulls/{number}"),
                        Freshness::Revalidate,
                    ),
                )
                .await
        })
    };
    let gate = read(8);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !f
            .mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.ends_with("/pulls/8"))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let pending = f.client.status().outstanding_requests;
    let ordinary = read(9);
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.client.status().outstanding_requests != pending + 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let before = f.metadata_calls();
    f.age_metadata(60_000);
    // An existing ordinary reader must be promoted, not duplicated, when the
    // report has collected five groups and needs its final personal metadata.
    let shared = read(7);
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.client.status().outstanding_requests != pending + 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let coalesced = f.client.status().coalesced_requests;
    f.mock.reviews_release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.client.status().coalesced_requests == coalesced {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.mock.core_release.notify_one();
    let confirmed = tokio::time::timeout(Duration::from_millis(500), async {
        while f.metadata_calls() == before {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let still_collecting = !report.is_finished();
    f.mock.ordinary_release.notify_one();
    f.mock.release.notify_one();
    gate.await.unwrap().unwrap();
    ordinary.await.unwrap().unwrap();
    shared.await.unwrap().unwrap();
    let report = report.await.unwrap().unwrap();
    assert!(
        confirmed.is_ok(),
        "Required metadata stayed behind an unrelated blocked read"
    );
    assert!(
        still_collecting,
        "The final conversation source must remain required"
    );
    assert!(report.complete, "{:?}", report.data.errors);
    assert_eq!(f.metadata_calls(), before + 1);
}

#[tokio::test]
async fn early_personal_confirmation_is_promoted_when_app_ci_and_four_details_finish() {
    for progressed in [false, true] {
        let f = Fixture::with_app(true).await;
        assert!(
            f.client
                .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
                .await
                .unwrap()
                .complete
        );
        f.warm_metadata().await;
        f.age_metadata(60_000);
        f.mock.pause_reviews.store(true, Ordering::Relaxed);
        f.mock.pause_graph.store(true, Ordering::Relaxed);
        let read = |number| {
            let client = f.client.clone();
            tokio::spawn(async move {
                crate::client::READ_DEADLINE
                    .scope(
                        tokio::time::Instant::now() + Duration::from_secs(15),
                        crate::client::INTERACTIVE_READ.scope(
                            Arc::new(AtomicBool::new(true)),
                            client.get(
                                &format!("repos/acme/demo/pulls/{number}"),
                                Freshness::Revalidate,
                            ),
                        ),
                    )
                    .await
            })
        };
        let gate = read(8);
        tokio::time::timeout(Duration::from_secs(2), async {
            while !f
                .mock
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.ends_with("/pulls/8"))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let ordinary = read(9);
        tokio::time::timeout(Duration::from_secs(2), async {
            while f.client.status().outstanding_requests != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let shared = read(7);
        tokio::time::timeout(Duration::from_secs(2), async {
            while f.client.status().outstanding_requests != 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let before = f.metadata_calls();
        let coalesced = f.client.status().coalesced_requests;
        let reader = f.client.clone();
        let report = tokio::spawn(async move {
            crate::client::READ_DEADLINE
                .scope(
                    tokio::time::Instant::now() + Duration::from_secs(15),
                    reader.pr_report("acme/demo", 7, Freshness::default()),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let calls = f.mock.calls.lock().unwrap().clone();
                if calls.iter().any(|p| p.ends_with("/reviews"))
                    && calls.iter().any(|p| p == "/graphql")
                    && f.client.status().coalesced_requests > coalesced
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // All three core requests are still queued/held. The early request must
        // only gain completion priority after the fifth source group finishes.
        if progressed {
            f.mock.reviews_release.notify_one();
            assert!(
                f.client
                    .ci_for_pr("acme/demo", 7, Freshness::default())
                    .await
                    .unwrap()
                    .complete
            );
            tokio::time::timeout(Duration::from_secs(2), async {
                while f.client.status().outstanding_requests != 4 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        tokio::task::yield_now().await;
        f.mock.core_release.notify_one();
        let confirmed = tokio::time::timeout(Duration::from_millis(500), async {
            while f.metadata_calls() == before {
                tokio::task::yield_now().await;
            }
        })
        .await;
        let still_collecting = !report.is_finished();
        f.mock.reviews_release.notify_one();
        f.mock.ordinary_release.notify_one();
        f.mock.release.notify_one();
        gate.await.unwrap().unwrap();
        ordinary.await.unwrap().unwrap();
        shared.await.unwrap().unwrap();
        let report = report.await.unwrap().unwrap();
        assert_eq!(
            confirmed.is_ok(),
            progressed,
            "Early personal confirmation must gain a completion turn only after five groups finish"
        );
        assert!(still_collecting);
        assert!(report.complete, "{:?}", report.data.errors);
        assert_eq!(f.metadata_calls(), before + 1);
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

#[tokio::test]
async fn last_report_detail_takes_a_completion_turn_before_unrelated_slow_details() {
    for coalesced in [false, true] {
        let f = Fixture::new().await;
        let freshness = Freshness::MaxAge(Duration::from_secs(30));
        assert!(
            f.client
                .pr_report("acme/demo", 7, freshness)
                .await
                .unwrap()
                .complete
        );
        rusqlite::Connection::open(f.dir.path().join("cache.sqlite"))
            .unwrap()
            .execute(
                "DELETE FROM cache WHERE key LIKE '%/issues/7/timeline%'",
                [],
            )
            .unwrap();
        let read = |path: &'static str| {
            let client = f.client.clone();
            tokio::spawn(async move {
                crate::client::INTERACTIVE_READ
                    .scope(
                        Arc::new(AtomicBool::new(true)),
                        client.get(path, Freshness::Revalidate),
                    )
                    .await
            })
        };
        let gate = read("repos/acme/demo/issues/8/comments");
        tokio::time::timeout(Duration::from_secs(2), async {
            while !f
                .mock
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.ends_with("/issues/8/comments"))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let ordinary = read("repos/acme/demo/issues/9/comments");
        tokio::time::timeout(Duration::from_secs(2), async {
            while f.client.status().outstanding_requests != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let shared = coalesced.then(|| read("repos/acme/demo/issues/7/timeline?per_page=100"));
        let before = f.client.status().coalesced_requests;
        let reader = f.client.clone();
        let report = tokio::spawn(async move {
            crate::client::INTERACTIVE_READ
                .scope(
                    Arc::new(AtomicBool::new(true)),
                    reader.pr_report("acme/demo", 7, freshness),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while f.client.status().outstanding_requests != 3
                || (coalesced && f.client.status().coalesced_requests == before)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // Let the remaining cached groups finish while the detail socket is held.
        tokio::time::sleep(Duration::from_millis(100)).await;
        f.mock.detail_release.notify_one();
        let mut report = report;
        let completed = tokio::time::timeout(Duration::from_millis(500), &mut report).await;
        f.mock.ordinary_detail_release.notify_one();
        gate.await.unwrap().unwrap();
        ordinary.await.unwrap().unwrap();
        if let Some(shared) = shared {
            shared.await.unwrap().unwrap();
        }
        let (finished_early, result) = match completed {
            Ok(result) => (true, result),
            Err(_) => (false, report.await),
        };
        assert!(result.unwrap().unwrap().complete);
        assert!(
            finished_early,
            "The final timeline remained behind an unrelated blocked detail read (coalesced={coalesced})"
        );
    }
}

#[tokio::test]
async fn two_pending_report_sources_progress_without_taking_the_ordinary_turn() {
    pending_sources_progress(TailCollection::Full).await;
}

#[tokio::test]
async fn background_detail_tail_progresses_without_taking_the_ordinary_turn() {
    pending_sources_progress(TailCollection::Details).await;
}

#[tokio::test]
async fn ci_tail_progresses_without_taking_the_ordinary_turn() {
    pending_sources_progress(TailCollection::Ci).await;
}

#[derive(Clone, Copy)]
enum TailCollection {
    Full,
    Details,
    Ci,
}

async fn pending_sources_progress(collection: TailCollection) {
    let ci = matches!(collection, TailCollection::Ci);
    let (gate_path, ordinary_path, shared_path) = if ci {
        (
            "repos/acme/demo/pulls/8",
            "repos/acme/demo/pulls/9",
            "repos/acme/demo/commits/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/status?per_page=100",
        )
    } else {
        (
            "repos/acme/demo/issues/8/comments",
            "repos/acme/demo/issues/9/comments",
            "repos/acme/demo/issues/7/timeline?per_page=100",
        )
    };

    for (coalesced, interactive) in [(false, false), (false, true), (true, false), (true, true)] {
        let f = Fixture::new().await;
        let freshness = Freshness::MaxAge(Duration::from_secs(30));
        assert!(
            f.client
                .pr_report("acme/demo", 7, freshness)
                .await
                .unwrap()
                .complete
        );
        f.mock.calls.lock().unwrap().clear();
        rusqlite::Connection::open(f.dir.path().join("cache.sqlite"))
            .unwrap()
            .execute(
                if ci { "DELETE FROM cache WHERE key LIKE '%/check-runs?%' OR key LIKE '%/status?%'" } else { "DELETE FROM cache WHERE key LIKE '%/issues/7/timeline%' OR key LIKE '%/pulls/7/reviews%'" },
                [],
            )
            .unwrap();
        let read = |path: &'static str| {
            let client = f.client.clone();
            tokio::spawn(async move {
                crate::client::INTERACTIVE_READ
                    .scope(
                        Arc::new(AtomicBool::new(interactive)),
                        client.get(path, Freshness::Revalidate),
                    )
                    .await
            })
        };
        let gate = read(gate_path);
        tokio::time::timeout(Duration::from_secs(2), async {
            while !f
                .mock
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.ends_with(gate_path))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let ordinary = read(ordinary_path);
        tokio::time::timeout(Duration::from_secs(2), async {
            while f.client.status().outstanding_requests != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let shared = coalesced.then(|| read(shared_path));
        let before = f.client.status().coalesced_requests;
        let reader = f.client.clone();
        let report = tokio::spawn(async move {
            crate::client::INTERACTIVE_READ
                .scope(Arc::new(AtomicBool::new(interactive)), async {
                    if ci {
                        reader
                            .ci_report("acme/demo", &"a".repeat(40), None, freshness)
                            .await
                            .map(|report| report.errors.is_empty())
                    } else if matches!(collection, TailCollection::Details) {
                        reader
                            .refresh_pr_details("acme/demo", 7, freshness)
                            .await
                            .map(|errors| errors.is_empty())
                    } else {
                        reader
                            .pr_report("acme/demo", 7, freshness)
                            .await
                            .map(|report| report.complete)
                    }
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while f.client.status().outstanding_requests != 4
                || (coalesced && f.client.status().coalesced_requests == before)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // Other groups finish from cache while both remaining collections
        // wait for their occupied lane. Neither depends on the other.
        tokio::time::sleep(Duration::from_millis(100)).await;
        if ci {
            f.mock.core_release.notify_one();
        } else {
            f.mock.detail_release.notify_one();
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while !f
                .mock
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.ends_with(ordinary_path))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Ordinary reads must retain their alternating turn");
        let calls = f.mock.calls.lock().unwrap().clone();
        let ordinary_at = calls
            .iter()
            .position(|p| p.ends_with(ordinary_path))
            .unwrap();
        let completed_before_ordinary = calls[..ordinary_at]
            .iter()
            .filter(|p| {
                if ci {
                    p.ends_with("/check-runs") || p.ends_with("/status")
                } else {
                    p.ends_with("/reviews") || p.ends_with("/timeline")
                }
            })
            .count();
        if ci {
            f.mock.ordinary_release.notify_one();
        } else {
            f.mock.ordinary_detail_release.notify_one();
        }
        gate.await.unwrap().unwrap();
        ordinary.await.unwrap().unwrap();
        if let Some(shared) = shared {
            shared.await.unwrap().unwrap();
        }
        assert!(report.await.unwrap().unwrap());
        assert_eq!(
            completed_before_ordinary, 1,
            "Two required collections must make progress, then yield the next turn (coalesced={coalesced}, interactive={interactive}): {calls:?}"
        );
    }
}
