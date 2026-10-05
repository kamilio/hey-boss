use super::*;

const NEXT_BASE: &str = "dddddddddddddddddddddddddddddddddddddddd";

async fn base_changed(installation: bool) -> Fixture {
    let f = Fixture::with_installation(installation).await;
    f.seed().await;
    // Fresh enough for collection, but old enough to require final selectors.
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        - 20_000;
    let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/pulls/7'", [at]).unwrap();
    {
        let mut s = f.data.lock().unwrap();
        s.rest["base"]["sha"] = json!(NEXT_BASE);
        s.graph["data"]["repository"]["pullRequest"]["baseRefOid"] = json!(NEXT_BASE);
    }
    f
}

#[tokio::test]
async fn base_only_retry_reuses_fresh_ci_without_waiting_for_four_rest_reads() {
    for installation in [false, true] {
        let f = base_changed(installation).await;
        f.data.lock().unwrap().stall_checks = true;
        let report = tokio::time::timeout(
            Duration::from_secs(1),
            f.client
                .required_checks_for_pr("acme/demo", 7, Freshness::default()),
        )
        .await
        .expect("base-only retry unnecessarily revalidated unchanged CI")
        .unwrap();
        assert_eq!(report.state, "satisfied", "{:?}", report.errors);
        assert_eq!(report.pr_base_sha.as_deref(), Some(NEXT_BASE));
        let s = f.data.lock().unwrap();
        assert_eq!(
            s.calls
                .iter()
                .filter(|(p, _)| p.contains("/commits/"))
                .count(),
            0
        );
        assert_eq!(
            s.calls
                .iter()
                .filter(|(p, _)| p.ends_with("/pulls/7"))
                .count(),
            2,
            "retry must still confirm personal metadata"
        );
        assert!(s.calls.iter().any(|(p, _)| p.contains("/rules/branches/")));
        assert!(s.calls.iter().any(|(p, _)| p.contains("/branches/main")));
    }
}

#[tokio::test]
async fn base_only_retry_evaluates_new_requirements_against_the_complete_ci_roster() {
    let f = base_changed(false).await;
    {
        let mut s = f.data.lock().unwrap();
        s.stall_checks = true;
        s.rules[0]["parameters"]["required_status_checks"]
            .as_array_mut()
            .unwrap()
            .push(json!({"context":"deploy"}));
    }
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(report.state, "missing");
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.context == "deploy" && check.state == "missing")
    );
    assert!(report.errors.is_empty());
}

#[tokio::test]
async fn explicit_refresh_and_zero_age_still_revalidate_ci_on_base_only_retry() {
    for freshness in [Freshness::Revalidate, Freshness::MaxAge(Duration::ZERO)] {
        let f = Fixture::new().await;
        f.seed().await;
        let mut next = metadata();
        next["base"]["sha"] = json!(NEXT_BASE);
        f.data.lock().unwrap().rest_after_read = Some(next);
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, freshness)
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied");
        assert_eq!(report.pr_base_sha.as_deref(), Some(NEXT_BASE));
        assert_eq!(
            f.data
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|(p, _)| p.contains("/commits/"))
                .count(),
            8
        );
    }
}

#[tokio::test]
async fn changed_commit_retry_revalidates_even_fresh_new_commit_cache() {
    for change_merge in [false, true] {
        let f = base_changed(false).await;
        for suffix in [
            "check-runs?filter=latest&per_page=100",
            "status?per_page=100",
        ] {
            f.client
                .get(
                    &format!("repos/acme/demo/commits/{NEXT_BASE}/{suffix}"),
                    Freshness::Revalidate,
                )
                .await
                .unwrap();
        }
        {
            let mut s = f.data.lock().unwrap();
            if change_merge {
                s.rest["merge_commit_sha"] = json!(NEXT_BASE);
            } else {
                s.rest["head"]["sha"] = json!(NEXT_BASE);
            }
            s.calls.clear();
        }
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied");
        assert_eq!(
            f.data
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|(p, _)| p.contains("/commits/"))
                .count(),
            4
        );
        if change_merge {
            assert_eq!(report.merge_sha.as_deref(), Some(NEXT_BASE));
        } else {
            assert_eq!(report.head_sha, NEXT_BASE);
        }
    }
}

