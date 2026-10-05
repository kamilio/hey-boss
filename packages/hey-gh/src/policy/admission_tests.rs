use super::*;
use std::time::Duration;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMPLETION: &str = "account-status-validated:ci:acme/demo/7";

async fn fixture() -> (tempfile::TempDir, Client) {
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(
        crate::Config {
            cache_path: dir.path().join("cache.sqlite"),
            rest_url: "http://127.0.0.1:1/".parse().unwrap(),
            graphql_url: "http://127.0.0.1:1/graphql".parse().unwrap(),
            ..Default::default()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    client
        .save_derived(
            "http://127.0.0.1:1/repos/acme/demo/pulls/7",
            json!({
                "node_id":"PR_7", "number":7,"state":"open", "head":{"sha":HEAD},
                "base":{"ref":"main", "sha":HEAD}, "merge_commit_sha":null
            }),
        )
        .await
        .unwrap();
    client.observe("ci://github.com/acme/demo/7", &json!({
        "head_sha":HEAD, "merge_sha":null, "errors":[], "jobs":[{"payload":"x".repeat(100_000)}]
    })).await.unwrap();
    for (path, data) in [
        (
            "check-runs?filter=latest&per_page=100",
            json!({"total_count":0,"check_runs":[]}),
        ),
        (
            "status?per_page=100",
            json!({"total_count":0,"statuses":[]}),
        ),
    ] {
        client
            .save_derived(
                &format!("http://127.0.0.1:1/repos/acme/demo/commits/{HEAD}/{path}"),
                data,
            )
            .await
            .unwrap();
    }
    (dir, client)
}

async fn admitted(client: &Client) -> bool {
    client
        .policy_ci_cached("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(60)))
        .await
        .unwrap()
}

#[tokio::test]
async fn policy_admission_requires_recent_completion_before_loading_ci() {
    let (dir, client) = fixture().await;
    assert!(!admitted(&client).await);
    assert_eq!(client.status().cache_hits, 0);
    // The saved data is the conservative CI validation clock, not completion
    // time. Admission uses the wrapper only; actual sources still gate it.
    client.save_derived(COMPLETION, json!(1)).await.unwrap();
    assert!(admitted(&client).await);
    assert!(client.status().cache_hits > 0);
    let db = rusqlite::Connection::open(dir.path().join("cache.sqlite")).unwrap();
    for clock in [0, 1, crate::now_ms() + 3_600_000] {
        db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1,'$.fetched_at_ms',?1) WHERE key=?2", rusqlite::params![clock, COMPLETION]).unwrap();
        let hits = client.status().cache_hits;
        assert!(!admitted(&client).await, "invalid completion clock {clock}");
        assert_eq!(client.status().cache_hits, hits);
    }
    assert_eq!(client.status().network_requests, 0);
}

#[tokio::test]
async fn policy_admission_completion_cannot_replace_source_freshness_or_head_identity() {
    let (dir, client) = fixture().await;
    client
        .save_derived(COMPLETION, json!(crate::now_ms()))
        .await
        .unwrap();
    assert!(admitted(&client).await);
    let db = rusqlite::Connection::open(dir.path().join("cache.sqlite")).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',1) WHERE key LIKE '%/status?%'", []).unwrap();
    assert!(!admitted(&client).await);
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key LIKE '%/status?%'", [crate::now_ms()]).unwrap();
    assert!(admitted(&client).await);
    client
        .observe(
            "ci://github.com/acme/demo/7",
            &json!({
                "head_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "merge_sha":null, "errors":[]
            }),
        )
        .await
        .unwrap();
    assert!(!admitted(&client).await);
    assert_eq!(client.status().network_requests, 0);
}

#[tokio::test]
async fn policy_admission_accepts_fresh_complete_versions_without_renewing_offline_clocks() {
    let (dir, client) = fixture().await;
    let path = format!("http://127.0.0.1:1/repos/acme/demo/commits/{HEAD}/status?per_page=100");
    client.save_derived(&path, json!({"sha":HEAD,"total_count":1,"statuses":[{
        "id":11,"node_id":"SC_11","updated_at":"2026-09-19T00:00:00Z",
        "context":"deploy","state":"success","description":null,"target_url":"https://checks.example/head"
    }]})).await.unwrap();
    let now = crate::now_ms();
    let old = now - 120_000;
    let db = rusqlite::Connection::open(dir.path().join("cache.sqlite")).unwrap();
    db.execute(
        "UPDATE cache SET response=json_set(response,'$.validated_at_ms',?1) WHERE key=?2",
        rusqlite::params![old, path],
    )
    .unwrap();
    client.save_derived(crate::dashboard::DISCOVERY_CACHE, json!({
        "pulls":[{"id":"PR_7","number":7,"headRefOid":HEAD,"repository":{"nameWithOwner":"acme/demo"},
            "commits":{"nodes":[{"commit":{"oid":HEAD,
                "status":{"id":"S_head","contexts":[{"id":"SC_11","updatedAt":"2026-09-19T00:00:00Z",
                    "context":"deploy","state":"SUCCESS","description":null,"targetUrl":"https://checks.example/head"}]},
                "statusCheckRollup":{"contexts":{"checkRunCount":0,"statusContextCount":1}}
            }}]}}],
        "validatedAtMs":now,"validatedAtByPr":{"acme/demo/7":now}
    })).await.unwrap();
    client.save_derived(COMPLETION, json!(1)).await.unwrap();
    let before = client.bootstrap().await.unwrap().cursor;
    assert!(
        admitted(&client).await,
        "fresh matching versions must make policy eligible"
    );
    assert_eq!(client.status().network_requests, 0);
    assert_eq!(client.status().outstanding_requests, 0);
    assert_eq!(client.bootstrap().await.unwrap().cursor, before);
    let clocks = crate::report::VALIDATIONS
        .scope(std::cell::RefCell::new(Vec::new()), async {
            crate::report::ci_discovery_scope(
                "acme/demo",
                7,
                crate::entity::scope(async {
                    let pr = client
                        .peek_ci_pull_request("acme/demo", 7)
                        .await
                        .unwrap()
                        .unwrap();
                    crate::entity::set(client.pr_owner("acme/demo", 7, &pr.data).await.unwrap());
                    let ci = client
                        .required_ci_report("acme/demo", HEAD, None, Freshness::CachedOnly)
                        .await
                        .unwrap();
                    assert!(ci.errors.is_empty());
                }),
            )
            .await;
            crate::report::VALIDATIONS.with(|v| v.borrow().clone())
        })
        .await;
    assert!(
        clocks
            .iter()
            .any(|v| v.resource == path && v.validated_at_ms == old)
    );
    assert_eq!(client.peek_get(&path).await.unwrap().validated_at_ms, old);

    // A fresh completion does not authorize stale, mismatched, or incomplete
    // version evidence. Nor may a failed shortcut start a REST validation.
    for mutation in ["stale", "changed", "missing", "old_payload"] {
        let (key, raw): (String, String) = db
            .query_row(
                "SELECT key,response FROM cache WHERE key=?1",
                [crate::dashboard::DISCOVERY_CACHE],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let mut proof: Value = serde_json::from_str(&raw).unwrap();
        match mutation {
            "stale" => proof["data"]["validatedAtByPr"]["acme/demo/7"] = json!(old),
            "changed" => {
                proof["data"]["pulls"][0]["commits"]["nodes"][0]["commit"]["status"]["contexts"]
                    [0]["state"] = json!("FAILURE")
            }
            "missing" => {
                proof["data"]["pulls"][0]["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]
                    ["statusContextCount"] = json!(2)
            }
            "old_payload" => {
                db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',1) WHERE key=?1",[&path]).unwrap();
            }
            _ => unreachable!(),
        }
        db.execute(
            "UPDATE cache SET response=?1 WHERE key=?2",
            [proof.to_string(), key.clone()],
        )
        .unwrap();
        assert!(
            !tokio::time::timeout(Duration::from_secs(1), admitted(&client))
                .await
                .expect("admission cannot wait for GitHub"),
            "{mutation}"
        );
        assert_eq!(client.status().network_requests, 0, "{mutation}");
        assert_eq!(client.status().outstanding_requests, 0, "{mutation}");
        assert_eq!(client.bootstrap().await.unwrap().cursor, before);
        db.execute("UPDATE cache SET response=?1 WHERE key=?2", [raw, key])
            .unwrap();
    }
}

#[tokio::test]
async fn admission_cache_probe_never_joins_requests_or_mints_app_credentials() {
    use axum::{Json, Router, extract::State, routing::get};
    use std::sync::Arc;
    use tokio::sync::Notify;
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let app = Router::new()
        .route(
            "/busy",
            get(
                |State((started, release)): State<(Arc<Notify>, Arc<Notify>)>| async move {
                    started.notify_one();
                    release.notified().await;
                    Json(json!({"ok":true}))
                },
            ),
        )
        .route("/outside", get(|| async { Json(json!({"ok":true})) }))
        .with_state((started.clone(), release.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(
        crate::Config {
            cache_path: dir.path().join("cache.sqlite"),
            rest_url: base.parse().unwrap(),
            graphql_url: format!("{base}graphql").parse().unwrap(),
            installation: Some(
                crate::AppInstallation::new(
                    "synthetic-client".into(),
                    42,
                    vec!["acme/demo".into()],
                    include_str!("../../tests/fixtures/github-app-test-key.pem"),
                )
                .unwrap(),
            ),
            queue_timeout: Duration::from_secs(2),
            ..Default::default()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    client
        .save_derived(&format!("{base}cached"), json!({"ok":true}))
        .await
        .unwrap();
    let reader = client.clone();
    let active = tokio::spawn(async move { reader.get("busy", Freshness::Revalidate).await });
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_millis(500),
        crate::client::CACHE_PROBE.scope((), async {
            let fresh = Freshness::MaxAge(Duration::from_secs(60));
            assert!(client.get("cached", fresh).await.is_ok());
            for path in [
                "busy",
                "missing",
                &format!("repos/acme/demo/commits/{HEAD}/status?per_page=100"),
            ] {
                assert!(
                    matches!(client.get(path, fresh).await, Err(Error::CacheMiss)),
                    "{path}"
                );
            }
            for freshness in [Freshness::Revalidate, Freshness::MaxAge(Duration::ZERO)] {
                assert!(matches!(
                    client.get("cached", freshness).await,
                    Err(Error::CacheMiss)
                ));
            }
            assert!(matches!(
                client.ci_selector_response("acme/demo", 7, fresh).await,
                Err(Error::CacheMiss)
            ));
        }),
    )
    .await
    .expect("cache admission cannot wait on the active caller");
    assert_eq!(client.status().network_requests, 1);
    assert_eq!(client.status().outstanding_requests, 1);
    assert_eq!(client.status().coalesced_requests, 0);
    release.notify_one();
    assert!(active.await.unwrap().is_ok());
    assert!(
        client.get("outside", Freshness::Revalidate).await.is_ok(),
        "probe scope must not leak to normal reads"
    );
    assert_eq!(client.status().network_requests, 2);
    server.abort();
}

#[tokio::test]
async fn policy_admission_checkpoints_deferred_schedule_once_per_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let client = Client::with_token(
        crate::Config {
            cache_path: path.clone(),
            rest_url: "http://127.0.0.1:1/".parse().unwrap(),
            graphql_url: "http://127.0.0.1:1/graphql".parse().unwrap(),
            ..Default::default()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    let pulls: Vec<_> = (1..=512)
        .map(|n| {
            json!({
                "id":format!("PR_{n}"), "number":n, "repository":{"nameWithOwner":"acme/demo"},
                "headRefOid":HEAD, "state":"OPEN"
            })
        })
        .collect();
    client
        .save_derived(
            "account-discovery-complete:v1",
            json!({
                "pulls":pulls, "validatedAtMs":crate::now_ms()
            }),
        )
        .await
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE schedule_writes (key TEXT);
        CREATE TRIGGER schedule_insert AFTER INSERT ON cache WHEN NEW.key='account-status-schedule:policy'
        BEGIN INSERT INTO schedule_writes VALUES(NEW.key); END;
        CREATE TRIGGER schedule_update AFTER UPDATE ON cache WHEN NEW.key='account-status-schedule:policy'
        BEGIN INSERT INTO schedule_writes VALUES(NEW.key); END;").unwrap();
    client
        .hydrate_pr_status_policy(Freshness::MaxAge(Duration::from_secs(60)))
        .await
        .unwrap();
    let cycle = client
        .peek_derived("account-status-cycle:policy")
        .await
        .unwrap()
        .unwrap()
        .data;
    assert_eq!(cycle["total"], 512);
    assert_eq!(cycle["attempted"], 0);
    assert_eq!(cycle["waiting_for_ci"], 512);
    assert_eq!(cycle["cycle_budget_exhausted"], false);
    let writes: u64 = db
        .query_row("SELECT count(*) FROM schedule_writes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(writes, 2, "initial reconcile and final deferral checkpoint");
    let saved = client
        .peek_derived("account-status-schedule:policy")
        .await
        .unwrap()
        .unwrap()
        .data;
    assert_eq!(saved["next"], json!(["acme/demo", 1]));
    assert_eq!(client.status().network_requests, 0);
}
