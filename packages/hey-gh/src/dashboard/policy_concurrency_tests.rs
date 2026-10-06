use super::*;

struct Fixture {
    client: Client,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new(queue_capacity: usize) -> Self {
        Self::with_origin(queue_capacity, "http://127.0.0.1:9").await
    }

    async fn with_origin(queue_capacity: usize, origin: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            crate::Config {
                cache_path: dir.path().join("cache.sqlite"),
                rest_url: format!("{origin}/").parse().unwrap(),
                graphql_url: format!("{origin}/graphql").parse().unwrap(),
                queue_capacity,
                report_timeout: Duration::from_secs(10),
                ..Default::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let mut nodes = Vec::new();
        for number in 1..=3 {
            let head = format!("{number:040x}");
            nodes.push(
                json!({"id":format!("PR_{number}"),"number":number,"state":"OPEN",
                "headRefOid":head,"repository":{"nameWithOwner":"acme/demo"}}),
            );
            client.save_derived(&format!("{origin}/repos/acme/demo/pulls/{number}"), json!({
                "node_id":format!("PR_{number}"),"number":number,"state":"open","merged":false,
                "head":{"sha":head},"base":{"ref":"main","sha":head},"merge_commit_sha":null
            })).await.unwrap();
            client
                .observe(
                    &format!("ci://github.com/acme/demo/{number}"),
                    &json!({
                        "head_sha":head,"merge_sha":null,"errors":[]
                    }),
                )
                .await
                .unwrap();
            client
                .save_derived(
                    &format!("account-status-validated:ci:acme/demo/{number}"),
                    json!(crate::now_ms()),
                )
                .await
                .unwrap();
            client.save_derived(&format!("{origin}/repos/acme/demo/commits/{head}/check-runs?filter=latest&per_page=100"),json!({"total_count":0,"check_runs":[]})).await.unwrap();
            client.save_derived(&format!("{origin}/repos/acme/demo/commits/{head}/status?per_page=100"),json!({
                "sha":head,"total_count":1,"statuses":[{"id":number,"context":"build","state":if number == 2 {"failure"} else {"success"}}]
            })).await.unwrap();
        }
        client
            .save_derived(
                DISCOVERY_CACHE,
                json!({"pulls":nodes,"validatedAtMs":crate::now_ms()}),
            )
            .await
            .unwrap();
        client.save_derived(&format!("{origin}/repos/acme/demo/branches/main"),json!({
            "commit":{"sha":format!("{:040x}",4)},"protected":true,
            "protection":{"enabled":false,"required_status_checks":{"enforcement_level":"off","contexts":[],"checks":[]}}
        })).await.unwrap();
        client.save_derived(&format!("{origin}/repos/acme/demo/rules/branches/main"),json!([{
            "type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,
            "required_status_checks":[{"context":"build","integration_id":null}]}
        }])).await.unwrap();
        Self { client, _dir: dir }
    }

    fn start(&self) -> tokio::task::JoinHandle<Result<Vec<String>>> {
        let client = self.client.clone();
        tokio::spawn(async move {
            client
                .hydrate_pr_status_policy(Freshness::MaxAge(Duration::from_secs(60)))
                .await
        })
    }

    async fn report(&self, number: u64) -> Option<Value> {
        self.client
            .stored_snapshot(&format!("required_checks://github.com/acme/demo/{number}"))
            .await
            .unwrap()
    }

    async fn wait_report(&self, number: u64) -> Value {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(report) = self.report(number).await {
                    break report;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("cached policy waited for another PR's locked report")
    }
}

#[tokio::test]
async fn cached_policy_progresses_while_another_pr_waits_and_keeps_evidence_separate() {
    let f = Fixture::new(256).await;
    let lock = f.client.report_lock("required-checks:acme/demo#1");
    let guard = lock.lock().await;
    let cycle = f.start();
    let failed = f.wait_report(2).await;
    let passed = f.wait_report(3).await;
    assert_eq!(failed["state"], "failure");
    assert_eq!(passed["state"], "satisfied");
    assert_eq!(failed["head_sha"], format!("{:040x}", 2));
    assert_eq!(passed["head_sha"], format!("{:040x}", 3));
    assert!(f.report(1).await.is_none());
    assert!(!cycle.is_finished());
    drop(guard);
    assert!(cycle.await.unwrap().unwrap().is_empty());
    assert_eq!(f.wait_report(1).await["state"], "satisfied");
    assert_eq!(f.client.status().network_requests, 0);
    assert_eq!(f.client.status().queue_full_rejections, 0);
}

#[tokio::test]
async fn an_offline_policy_read_never_waits_for_the_same_pr_or_publishes_over_its_owner() {
    let f = Fixture::new(256).await;
    let lock = f.client.report_lock("required-checks:acme/demo#1");
    let _owner = lock.lock().await;
    let before = f.client.bootstrap().await.unwrap();
    let metadata = f
        .client
        .pull_request("acme/demo", 1, Freshness::CachedOnly)
        .await
        .unwrap();
    let report = tokio::time::timeout(
        Duration::from_millis(500),
        f.client
            .required_checks_for_pr("acme/demo", 1, Freshness::CachedOnly),
    )
    .await
    .expect("offline policy waited for its live owner")
    .unwrap();
    assert_eq!(report.state, "satisfied");
    assert_eq!(report.head_sha, format!("{:040x}", 1));
    assert_eq!(report.cursor, before.cursor);
    assert!(
        report
            .validations
            .iter()
            .any(|v| v.resource.ends_with("/pulls/1")
                && v.validated_at_ms == metadata.validated_at_ms)
    );
    let after = f.client.bootstrap().await.unwrap();
    assert_eq!(after.cursor, before.cursor);
    assert_eq!(
        serde_json::to_value(after.snapshots).unwrap(),
        serde_json::to_value(before.snapshots).unwrap()
    );
    assert_eq!(f.client.status().network_requests, 0);
}

#[tokio::test]
async fn a_contended_offline_policy_preserves_missing_evidence_and_denials() {
    for absent_metadata in [false, true] {
        let f = Fixture::new(256).await;
        if absent_metadata {
            let db = rusqlite::Connection::open(f._dir.path().join("cache.sqlite")).unwrap();
            db.execute("DELETE FROM cache WHERE key LIKE '%/pulls/1'", [])
                .unwrap();
        } else {
            f.client
                .save_derived(
                    "policy-error://github.com/repos/acme/demo/rules/branches/main",
                    json!({"status":403,"message":"Policy inaccessible"}),
                )
                .await
                .unwrap();
        }
        let lock = f.client.report_lock("required-checks:acme/demo#1");
        let _owner = lock.lock().await;
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            f.client
                .required_checks_for_pr("acme/demo", 1, Freshness::CachedOnly),
        )
        .await
        .expect("unavailable offline policy waited for its live owner");
        if absent_metadata {
            assert!(matches!(result, Err(Error::CacheMiss)));
        } else {
            let report = result.unwrap();
            assert_eq!(report.state, "unknown");
            assert!(report.errors.iter().any(|e| e.source == "rulesets"));
        }
        assert!(f.report(1).await.is_none());
        assert_eq!(f.client.status().network_requests, 0);
    }
}

