use super::*;
use crate::issues::allocations;

struct Fixture {
    store: Store,
    settings: Settings,
    _root: crate::admin::Temporary,
}

impl Fixture {
    fn new(role: &str) -> Self {
        let root = crate::admin::Temporary::new().unwrap();
        let store = Store::open(&root.0.join("issues.db")).unwrap();
        store
            .db
            .execute("UPDATE fleet_meta SET role=?1,node='local'", [role])
            .unwrap();
        store.db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('named:Queue','Queue',2);
            INSERT INTO agents VALUES('creator','{}',0);
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,assignment_target)
            VALUES('named:Queue',1,'Queued task','','open','creator',0,0,7,'[]','machine:local');
            INSERT INTO fleet_allocations VALUES('named:Queue',1,'local');").unwrap();
        let settings = Settings {
            enabled: true,
            directory: root.0.to_string_lossy().into(),
            projects: vec!["named:Queue".into()],
            ..Settings::default()
        };
        Self {
            store,
            settings,
            _root: root,
        }
    }

    fn expire(&self) {
        self.store
            .db
            .execute("UPDATE fleet_allocation_deadlines SET expires_at=0", [])
            .unwrap();
        self.store
            .db
            .execute(
                "UPDATE worker_allocation_clock SET renewed_tick=0,boot=''",
                [],
            )
            .unwrap();
    }

    fn maintain(&self) {
        allocations::maintain(&self.store.db).unwrap();
    }

    fn count(&self, table: &str) -> i64 {
        self.store
            .db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn eligible(&self) -> usize {
        candidates(&self.store.db, &self.settings, 10)
            .unwrap()
            .len()
    }

    fn run(&self, state: &str, finished: Option<i64>, claimed: bool) {
        self.store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at,claimed_at,reservation_expires,retry_at)
            VALUES('old','named:Queue',1,'{}','creator',?1,?2,?3,'local',1,1,?4,?5,0,9223372036854775807)",
            params![state,std::process::id(),crate::agents::process_identity(std::process::id()).unwrap(),finished,claimed.then_some(1)]).unwrap();
    }
}

