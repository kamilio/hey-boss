use super::*;
use crate::jobs::{Definition, Operation as Job};
use serde_json::json;

#[test]
fn latest_job_summary_prefers_execution_over_non_target_peer_observations() {
    check_non_target_observations("pending");
}

#[test]
fn latest_job_summary_prefers_execution_over_non_target_peer_skips() {
    check_non_target_observations("skipped");
}

fn check_non_target_observations(observation_state: &str) {
    let f = Fixture::new();
    let db = f.db();
    install(&db, "mac").unwrap();
    seed(&db);
    db.execute_batch("UPDATE fleet_meta SET role='peer',node='mac'; UPDATE distributed_meta SET active=1,slot=0;").unwrap();
    let request = |operation: Job| -> crate::issues::Request {
        serde_json::from_value(json!({"version":1,"project":{"id":"p","name":"Project"},"actor":{"id":"human:test","kind":"human","machine":"mac","host":"test","cwd":"/tmp","source":"test"},"request_id":operation.writes().then(||hey_boss_sync::digest(serde_json::to_string(&operation).unwrap().as_bytes())),"operation":{"action":"job","operation":operation}})).unwrap()
    };
    let mut store = crate::issues::Store::open(&f.0.join("issues.db")).unwrap();
    store
        .execute(&request(Job::Create {
            id: "mac-only".into(),
            definition: Definition {
                name: "Mac schedule".into(),
                cron: "0 * * * *".into(),
                timezone: "UTC".into(),
                harness: "codex".into(),
                model: "gpt-6-astra".into(),
                machines: vec!["mac".into()],
                all_machines: false,
                each_machine: false,
                thread_mode: Default::default(),
            },
            markdown: include_str!("../../../jobs/fixtures/original.md").into(),
            enabled: true,
        }))
        .unwrap();
    let next: i64 = db
        .query_row("SELECT next_at FROM scheduled_jobs", [], |r| r.get(0))
        .unwrap();
    let due = store.due_jobs(next, 1).unwrap().remove(0);
    let run = store.enqueue_job(&due).unwrap();
    db.execute("UPDATE scheduled_job_runs SET state='running',machine='mac',worker_id='mac-worker',session_id='mac-session',started_at=created_at+1 WHERE id=?1", [&run.id]).unwrap();
    let mut engine = hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap();
    capture_all(&db, &mut engine);
    let mut execution = replica::rows(&db, "SELECT * FROM scheduled_job_runs", &[])
        .unwrap()
        .remove(0);
    execution.as_object_mut().unwrap().remove("sequence");
    let import = |row: &Value, origin: &str, clock| {
        let tx = db.unchecked_transaction().unwrap();
        projection::reduce(
            &tx,
            &Revision {
                protocol: PROTOCOL,
                origin: origin.into(),
                clock,
                table: "scheduled_job_runs".into(),
                before: None,
                after: Some(row.clone()),
                resource: None,
                archive: None,
            },
        )
        .unwrap();
        materialize::drain(&tx).unwrap();
        tx.commit().unwrap();
    };
    let mut pending = execution.clone();
    pending["state"] = json!(observation_state);
    pending["reason"] = json!(if observation_state == "skipped" {
        "overlap"
    } else {
        "Waiting for an eligible worker on mac"
    });
    if observation_state == "skipped" {
        pending["finished_at"] = execution["created_at"].clone();
    }
    for field in ["machine", "worker_id", "session_id", "started_at"] {
        pending[field] = Value::Null;
    }
    for (n, origin) in [(1, "devbox"), (2, "devboxpoe:baseline:seed")] {
        pending["id"] = json!(format!("peer-{n}"));
        pending["created_at"] = json!(execution["created_at"].as_i64().unwrap() + n);
        import(&pending, origin, 100);
    }
    let summary = |store: &mut crate::issues::Store| {
        let view = store
            .execute(&request(Job::View {
                id: "mac-only".into(),
            }))
            .unwrap();
        let list = store
            .execute(&request(Job::List {
                after: None,
                limit: 10,
                include_deleted: false,
            }))
            .unwrap();
        assert_eq!(view["job"]["last_run"], list["jobs"][0]["last_run"]);
        view["job"].clone()
    };
    let history = |store: &mut crate::issues::Store| {
        store
            .execute(&request(Job::History {
                id: "mac-only".into(),
                before: None,
                limit: 100,
            }))
            .unwrap()
    };
    let before = history(&mut store);
    assert_eq!(before["runs"].as_array().unwrap().len(), 3);
    assert_eq!(summary(&mut store)["last_run"]["id"], run.id);
    assert_eq!(summary(&mut store)["active_run"]["id"], run.id);
    assert_eq!(
        history(&mut store),
        before,
        "Summary must not rewrite history"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM scheduled_job_runs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );

    // A completed execution, including failure, still outranks observations.
    for (n, state) in ["succeeded", "failed", "cancelled"].into_iter().enumerate() {
        let mut finished = execution.clone();
        finished["state"] = json!(state);
        finished["finished_at"] = json!(execution["created_at"].as_i64().unwrap() + 10);
        import(&finished, "mac", 1000 + n as i64);
        assert_eq!(summary(&mut store)["last_run"]["state"], state);
    }
    // Only the exact occurrence and immutable snapshot may replace an observation.
    for field in [
        "scheduled_at",
        "revision",
        "instruction_digest",
        "request_key",
    ] {
        let mut distinct = pending.clone();
        distinct["id"] = json!(format!("distinct-{field}"));
        if field == "scheduled_at" {
            distinct[field] = json!(run.scheduled_at + 3_600_000);
        } else if field == "request_key" {
            distinct["trigger"] = json!("manual");
            distinct[field] = json!("manual-retry");
        } else {
            let mut snapshot: Value =
                serde_json::from_str(distinct["snapshot"].as_str().unwrap()).unwrap();
            snapshot[field] = if field == "revision" {
                json!(2)
            } else {
                json!("different")
            };
            distinct["snapshot"] = json!(snapshot.to_string());
        }
        import(&distinct, "devbox", 2000);
        assert_eq!(
            summary(&mut store)["last_run"]["id"],
            distinct["id"],
            "{field}"
        );
    }
    // Real failure, waiting and skips on an eligible peer must remain visible.
    for (n, state, origin) in [
        (0, "failed", "devbox"),
        (1, "pending", "mac:baseline:seed"),
        (2, "skipped", "mac:baseline:seed"),
    ] {
        let mut genuine = pending.clone();
        genuine["id"] = json!(format!("genuine-{n}"));
        genuine["state"] = json!(state);
        genuine["finished_at"] = if state == "pending" {
            Value::Null
        } else {
            genuine["created_at"].clone()
        };
        import(&genuine, origin, 3000);
        assert_eq!(summary(&mut store)["last_run"]["id"], genuine["id"]);
    }
}
