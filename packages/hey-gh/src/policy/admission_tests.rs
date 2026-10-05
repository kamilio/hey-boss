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