async fn pause_confirmation(
    f: &Fixture,
) -> (
    Arc<tokio::sync::Notify>,
    tokio::task::JoinHandle<hey_gh::Result<hey_gh::RequiredChecksReport>>,
) {
    let gate = Arc::new(tokio::sync::Notify::new());
    f.data.lock().unwrap().policy_graph_gate = Some(gate.clone());
    let client = f.client.clone();
    let task = tokio::spawn(async move {
        client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if f.data.lock().unwrap().calls.iter().any(|(_, b)| {
                b["query"]
                    .as_str()
                    .is_some_and(|q| q.contains("RequiredPolicySelectors"))
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("read did not reach final selector confirmation");
    (gate, task)
}

#[tokio::test]
async fn base_only_retry_obeys_newer_ci_failure_and_expired_evidence() {
    for expire in [false, true] {
        let f = base_changed(false).await;
        let (gate, task) = pause_confirmation(&f).await;
        if expire {
            let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
            db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',1) WHERE key LIKE '%/commits/%'", []).unwrap();
            f.data.lock().unwrap().check_conclusion = "failure";
        } else {
            // Another reader has already observed a failure on the same SHA.
            f.data.lock().unwrap().check_conclusion = "failure";
            f.client
                .get(
                    &format!(
                        "repos/acme/demo/commits/{MERGE}/check-runs?filter=latest&per_page=100"
                    ),
                    Freshness::Revalidate,
                )
                .await
                .unwrap();
            f.data.lock().unwrap().stall_checks = true;
        }
        gate.notify_one();
        let report = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(report.state, "failure", "{:?}", report.errors);
        assert!(report.errors.is_empty());
        assert_eq!(
            f.data
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|(p, _)| p.contains("/commits/"))
                .count(),
            if expire { 4 } else { 1 }
        );
    }
}

#[tokio::test]
async fn base_only_retry_cannot_hide_ci_access_denial() {
    let f = base_changed(false).await;
    let (gate, task) = pause_confirmation(&f).await;
    f.data.lock().unwrap().deny_checks = true;
    assert!(
        f.client
            .get(
                &format!("repos/acme/demo/commits/{MERGE}/check-runs?filter=latest&per_page=100"),
                Freshness::Revalidate
            )
            .await
            .is_err()
    );
    gate.notify_one();
    match task.await.unwrap() {
        Ok(report) => {
            assert_eq!(report.state, "unknown");
            assert!(!report.errors.is_empty());
        }
        Err(error) => assert!(matches!(error, hey_gh::Error::GitHub { status: 403, .. })),
    }
}

#[tokio::test]
async fn base_only_retry_still_requires_final_confirmation() {
    let f = base_changed(false).await;
    let mut next = metadata();
    next["head"]["sha"] = json!(NEXT_BASE);
    f.data.lock().unwrap().rest_after_read = Some(next);
    assert!(
        matches!(f.client.required_checks_for_pr("acme/demo", 7, Freshness::default()).await,
        Err(hey_gh::Error::Invalid(message)) if message.contains("changed repeatedly"))
    );
}

#[tokio::test]
async fn repository_identity_change_forces_ci_refresh() {
    for field in ["id", "node_id"] {
        let f = base_changed(false).await;
        f.data.lock().unwrap().rest["base"]["repo"][field] = if field == "id" {
            json!(456)
        } else {
            json!("R_recreated")
        };
        let report = f
            .client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert_eq!(report.state, "satisfied");
        assert_eq!(
            f.data
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|(p, _)| p.contains("/commits/"))
                .count(),
            4
        );
    }
}

#[tokio::test]
async fn repository_generation_change_rejects_inflight_policy() {
    let f = base_changed(false).await;
    let (gate, task) = pause_confirmation(&f).await;
    let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
    db.execute("INSERT OR REPLACE INTO repository_generation(scope,repository,generation) SELECT DISTINCT scope,'acme/demo',1 FROM cache", []).unwrap();
    gate.notify_one();
    assert!(
        matches!(task.await.unwrap(), Err(hey_gh::Error::Invalid(message)) if message.contains("entity changed"))
    );
}

#[tokio::test]
async fn incomplete_first_pass_cannot_reuse_other_fresh_ci_sources() {
    let f = base_changed(false).await;
    let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
    db.execute(
        "DELETE FROM cache WHERE key LIKE ?1",
        [format!("%/commits/{HEAD}/check-runs%")],
    )
    .unwrap();
    f.data.lock().unwrap().deny_checks = true;
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(report.state, "unknown");
    assert!(!report.errors.is_empty());
    assert_eq!(
        f.data
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(p, _)| p.contains("/commits/"))
            .count(),
        5
    );
}

#[tokio::test]
async fn offline_policy_keeps_original_selectors_and_does_not_retry() {
    let f = base_changed(false).await;
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(report.pr_base_sha.as_deref(), Some(BASE));
    assert_eq!(report.state, "satisfied");
    assert!(f.data.lock().unwrap().calls.is_empty());
}
