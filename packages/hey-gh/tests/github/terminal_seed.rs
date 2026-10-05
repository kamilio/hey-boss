use super::*;

async fn seed(merged: bool, installation: bool) -> (Harness, Client) {
    let h = Harness::new().await;
    h.mode(if merged {
        "ci-rest-terminal-merged"
    } else {
        "ci-rest-terminal-closed"
    });
    let config = if installation {
        ci_app_selectors::app_config(&h)
    } else {
        h.config()
    };
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    let warm = c
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(
        warm.complete,
        "{:?} {:?}",
        warm.data.errors, warm.data.ci.errors
    );
    assert_eq!(warm.data.pull_request["state"], "closed");
    assert_eq!(warm.data.pull_request["merged"], merged);
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',0) WHERE key LIKE '%repos/%'", []).unwrap();
    h.phase(1);
    (h, c)
}

#[tokio::test]
async fn terminal_ci_starts_sources_while_metadata_is_waiting() {
    for merged in [false, true] {
        for installation in [false, true] {
            let (h, c) = seed(merged, installation).await;
            let before = h.calls().len();
            let worker = c.clone();
            let read = tokio::spawn(async move {
                worker.ci_for_pr("acme/demo", 7, Freshness::default()).await
            });
            let started = tokio::time::timeout(Duration::from_millis(500), async {
                loop {
                    if c.status().outstanding_requests > 1
                        || h.calls()[before..]
                            .iter()
                            .any(|call| call.path.ends_with("/check-runs"))
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await;
            assert!(
                started.is_ok(),
                "terminal CI waited for metadata: merged={merged}, app={installation}"
            );
            assert!(
                !read.is_finished(),
                "a stale terminal seed cannot certify CI"
            );
            h.mock.release.notify_one();
            let report = read.await.unwrap().unwrap();
            assert!(report.complete);
            assert_eq!(report.data.head_sha, HEAD);
            assert_eq!(report.data.merge_sha.as_deref(), Some(MERGE));
            assert!(report.validations.iter().all(|v| v.validated_at_ms > 0));
            assert_eq!(
                h.calls()[before..]
                    .iter()
                    .filter(|call| call.path.ends_with("/pulls/7"))
                    .count(),
                1
            );
            assert!(
                h.calls()[before..]
                    .iter()
                    .all(|call| call.path != "/graphql"),
                "terminal seeds must not request the open-only selector proof"
            );
        }
    }
}

#[tokio::test]
async fn terminal_ci_does_not_request_an_open_only_selector_proof() {
    let (h, c) = seed(false, false).await;
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.data.mergeable',json('true')) WHERE key LIKE '%/pulls/7'", []).unwrap();
    let before = h.calls().len();
    h.mock.release.notify_one();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    assert!(
        h.calls()[before..]
            .iter()
            .all(|call| call.path != "/graphql")
    );
}

#[tokio::test]
async fn terminal_report_starts_details_while_metadata_is_waiting() {
    for merged in [false, true] {
        let (h, c) = seed(merged, false).await;
        let before = h.calls().len();
        let worker = c.clone();
        let read =
            tokio::spawn(
                async move { worker.pr_report("acme/demo", 7, Freshness::default()).await },
            );
        let started = tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if h.calls()[before..]
                    .iter()
                    .any(|call| call.path.ends_with("/issues/7/comments"))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(started.is_ok(), "terminal details waited for metadata");
        assert!(
            !read.is_finished(),
            "a stale terminal seed cannot certify details"
        );
        h.mock.release.notify_one();
        let report = read.await.unwrap().unwrap();
        assert!(report.complete);
        assert_eq!(report.data.pull_request["title"], "fresh terminal title");
        assert_eq!(report.data.pull_request["state"], "closed");
        assert_eq!(report.data.pull_request["merged"], merged);
        assert!(report.validations.iter().all(|v| v.validated_at_ms > 0));
    }
}

#[tokio::test]
async fn terminal_seed_never_bypasses_metadata_denial() {
    for merged in [false, true] {
        for installation in [false, true] {
            for full in [false, true] {
                let (h, c) = seed(merged, installation).await;
                h.phase(2);
                let result = if full {
                    c.pr_report("acme/demo", 7, Freshness::default())
                        .await
                        .map(|_| ())
                } else {
                    c.ci_for_pr("acme/demo", 7, Freshness::default())
                        .await
                        .map(|_| ())
                };
                assert!(
                    matches!(result, Err(Error::GitHub { status: 403, .. })),
                    "{result:?}"
                );
                let cached = c
                    .pull_request("acme/demo", 7, Freshness::CachedOnly)
                    .await
                    .unwrap();
                assert_eq!(cached.validated_at_ms, 0);
                assert_eq!(cached.data["title"], "old terminal title");
            }
        }
    }
}

#[tokio::test]
async fn terminal_seed_preserves_explicit_validation_and_offline_reads() {
    for merged in [false, true] {
        for freshness in [
            Freshness::Revalidate,
            Freshness::MaxAge(Duration::ZERO),
            Freshness::CachedOnly,
        ] {
            let (h, c) = seed(merged, false).await;
            let before = h.calls().len();
            if matches!(freshness, Freshness::CachedOnly) {
                let report = c.pr_report("acme/demo", 7, freshness).await.unwrap();
                assert!(report.complete);
                assert_eq!(h.calls().len(), before);
                assert_eq!(report.data.pull_request["title"], "old terminal title");
                assert!(report.validations.iter().any(|v| v.validated_at_ms == 0));
            } else {
                let worker = c.clone();
                let read =
                    tokio::spawn(async move { worker.pr_report("acme/demo", 7, freshness).await });
                until(|| h.calls().len() > before).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                assert_eq!(c.status().outstanding_requests, 1);
                assert!(
                    h.calls()[before..]
                        .iter()
                        .all(|call| call.path.ends_with("/pulls/7"))
                );
                read.abort();
                let _ = read.await;
            }
        }
    }
}

#[tokio::test]
async fn reopened_terminal_seed_recollects_changed_commits_and_publishes_current_lifecycle() {
    for full in [false, true] {
        let (h, c) = seed(false, false).await;
        h.mode("account-head-change");
        if full {
            let report = c
                .pr_report("acme/demo", 7, Freshness::default())
                .await
                .unwrap();
            assert!(report.complete);
            assert_eq!(report.data.pull_request["state"], "open");
            assert_eq!(report.data.ci.head_sha, NEW_HEAD);
            assert_eq!(report.data.ci.merge_sha, None);
            assert!(
                report
                    .data
                    .ci
                    .check_runs
                    .iter()
                    .all(|run| run["head_sha"] == NEW_HEAD)
            );
        } else {
            let report = c
                .ci_for_pr("acme/demo", 7, Freshness::default())
                .await
                .unwrap();
            assert!(report.complete);
            assert_eq!(report.data.head_sha, NEW_HEAD);
            assert_eq!(report.data.merge_sha, None);
            assert!(
                report
                    .data
                    .check_runs
                    .iter()
                    .all(|run| run["head_sha"] == NEW_HEAD)
            );
        }
    }
}

#[tokio::test]
async fn malformed_terminal_lifecycle_cannot_start_speculative_sources() {
    for mutation in [
        "json_remove(response,'$.data.merged')",
        "json_set(response,'$.data.merged','true')",
        "json_set(response,'$.data.state','unknown')",
    ] {
        let (h, c) = seed(true, false).await;
        let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
        db.execute(
            &format!("UPDATE cache SET response={mutation} WHERE key LIKE '%/pulls/7'"),
            [],
        )
        .unwrap();
        let before = h.calls().len();
        let worker = c.clone();
        let read =
            tokio::spawn(
                async move { worker.pr_report("acme/demo", 7, Freshness::default()).await },
            );
        until(|| h.calls().len() > before).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(c.status().outstanding_requests, 1);
        assert!(
            h.calls()[before..]
                .iter()
                .all(|call| call.path.ends_with("/pulls/7"))
        );
        read.abort();
        let _ = read.await;
    }
}