#[tokio::test]
async fn offline_policy_cursor_does_not_skip_a_publication_during_cached_collection() {
    let f = Fixture::new(256).await;
    let lock = f.client.report_lock("required-checks:acme/demo#1");
    let _owner = lock.lock().await;
    let before = f.client.bootstrap().await.unwrap();
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let reader = tokio::spawn({
        let (client, entered, release) = (f.client.clone(), entered.clone(), release.clone());
        async move {
            crate::client::CACHE_LOOKUP_GATE
                .scope(
                    std::cell::RefCell::new(Some((entered, release))),
                    client.required_checks_for_pr("acme/demo", 1, Freshness::CachedOnly),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    let resource = "branch://github.com/acme/demo/other";
    f.client
        .observe(resource, &json!({"sha":format!("{:040x}", 5)}))
        .await
        .unwrap();
    let published = f.client.bootstrap().await.unwrap();
    assert_ne!(published.cursor, before.cursor);
    release.notify_one();
    let report = reader.await.unwrap().unwrap();
    assert_eq!(
        report.cursor, before.cursor,
        "concurrent changes must remain replayable"
    );
    assert_eq!(f.client.bootstrap().await.unwrap().cursor, published.cursor);
    assert!(f.report(1).await.is_none());
    assert_eq!(f.client.status().network_requests, 0);
}

#[tokio::test]
async fn confirmation_socket_does_not_block_cached_policy_or_publish_early() {
    use std::sync::Arc;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let f = Fixture::with_origin(256, &origin).await;
    let body = f
        .client
        .peek_get("repos/acme/demo/pulls/1")
        .await
        .unwrap()
        .data;
    let router = axum::Router::new().route(
        "/repos/acme/demo/pulls/1",
        axum::routing::get({
            let entered = entered.clone();
            let release = release.clone();
            move || {
                let entered = entered.clone();
                let release = release.clone();
                let body = body.clone();
                async move {
                    entered.notify_one();
                    release.notified().await;
                    axum::Json(body)
                }
            }
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // Valid for collection/admission, but outside the final 15-second bound.
    let db = rusqlite::Connection::open(f._dir.path().join("cache.sqlite")).unwrap();
    db.execute(
        "UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key=?2",
        rusqlite::params![
            crate::now_ms() - 20_000,
            format!("{origin}/repos/acme/demo/pulls/1")
        ],
    )
    .unwrap();
    drop(db);
    let cycle = f.start();
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert_eq!(f.wait_report(2).await["state"], "failure");
    assert_eq!(f.wait_report(3).await["state"], "satisfied");
    assert!(
        f.report(1).await.is_none(),
        "unconfirmed policy was published"
    );
    assert_eq!(f.client.status().network_requests, 1);
    release.notify_one();
    let result = cycle.await.unwrap().unwrap();
    server.abort();
    assert!(result.is_empty(), "{result:?}");
    assert_eq!(f.wait_report(1).await["state"], "satisfied");
    assert_eq!(f.client.status().network_requests, 1);
}

#[tokio::test]
async fn policy_overlap_is_bounded_and_small_queues_remain_sequential() {
    for (capacity, second_can_progress) in [(256, true), (8, false)] {
        let f = Fixture::new(capacity).await;
        let first = f.client.report_lock("required-checks:acme/demo#1");
        let second = f.client.report_lock("required-checks:acme/demo#2");
        let first_guard = first.lock().await;
        let second_guard = second.lock().await;
        let cycle = f.start();
        // Polling persisted admission avoids assuming the spawned cycle ran.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if f.client
                    .peek_derived("account-status-schedule:policy")
                    .await
                    .unwrap()
                    .is_some_and(|v| {
                        v.data["next"]
                            == json!(["acme/demo", if second_can_progress { 3 } else { 2 }])
                    })
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("policy admissions did not reach their bounded capacity");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            f.report(3).await.is_none(),
            "third read exceeded policy concurrency"
        );
        drop(second_guard);
        if second_can_progress {
            assert_eq!(f.wait_report(3).await["state"], "satisfied");
        } else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(
                f.report(2).await.is_none(),
                "small queue allowed overlapping reads"
            );
        }
        drop(first_guard);
        assert!(cycle.await.unwrap().unwrap().is_empty());
        assert_eq!(f.client.status().network_requests, 0);
    }
}
