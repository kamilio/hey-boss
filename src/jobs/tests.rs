use super::*;
use crate::jobs::Definition;
const ORIGINAL: &str = include_str!("fixtures/original.md");
const REVISED: &str = include_str!("fixtures/revised.md");
struct Fixture {
    store: Store,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "scheduled-jobs-test-{}",
            super::super::super::worker::random_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let store = Store::open(&root.join("issues.db")).unwrap();
        store
            .db
            .execute(
                "INSERT INTO projects(id,name,next_number) VALUES('named:Jobs','Jobs',1)",
                [],
            )
            .unwrap();
        Self { store, root }
    }
    fn reopen(&mut self) {
        self.store = Store::open(&self.root.join("issues.db")).unwrap();
    }
    fn op(&mut self, op: JobOperation, now: i64) -> Result<Value> {
        self.store.execute_job_at(&request(op), now)
    }
    fn create(&mut self, id: &str, now: i64) -> Value {
        self.op(
            JobOperation::Create {
                id: id.into(),
                definition: definition(),
                markdown: ORIGINAL.into(),
                enabled: true,
            },
            now,
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn definition() -> Definition {
    Definition {
        name: "Schedule fixture".into(),
        cron: "0 * * * *".into(),
        timezone: "UTC".into(),
        harness: "codex".into(),
        model: "gpt-6-astra".into(),
    }
}
fn request(op: JobOperation) -> Request {
    serde_json::from_value(json!({"version":1,"project":{"id":"named:Jobs","name":"Jobs"},"actor":{"id":"human:fixture","kind":"human","machine":"test","host":"test","cwd":"/tmp","source":"test"},"request_id":if op.writes(){Some(format!("{:x}",sha2::Sha256::digest(serde_json::to_vec(&op).unwrap())))}else{None},"operation":{"action":"job","operation":op}})).unwrap()
}
fn task(db: &Connection, s: &Snapshot, _run: &str) -> Result<i64> {
    let n: i64 = db.query_row(
        "SELECT next_number FROM projects WHERE id=?1",
        [&s.project_id],
        |r| r.get(0),
    )?;
    db.execute(
        "UPDATE projects SET next_number=next_number+1 WHERE id=?1",
        [&s.project_id],
    )?;
    db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,draft) VALUES(?1,?2,?3,'','open','human:fixture',0,0,1,'[]',1)",params![s.project_id,n,s.definition.name])?;
    Ok(n)
}
fn reserve(f: &mut Fixture, now: i64) -> Run {
    let due = f.store.due_jobs(now, 100).unwrap();
    assert_eq!(due.len(), 1);
    f.store.commit_job_occurrence(&due[0], task).unwrap()
}
use sha2::Digest;
use std::path::PathBuf;
const H: i64 = 3_600_000;

#[test]
fn scheduled_tasks_never_enter_regular_pickup_after_unassignment_or_restart() {
    let mut f = Fixture::new();
    f.create("job", 0);
    let run = reserve(&mut f, H);
    f.store
        .db
        .execute("UPDATE issues SET draft=0,assignee=NULL", [])
        .unwrap();
    f.reopen();
    assert!(
        !f.store
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE project_id=?1 AND number=?2)",
                params![run.snapshot.project_id, run.task_number],
                |r| r.get::<_, bool>(0)
            )
            .unwrap(),
        "a dedicated job task must never enter the ordinary worker pool"
    );
    // New work created by the session is ordinary work.
    let followup = task(&f.store.db, &run.snapshot, "followup").unwrap();
    f.store
        .db
        .execute("UPDATE issues SET draft=0 WHERE number=?1", [followup])
        .unwrap();
    assert!(
        f.store
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE number=?1)",
                [followup],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
}

