use super::*;

async fn start(
    c: &Client,
) -> (
    hey_gh::api::Api,
    hey_gh::ApiClient,
    tokio::task::JoinHandle<()>,
) {
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sdk = hey_gh::ApiClient::new(
        format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap();
    let router = api.router();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (api, sdk, server)
}

async fn wait_cycle(sdk: &hey_gh::ApiClient, after: u64) -> hey_gh::api::WatchStatus {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let status = sdk.watches().await.unwrap().into_iter().next().unwrap();
            if status
                .policy_last_cycle
                .as_ref()
                // Startup may finish a cache-only deferral before account CI
                // records its completion. Wait for the woken policy work.
                .is_some_and(|cycle| cycle.finished_at_ms > after && cycle.attempted > 0)
                && status.ci_last_cycle.is_some()
                && (status.policy_last_success_at_ms.is_some()
                    || status.policy_last_error.is_some())
            {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

async fn seed(h: &Harness, timeout: Duration) -> Client {
    h.phase(2);
    let c = Client::with_token(
        Config {
            report_timeout: timeout,
            ..h.config()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    for repo in ["acme/demo", "acme/other"] {
        assert!(
            c.ci_for_pr(repo, 7, Freshness::Revalidate)
                .await
                .unwrap()
                .complete
        );
    }
    c.save_account_watch(60).await.unwrap();
    c
}

#[tokio::test]
async fn interrupted_policy_rotation_keeps_ci_health_and_resumes_the_next_pr_after_restart() {
    let h = Harness::new().await;
    h.mode("account-policy-rotation-stalled");
    // This budget also covers the healthy CI rotation. The policy request is
    // gated indefinitely, so a subsecond budget only adds a host-load race.
    let c = seed(&h, Duration::from_secs(2)).await;
    let (api, sdk, server) = start(&c).await;
    // Either CI row can become ready first. The healthy neighbor may complete
    // a policy turn while the stalled target still awaits its CI completion.
    // Observe the actual interruption, not merely the first attempted turn.
    let status = tokio::time::timeout(Duration::from_secs(8), async {
        let mut after = 0;
        loop {
            let status = wait_cycle(&sdk, after).await;
            let cycle = status.policy_last_cycle.as_ref().unwrap();
            if cycle.interrupted > 0 {
                break status;
            }
            assert_eq!(cycle.failed, 0, "{cycle:?}; {:?}", status.policy_last_error);
            assert!(
                cycle.waiting_for_ci > 0,
                "unexpected policy turn: {cycle:?}"
            );
            after = cycle.finished_at_ms;
        }
    })
    .await
    .expect("stalled policy target never interrupted its rotation");
    let cycle = status.policy_last_cycle.unwrap();
    assert_eq!(cycle.attempted, 1);
    assert_eq!(
        cycle.interrupted, 1,
        "{cycle:?}; {:?}",
        status.policy_last_error
    );
    assert_eq!(cycle.deferred, 1);
    assert!(cycle.cycle_budget_exhausted);
    assert!(status.policy_last_error.is_some());
    assert!(
        status
            .policy_last_success_at_ms
            .is_none_or(|at| at <= cycle.started_at_ms)
    );
    assert_eq!(status.ci_last_cycle.unwrap().succeeded, 2);
    assert!(status.ci_last_error.is_none());
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    let next: String = db
        .query_row(
            "SELECT response FROM cache WHERE key='account-status-next:policy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&next).unwrap()["data"],
        json!(["acme/other", 7])
    );
    let published: usize = db
        .query_row(
            "SELECT count(*) FROM snapshots WHERE resource LIKE 'required_checks://%/acme/demo/7'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        published, 0,
        "an interrupted read cannot publish a policy conclusion"
    );
    drop(db);
    api.stop().await;
    server.abort();
    let _ = server.await;
    drop(api);
    h.mode("account-policy-rotation");
    h.mock.release.notify_waiters();
    until(|| c.status().outstanding_requests == 0).await;
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    let schedule: String = db
        .query_row(
            "SELECT response FROM cache WHERE key='account-status-schedule:policy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let schedule = serde_json::from_str::<Value>(&schedule).unwrap();
    let schedule = &schedule["data"];
    // A wake may already have visited the ordinary cursor, leaving an urgent
    // retry ahead of it. The restart must preserve both rotation lanes.
    assert!(schedule["resume"].as_array().unwrap().is_empty());
    let next = if schedule["prefer_urgent"] == true
        && let Some(first) = schedule["urgent"].as_array().unwrap().first()
    {
        first
    } else {
        &schedule["next"]
    };
    let resume_repo = next[0].as_str().unwrap().to_owned();
    // A readiness wake may already have started the next turn before stop.
    // Force both cached rules pages stale to observe the persisted resume order.
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',1) WHERE key LIKE '%/rules/branches/%'",[]).unwrap();
    drop(db);
    drop(c);
    let before = h.calls().len();
    let c = Client::with_token(
        Config {
            report_timeout: Duration::from_secs(6),
            ..h.config()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    let (api, sdk, server) = start(&c).await;
    // Restarted CI can make the two rows ready in separate turns. A first
    // successful policy turn is not necessarily the completed rotation.
    let resumed = tokio::time::timeout(Duration::from_secs(8), async {
        let mut after = cycle.finished_at_ms;
        loop {
            let status = wait_cycle(&sdk, after).await;
            let cycle = status.policy_last_cycle.as_ref().unwrap();
            assert_eq!(cycle.failed, 0, "{status:?}");
            assert_eq!(cycle.interrupted, 0, "{status:?}");
            if cycle.succeeded == 2 {
                break status;
            }
            assert!(cycle.waiting_for_ci > 0, "{status:?}");
            after = cycle.finished_at_ms;
        }
    })
    .await
    .expect("restarted policy rotation did not finish both PRs");
    api.stop().await;
    server.abort();
    let _ = server.await;
    assert_eq!(
        resumed.policy_last_cycle.as_ref().unwrap().succeeded,
        2,
        "{resumed:?}"
    );
    assert!(resumed.policy_last_error.is_none());
    assert!(resumed.policy_last_success_at_ms.is_some());
    let rules: Vec<_> = h.calls()[before..]
        .iter()
        .filter(|call| call.path.contains("/rules/branches/"))
        .map(|call| call.path.clone())
        .collect();
    assert_eq!(
        rules.first().map(String::as_str),
        Some(format!("/repos/{resume_repo}/rules/branches/main").as_str())
    );
}

#[tokio::test]
async fn policy_access_failure_stays_explicit_without_overwriting_ci_health() {
    let h = Harness::new().await;
    h.mode("account-policy-rotation-denied");
    let c = seed(&h, Duration::from_secs(5)).await;
    let (api, sdk, server) = start(&c).await;
    // CI can wake policy after only one row finishes. A partial turn must
    // retain its denial while waiting for the other row's readiness wake.
    let status = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let status = sdk.watches().await.unwrap().into_iter().next().unwrap();
            if let Some(cycle) = &status.policy_last_cycle {
                assert_eq!(cycle.interrupted, 0, "{status:?}");
                assert_eq!(cycle.succeeded, 0, "{status:?}");
                assert_eq!(cycle.failed + cycle.waiting_for_ci, 2, "{status:?}");
                // Health is published after the durable cycle, so even the
                // final cycle can briefly be paired with an older CI deferral.
                if cycle.waiting_for_ci == 0
                    && status
                        .policy_last_error
                        .as_deref()
                        .is_some_and(|error| error.contains("Synthetic policy access denied"))
                    && status.ci_last_success_at_ms.is_some()
                {
                    break status;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("policy did not observe both access failures after CI completed");
    api.stop().await;
    server.abort();
    let _ = server.await;
    assert_eq!(status.policy_last_cycle.unwrap().failed, 2);
    assert!(
        status
            .policy_last_error
            .unwrap()
            .contains("Synthetic policy access denied")
    );
    assert_eq!(status.ci_last_cycle.unwrap().succeeded, 2);
    assert!(status.ci_last_error.is_none());
    let page = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        page.pull_requests
            .iter()
            .all(|row| row["sourceErrors"]["ci"].is_null())
    );
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    let rows:usize=db.query_row("SELECT count(*) FROM snapshots WHERE resource LIKE 'required_checks://%/acme/%/7' AND json_extract(data,'$.state')='unknown' AND json_array_length(data,'$.errors')>0",[],|r|r.get(0)).unwrap();
    assert_eq!(rows, 2);
}

#[tokio::test]
async fn policy_waits_for_fresh_ci_without_upstream_probes_and_wakes_on_completion() {
    let h = Harness::new().await;
    h.mode("account-policy-rotation");
    let c = seed(&h, Duration::from_secs(10)).await;
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',1) WHERE key LIKE '%/check-runs?%' OR key LIKE '%/status?%'",[]).unwrap();
    drop(db);
    h.mode("account-policy-rotation-gated");
    let before = h.calls().len();
    let (api, sdk, server) = start(&c).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let watches = sdk.watches().await.unwrap();
            // The durable cycle is visible before the monitor publishes its
            // health fields. Wait for both parts of the observation.
            if let Some(cycle) = &watches[0].policy_last_cycle
                && watches[0].policy_last_error.is_some()
            {
                assert_eq!(cycle.attempted, 0);
                assert_eq!(cycle.deferred, 2);
                assert!(!cycle.cycle_budget_exhausted);
                assert!(
                    watches[0]
                        .policy_last_error
                        .as_deref()
                        .is_some_and(|e| e.contains("awaits recent CI"))
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        h.calls()[before..]
            .iter()
            .all(|call| !call.path.contains("/branches/") && !call.path.contains("/compare/")),
        "policy must not send probes while CI prerequisites are stale"
    );
    h.mode("account-policy-rotation");
    h.mock.release.notify_waiters();
    // The configured interval is 60 seconds; a CI completion must wake policy
    // within this shorter bound, without a new watch registration.
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let watches = sdk.watches().await.unwrap();
            if watches[0]
                .policy_last_cycle
                .as_ref()
                .is_some_and(|c| c.succeeded == 2)
                && watches[0].policy_last_error.is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    api.stop().await;
    server.abort();
    let _ = server.await;
}
