use super::*;

#[tokio::test]
async fn a_newer_queued_open_version_prevents_retiring_an_older_close() {
    for (closed_day, open_day, retired) in [(1, 2, false), (2, 1, true)] {
        let f = Fixture::new("CLOSED").await;
        let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
        db.execute("UPDATE snapshots SET data=json_set(data,'$.pullRequest.updatedAt',?1) WHERE resource=?2",
            [format!("2026-10-0{closed_day}T00:00:00Z"),f.resource.clone()]).unwrap();
        drop(db);
        let mut node = f.node.clone();
        node["updatedAt"] = json!(format!("2026-10-0{open_day}T00:00:00Z"));
        f.client
            .observe(&f.client.roster_resource(), &json!([node]))
            .await
            .unwrap();
        f.client
            .save_derived("account-status-pending:policy", json!([node]))
            .await
            .unwrap();
        assert_eq!(f.collect().await.is_empty(), retired);
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    store: crate::store::Store,
    scope: String,
    client: Client,
    node: Value,
    resource: String,
}

impl Fixture {
    async fn new(state: &str) -> Self {
        Self::with_installation(state, false).await
    }

    async fn with_installation(state: &str, installation: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            crate::Config {
                installation: installation.then(|| {
                    crate::AppInstallation::new(
                        "synthetic-client".into(),
                        42,
                        vec!["acme/demo".into()],
                        include_str!("../../tests/fixtures/github-app-test-key.pem"),
                    )
                    .unwrap()
                }),
                cache_path: dir.path().join("cache.sqlite"),
                rest_url: "http://127.0.0.1:9/".parse().unwrap(),
                graphql_url: "http://127.0.0.1:9/graphql".parse().unwrap(),
                report_timeout: Duration::from_secs(2),
                ..Default::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let node = json!({"id":"PR_7","number":7,"state":"OPEN","headRefOid":"head",
            "repository":{"nameWithOwner":"acme/demo"}});
        let resource = format!("{}acme/demo/7", client.status_prefix());
        client
            .observe(&client.roster_resource(), &json!([node]))
            .await
            .unwrap();
        client
            .save_derived(
                DISCOVERY_CACHE,
                json!({"pulls":[],"validatedAtMs":crate::now_ms()}),
            )
            .await
            .unwrap();
        client
            .save_derived("account-status-pending:policy", json!([node]))
            .await
            .unwrap();
        let db = rusqlite::Connection::open(dir.path().join("cache.sqlite")).unwrap();
        let scope: String = db
            .query_row("SELECT scope FROM snapshots LIMIT 1", [], |r| r.get(0))
            .unwrap();
        drop(db);
        let store = crate::store::Store::open(
            &dir.path().join("cache.sqlite"),
            Duration::from_secs(3600),
            1000,
            4 * 1024 * 1024,
        )
        .unwrap();
        store
            .accept_rest_identity(&scope, "acme/demo", 7, "PR_7", crate::now_ms(), &[])
            .await
            .unwrap();
        let mut terminal = node.clone();
        terminal["state"] = json!(state);
        terminal["removed"] = json!(true);
        terminal["sourceErrors"] = json!({"details":"review access denied"});
        let owner = crate::store::PrOwner {
            repository: "acme/demo".into(),
            number: 7,
            node_id: Some("PR_7".into()),
            generation: 0,
        };
        store
            .observe_validated_owned(
                &scope,
                &[(resource.clone(), json!({"pullRequest":terminal}))],
                &[(resource.clone(), crate::now_ms())],
                &owner,
            )
            .await
            .unwrap();
        Self {
            dir,
            store,
            scope,
            client,
            node,
            resource,
        }
    }
    async fn collect(&self) -> Vec<Value> {
        self.client
            .collect_pr_status_inner(
                Freshness::MaxAge(Duration::from_secs(30)),
                Refresh::Policy,
                true,
            )
            .await
            .unwrap();
        self.client
            .derived("account-status-pending:policy")
            .await
            .unwrap()
            .unwrap()
            .data
            .as_array()
            .unwrap()
            .clone()
    }
}

#[tokio::test]
async fn terminal_policy_entries_retire_before_ci_admission_and_stay_retired_after_restart() {
    for state in ["CLOSED", "MERGED"] {
        let f = Fixture::new(state).await;
        let before = f.client.stored_snapshot(&f.resource).await.unwrap();
        assert!(
            f.collect().await.is_empty(),
            "{state} kept waiting for terminal CI"
        );
        let cycle = f
            .client
            .derived("account-status-cycle:policy")
            .await
            .unwrap()
            .unwrap()
            .data;
        assert_eq!(cycle["total"], 0);
        assert_eq!(cycle["waiting_for_ci"], 0);
        assert_eq!(f.client.stored_snapshot(&f.resource).await.unwrap(), before);
        assert_eq!(f.client.status().outstanding_requests, 0);
        assert_eq!(f.client.status().network_requests, 0);
        let restarted = Client::with_token(
            crate::Config {
                cache_path: f.dir.path().join("cache.sqlite"),
                rest_url: "http://127.0.0.1:9/".parse().unwrap(),
                graphql_url: "http://127.0.0.1:9/graphql".parse().unwrap(),
                ..Default::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        restarted
            .collect_pr_status_inner(
                Freshness::MaxAge(Duration::from_secs(30)),
                Refresh::Policy,
                true,
            )
            .await
            .unwrap();
        assert_eq!(
            restarted
                .derived("account-status-pending:policy")
                .await
                .unwrap()
                .unwrap()
                .data,
            json!([])
        );
        if state == "CLOSED" {
            restarted
                .save_derived(
                    DISCOVERY_CACHE,
                    json!({"pulls":[f.node],"validatedAtMs":crate::now_ms()}),
                )
                .await
                .unwrap();
            restarted
                .collect_pr_status_inner(
                    Freshness::MaxAge(Duration::from_secs(30)),
                    Refresh::Policy,
                    true,
                )
                .await
                .unwrap();
            assert_eq!(
                restarted
                    .derived("account-status-pending:policy")
                    .await
                    .unwrap()
                    .unwrap()
                    .data,
                json!([f.node])
            );
        }
        assert_eq!(restarted.status().network_requests, 0);
    }
}

#[tokio::test]
async fn open_unknown_and_rediscovered_prs_keep_waiting_for_ci() {
    for state in ["OPEN", "UNKNOWN", "CLOSED"] {
        let f = Fixture::new(state).await;
        if state == "CLOSED" {
            f.client
                .save_derived(
                    DISCOVERY_CACHE,
                    json!({"pulls":[f.node],"validatedAtMs":crate::now_ms()}),
                )
                .await
                .unwrap();
        }
        assert_eq!(f.collect().await, vec![f.node]);
    }
}

#[tokio::test]
async fn missing_discovery_or_unowned_terminal_status_cannot_retire_work() {
    for mutation in [
        "discovery",
        "owner",
        "clock",
        "future_clock",
        "identity",
        "generation",
        "body_identity",
    ] {
        let f = Fixture::new("CLOSED").await;
        let db = rusqlite::Connection::open(f.dir.path().join("cache.sqlite")).unwrap();
        match mutation {
            "discovery" => {
                db.execute("DELETE FROM cache WHERE key=?1", [DISCOVERY_CACHE])
                    .unwrap();
            }
            "owner" => {
                db.execute("DELETE FROM source_owner", []).unwrap();
            }
            "clock" => {
                db.execute("DELETE FROM snapshot_validation", []).unwrap();
            }
            "future_clock" => {
                db.execute(
                    "UPDATE snapshot_validation SET validated_at_ms=?1",
                    [crate::now_ms() + 60_000],
                )
                .unwrap();
            }
            "body_identity" => {
                db.execute("UPDATE snapshots SET data=json_set(data,'$.pullRequest.id','wrong-node') WHERE resource=?1",[&f.resource]).unwrap();
            }
            "identity" => {
                db.execute("UPDATE pr_identity SET node_id='replacement'", [])
                    .unwrap();
            }
            "generation" => {
                db.execute(
                    "INSERT INTO repository_generation VALUES(?1,'acme/demo',1)",
                    [&f.scope],
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        drop(db);
        assert_eq!(f.collect().await, vec![f.node], "{mutation}");
    }
}

#[tokio::test]
async fn newer_cached_reopen_keeps_terminal_policy_work_pending() {
    let f = Fixture::new("CLOSED").await;
    f.store.put(&f.scope,"http://127.0.0.1:9/repos/acme/demo/pulls/7",&crate::Response {
        data:json!({"node_id":"PR_7","number":7,"state":"open","merged":false,"base":{"repo":{"full_name":"acme/demo"}}}),
        source:crate::Source::Network,fetched_at_ms:crate::now_ms(),validated_at_ms:crate::now_ms(),etag:None,last_modified:None,link:None,
    }).await.unwrap();
    assert_eq!(f.collect().await, vec![f.node]);
}

#[tokio::test]
async fn cached_installation_reopen_rejects_retirement_without_using_app_for_lifecycle() {
    let f = Fixture::with_installation("CLOSED", true).await;
    let stamp = crate::now_ms() - 1000;
    let mut response = crate::Response {
        data: json!({"node_id":"PR_7","number":7,"state":"closed","merged":false,"base":{"repo":{"full_name":"acme/demo"}}}),
        source: crate::Source::Network,
        fetched_at_ms: stamp,
        validated_at_ms: stamp,
        etag: None,
        last_modified: None,
        link: None,
    };
    f.store
        .put(
            &f.scope,
            "http://127.0.0.1:9/repos/acme/demo/pulls/7",
            &response,
        )
        .await
        .unwrap();
    response.data["state"] = json!("open");
    response.validated_at_ms = stamp + 1;
    f.store
        .put(
            &f.scope,
            "http://127.0.0.1:9/repos/acme/demo/pulls/7#installation-ci-pr",
            &response,
        )
        .await
        .unwrap();
    assert_eq!(f.collect().await, vec![f.node.clone()]);
    assert_eq!(f.client.status().network_requests, 0);
    response.data["state"] = json!("closed");
    response.validated_at_ms = stamp + 2;
    f.store
        .put(
            &f.scope,
            "http://127.0.0.1:9/repos/acme/demo/pulls/7",
            &response,
        )
        .await
        .unwrap();
    assert!(f.collect().await.is_empty());
    assert_eq!(f.client.status().network_requests, 0);
}

#[tokio::test]
async fn additive_discovery_cannot_retire_an_absent_terminal_pr() {
    let f = Fixture::new("CLOSED").await;
    let epoch = f
        .client
        .derived(DISCOVERY_CACHE)
        .await
        .unwrap()
        .unwrap()
        .fetched_at_ms;
    f.client.save_derived("account-discovery-additions:v1",json!({"collection_epoch":epoch,
        "pulls":[{"id":"PR_8","number":8,"state":"OPEN","repository":{"nameWithOwner":"acme/demo"}}]})).await.unwrap();
    let pending = f.collect().await;
    assert!(pending.contains(&f.node));
    assert_eq!(pending.len(), 2);
    assert_eq!(f.client.status().network_requests, 0);
}
