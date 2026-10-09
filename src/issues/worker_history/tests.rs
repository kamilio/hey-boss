use super::*;
use crate::issues::{Store, worker::Settings};

use std::collections::HashMap;

#[test]
fn retired_history_is_only_rebuilt_when_its_dependencies_change() {
    let root = std::env::temp_dir().join(format!(
        "hb-history-{}",
        crate::issues::worker::random_id().unwrap()
    ));
    let store = Store::open(&root.join("issues.db")).unwrap();
    let db = Store::open_connection(&root.join("issues.db")).unwrap();
    db.execute_batch(
        "INSERT INTO projects(id,name,next_number) VALUES('p','Project',1000);
        INSERT INTO agents VALUES('codex:agent','{}',0);",
    )
    .unwrap();
    for w in 0..30 {
        db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES(?1,'managed',?2,1,0)", rusqlite::params![format!("worker-{w}"), serde_json::to_string(&Settings::default()).unwrap()]).unwrap();
        for n in 0..20 {
            let number = w * 20 + n + 1;
            db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('p',?1,'Task','','open','codex:agent',0,0,1,'[]')", [number]).unwrap();
            db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id,finished_at,summary) VALUES(?1,'p',?2,'{\"issue\":{\"title\":\"Task\"}}','codex:agent','completed',1,'start','machine',?2,0,?3,1,?4)", rusqlite::params![format!("run-{number}"),number,format!("worker-{w}"),"s".repeat(4096)]).unwrap();
        }
    }
    install(&db).unwrap();
    let mut versions = HashMap::new();
    let (first, initial_allocations) = crate::test_allocations::measure(|| {
        store.fleet_workers_incremental(&mut versions).unwrap()
    });
    assert_eq!(
        first
            .iter()
            .map(|w| w["runs"].as_array().unwrap().len())
            .sum::<usize>(),
        600
    );
    assert!(serde_json::to_vec(&first).unwrap().len() > 2_000_000);
    for _ in 0..3 {
        let (quiet, quiet_allocations) = crate::test_allocations::measure(|| {
            store.fleet_workers_incremental(&mut versions).unwrap()
        });
        assert!(
            quiet_allocations < initial_allocations / 4,
            "Quiet poll rebuilt history: {quiet_allocations} / {initial_allocations}"
        );
        assert!(quiet.iter().all(|w| w.get("runs").is_none()));
        assert!(serde_json::to_vec(&quiet).unwrap().len() < 40_000);
    }
    for sql in [
        "UPDATE worker_runs SET summary='same timestamp correction' WHERE id='run-1'",
        "INSERT INTO worker_events(run_id,created_at,text) VALUES('run-1',1,'late event')",
        "UPDATE worker_runs SET retry_at=123,retry_count=1 WHERE id='run-1'",
        "UPDATE issues SET state='closed' WHERE project_id='p' AND number=1",
        "UPDATE worker_runs SET finished_at=NULL,state='running',stop_requested=1 WHERE id='run-1'",
        "UPDATE worker_runs SET finished_at=2,state='completed' WHERE id='run-1'",
    ] {
        db.execute_batch(sql).unwrap();
        let update = store.fleet_workers_incremental(&mut versions).unwrap();
        let rebuilt: Vec<_> = update.iter().filter(|w| w.get("runs").is_some()).collect();
        assert_eq!(rebuilt.len(), 1, "{sql}");
        assert_eq!(rebuilt[0]["id"], "worker-0");
        let full = store.fleet_workers().unwrap();
        assert_eq!(
            rebuilt[0],
            full.iter().find(|w| w["id"] == "worker-0").unwrap()
        );
    }
    // A new attempt affects the previous worker's retry display too.
    db.execute_batch(
        "UPDATE issues SET state='open' WHERE project_id='p' AND number=1;
        UPDATE worker_runs SET retry_at=123 WHERE id='run-1';",
    )
    .unwrap();
    store.fleet_workers_incremental(&mut versions).unwrap();
    db.execute_batch("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id)
        VALUES('retry','p',1,'{\"issue\":{\"title\":\"Task\"}}','codex:agent','running',1,'start','machine',1000,0,'worker-1');").unwrap();
    let update = store.fleet_workers_incremental(&mut versions).unwrap();
    assert_eq!(update.iter().filter(|w| w.get("runs").is_some()).count(), 2);
    let retried = update.iter().find(|w| w["id"] == "worker-1").unwrap();
    assert_eq!(retried["active"], 1);
    assert_eq!(retried["runs"].as_array().unwrap().len(), 21);
    let previous = update.iter().find(|w| w["id"] == "worker-0").unwrap();
    assert!(
        previous["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "run-1")
            .unwrap()["retry_at"]
            .is_null()
    );
    db.execute_batch("UPDATE agents SET last_seen=100 WHERE id='codex:agent'")
        .unwrap();
    assert!(
        store
            .fleet_workers_incremental(&mut versions)
            .unwrap()
            .iter()
            .all(|w| w.get("runs").is_none())
    );
    db.execute_batch("UPDATE agents SET metadata='{\"model\":\"updated\"}' WHERE id='codex:agent'")
        .unwrap();
    assert!(
        store
            .fleet_workers_incremental(&mut versions)
            .unwrap()
            .iter()
            .all(|w| w["runs"][0]["model"] == "updated")
    );
    let reconnect = store
        .fleet_workers_incremental(&mut HashMap::new())
        .unwrap();
    assert_eq!(reconnect, store.fleet_workers().unwrap());
    drop(store);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
