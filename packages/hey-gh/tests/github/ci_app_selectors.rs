use super::*;

pub(super) fn app_config(h: &Harness) -> Config {
    Config {
        installation: Some(
            hey_gh::AppInstallation::new(
                "test-client".into(),
                42,
                vec!["acme/demo".into()],
                include_str!("../fixtures/github-app-test-key.pem"),
            )
            .unwrap(),
        ),
        queue_timeout: Duration::from_secs(5),
        ..h.config()
    }
}

pub(super) fn expire_metadata(h: &Harness) {
    rusqlite::Connection::open(h.config().cache_path).unwrap().execute(
        "UPDATE cache SET response=json_set(response,'$.validated_at_ms',0) WHERE key LIKE '%/pulls/7' OR key LIKE '%/pulls/7#%'", [],
    ).unwrap();
}

async fn seeded() -> (Harness, Client) {
    let h = Harness::new().await;
    h.mode("account-ci-selectors");
    h.phase(2);
    let c = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    expire_metadata(&h);
    h.mode("ci-point-empty-status");
    (h, c)
}

#[tokio::test]
async fn app_ci_selectors_keep_generic_identical_queries_and_caches_personal() {
    let (h, c) = seeded().await;
    let before = h.calls().len();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    let query = h.calls()[before..]
        .iter()
        .find(|v| v.path == "/graphql")
        .unwrap()
        .clone();
    assert_eq!(query.token, "Bearer synthetic-app-token");
    let query_text = query.body["query"].as_str().unwrap();
    let variables = query.body["variables"].clone();
    assert!(matches!(
        c.graphql(query_text, variables.clone(), Freshness::CachedOnly)
            .await,
        Err(Error::CacheMiss)
    ));
    c.graphql(query_text, variables.clone(), Freshness::default())
        .await
        .unwrap();
    assert_eq!(h.calls().last().unwrap().token, "Bearer synthetic-token");
    // A fresh generic cache cannot satisfy a stale installation selector.
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute(
            "DELETE FROM cache WHERE key LIKE '%#installation-ci-selectors%'",
            [],
        )
        .unwrap();
    let before = h.calls().len();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    assert!(
        h.calls()[before..]
            .iter()
            .any(|v| v.path == "/graphql" && v.token == "Bearer synthetic-app-token")
    );
    let mint = h
        .calls()
        .into_iter()
        .find(|v| v.path.contains("access_tokens"))
        .unwrap();
    assert_eq!(
        mint.body["permissions"],
        json!({"actions":"read","checks":"read","statuses":"read","metadata":"read","contents":"read","pull_requests":"read"})
    );
    assert!(
        h.calls()
            .iter()
            .filter(|v| v.path.ends_with("/pulls/7"))
            .all(|v| v.token == "Bearer synthetic-app-token")
    );
}

#[tokio::test]
async fn app_ci_selector_status_evidence_survives_restart_without_minting() {
    let (h, c) = seeded().await;
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute(
            "DELETE FROM cache WHERE key LIKE '%/commits/%/status?%'",
            [],
        )
        .unwrap();
    let before = h.calls().len();
    let restarted = Client::with_token(app_config(&h), "synthetic-token".into()).unwrap();
    let report = restarted
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(report.complete, "{:?}", report.data.errors);
    assert!(report.data.commit_statuses.is_empty());
    assert!(
        report
            .validations
            .iter()
            .any(|v| v.resource.contains("#commit-statuses:"))
    );
    assert_eq!(h.calls().len(), before);
    let refreshed = restarted
        .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(refreshed.complete);
    let calls = h.calls();
    let statuses: Vec<_> = calls[before..]
        .iter()
        .filter(|v| v.path.ends_with("/status"))
        .collect();
    assert_eq!(statuses.len(), 2);
    assert!(
        statuses
            .iter()
            .all(|v| v.token == "Bearer synthetic-app-token")
    );
}

#[tokio::test]
async fn app_ci_selector_errors_do_not_fallback_to_personal_reads() {
    for mode in ["ci-point-denied", "ci-point-error", "ci-point-limited"] {
        let (h, c) = seeded().await;
        h.mode(mode);
        let before = h.calls().len();
        let result = c.ci_for_pr("acme/demo", 7, Freshness::default()).await;
        assert!(result.is_err(), "{mode}: {result:?}");
        let reads = h.calls()[before..].to_vec();
        assert!(!reads.is_empty());
        assert!(
            reads
                .iter()
                .all(|v| v.path == "/graphql" && v.token == "Bearer synthetic-app-token"),
            "{mode}: {reads:?}"
        );
    }
}

#[tokio::test]
async fn app_ci_selectors_use_the_caller_bound_instead_of_optional_two_second_timeout() {
    let (h, c) = seeded().await;
    h.mode("ci-point-slow");
    let before = h.calls().len();
    let result = tokio::time::timeout(
        Duration::from_secs(4),
        c.ci_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.complete);
    let reads = h.calls()[before..].to_vec();
    assert!(
        reads.iter().all(|v| !v.path.ends_with("/pulls/7")),
        "{reads:?}"
    );
    assert_eq!(reads.iter().filter(|v| v.path == "/graphql").count(), 1);
}