#[test]
fn expired_assignment_recovers_once_after_capacity_returns() {
    let mut f = Fixture::new("peer");
    let first = f
        .store
        .register_worker(Some("first"), &f.settings, "local")
        .unwrap();
    let second = f
        .store
        .register_worker(Some("second"), &f.settings, "local")
        .unwrap();
    // A scheduled job / upgrade owns the worker's capacity while the assignment ages.
    f.store.worker_set_upgrading(&first, true).unwrap();
    f.store.worker_set_upgrading(&second, true).unwrap();
    f.expire();
    assert_eq!(f.eligible(), 0);
    assert!(
        reserve(&mut f.store, "local", Some(&first))
            .unwrap()
            .is_none()
    );
    assert_eq!(f.count("worker_runs"), 0);
    f.store.worker_set_upgrading(&first, false).unwrap();
    f.store.worker_set_upgrading(&second, false).unwrap();
    let job = reserve(&mut f.store, "local", Some(&first))
        .unwrap()
        .expect("expired assignment must recover without reassignment");
    assert_eq!(job.number(), 1);
    for _ in 0..3 {
        f.maintain();
        assert!(
            reserve(&mut f.store, "local", Some(&second))
                .unwrap()
                .is_none()
        );
        assert!(
            reserve(&mut f.store, "local", Some(&first))
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(f.count("worker_runs"), 1);
    assert_eq!(f.count("worker_allocations"), 1);
    assert_eq!(f.count("fleet_allocation_deadlines"), 0);
    let issue: (String, i64) = f
        .store
        .db
        .query_row("SELECT assignment_target,version FROM issues", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(issue, ("machine:local".into(), 7));
    f.store
        .db
        .execute("UPDATE worker_allocation_observations SET ready_tick=0", [])
        .unwrap();
    assert!(allocations::launch(&f.store.db, &job.id).unwrap().is_none());
}

#[test]
fn expired_assignment_retains_routing_and_all_queue_holds() {
    for hold in [
        "UPDATE issues SET assignment_target='machine:remote'; UPDATE fleet_allocations SET node='remote'",
        "UPDATE issues SET draft=1",
        "UPDATE issues SET blockers='[2]'",
        "INSERT INTO project_settings(project_id,prompt,version,paused) VALUES('named:Queue','',1,1)",
        "UPDATE projects SET hidden_at=1",
        "UPDATE issues SET attempt_hold='{}'",
        "UPDATE issues SET assignee='creator'",
    ] {
        let f = Fixture::new("peer");
        f.store.db.execute_batch(hold).unwrap();
        f.expire();
        f.maintain();
        assert_eq!(f.eligible(), 0, "{hold}");
        assert_eq!(f.count("worker_runs"), 0);
        assert_eq!(f.count("fleet_allocations"), 1);
    }
    let f = Fixture::new("peer");
    f.run("failed", Some(2), false);
    f.store
        .db
        .execute("UPDATE worker_allocations SET released=1", [])
        .unwrap();
    f.expire();
    f.maintain();
    assert_eq!(f.eligible(), 0, "finished attempt keeps its retry hold");
    f.store
        .db
        .execute("UPDATE worker_runs SET retry_at=0", [])
        .unwrap();
    assert_eq!(
        f.eligible(),
        1,
        "finished history is distinct from active ownership"
    );
}

#[test]
fn expired_assignment_preserves_legacy_deadlines_until_peer_transition() {
    for role in ["controller", "agent", "standalone"] {
        let f = Fixture::new(role);
        f.expire();
        f.maintain();
        assert_eq!(f.count("fleet_allocation_deadlines"), 1);
        assert_eq!(f.eligible(), 0);
        f.store
            .db
            .execute("UPDATE fleet_meta SET role='peer'", [])
            .unwrap();
        f.maintain();
        assert_eq!(f.count("fleet_allocation_deadlines"), 0);
        assert_eq!(f.eligible(), 1);
    }
}

#[test]
fn expired_assignment_does_not_release_live_local_or_remote_attempts() {
    for claimed in [false, true] {
        let f = Fixture::new("controller");
        f.run(
            if claimed { "running" } else { "awaiting_claim" },
            None,
            claimed,
        );
        if claimed {
            f.store
                .db
                .execute("UPDATE issues SET assignee='creator'", [])
                .unwrap();
        }
        f.store
            .db
            .execute("UPDATE fleet_meta SET role='peer'", [])
            .unwrap();
        f.expire();
        f.maintain();
        assert_eq!(f.eligible(), 0);
        assert!(allocations::owns(&f.store.db, "old", "named:Queue", 1).unwrap());
        f.store
            .db
            .execute_batch(
                "UPDATE worker_allocation_observations SET deadline_tick=0;
            UPDATE worker_allocation_presence SET seen_tick=0;
            UPDATE worker_allocation_clock SET renewed_tick=0",
            )
            .unwrap();
        allocations::receive_presence(&f.store.db, &[], std::time::Duration::ZERO).unwrap();
        assert!(allocations::owns(&f.store.db, "old", "named:Queue", 1).unwrap());
        assert_eq!(f.count("worker_runs"), 1);
        assert_eq!(f.eligible(), 0);
    }
    let f = Fixture::new("peer");
    f.store
        .db
        .execute_batch(
            "UPDATE fleet_meta SET node='observer';
        UPDATE issues SET assignment_target='machine:remote';
        UPDATE fleet_allocations SET node='remote'",
        )
        .unwrap();
    f.store.db.execute_batch("INSERT INTO worker_allocations(id,project_id,issue_number,machine,ticket,launched,settle_ms,lease_ms)
        VALUES('remote','named:Queue',1,'remote',1,1,8,24)").unwrap();
    f.expire();
    f.maintain();
    for tick in [100_000, 200_000] {
        allocations::reconcile(
            &f.store.db,
            &allocations::Clock {
                boot: "observer".into(),
                tick,
            },
        )
        .unwrap();
        assert!(allocations::owns(&f.store.db, "remote", "named:Queue", 1).unwrap());
        assert_eq!(
            f.store
                .db
                .query_row("SELECT id FROM worker_allocation_winners", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "remote"
        );
        assert_eq!(
            f.eligible(),
            0,
            "missing remote presence cannot remove its launch fence"
        );
    }
}

#[test]
fn expired_assignment_recovery_leaves_other_deadlines_and_replays_unchanged() {
    for change in [
        "UPDATE fleet_allocation_deadlines SET expires_at=9223372036854775807",
        "UPDATE issues SET assignment_target=NULL",
        "UPDATE fleet_allocations SET node='other'",
    ] {
        let f = Fixture::new("peer");
        f.expire();
        f.store.db.execute_batch(change).unwrap();
        f.maintain();
        assert_eq!(f.count("fleet_allocation_deadlines"), 1, "{change}");
    }
    let f = Fixture::new("peer");
    f.expire();
    f.maintain();
    assert_eq!(f.count("fleet_allocation_deadlines"), 0);
    let changes = || {
        f.store
            .db
            .query_row("SELECT total_changes()", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    let before = changes();
    for _ in 0..3 {
        allocations::reconcile(&f.store.db, &allocations::Clock::read().unwrap()).unwrap();
    }
    assert_eq!(
        changes(),
        before,
        "Repeated observations do not rewrite routing or ownership"
    );
    assert_eq!(f.eligible(), 1);
}

#[test]
fn expired_assignment_competing_workers_and_replica_observation_keep_one_attempt() {
    let mut f = Fixture::new("peer");
    for id in ["first", "second"] {
        f.store
            .register_worker(Some(id), &f.settings, "local")
            .unwrap();
    }
    f.expire();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|id| {
            let path = f._root.0.join("issues.db");
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = Store::open(&path).unwrap();
                barrier.wait();
                // Like the scheduler, retry a contended maintenance transaction
                // on the next poll; a busy result is not a reservation.
                for _ in 0..20 {
                    match reserve(&mut store, "local", Some(id)) {
                        Err(error) if error.code == "database_busy" => {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        result => return result.unwrap(),
                    }
                }
                panic!("Worker never got past writer contention")
            })
        })
        .collect();
    let jobs: Vec<_> = handles
        .into_iter()
        .filter_map(|h| h.join().unwrap())
        .collect();
    assert_eq!(jobs.len(), 1);
    assert_eq!(f.count("worker_runs"), 1);
    let job = &jobs[0];
    f.store
        .db
        .execute("UPDATE worker_allocation_observations SET ready_tick=0", [])
        .unwrap();
    assert!(allocations::launch(&f.store.db, &job.id).unwrap().is_none());

    let observer = Fixture::new("peer");
    observer
        .store
        .db
        .execute("UPDATE fleet_meta SET node='observer'", [])
        .unwrap();
    let allocation: (i64, i64, i64, i64) = f
        .store
        .db
        .query_row(
            "SELECT ticket,launched,settle_ms,lease_ms FROM worker_allocations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    observer.store.db.execute("INSERT INTO worker_allocations(id,project_id,issue_number,machine,ticket,launched,settle_ms,lease_ms) VALUES(?1,'named:Queue',1,'local',?2,?3,?4,?5)", params![job.id,allocation.0,allocation.1,allocation.2,allocation.3]).unwrap();
    observer.expire();
    for tick in [0, 100_000] {
        allocations::reconcile(
            &observer.store.db,
            &allocations::Clock {
                boot: "observer-boot".into(),
                tick,
            },
        )
        .unwrap();
        assert_eq!(observer.eligible(), 0);
        assert!(allocations::owns(&observer.store.db, &job.id, "named:Queue", 1).unwrap());
        assert_eq!(observer.count("worker_runs"), 0);
    }
    assert_eq!(f.count("worker_runs"), 1);
}
