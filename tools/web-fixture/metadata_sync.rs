//! Drive two private copies of the CLI through its real replication protocol.
use super::*;
use std::io::Write;

fn database(f: &Fixture, request: Value) -> Result<Value> {
    let mut child = f
        .command(&["fleet", "database", "--path", f.db.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    writeln!(child.stdin.take().unwrap(), "{request}")?;
    let output = child.wait_with_output()?;
    assert!(output.status.success(), "fleet database: {}", output.status);
    Ok(serde_json::from_slice(&output.stdout)?)
}
fn rows(f: &Fixture, request: Value) -> Result<Value> {
    let reply = database(f, request)?;
    assert_eq!(reply["ok"], true, "{reply}");
    Ok(reply["rows"].clone())
}
fn sql(f: &Fixture, statement: &str) -> Result<Value> {
    rows(f, json!({"sql":statement,"args":[]}))
}
fn replica(f: &Fixture, request: Value) -> Result<Value> {
    Ok(serde_json::from_str(
        rows(f, request)?[0][0]
            .as_str()
            .ok_or("missing replica result")?,
    )?)
}

pub(super) fn verify(peer: &Fixture, project: &str) -> Result<Value> {
    let main = Fixture::new(&peer.binary)?;
    main.init(project)?;
    replica(
        &main,
        json!({"replica":"capture","role":"controller","node":"main"}),
    )?;
    replica(
        peer,
        json!({"replica":"capture","role":"agent","node":"peer"}),
    )?;
    let issue = |f: &Fixture, args: &[&str]| run(f.issue(project, args));
    issue(
        &main,
        &[
            "create",
            "--title",
            "Metadata stays saved",
            "--body",
            "Verify labels and lifecycle.",
        ],
    )?;
    issue(&main, &["close", "1"])?;
    let snapshot = || replica(&main, json!({"replica":"snapshot","node":"peer"}));
    let pull = |payload: Value, receipts: Value| {
        replica(
            peer,
            json!({"replica":"pull","node":"peer","payload":payload,"receipts":receipts}),
        )
    };
    pull(snapshot()?, json!([]))?;
    let view = || issue(peer, &["view", "1"]);
    let before = view()?;
    let label = [
        "edit",
        "1",
        "--label",
        "rework",
        "--request-id",
        "label-once",
    ];
    let revision = before["issue"]["version"].to_string();
    for args in [
        &label[..],
        &[
            "reopen",
            "1",
            "--if-version",
            &revision,
            "--request-id",
            "reopen-once",
        ],
    ] {
        let rejected = peer.issue(project, args).output()?;
        let reply: Value = serde_json::from_slice(&rejected.stdout)?;
        assert_eq!(rejected.status.code(), Some(4), "{reply}");
        assert_eq!(
            reply["error"]["code"], "fleet_allocation_missing",
            "{reply}"
        );
        assert!(
            reply["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not saved")
        );
        assert_eq!(view()?, before);
    }
    assert_eq!(
        sql(peer, "SELECT count(*) FROM fleet_outbox")?,
        json!([[0]])
    );
    assert_eq!(
        sql(
            peer,
            "SELECT count(*) FROM requests WHERE request_id IN ('label-once','reopen-once')"
        )?,
        json!([[0]])
    );
    sql(
        &main,
        "INSERT INTO fleet_allocations VALUES('named:Metadata sync QA',1,'peer')",
    )?;
    pull(snapshot()?, json!([]))?;
    let sync = || -> Result<()> {
        let entries = sql(
            peer,
            "SELECT json_object('seq',seq,'table_name',table_name,'before_json',before_json,'after_json',after_json,'created_at',created_at) FROM fleet_outbox ORDER BY seq",
        )?;
        let changes: Vec<Value> = entries
            .as_array()
            .unwrap()
            .iter()
            .map(|r| serde_json::from_str(r[0].as_str().unwrap()))
            .collect::<std::result::Result<_, _>>()?;
        assert!(!changes.is_empty());
        let accept = json!({"replica":"accept","node":"peer","changes":changes});
        let receipts = replica(&main, accept.clone())?;
        let applied = receipts.as_array().ok_or("missing receipts")?;
        assert_eq!(applied.len(), changes.len());
        assert!(
            applied.iter().all(|r| r["state"] == "applied"),
            "{receipts}"
        );
        assert_eq!(replica(&main, accept)?, receipts);
        let cursor = sql(
            peer,
            "SELECT CAST(value AS INTEGER) FROM fleet_state WHERE key='cursor'",
        )?[0][0]
            .as_i64()
            .unwrap();
        let payload = replica(
            &main,
            json!({"replica":"incremental","node":"peer","cursor":cursor}),
        )?;
        assert!(payload["cursor"].as_i64().unwrap() > cursor);
        pull(payload, receipts)?;
        assert_eq!(
            sql(peer, "SELECT count(*) FROM fleet_outbox")?,
            json!([[0]])
        );
        for query in [
            "SELECT title,body,labels,state,version FROM issues",
            "SELECT body FROM comments ORDER BY body",
            "SELECT action,count(*) FROM events GROUP BY action ORDER BY action",
        ] {
            assert_eq!(sql(peer, query)?, sql(&main, query)?, "{query}");
        }
        Ok(())
    };
    let edited = issue(peer, &label)?;
    assert_eq!(issue(peer, &label)?, edited);
    issue(
        &main,
        &["edit", "1", "--title", "Concurrent title survives"],
    )?;
    issue(&main, &["comment", "1", "--body", "Supervisor finding"])?;
    issue(peer, &["comment", "1", "--body", "Companion finding"])?;
    sync()?;
    let merged = view()?;
    assert_eq!(merged["issue"]["title"], "Concurrent title survives");
    assert!(
        merged["issue"]["labels"]
            .as_array()
            .unwrap()
            .contains(&json!("rework"))
    );
    assert_eq!(
        sql(peer, "SELECT body FROM comments ORDER BY body")?,
        json!([["Companion finding"], ["Supervisor finding"]])
    );
    let stale = snapshot()?;
    issue(
        peer,
        &[
            "reopen",
            "1",
            "--if-version",
            &merged["issue"]["version"].to_string(),
            "--request-id",
            "reopen-once",
        ],
    )?;
    sync()?;
    let reopened = view()?;
    assert_eq!(reopened["issue"]["state"], "open");
    let rejected = database(
        peer,
        json!({"replica":"pull","node":"peer","payload":stale,"receipts":[]}),
    )?;
    assert_eq!(rejected["ok"], false);
    assert!(
        rejected["error"]
            .as_str()
            .unwrap()
            .contains("Stale fleet pull"),
        "{rejected}"
    );
    assert_eq!(view()?, reopened);
    issue(&main, &["edit", "1", "--label", "later"])?;
    pull(snapshot()?, json!([]))?;
    assert!(
        view()?["issue"]["labels"]
            .as_array()
            .unwrap()
            .contains(&json!("later"))
    );
    issue(peer, &["close", "1", "--comment", "Verified completion"])?;
    sync()?;
    assert_eq!(view()?["issue"]["state"], "closed");
    for f in [&main, peer] {
        assert_eq!(sql(f, "PRAGMA integrity_check")?, json!([["ok"]]));
    }
    // Leave the viewer on an unallocated companion so edits exercise the denial path.
    sql(&main, "DELETE FROM fleet_allocations")?;
    pull(snapshot()?, json!([]))?;
    Ok(json!({"number":1}))
}