#[test]
fn edits_pause_resume_delete_schedule_forward_and_keep_revisions() {
    let mut f = Fixture::new();
    let created = f.create("job", 0);
    assert_eq!(created["job"]["revision"], 1);
    assert!(f.store.due_jobs(H - 1, 10).unwrap().is_empty());
    let prepared = f.store.due_jobs(H, 10).unwrap().remove(0);
    let edited = f
        .op(
            JobOperation::Edit {
                id: "job".into(),
                if_revision: 1,
                definition: definition(),
                markdown: Some(REVISED.into()),
            },
            H + 1,
        )
        .unwrap();
    assert_eq!(edited["job"]["next_at"], 2 * H);
    assert!(f.store.commit_job_occurrence(&prepared, task).is_err());
    assert!(
        f.op(
            JobOperation::SetEnabled {
                id: "job".into(),
                if_revision: 1,
                enabled: false
            },
            H + 2
        )
        .is_err()
    );
    f.op(
        JobOperation::SetEnabled {
            id: "job".into(),
            if_revision: 2,
            enabled: false,
        },
        H + 2,
    )
    .unwrap();
    assert!(f.store.due_jobs(10 * H, 10).unwrap().is_empty());
    f.op(
        JobOperation::SetEnabled {
            id: "job".into(),
            if_revision: 3,
            enabled: true,
        },
        10 * H,
    )
    .unwrap();
    assert!(f.store.due_jobs(10 * H, 10).unwrap().is_empty());
    let run = reserve(&mut f, 11 * H);
    f.op(
        JobOperation::Delete {
            id: "job".into(),
            if_revision: 4,
        },
        12 * H,
    )
    .unwrap();
    assert!(f.store.due_jobs(20 * H, 10).unwrap().is_empty());
    f.reopen();
    assert_eq!(f.store.job_instructions(&run.snapshot).unwrap(), REVISED);
    let old = f
        .op(
            JobOperation::Revision {
                id: "job".into(),
                revision: 1,
            },
            20 * H,
        )
        .unwrap();
    assert_eq!(old["instructions"]["markdown"], ORIGINAL);
    assert_eq!(
        f.op(
            JobOperation::History {
                id: "job".into(),
                before: None,
                limit: 10
            },
            20 * H
        )
        .unwrap()["runs"][0]["id"],
        run.id
    );
    f.store
        .start_job_run(&run.id, "companion", "session", 20 * H)
        .unwrap();
    f.store
        .finish_job_run(&run.id, "succeeded", None, 21 * H)
        .unwrap();
}
#[test]
fn downtime_coalesces_overlap_is_recorded_and_other_jobs_remain_independent() {
    let mut f = Fixture::new();
    f.create("job", 0);
    let first = reserve(&mut f, 100 * H + 123);
    assert_eq!(first.scheduled_at, 100 * H);
    assert_eq!(first.state, "pending");
    assert!(f.store.due_jobs(100 * H + 124, 10).unwrap().is_empty());
    f.create("other", 100 * H + 200);
    let due = f.store.due_jobs(101 * H, 10).unwrap();
    assert_eq!(due.len(), 2);
    let mut runs = Vec::new();
    for occurrence in due {
        runs.push(f.store.commit_job_occurrence(&occurrence, task).unwrap());
    }
    assert_eq!(runs.iter().filter(|r| r.state == "skipped").count(), 1);
    let skipped = runs.iter().find(|r| r.state == "skipped").unwrap();
    assert_eq!(skipped.reason.as_deref(), Some("overlap"));
    assert!(skipped.task_number.is_none());
    f.store
        .start_job_run(&first.id, "peer", "one", 101 * H + 1)
        .unwrap();
    assert!(
        f.store
            .start_job_run(&first.id, "peer", "two", 101 * H + 2)
            .is_err()
    );
    f.store
        .finish_job_run(&first.id, "succeeded", None, 101 * H + 3)
        .unwrap();
    let due = f.store.due_jobs(102 * H, 10).unwrap();
    let job = due.iter().find(|d| d.snapshot.job_id == "job").unwrap();
    assert_eq!(
        f.store.commit_job_occurrence(job, task).unwrap().state,
        "pending"
    );
}
#[test]
fn atomic_creation_retries_manual_keys_and_immutable_execution_snapshot() {
    let mut f = Fixture::new();
    f.create("job", 0);
    let occurrence = f.store.due_jobs(H, 10).unwrap().remove(0);
    assert!(
        f.store
            .commit_job_occurrence(&occurrence, |db, s, r| {
                task(db, s, r)?;
                Err(Error::invalid("Synthetic rollback"))
            })
            .is_err()
    );
    assert_eq!(
        f.store
            .db
            .query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let a = f.store.commit_job_occurrence(&occurrence, task).unwrap();
    let b = f
        .store
        .commit_job_occurrence(&occurrence, |_, _, _| panic!("must deduplicate"))
        .unwrap();
    assert_eq!(a.id, b.id);
    f.store
        .finish_job_run(&a.id, "cancelled", Some("fixture"), H + 1)
        .unwrap();
    let manual = f
        .store
        .manual_job("named:Jobs", "job", "button-1", H + 2)
        .unwrap();
    let m = f.store.commit_job_occurrence(&manual, task).unwrap();
    assert_eq!(m.trigger, "manual");
    f.op(
        JobOperation::Edit {
            id: "job".into(),
            if_revision: 1,
            definition: definition(),
            markdown: Some(REVISED.into()),
        },
        H + 3,
    )
    .unwrap();
    let retry = f
        .store
        .manual_job("named:Jobs", "job", "button-1", H + 4)
        .unwrap();
    assert_eq!(
        f.store
            .commit_job_occurrence(&retry, |_, _, _| panic!())
            .unwrap()
            .id,
        m.id
    );
    f.reopen();
    assert_eq!(f.store.job_instructions(&m.snapshot).unwrap(), ORIGINAL);
    let loaded = f
        .op(
            JobOperation::Run {
                id: "job".into(),
                run_id: m.id.clone(),
            },
            H + 5,
        )
        .unwrap();
    assert_eq!(loaded["run"]["snapshot"]["revision"], 1);
    assert_eq!(loaded["instructions"]["markdown"], ORIGINAL);
    assert!(
        f.store
            .finish_job_run(&m.id, "succeeded", None, H + 6)
            .is_err()
    );
}
#[test]
fn concurrent_occurrence_commits_create_one_task_and_identity() {
    let mut f = Fixture::new();
    f.create("job", 0);
    let occurrence = f.store.due_jobs(H, 10).unwrap().remove(0);
    let path = f.root.join("issues.db");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            let o = occurrence.clone();
            let b = barrier.clone();
            std::thread::spawn(move || {
                let mut s = Store::open(&path).unwrap();
                b.wait();
                s.commit_job_occurrence(&o, task).unwrap().id
            })
        })
        .collect();
    let ids: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(ids.iter().all(|id| id == &ids[0]));
    assert_eq!(
        f.store
            .db
            .query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn exact_markdown_sync_survives_restart_without_authority_and_detects_corruption() {
    let mut authority = Fixture::new();
    authority.create("job", 0);
    let run = reserve(&mut authority, H);
    let response = authority
        .op(
            JobOperation::Run {
                id: "job".into(),
                run_id: run.id.clone(),
            },
            H,
        )
        .unwrap();
    let mut companion = Fixture::new();
    let root = companion.root.join("issues.jobs");
    companion.store.cache_job_response(&response).unwrap();
    drop(authority);
    companion.reopen();
    let cached = companion.store.cached_job_run(&run.id).unwrap();
    assert_eq!(companion.store.job_instructions(&cached).unwrap(), ORIGINAL);
    let mut corrupt = response.clone();
    corrupt["instructions"]["digest"] = json!("0".repeat(64));
    assert!(files::cache_response(&root, &corrupt).is_err());
    assert!(!root.join(format!("{}.md", "0".repeat(64))).exists());
}
#[test]
fn revisions_round_trip_crlf_trailing_spaces_and_reject_symlinks() {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new();
    let exact = ORIGINAL.replace('\n', "\r\n") + "  \r\n";
    f.op(
        JobOperation::Create {
            id: "bytes".into(),
            definition: definition(),
            markdown: exact.clone(),
            enabled: true,
        },
        0,
    )
    .unwrap();
    let value = f
        .op(
            JobOperation::Revision {
                id: "bytes".into(),
                revision: 1,
            },
            0,
        )
        .unwrap();
    assert_eq!(
        value["instructions"]["markdown"]
            .as_str()
            .unwrap()
            .as_bytes(),
        exact.as_bytes()
    );
    let sha = value["instructions"]["digest"].as_str().unwrap();
    let root = f.root.join("issues.jobs");
    let path = root.join(format!("{sha}.md"));
    fs::remove_file(&path).unwrap();
    symlink(f.root.join("issues.db"), &path).unwrap();
    assert!(files::load(&root, sha).is_err());
    assert_eq!(
        f.store
            .db
            .query_row("SELECT count(*) FROM scheduled_jobs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn preview_and_reads_do_not_create_tasks_and_invalid_edits_do_not_change_jobs() {
    let mut f = Fixture::new();
    f.create("job", 0);
    let invalid = Definition {
        cron: "0 0 31 2 *".into(),
        ..definition()
    };
    assert!(
        f.op(
            JobOperation::Edit {
                id: "job".into(),
                if_revision: 1,
                definition: invalid,
                markdown: Some(REVISED.into())
            },
            H
        )
        .is_err()
    );
    let dates = f
        .op(
            JobOperation::Next {
                id: "job".into(),
                after: 0,
                through: 4 * H,
                limit: 3,
            },
            0,
        )
        .unwrap();
    assert_eq!(dates["occurrences"], json!([H, 2 * H, 3 * H]));
    assert_eq!(dates["revision"], 1);
    assert_eq!(
        f.store
            .db
            .query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        f.store
            .db
            .query_row("SELECT count(*) FROM project_chiefs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    f.op(
        JobOperation::SetEnabled {
            id: "job".into(),
            if_revision: 1,
            enabled: false,
        },
        H,
    )
    .unwrap();
    assert_eq!(
        f.op(
            JobOperation::Next {
                id: "job".into(),
                after: 0,
                through: 4 * H,
                limit: 3
            },
            H
        )
        .unwrap()["occurrences"],
        json!([])
    );
    let manual = f
        .store
        .manual_job("named:Jobs", "job", "paused-manual", 2 * H)
        .unwrap();
    assert_eq!(
        f.store.commit_job_occurrence(&manual, task).unwrap().state,
        "pending"
    );
}
#[test]
fn companion_storage_rejects_direct_mutations_and_request_content_reuse() {
    let mut f = Fixture::new();
    f.create("job", 0);
    let op = JobOperation::SetEnabled {
        id: "job".into(),
        if_revision: 1,
        enabled: false,
    };
    let mut first = request(op.clone());
    first.request_id = Some("same-key".into());
    f.store.execute_job_at(&first, H).unwrap();
    first.operation = Operation::Job {
        operation: JobOperation::Delete {
            id: "job".into(),
            if_revision: 2,
        },
    };
    assert_eq!(
        f.store.execute_job_at(&first, 2 * H).unwrap_err().code,
        "conflict"
    );
    f.store
        .db
        .execute("UPDATE fleet_meta SET role='agent'", [])
        .unwrap();
    assert_eq!(f.op(op, 3 * H).unwrap_err().code, "fleet_unavailable");
    assert!(f.store.due_jobs(3 * H, 10).is_err());
}
#[test]
fn receipts_are_durable_and_queries_use_due_and_history_indexes() {
    let mut f = Fixture::new();
    let saved = f.create("job", 0);
    f.reopen();
    assert_eq!(f.create("job", H)["job"], saved["job"]);
    f.create("another", 0);
    let query = |sql: &str| {
        f.store
            .db
            .query_collect(sql, [], |r| r.get::<_, String>(3))
            .unwrap()
            .join(" ")
    };
    assert!(query("EXPLAIN QUERY PLAN SELECT id FROM scheduled_jobs WHERE enabled=1 AND deleted_at IS NULL AND next_at<=100 ORDER BY next_at,id LIMIT 10").contains("scheduled_jobs_due"));
    assert!(query("EXPLAIN QUERY PLAN SELECT id FROM scheduled_job_runs WHERE job_id='job' AND sequence<100 ORDER BY sequence DESC LIMIT 10").contains("scheduled_job_history"));
    assert!(f.store.due_jobs(H, 0).is_err());
    assert!(f.store.due_jobs(H, 101).is_err());
}

#[test]
fn history_pages_keep_stable_cursors_across_new_runs_and_restart() {
    let mut f = Fixture::new();
    f.create("job", 0);
    let mut ids = Vec::new();
    for n in 1..=4 {
        let occurrence = f
            .store
            .manual_job("named:Jobs", "job", &format!("click-{n}"), n * H)
            .unwrap();
        let run = f.store.commit_job_occurrence(&occurrence, task).unwrap();
        f.store
            .finish_job_run(&run.id, "cancelled", None, n * H + 1)
            .unwrap();
        ids.push(run.id);
    }
    let first = f
        .op(
            JobOperation::History {
                id: "job".into(),
                before: None,
                limit: 2,
            },
            5 * H,
        )
        .unwrap();
    assert_eq!(first["runs"][0]["id"], ids[3]);
    assert_eq!(first["runs"][1]["id"], ids[2]);
    let before = first["next_cursor"].as_i64().unwrap();
    let occurrence = f
        .store
        .manual_job("named:Jobs", "job", "another-click", 5 * H)
        .unwrap();
    f.store.commit_job_occurrence(&occurrence, task).unwrap();
    f.reopen();
    let second = f
        .op(
            JobOperation::History {
                id: "job".into(),
                before: Some(before),
                limit: 2,
            },
            6 * H,
        )
        .unwrap();
    assert_eq!(second["runs"][0]["id"], ids[1]);
    assert_eq!(second["runs"][1]["id"], ids[0]);
    assert!(second["next_cursor"].is_null());
}

#[test]
fn runner_owns_top_task_and_fences_handoff_and_late_reports() {
    use crate::jobs::execution::Report;
    let mut f = Fixture::new();
    f.create("job", 0);
    let old = task(
        &f.store.db,
        &f.store.due_jobs(H, 10).unwrap()[0].snapshot,
        "old",
    )
    .unwrap();
    let occurrence = f.store.due_jobs(H, 10).unwrap().remove(0);
    let run = f.store.enqueue_job(&occurrence).unwrap();
    assert!(f.store.db.query_row("SELECT a.sort_order<b.sort_order FROM issues a,issues b WHERE a.number=?1 AND b.number=?2",params![run.task_number,old],|r|r.get::<_,bool>(0)).unwrap());
    assert_eq!(
        f.store
            .db
            .query_row(
                "SELECT body FROM issues WHERE number=?1",
                [run.task_number],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        ORIGINAL
    );
    assert!(f.store.assign_job(&run.id, "first").unwrap());
    assert!(!f.store.assign_job(&run.id, "second").unwrap());
    f.reopen();
    assert!(
        !f.store.assign_job(&run.id, "second").unwrap(),
        "restart/disconnect never releases an owner"
    );
    let dispatch = f.store.job_dispatches("first").unwrap().remove(0);
    let mut ack = Report::pending(&dispatch);
    ack.state = "released".into();
    ack.finished_at = Some(H + 1);
    assert!(
        f.store.accept_job_report("first", &ack).is_err(),
        "release needs an explicit revoke"
    );
    f.store.revoke_job(&run.id, "first", 1).unwrap();
    assert!(
        !f.store.assign_job(&run.id, "second").unwrap(),
        "revoke is not acknowledgement"
    );
    assert!(f.store.accept_job_report("first", &ack).unwrap());
    assert!(f.store.assign_job(&run.id, "second").unwrap());
    let second = f.store.job_dispatches("second").unwrap().remove(0);
    assert_eq!(second.generation, 2);
    ack.state = "running".into();
    ack.started_at = Some(H + 2);
    ack.finished_at = None;
    ack.session_id = Some("old-session".into());
    assert!(!f.store.accept_job_report("first", &ack).unwrap());
    let mut report = Report::pending(&second);
    report.state = "running".into();
    report.started_at = Some(H + 3);
    report.session_id = Some("exact-session".into());
    f.store.accept_job_report("second", &report).unwrap();
    report.state = "succeeded".into();
    report.finished_at = Some(H + 4);
    f.store.accept_job_report("second", &report).unwrap();
    assert_eq!(
        get_run(&f.store.db, &run.id).unwrap().session_id.as_deref(),
        Some("exact-session")
    );
    assert_eq!(
        f.store
            .db
            .query_row(
                "SELECT state FROM issues WHERE number=?1",
                [run.task_number],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "closed"
    );
    assert_eq!(
        f.store
            .db
            .query_row("SELECT count(*) FROM worker_runs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn manual_overlap_stop_pause_and_failure_keep_honest_task_status() {
    use crate::jobs::execution::Report;
    let mut f = Fixture::new();
    f.create("job", 0);
    f.op(
        JobOperation::SetEnabled {
            id: "job".into(),
            if_revision: 1,
            enabled: false,
        },
        1,
    )
    .unwrap();
    let run = f.op(JobOperation::RunNow { id: "job".into() }, H).unwrap()["run"].clone();
    assert_eq!(
        f.op(JobOperation::RunNow { id: "job".into() }, H + 1)
            .unwrap()["run"]["id"],
        run["id"]
    );
    let occurrence = f
        .store
        .manual_job("named:Jobs", "job", "other", H + 1)
        .unwrap();
    assert_eq!(f.store.enqueue_job(&occurrence).unwrap().state, "skipped");
    let id = run["id"].as_str().unwrap();
    f.store.assign_job(id, "local").unwrap();
    let dispatch = f.store.job_dispatches("local").unwrap().remove(0);
    f.op(
        JobOperation::Stop {
            id: "job".into(),
            run_id: id.into(),
        },
        H + 2,
    )
    .unwrap();
    assert!(f.store.job_dispatches("local").unwrap()[0].revoke);
    assert_eq!(
        get_run(&f.store.db, id).unwrap().state,
        "pending",
        "stop waits for the process acknowledgement"
    );
    let mut report = Report::pending(&dispatch);
    report.state = "released".into();
    report.finished_at = Some(H + 3);
    f.store.accept_job_report("local", &report).unwrap();
    assert_eq!(get_run(&f.store.db, id).unwrap().state, "cancelled");
    assert!(!get_job(&f.store.db, "named:Jobs", "job").unwrap().enabled);
    let state: String = f
        .store
        .db
        .query_row(
            "SELECT state FROM issues WHERE number=?1",
            [run["task_number"].as_i64().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "open");
    let occurrence = f
        .store
        .manual_job("named:Jobs", "job", "failed", H + 4)
        .unwrap();
    let run = f.store.enqueue_job(&occurrence).unwrap();
    f.store.assign_job(&run.id, "local").unwrap();
    let mut report = Report::pending(&f.store.job_dispatches("local").unwrap().remove(0));
    report.state = "failed".into();
    report.finished_at = Some(H + 5);
    report.reason = Some("Chosen model is unavailable".into());
    f.store.accept_job_report("local", &report).unwrap();
    assert_eq!(get_run(&f.store.db, &run.id).unwrap().state, "failed");
    assert_eq!(
        f.store
            .db
            .query_row(
                "SELECT state FROM issues WHERE number=?1",
                [run.task_number],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "open"
    );
    assert!(
        !f.store
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE number=?1)",
                [run.task_number],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
}

#[test]
fn job_list_includes_latest_and_active_even_after_overlap_and_deletion() {
    let mut f = Fixture::new();
    f.create("overview", 0);
    let run = f
        .op(
            JobOperation::RunNow {
                id: "overview".into(),
            },
            1,
        )
        .unwrap()["run"]
        .clone();
    let list = f
        .op(
            JobOperation::List {
                after: None,
                limit: 10,
                include_deleted: true,
            },
            2,
        )
        .unwrap();
    assert_eq!(list["jobs"][0]["last_run"]["id"], run["id"]);
    assert_eq!(list["jobs"][0]["active_run"]["id"], run["id"]);
    f.op(
        JobOperation::Stop {
            id: "overview".into(),
            run_id: run["id"].as_str().unwrap().into(),
        },
        3,
    )
    .unwrap();
    f.op(
        JobOperation::Delete {
            id: "overview".into(),
            if_revision: 1,
        },
        4,
    )
    .unwrap();
    let list = f
        .op(
            JobOperation::List {
                after: None,
                limit: 10,
                include_deleted: true,
            },
            5,
        )
        .unwrap();
    assert_eq!(list["jobs"][0]["last_run"]["state"], "cancelled");
    assert!(list["jobs"][0]["active_run"].is_null());
}

#[test]
fn job_conversations_use_saved_execution_identity_and_hide_hidden_projects() {
    let mut f = Fixture::new();
    f.create("conversation", 0);
    let run = f
        .op(
            JobOperation::RunNow {
                id: "conversation".into(),
            },
            1,
        )
        .unwrap()["run"]
        .clone();
    let id = run["id"].as_str().unwrap();
    f.store.db.execute("UPDATE scheduled_job_runs SET session_id='aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',machine='node-one',state='running',started_at=2 WHERE id=?1",[id]).unwrap();
    let reference = format!("job:{id}");
    let saved = crate::issues::provenance::saved_run(&f.store.db, &reference)
        .unwrap()
        .unwrap();
    assert_eq!(saved["machine"], "node-one");
    assert_eq!(
        saved["actor_id"],
        "codex:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    );
    assert_eq!(saved["standalone"], true);
    assert_eq!(saved["number"], run["task_number"]);
    let path = f
        .root
        .join("sessions/2026/10/09/rollout-test-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Synthetic result"}]}}).to_string()+"\n").unwrap();
    let page = crate::agent_conversations::window_page(
        &f.store.db,
        &f.root,
        &reference,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(page["messages"][0]["text"], "Synthetic result");
    let peer = Fixture::new();
    peer.store.db.execute("INSERT INTO local_job_executions(run_id,generation,dispatch,report) VALUES(?1,1,?2,?3)",params![id,json!({"run":run,"node":"node-one"}).to_string(),json!({"session_id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","state":"succeeded","started_at":2,"finished_at":3}).to_string()]).unwrap();
    let remote = crate::jobs::conversation::saved(&peer.store.db, &reference)
        .unwrap()
        .unwrap();
    assert_eq!(remote["session_id"], saved["session_id"]);
    assert_eq!(remote["machine"], "node-one");
    assert_eq!(remote["state"], "succeeded");
    f.store
        .db
        .execute("UPDATE projects SET hidden_at=3 WHERE id='named:Jobs'", [])
        .unwrap();
    assert!(
        crate::issues::provenance::saved_run(&f.store.db, &reference)
            .unwrap()
            .is_none()
    );
}