#[tokio::test]
async fn app_ci_selectors_never_coalesce_with_generic_personal_query() {
    let (h, c) = seeded().await;
    c.ci_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    let query = h
        .calls()
        .into_iter()
        .find(|v| v.path == "/graphql")
        .unwrap()
        .body;
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute("DELETE FROM cache WHERE key LIKE '%/graphql#%'", [])
        .unwrap();
    h.mode("ci-point-stalled");
    let before = h.calls().len();
    let ci_client = c.clone();
    let ci = tokio::spawn(async move {
        ci_client
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
    });
    let personal_client = c.clone();
    let personal = tokio::spawn(async move {
        personal_client
            .graphql(
                query["query"].as_str().unwrap(),
                query["variables"].clone(),
                Freshness::Revalidate,
            )
            .await
    });
    // Loopback keeps one GraphQL socket, so distinct requests can be queued
    // together without both reaching the server until the first is released.
    tokio::time::timeout(Duration::from_secs(1), async {
        while c.status().outstanding_requests != 2 || h.calls().len() == before {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("authentication routes must have distinct in-flight requests");
    h.mock.release.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), async {
        while h.calls()[before..]
            .iter()
            .filter(|v| v.path == "/graphql")
            .count()
            < 2
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "two independent authentication routes must reach GitHub: {:?}",
            &h.calls()[before..]
        )
    });
    let calls = h.calls()[before..].to_vec();
    assert!(calls.iter().any(|v| v.token == "Bearer synthetic-token"));
    assert!(
        calls
            .iter()
            .any(|v| v.token == "Bearer synthetic-app-token")
    );
    h.mock.release.notify_waiters();
    assert!(ci.await.unwrap().unwrap().complete);
    personal.await.unwrap().unwrap();
}

#[tokio::test]
async fn app_ci_selectors_keep_other_repositories_personal() {
    let h = Harness::new().await;
    h.phase(2);
    h.mode("account-ci-selectors");
    let mut config = app_config(&h);
    config.installation = Some(
        hey_gh::AppInstallation::new(
            "test-client".into(),
            42,
            vec!["acme/other".into()],
            include_str!("../fixtures/github-app-test-key.pem"),
        )
        .unwrap(),
    );
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    expire_metadata(&h);
    h.mode("ci-point-valid");
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap()
            .complete
    );
    assert!(
        h.calls()
            .iter()
            .all(|v| v.token == "Bearer synthetic-token")
    );
    assert!(h.calls().iter().any(|v| v.path == "/graphql"));
}

#[tokio::test]
async fn app_ci_selector_deadline_remains_bounded_without_personal_fallback() {
    let (h, _c) = seeded().await;
    h.mode("ci-point-stalled");
    let c = Client::with_token(
        Config {
            report_timeout: Duration::from_millis(150),
            ..app_config(&h)
        },
        "synthetic-token".into(),
    )
    .unwrap();
    let before = h.calls().len();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        c.ci_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(Error::Deadline)), "{result:?}");
    let calls = h.calls()[before..].to_vec();
    assert!(calls.iter().all(|v| !v.path.ends_with("/pulls/7")));
    assert!(
        calls
            .iter()
            .any(|v| v.path == "/graphql" && v.token == "Bearer synthetic-app-token")
    );
}

#[tokio::test]
async fn app_ci_selector_and_personal_graphql_primary_quotas_are_independent() {
    for mode in ["ci-point-personal-quota", "ci-point-app-quota"] {
        let (h, c) = seeded().await;
        h.mode(mode);
        if mode == "ci-point-personal-quota" {
            assert!(
                c.graphql(
                    "query { viewer { login } }",
                    json!({}),
                    Freshness::Revalidate
                )
                .await
                .is_err()
            );
            assert!(
                c.ci_for_pr("acme/demo", 7, Freshness::default())
                    .await
                    .unwrap()
                    .complete
            );
        } else {
            let before = h.calls().len();
            assert!(
                c.ci_for_pr("acme/demo", 7, Freshness::default())
                    .await
                    .is_err()
            );
            assert!(
                h.calls()[before..]
                    .iter()
                    .all(|v| v.path == "/graphql" && v.token == "Bearer synthetic-app-token")
            );
            c.graphql(
                "query { viewer { login } }",
                json!({}),
                Freshness::Revalidate,
            )
            .await
            .unwrap();
        }
        let quotas = c.status().rate_limits;
        assert!(quotas.contains_key(if mode == "ci-point-personal-quota" {
            "graphql"
        } else {
            "installation/graphql"
        }));
    }
}

#[tokio::test]
async fn app_ci_selector_renews_expired_tokens_and_preserves_mint_errors() {
    for mode in ["ci-point-renew", "ci-point-mint-denied"] {
        let (h, old_client) = seeded().await;
        h.mode(mode);
        let before = h.calls().len();
        let c = if mode == "ci-point-renew" {
            old_client
        } else {
            Client::with_token(app_config(&h), "synthetic-token".into()).unwrap()
        };
        let result = c.ci_for_pr("acme/demo", 7, Freshness::default()).await;
        let reads = h.calls()[before..].to_vec();
        assert!(reads.iter().all(|v| v.path.contains("access_tokens")
            || (v.path == "/graphql" && v.token == "Bearer synthetic-app-token")));
        assert_eq!(
            reads
                .iter()
                .filter(|v| v.path.contains("access_tokens"))
                .count(),
            1
        );
        if mode == "ci-point-renew" {
            assert!(result.unwrap().complete);
            assert_eq!(reads.iter().filter(|v| v.path == "/graphql").count(), 2);
        } else {
            assert!(result.is_err());
            assert!(reads.iter().all(|v| v.path.contains("access_tokens")));
        }
    }
}
