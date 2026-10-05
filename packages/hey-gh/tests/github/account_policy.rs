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
                .is_some_and(|cycle| cycle.finished_at_ms > after)
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
    let c = seed(&h, Duration::from_millis(400)).await;
    let (api, sdk, server) = start(&c).await;
    let status = wait_cycle(&sdk, 0).await;
    let cycle = status.policy_last_cycle.unwrap();
    assert_eq!(cycle.attempted, 1);
    assert_eq!(cycle.interrupted, 1);
    assert_eq!(cycle.deferred, 1);
    assert!(cycle.cycle_budget_exhausted);
    assert!(status.policy_last_error.is_some());
    assert!(status.policy_last_success_at_ms.is_none());
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
    let resumed = wait_cycle(&sdk, cycle.finished_at_ms).await;
    api.stop().await;
    server.abort();
    let _ = server.await;
    assert_eq!(resumed.policy_last_cycle.unwrap().succeeded, 2);
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
    let status = wait_cycle(&sdk, 0).await;
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
            if let Some(cycle) = &watches[0].policy_last_cycle {
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
