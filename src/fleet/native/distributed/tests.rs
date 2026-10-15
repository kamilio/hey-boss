use super::*;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-distributed-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn db(&self) -> Connection {
        crate::issues::Store::open(&self.0.join("issues.db"))
            .unwrap()
            .into_database()
    }
    fn context(&self, node: &str) -> super::super::context::Context {
        super::super::context::Context {
            home: self.0.clone(),
            state: self.0.clone(),
            desired: self.0.join("desired.json"),
            binary: std::env::current_exe().unwrap(),
            path: self.0.join("issues.db"),
            node: node.into(),
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "Preflight an explicitly supplied database using a private local backup"]
fn preflight_existing_store_without_changing_the_source() {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    fn copy_resources(source: &Path, destination: &Path) {
        if !source.exists() {
            return;
        }
        std::fs::create_dir_all(destination).unwrap();
        std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o700)).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                copy_resources(&entry.path(), &target);
            } else if kind.is_file() {
                std::fs::hard_link(entry.path(), target).unwrap();
            } else {
                panic!("Unexpected resource file type");
            }
        }
    }
    let source = std::path::PathBuf::from(
        std::env::var_os("HEY_BOSS_SYNC_PREFLIGHT_DB").expect("Set HEY_BOSS_SYNC_PREFLIGHT_DB"),
    );
    let f = Fixture::new();
    std::fs::set_permissions(&f.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let original = crate::issues::Store::open_read_connection(&source).unwrap();
    let snapshot = original.read_transaction().unwrap();
    // BEGIN DEFERRED alone does not pin a WAL snapshot. Read first so ongoing
    // production writes cannot repeatedly restart the private backup.
    snapshot
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    crate::issues::archive::backup_store(&snapshot, &f.0.join("issues.db")).unwrap();
    snapshot.commit().unwrap();
    drop(original);
    // A saved shadow rehearsal can resume without repeating its full backfill.
    // Copy the append-only journal after the application snapshot so it covers
    // every already-exported checkpoint, just as independent recovery requires.
    let journal = source.with_extension("sync.db");
    if journal.exists() {
        let saved =
            Connection::open_with_flags(&journal, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let snapshot = saved.read_transaction().unwrap();
        snapshot
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
        snapshot
            .backup("main", f.0.join("issues.sync.db"), None)
            .unwrap();
        snapshot.commit().unwrap();
    }
    eprintln!("Preflight private snapshot copied");
    for extension in ["attachments", "jobs"] {
        copy_resources(
            &source.with_extension(extension),
            &f.0.join("issues.db").with_extension(extension),
        );
    }
    let db = f.db();
    let node: String = db
        .query_row("SELECT node FROM fleet_meta", [], |r| r.get(0))
        .unwrap();
    let count = |table| {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    let before = [
        count("projects"),
        count("issues"),
        count("comments"),
        count("events"),
    ];
    install(&db, &node).unwrap();
    // Refresh capture/index additions when validating a prepared shadow copy.
    db.execute_batch(SCHEMA).unwrap();
    capture::install(&db).unwrap();
    let ctx = f.context(&node);
    activation::enroll(&ctx, &node, 0).unwrap();
    let mut engine = hey_boss_sync::Engine::open(&ctx.path.with_extension("sync.db")).unwrap();
    let start = std::time::Instant::now();
    for page in 0..100_000 {
        archives::prepare(&db, &mut engine).unwrap();
        let done = baseline(&db, &mut engine, true).unwrap();
        capture_all(&db, &mut engine);
        if page % 100 == 0 {
            eprintln!(
                "Preflight page {page}, {} records, {:?}",
                engine.head().unwrap(),
                start.elapsed()
            );
        }
        if done {
            break;
        }
        assert!(page < 99_999, "Preflight did not converge");
    }
    db.execute(
        "UPDATE distributed_members SET ready=1 WHERE node=?1",
        [&node],
    )
    .unwrap();
    capture_all(&db, &mut engine);
    drop(engine);
    let probe = db.unchecked_transaction().unwrap();
    probe
        .execute("UPDATE distributed_meta SET active=1", [])
        .unwrap();
    materialize::apply(&probe).unwrap();
    let rejected = replica::rows(&probe, "SELECT table_name,reason,count(*) AS affected FROM distributed_rejections GROUP BY table_name,reason LIMIT 10", &[]).unwrap();
    if !rejected.is_empty() {
        eprintln!(
            "Preflight initial projection diagnostics: {}",
            serde_json::to_string(&rejected).unwrap()
        );
    }
    probe.rollback().unwrap();
    let cutover = std::time::Instant::now();
    eprintln!("Preflight starting activation after {:?}", start.elapsed());
    activation::activate(&ctx).unwrap();
    eprintln!("Preflight activation completed in {:?}", cutover.elapsed());
    let after = [
        count("projects"),
        count("issues"),
        count("comments"),
        count("events"),
    ];
    assert_eq!(before, after);
    eprintln!(
        "Preflight passed in {:?}; projects/tasks/comments/events: {after:?}",
        start.elapsed()
    );
}

fn seed(db: &Connection) {
    db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('p','Project',2);
        INSERT INTO agents VALUES('human:test','{}',0);
        INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels)
        VALUES('p',1,'Task','Original requirements','open','human:test',1,1,1,'[]');").unwrap();
}
fn capture_all(db: &Connection, engine: &mut hey_boss_sync::Engine) {
    while export(db, engine).unwrap() > 0 {}
    while import(db, engine).unwrap() > 0 {}
}
fn transfer(from: &hey_boss_sync::Engine, to: &mut hey_boss_sync::Engine) {
    loop {
        let batch = from.batch(to.received(from.id()).unwrap()).unwrap();
        if batch.records.is_empty() {
            break;
        }
        to.ingest(&batch).unwrap();
    }
}

#[test]
fn baseline_bounds_large_documents_without_skipping_rows() {
    let f = Fixture::new();
    let db = f.db();
    seed(&db);
    let body = "x".repeat(1_500_000);
    db.execute("UPDATE issues SET body=?1", [&body]).unwrap();
    for number in 2..=3 {
        db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('p',?1,'Large document',?2,'open','human:test',1,1,1,'[]')", rusqlite::params![number, body]).unwrap();
    }
    install(&db, "a").unwrap();
    let mut engine = hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap();
    for _ in 0..2 {
        baseline(&db, &mut engine, true).unwrap();
    }
    for number in 1..=3 {
        let before = engine.head().unwrap();
        baseline(&db, &mut engine, true).unwrap();
        assert_eq!(
            engine.head().unwrap() - before,
            1,
            "Each page should hold only one large document"
        );
        assert_eq!(
            db.query_row(
                "SELECT after_rowid FROM distributed_baseline WHERE table_name='issues'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            number
        );
        assert_eq!(
            db.query_row(
                "SELECT complete FROM distributed_baseline WHERE table_name='issues'",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap(),
            number == 3
        );
    }
}

#[test]
fn cutover_refuses_unacknowledged_edits_that_predate_shadow_capture() {
    let f = Fixture::new();
    let db = f.db();
    let node = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    db.execute("UPDATE fleet_meta SET node=?1,role='agent'", [node])
        .unwrap();
    db.execute_batch(
        "INSERT INTO fleet_outbox(table_name,after_json,created_at) VALUES('projects','{}',1);",
    )
    .unwrap();
    let ctx = f.context(node);
    activation::enroll(&ctx, node, 1).unwrap();
    let error = activation::activate(&ctx).unwrap_err().to_string();
    assert!(error.contains("Legacy edits"), "{error}");
    assert_eq!(activation::status(&ctx).unwrap()["legacy_pending"], 1);
    assert_eq!(
        db.query_row("SELECT role FROM fleet_meta", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "agent"
    );
}

#[test]
fn cutover_requires_caught_up_membership_and_keeps_offline_number_namespaces() {
    let f = Fixture::new();
    let db = f.db();
    let node = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let ctx = f.context(node);
    db.execute("UPDATE fleet_meta SET node=?1", [node]).unwrap();
    seed(&db);
    assert!(activation::activate(&ctx).is_err());
    activation::enroll(&ctx, node, 1).unwrap();
    assert!(activation::enroll(&ctx, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", 1).is_err());
    assert!(activation::enroll(&ctx, node, 2).is_err());
    assert!(activation::activate(&ctx).is_err());
    let mut engine = hey_boss_sync::Engine::open(&ctx.path.with_extension("sync.db")).unwrap();
    while !baseline(&db, &mut engine, true).unwrap() {}
    db.execute("UPDATE distributed_members SET ready=1", [])
        .unwrap();
    capture_all(&db, &mut engine);
    drop(engine);
    activation::activate(&ctx).unwrap();
    assert_eq!(
        db.query_row("SELECT next_number FROM projects WHERE id='p'", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        (1i64 << 32) + 1
    );
    db.execute(
        "INSERT INTO projects(id,name,next_number) VALUES('new','Offline project',1)",
        [],
    )
    .unwrap();
    assert_eq!(
        db.query_row("SELECT next_number FROM projects WHERE id='new'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        (1i64 << 32) + 1
    );
    // Reject exhausted/wrong namespaces even when an older worker writes SQL.
    assert!(db.execute_batch("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('p',8589934592,'Overflow','','open','human:test',1,1,1,'[]')").is_err());
    // Retrying activation must not discard pending edits or renumber tasks.
    activation::activate(&ctx).unwrap();
    assert_eq!(
        db.query_row("SELECT role FROM fleet_meta", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "peer"
    );
}

#[test]
fn job_history_keeps_overlapping_runs_without_importing_executable_ownership() {
    use crate::jobs::{Definition, Operation as Job};
    use serde_json::json;
    let fa = Fixture::new();
    let a = fa.db();
    install(&a, "a").unwrap();
    seed(&a);
    a.execute_batch(
        "UPDATE fleet_meta SET role='peer',node='a'; UPDATE distributed_meta SET active=1,slot=0;",
    )
    .unwrap();
    let fb = Fixture::new();
    let b = fb.db();
    install(&b, "b").unwrap();
    b.execute_batch("UPDATE fleet_meta SET role='peer',node='b'; UPDATE distributed_meta SET active=1,slot=1,controller='a';").unwrap();
    let request = |operation: Job| -> crate::issues::Request {
        serde_json::from_value(json!({"version":1,"project":{"id":"p","name":"Project"},"actor":{"id":"human:test","kind":"human","machine":"a","host":"test","cwd":"/tmp","source":"test"},"request_id":operation.writes().then(||hey_boss_sync::digest(serde_json::to_string(&operation).unwrap().as_bytes())),"operation":{"action":"job","operation":operation}})).unwrap()
    };
    let mut sa = crate::issues::Store::open(&fa.0.join("issues.db")).unwrap();
    let mut sb = crate::issues::Store::open(&fb.0.join("issues.db")).unwrap();
    sa.execute(&request(Job::Create {
        id: "test-job".into(),
        definition: Definition {
            name: "Schedule fixture".into(),
            cron: "0 * * * *".into(),
            timezone: "UTC".into(),
            harness: "codex".into(),
            model: "gpt-6-astra".into(),
        },
        markdown: include_str!("../../../jobs/fixtures/original.md").into(),
        enabled: true,
    }))
    .unwrap();
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&fb.0.join("sync.db")).unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    let ra = sa
        .execute(&request(Job::RunNow {
            id: "test-job".into(),
        }))
        .unwrap()["run"]
        .clone();
    let rb = sb
        .execute(&request(Job::RunNow {
            id: "test-job".into(),
        }))
        .unwrap()["run"]
        .clone();
    assert_ne!(ra["id"], rb["id"]);
    // The local observation appears immediately, before its first export.
    assert_eq!(
        sb.execute(&request(Job::History {
            id: "test-job".into(),
            before: None,
            limit: 10
        }))
        .unwrap()["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for (db, id) in [(&a, &ra["id"]), (&b, &rb["id"])] {
        db.execute(
            "UPDATE scheduled_job_runs SET state='succeeded',finished_at=created_at+1 WHERE id=?1",
            [id.as_str()],
        )
        .unwrap();
    }
    capture_all(&a, &mut ea);
    capture_all(&b, &mut eb);
    transfer(&ea, &mut eb);
    transfer(&eb, &mut ea);
    capture_all(&a, &mut ea);
    capture_all(&b, &mut eb);
    for (db, store) in [(&a, &mut sa), (&b, &mut sb)] {
        let history = store
            .execute(&request(Job::History {
                id: "test-job".into(),
                before: None,
                limit: 10,
            }))
            .unwrap();
        assert_eq!(history["runs"].as_array().unwrap().len(), 2);
        assert!(
            history["runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["state"] == "succeeded")
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM scheduled_job_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let remote = store
            .execute(&request(Job::Run {
                id: "test-job".into(),
                run_id: rb["id"].as_str().unwrap().into(),
            }))
            .unwrap();
        assert_eq!(remote["run"]["id"], rb["id"]);
    }
}

#[test]
fn disconnected_status_preserves_companion_configuration_after_cutover() {
    let f = Fixture::new();
    let db = f.db();
    install(&db, "b").unwrap();
    db.execute_batch("UPDATE fleet_meta SET node='b',role='peer'; UPDATE distributed_meta SET active=1,controller='a',slot=1;").unwrap();
    let ctx = f.context("b");
    assert_eq!(ctx.service_role().unwrap(), "agent");
    let status =
        local::request(&ctx, &serde_json::json!({"kind":"status","view":"summary"})).unwrap();
    assert_eq!(status["machines"][0]["node"], "b");
    assert_eq!(status["sync"]["mode"], "distributed");
}

#[test]
fn deleted_attachments_keep_bytes_until_disconnected_peers_replay_their_facts() {
    let fa = Fixture::new();
    let a = fa.db();
    install(&a, "a").unwrap();
    seed(&a);
    let fb = Fixture::new();
    let b = fb.db();
    install(&b, "b").unwrap();
    b.execute_batch("UPDATE distributed_meta SET active=1,controller='a'; UPDATE fleet_meta SET role='peer',node='b';").unwrap();
    let bytes = b"Offline attachment content";
    let mut files = crate::attachments::DiskChange::default();
    let upload = crate::attachments::prepare_upload(
        &fa.0.join("issues.attachments"),
        "finding.txt",
        bytes,
        &mut files,
    )
    .unwrap();
    a.execute("INSERT INTO file_attachments VALUES(?1,'p','issue','1','finding.txt',?2,?3,'human:test',1)", rusqlite::params![upload.id,bytes.len(),hey_boss_sync::digest(bytes)]).unwrap();
    a.execute("DELETE FROM file_attachments WHERE id=?1", [&upload.id])
        .unwrap();
    drop(files); // Same file removal as a committed user deletion, before export.
    assert!(!fa.0.join("issues.attachments").join(&upload.id).exists());
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&fb.0.join("sync.db")).unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    assert_eq!(
        b.query_row("SELECT count(*) FROM file_attachments", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        std::fs::read(
            fb.0.join("issues.attachments/.replication")
                .join(hey_boss_sync::digest(bytes))
        )
        .unwrap(),
        bytes
    );
    // Replay after a projection-checkpoint loss neither resurrects metadata nor
    // loses the bytes needed to interpret the earlier immutable creation.
    b.execute("UPDATE distributed_meta SET projected=0", [])
        .unwrap();
    capture_all(&b, &mut eb);
    assert_eq!(
        b.query_row("SELECT count(*) FROM file_attachments", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn removed_commit_links_stay_removed_when_history_ids_sort_out_of_order() {
    let fa = Fixture::new();
    let a = fa.db();
    install(&a, "a").unwrap();
    seed(&a);
    let fb = Fixture::new();
    let b = fb.db();
    install(&b, "b").unwrap();
    b.execute_batch("UPDATE distributed_meta SET active=1,controller='a'; UPDATE fleet_meta SET role='peer',node='b';").unwrap();
    // Canonical identities a:10 and a:2 sort differently from event chronology.
    a.execute_batch("INSERT INTO events(id,project_id,issue_number,actor,action,created_at,data)
        VALUES(2,'p',1,'human:test','commit_attached',1,'{\"sha\":\"abcdef0123456789\",\"url\":\"https://example.invalid/commit/abcdef0123456789\",\"title\":\"Old result\"}');
        INSERT INTO events(id,project_id,issue_number,actor,action,created_at,data)
        VALUES(10,'p',1,'human:test','commit_removed',2,'{\"sha\":\"abcdef0123456789\"}');").unwrap();
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&fb.0.join("sync.db")).unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    for db in [&a, &b] {
        assert_eq!(
            db.query_row("SELECT count(*) FROM issue_commits", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
    a.execute_batch("INSERT INTO events(id,project_id,issue_number,actor,action,created_at,data)
        VALUES(11,'p',1,'human:test','commit_attached',3,'{\"sha\":\"abcdef0123456789\",\"url\":\"https://example.invalid/commit/abcdef0123456789\",\"title\":\"Reattached result\"}');").unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    assert_eq!(
        b.query_row("SELECT title FROM issue_commits", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Reattached result"
    );
    b.execute("UPDATE distributed_meta SET projected=0", [])
        .unwrap();
    capture_all(&b, &mut eb);
    assert_eq!(
        b.query_row("SELECT title FROM issue_commits", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Reattached result"
    );
}

#[test]
fn remote_dependencies_release_waiting_tasks_without_stealing_local_claims() {
    use serde_json::json;
    let fa = Fixture::new();
    let a = fa.db();
    install(&a, "a").unwrap();
    let fb = Fixture::new();
    let b = fb.db();
    install(&b, "b").unwrap();
    for (db, node) in [(&a, "a"), (&b, "b")] {
        db.execute("UPDATE fleet_meta SET node=?1,role='peer'", [node])
            .unwrap();
        db.execute("UPDATE distributed_meta SET active=1", [])
            .unwrap();
    }
    seed(&a);
    a.execute_batch("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,blockers)
        VALUES('p',2,'Waiting task','','blocked','human:test',1,1,1,'[]','[1]');").unwrap();
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&fb.0.join("sync.db")).unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    a.execute_batch(
        "UPDATE issues SET state='closed',closed_by='human:test',closed_at=3 WHERE number=1;",
    )
    .unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    let task =
        || replica::current_row(&b, "issues", &json!({"project_id":"p","number":2})).unwrap();
    assert_eq!(task()["state"], "open");
    b.execute_batch("INSERT INTO agents VALUES('worker:b','{\"machine\":\"b\"}',1);
        UPDATE issues SET assignee='worker:b' WHERE number=2;
        INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,claimed_at)
        VALUES('attempt-b','p',2,'{}','worker:b','running',100,'synthetic','b',4,4,4);").unwrap();
    capture_all(&b, &mut eb);
    a.execute_batch("UPDATE issues SET state='open',closed_by=NULL,closed_at=NULL WHERE number=1;")
        .unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    assert_eq!(task()["state"], "open");
    assert_eq!(task()["assignee"], "worker:b");
    assert_eq!(
        b.query_row(
            "SELECT stop_requested FROM worker_runs WHERE id='attempt-b'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        b.query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let version = task()["version"].clone();
    capture_all(&b, &mut eb);
    assert_eq!(task()["version"], version);
}

#[test]
fn remote_completion_preserves_both_attempts_and_never_cancels_local_execution() {
    use serde_json::json;
    let fa = Fixture::new();
    let a = fa.db();
    install(&a, "a").unwrap();
    let fb = Fixture::new();
    let b = fb.db();
    install(&b, "b").unwrap();
    for (db, node) in [(&a, "a"), (&b, "b")] {
        db.execute("UPDATE fleet_meta SET node=?1,role='peer'", [node])
            .unwrap();
        db.execute("UPDATE distributed_meta SET active=1", [])
            .unwrap();
    }
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&fb.0.join("sync.db")).unwrap();
    seed(&a);
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    for (db, node) in [(&a, "a"), (&b, "b")] {
        db.execute(
            "INSERT INTO agents VALUES(?1,?2,1)",
            rusqlite::params![node, json!({"machine":node}).to_string()],
        )
        .unwrap();
        db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,claimed_at) VALUES(?1,'p',1,'{}',?1,'running',100,'synthetic',?1,2,2,2)",[node]).unwrap();
        db.execute("UPDATE issues SET assignee=?1,version=2", [node])
            .unwrap();
    }
    // A completes while B is disconnected and still owns its process.
    a.execute_batch(
        "UPDATE issues SET state='closed',assignee=NULL,closed_by='a',closed_at=3,version=3;
        UPDATE worker_runs SET state='completed',finished_at=3,summary='Result A';",
    )
    .unwrap();
    capture_all(&a, &mut ea);
    capture_all(&b, &mut eb);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    let issue = replica::current_row(&b, "issues", &json!({"project_id":"p","number":1})).unwrap();
    assert_eq!(issue["state"], "open");
    assert_eq!(issue["assignee"], "b");
    assert_eq!(
        b.query_row(
            "SELECT stop_requested FROM worker_runs WHERE id='b'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        b.query_row("SELECT count(*) FROM distributed_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    b.execute_batch(
        "UPDATE issues SET state='closed',assignee=NULL,closed_by='b',closed_at=4,version=4;
        UPDATE worker_runs SET state='completed',finished_at=4,summary='Result B';",
    )
    .unwrap();
    capture_all(&b, &mut eb);
    transfer(&eb, &mut ea);
    capture_all(&a, &mut ea);
    assert_eq!(
        a.query_row("SELECT count(*) FROM worker_runs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        a.query_row(
            "SELECT count(*) FROM distributed_attempts WHERE finished_at IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        a.query_row("SELECT count(*) FROM distributed_rejections", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        b.query_row("SELECT count(*) FROM distributed_rejections", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn stale_completion_does_not_close_new_requirements_in_either_delivery_order() {
    use serde_json::json;
    let original = json!({"project_id":"p","number":1,"title":"Task","body":"Old","labels":"[]","state":"open","assignee":null,"closed_by":null,"closed_at":null});
    let mut closed = original.clone();
    closed["state"] = json!("closed");
    closed["closed_by"] = json!("a");
    closed["closed_at"] = json!(3);
    let mut edited = original.clone();
    edited["body"] = json!("New");
    let create = Revision {
        protocol: 1,
        origin: "seed".into(),
        clock: 0,
        table: "issues".into(),
        before: None,
        after: Some(original.clone()),
        resource: None,
        archive: None,
    };
    let done = Revision {
        protocol: 1,
        origin: "a".into(),
        clock: 5,
        table: "issues".into(),
        before: Some(original.clone()),
        after: Some(closed),
        resource: None,
        archive: None,
    };
    let edit = Revision {
        protocol: 1,
        origin: "b".into(),
        clock: 2,
        table: "issues".into(),
        before: Some(original),
        after: Some(edited),
        resource: None,
        archive: None,
    };
    for order in [[&create, &done, &edit], [&edit, &done, &create]] {
        let f = Fixture::new();
        let db = f.db();
        install(&db, "test").unwrap();
        for fact in order {
            projection::reduce(&db, fact).unwrap();
        }
        let row = projection::row(&db, "issues", &json!({"project_id":"p","number":1}))
            .unwrap()
            .unwrap();
        assert_eq!(row["body"], "New");
        assert_eq!(row["state"], "open");
        assert!(row["closed_by"].is_null());
    }
}

#[test]
fn cutover_rebuilds_a_large_outline_once_across_projection_pages() {
    use serde_json::json;
    let f = Fixture::new();
    let db = f.db();
    install(&db, "a").unwrap();
    seed(&db);
    let tx = db.unchecked_transaction().unwrap();
    for n in 0..260 {
        projection::reduce(&tx,&Revision {protocol:1,origin:"a".into(),clock:n+1,table:"mindmap_nodes".into(),before:None,
            after:Some(json!({"id":format!("node-{n:03}"),"project_id":"p","alias":null,"parent_id":null,"position":n,"kind":"markdown","title":"Node","body":"","reference":null,"reference_project":null,"created_at":1,"updated_at":1})),resource:None,archive:None}).unwrap();
    }
    tx.execute_batch("UPDATE distributed_meta SET active=1; UPDATE fleet_meta SET role='peer';
        CREATE TABLE outline_writes(n INTEGER NOT NULL); INSERT INTO outline_writes VALUES(0);
        CREATE TRIGGER count_outline_inserts AFTER INSERT ON mindmap_nodes BEGIN UPDATE outline_writes SET n=n+1; END;").unwrap();
    materialize::drain(&tx).unwrap();
    assert_eq!(
        tx.query_row("SELECT count(*) FROM mindmap_nodes", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        260
    );
    assert_eq!(
        tx.query_row("SELECT n FROM outline_writes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        260
    );
    tx.commit().unwrap();
}

#[test]
fn incoming_pages_wait_for_local_revisions_at_the_end_of_the_journal() {
    let fa = Fixture::new();
    let a = fa.db();
    install(&a, "a").unwrap();
    seed(&a);
    let fb = Fixture::new();
    let b = fb.db();
    install(&b, "b").unwrap();
    b.execute_batch("UPDATE distributed_meta SET active=1,controller='a'; UPDATE fleet_meta SET role='peer',node='b';").unwrap();
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&fb.0.join("sync.db")).unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    a.execute("UPDATE issues SET title='Remote edit'", [])
        .unwrap();
    a.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<140) INSERT INTO agents SELECT 'padding:'||x,'{}',0 FROM n;").unwrap();
    capture_all(&a, &mut ea);
    b.execute("UPDATE issues SET title='Local edit'", [])
        .unwrap();
    transfer(&ea, &mut eb);
    while export(&b, &mut eb).unwrap() > 0 {}
    assert_eq!(import(&b, &eb).unwrap(), 128);
    assert_eq!(
        b.query_row("SELECT title FROM issues", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "Local edit"
    );
    while import(&b, &eb).unwrap() > 0 {}
    assert_eq!(
        b.query_row("SELECT title FROM issues", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "Local edit"
    );
}

#[test]
fn paged_cold_history_resumes_after_restart_and_keeps_deletions() {
    use serde_json::json;
    let f = Fixture::new();
    let db = f.db();
    db.execute("UPDATE fleet_meta SET node='a'", []).unwrap();
    seed(&db);
    db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<70)
        INSERT INTO comments(id,project_id,issue_number,author,body,created_at) SELECT x,'p',1,'human:test','Cold finding '||x,1 FROM n;
        UPDATE issues SET state='closed',closed_by='human:test',closed_at=2;").unwrap();
    let removed = replica::rows(&db, "SELECT * FROM comments WHERE id=7", &[])
        .unwrap()
        .remove(0);
    assert!(crate::issues::archive::archive_issue(&db, "p", 1, 10_000_000_000).unwrap());
    while crate::issues::archive::cleanup_history(&db).unwrap() > 0 {}
    install(&db, "a").unwrap();
    let mut engine = hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap();
    loop {
        archives::prepare(&db, &mut engine).unwrap();
        if baseline(&db, &mut engine, true).unwrap() {
            break;
        }
    }
    capture_all(&db, &mut engine);
    db.execute_batch("UPDATE distributed_meta SET active=1; UPDATE fleet_meta SET role='peer';")
        .unwrap();
    let mut deleted = Revision {
        protocol: 1,
        origin: "b".into(),
        clock: 20,
        table: "comments".into(),
        before: Some(removed),
        after: None,
        resource: None,
        archive: None,
    };
    identity::normalize(&db, &mut deleted).unwrap();
    let tx = db.unchecked_transaction().unwrap();
    projection::reduce(&tx, &deleted).unwrap();
    materialize::apply(&tx).unwrap();
    tx.commit().unwrap();
    assert!(
        db.query_row("SELECT archive_key IS NOT NULL FROM issues", [], |r| r
            .get::<_, bool>(0))
            .unwrap()
    );
    assert!(
        db.query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap()
            < 69
    );
    drop(db);
    drop(engine);
    let db = f.db();
    let tx = db.unchecked_transaction().unwrap();
    materialize::drain(&tx).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        69
    );
    assert!(
        db.query_row("SELECT archive_key IS NULL FROM issues", [], |r| r
            .get::<_, bool>(0))
            .unwrap()
    );
    assert!(
        projection::row(&db, "comments", &json!({"id":"a:7"}))
            .unwrap()
            .is_none()
    );
    let key: String = db
        .query_row("SELECT key FROM distributed_archive_history", [], |r| {
            r.get(0)
        })
        .unwrap();
    // Reusing a cooled copy invalidates its old hydration checkpoint.
    db.execute(
        "UPDATE issues SET archive_key=?1,body='',archive_cleanup=1",
        [&key],
    )
    .unwrap();
    assert_eq!(
        db.query_row(
            "SELECT complete FROM distributed_archive_history",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    while crate::issues::archive::cleanup_history(&db).unwrap() > 0 {}
    crate::issues::archive::restore_issue(&db, "p", 1, 10_000_000_001).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        69
    );
}

#[test]
fn cold_history_before_images_supply_birth_without_reviving_deleted_comments() {
    use serde_json::json;
    let before = json!({"id":"a:7","project_id":"p","issue_number":1,"author":"human:test","body":"Before enrollment","created_at":1});
    let mut after = before.clone();
    after["body"] = json!("Edited offline");
    let edit = Revision {
        protocol: 1,
        origin: "b".into(),
        clock: 5,
        table: "comments".into(),
        before: Some(before.clone()),
        after: Some(after.clone()),
        resource: None,
        archive: None,
    };
    let deleted = Revision {
        protocol: 1,
        origin: "c".into(),
        clock: 6,
        table: "comments".into(),
        before: Some(before.clone()),
        after: None,
        resource: None,
        archive: None,
    };
    let key = json!({"id":"a:7"});
    for order in [[&edit, &deleted], [&deleted, &edit]] {
        let f = Fixture::new();
        let db = f.db();
        install(&db, "a").unwrap();
        projection::reduce(&db, &edit).unwrap();
        assert_eq!(
            projection::row(&db, "comments", &key).unwrap(),
            Some(after.clone())
        );
        for fact in order {
            projection::reduce(&db, fact).unwrap();
        }
        projection::seed_history(&db, "comments", &before, "archive").unwrap();
        assert!(projection::row(&db, "comments", &key).unwrap().is_none());
    }
}

#[test]
fn storage_transitions_cannot_overwrite_concurrent_requirements() {
    use serde_json::json;
    let original = json!({"project_id":"p","number":1,"title":"Task","body":"Original","labels":"[]","archive_key":null,"archived_comments":0,"state":"closed","assignee":null,"closed_by":"human:test","closed_at":1});
    let mut archived = original.clone();
    archived["archive_key"] = json!("cold-copy");
    archived["archived_comments"] = json!(2);
    let mut edited = original.clone();
    edited["body"] = json!("New requirements");
    let fact = |origin: &str, clock, before, after| Revision {
        protocol: 1,
        origin: origin.into(),
        clock,
        table: "issues".into(),
        before,
        after: Some(after),
        resource: None,
        archive: None,
    };
    let seed = fact("seed", 0, None, original.clone());
    let cool = fact("a", 10, Some(original.clone()), archived.clone());
    let warm = fact("a", 11, Some(archived.clone()), original.clone());
    let edit = fact("b", 2, Some(original), edited);
    for order in [
        vec![&seed, &edit, &cool],
        vec![&cool, &seed, &edit],
        vec![&seed, &edit, &cool, &warm],
        vec![&warm, &cool, &edit, &seed],
    ] {
        let f = Fixture::new();
        let db = f.db();
        install(&db, "a").unwrap();
        for fact in order {
            projection::reduce(&db, fact).unwrap();
        }
        let row = projection::row(&db, "issues", &json!({"project_id":"p","number":1}))
            .unwrap()
            .unwrap();
        assert_eq!(row["body"], "New requirements");
        assert_eq!(row["state"], "open");
        assert!(row["archive_key"].is_null());
    }
}

#[test]
fn archived_headers_replay_without_hydration_and_accept_merged_requirement_changes() {
    use serde_json::json;
    let f = Fixture::new();
    let db = f.db();
    db.execute("UPDATE fleet_meta SET node='a'", []).unwrap();
    seed(&db);
    db.execute_batch("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('p',1,'human:test','Retained history',1);
        UPDATE issues SET state='closed',closed_by='human:test',closed_at=2;").unwrap();
    assert!(crate::issues::archive::archive_issue(&db, "p", 1, 10_000_000_000).unwrap());
    while crate::issues::archive::cleanup_history(&db).unwrap() > 0 {}
    install(&db, "a").unwrap();
    let mut engine = hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap();
    loop {
        archives::prepare(&db, &mut engine).unwrap();
        if baseline(&db, &mut engine, true).unwrap() {
            break;
        }
    }
    capture_all(&db, &mut engine);
    let key = json!({"project_id":"p","number":1});
    let before = replica::current_row(&db, "issues", &key).unwrap();
    db.execute_batch("UPDATE distributed_meta SET active=1; UPDATE fleet_meta SET role='peer';")
        .unwrap();
    materialize::drain(&db).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM distributed_dirty", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let replayed = replica::current_row(&db, "issues", &key).unwrap();
    assert_eq!(replayed["body"], before["body"]);
    assert_eq!(replayed["version"], before["version"]);
    // Ordinary mutations still have to restore an archive first.
    assert!(
        db.execute("UPDATE issues SET body='Invalid direct mutation'", [])
            .is_err()
    );
    let mut original = projection::row(&db, "issues", &key).unwrap().unwrap();
    original["archive_key"] = Value::Null;
    original["archived_comments"] = json!(0);
    let mut edited = original.clone();
    edited["title"] = json!("Updated requirements from an offline peer");
    let tx = db.unchecked_transaction().unwrap();
    projection::reduce(
        &tx,
        &Revision {
            protocol: 1,
            origin: "offline".into(),
            clock: 1,
            table: "issues".into(),
            before: Some(original),
            after: Some(edited),
            resource: None,
            archive: None,
        },
    )
    .unwrap();
    materialize::drain(&tx).unwrap();
    tx.commit().unwrap();
    let merged = replica::current_row(&db, "issues", &key).unwrap();
    assert_eq!(merged["state"], "open");
    assert_eq!(merged["archive_key"], before["archive_key"]);
    crate::issues::archive::restore_issue(&db, "p", 1, 10_000_000_001).unwrap();
    assert_eq!(
        db.query_row("SELECT body FROM comments", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "Retained history"
    );
    assert_eq!(
        replica::current_row(&db, "issues", &key).unwrap()["body"],
        "Original requirements"
    );
}

#[test]
fn archived_inputs_remain_readable_after_relay_and_hot_history_cleanup() {
    use serde_json::json;
    let source = Fixture::new();
    let a = source.db();
    a.execute("UPDATE fleet_meta SET node='a'", []).unwrap();
    seed(&a);
    a.execute_batch("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('p',1,'human:test','Archived finding',1);
        UPDATE issues SET state='closed',closed_by='human:test',closed_at=2;").unwrap();
    assert!(crate::issues::archive::archive_issue(&a, "p", 1, 10_000_000_000).unwrap());
    while crate::issues::archive::cleanup_history(&a).unwrap() > 0 {}
    install(&a, "a").unwrap();
    let target = Fixture::new();
    let b = target.db();
    install(&b, "b").unwrap();
    b.execute("UPDATE fleet_meta SET node='b',role='peer'", [])
        .unwrap();
    b.execute("UPDATE distributed_meta SET active=1,controller='a'", [])
        .unwrap();
    let mut ea = hey_boss_sync::Engine::open(&source.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&target.0.join("sync.db")).unwrap();
    for _ in 0..100 {
        archives::prepare(&a, &mut ea).unwrap();
        if baseline(&a, &mut ea, true).unwrap() {
            break;
        }
    }
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    let row = replica::current_row(&b, "issues", &json!({"project_id":"p","number":1})).unwrap();
    assert_eq!(row["state"], "closed");
    assert_eq!(
        crate::issues::archive::issue_body(&b, "p", 1, row["archive_key"].as_str().unwrap())
            .unwrap(),
        "Original requirements"
    );
    let history = crate::issues::archive::history_connection(&b, "p", 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        history
            .query_row("SELECT body FROM comments", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "Archived finding"
    );
    drop(history);
    crate::issues::archive::restore_issue(&b, "p", 1, 10_000_000_001).unwrap();
    assert_eq!(
        b.query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    b.execute("UPDATE comments SET body='Edited after enrollment'", [])
        .unwrap();
    capture_all(&b, &mut eb);
    transfer(&eb, &mut ea);
    a.execute_batch("UPDATE distributed_meta SET active=1; UPDATE fleet_meta SET role='peer';")
        .unwrap();
    capture_all(&a, &mut ea);
    for _ in 0..10 {
        import(&a, &ea).unwrap();
    }
    assert_eq!(
        a.query_row("SELECT archive_key IS NULL FROM issues", [], |r| r
            .get::<_, bool>(0))
            .unwrap(),
        true
    );
    assert_eq!(
        a.query_row("SELECT body FROM comments", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "Edited after enrollment"
    );
    assert_eq!(
        a.query_row("SELECT count(*) FROM distributed_dirty", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    // A fresh replica may reduce the archive birth and hot restore in one page.
    let third = Fixture::new();
    let c = third.db();
    install(&c, "c").unwrap();
    c.execute_batch("UPDATE fleet_meta SET node='c',role='peer'; UPDATE distributed_meta SET active=1,controller='a';").unwrap();
    let mut ec = hey_boss_sync::Engine::open(&third.0.join("sync.db")).unwrap();
    transfer(&eb, &mut ec);
    capture_all(&c, &mut ec);
    for _ in 0..10 {
        import(&c, &ec).unwrap();
    }
    assert_eq!(
        c.query_row("SELECT body FROM comments", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "Edited after enrollment"
    );
    assert_eq!(
        c.query_row(
            "SELECT archive_key IS NULL AND draft=0 FROM issues",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap(),
        true
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM distributed_dirty", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn legacy_companion_comment_keeps_identity_across_hot_sync_archive_and_restore() {
    let source = Fixture::new();
    let a = source.db();
    a.execute("UPDATE fleet_meta SET node='a'", []).unwrap();
    seed(&a);
    a.execute_batch("INSERT INTO comments(id,project_id,issue_number,author,body,created_at) VALUES(7,'p',1,'human:test','From companion',1);
        INSERT INTO fleet_row_ids(origin,table_name,origin_id,local_id) VALUES('legacy-companion','comments',17,7);").unwrap();
    install(&a, "a").unwrap();
    let target = Fixture::new();
    let b = target.db();
    install(&b, "b").unwrap();
    b.execute_batch("UPDATE fleet_meta SET node='b',role='peer'; UPDATE distributed_meta SET active=1,controller='a';").unwrap();
    let mut ea = hey_boss_sync::Engine::open(&source.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&target.0.join("sync.db")).unwrap();
    while !baseline(&a, &mut ea, true).unwrap() {}
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    let original: i64 = b
        .query_row("SELECT id FROM comments", [], |r| r.get(0))
        .unwrap();
    a.execute_batch("UPDATE issues SET state='closed',closed_by='human:test',closed_at=2;")
        .unwrap();
    assert!(crate::issues::archive::archive_issue(&a, "p", 1, 10_000_000_000).unwrap());
    while crate::issues::archive::cleanup_history(&a).unwrap() > 0 {}
    archives::prepare(&a, &mut ea).unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    crate::issues::archive::restore_issue(&b, "p", 1, 10_000_000_001).unwrap();
    assert_eq!(
        replica::rows(&b, "SELECT id,body FROM comments ORDER BY id", &[]).unwrap(),
        vec![serde_json::json!({"id":original,"body":"From companion"})]
    );
    capture_all(&b, &mut eb);
    transfer(&eb, &mut ea);
    a.execute_batch("UPDATE distributed_meta SET active=1; UPDATE fleet_meta SET role='peer';")
        .unwrap();
    capture_all(&a, &mut ea);
    assert_eq!(
        a.query_row("SELECT body FROM comments", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "From companion"
    );
    assert_eq!(
        a.query_row(
            "SELECT count(*) FROM distributed_aliases WHERE table_name='comments'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn outline_cycles_and_alias_conflicts_converge_without_discarding_nodes() {
    use serde_json::json;
    let fa = Fixture::new();
    let a = fa.db();
    install(&a, "a").unwrap();
    seed(&a);
    let fb = Fixture::new();
    let b = fb.db();
    install(&b, "b").unwrap();
    seed(&b);
    let node = |id: &str, parent: &str| Revision {
        protocol: 1,
        origin: id.into(),
        clock: 1,
        table: "mindmap_nodes".into(),
        before: None,
        after: Some(
            json!({"id":id,"project_id":"p","alias":"same","parent_id":parent,"position":0,"kind":"markdown","title":id,"body":"","reference":null,"reference_project":null,"created_at":1,"updated_at":1}),
        ),
        resource: None,
        archive: None,
    };
    let one = node("one", "two");
    let two = node("two", "one");
    for (db, facts) in [(&a, [&one, &two]), (&b, [&two, &one])] {
        for fact in facts {
            let tx = db.unchecked_transaction().unwrap();
            projection::reduce(&tx, fact).unwrap();
            materialize::apply(&tx).unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(
            db.query_row("SELECT count(*) FROM mindmap_nodes", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM mindmap_nodes WHERE parent_id IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert!(
            db.query_row(
                "SELECT count(*) FROM distributed_rejections WHERE table_name='mindmap_nodes'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap()
                > 0
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM distributed_dirty WHERE table_name='mindmap_nodes'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
    assert_eq!(
        replica::rows(&a, "SELECT * FROM mindmap_nodes ORDER BY id", &[]).unwrap(),
        replica::rows(&b, "SELECT * FROM mindmap_nodes ORDER BY id", &[]).unwrap()
    );
}

#[test]
fn dependency_projection_does_not_load_large_requirement_bodies() {
    let f = Fixture::new();
    let db = f.db();
    install(&db, "a").unwrap();
    seed(&db);
    db.execute("UPDATE issues SET body=?1", ["x".repeat(1_500_000)])
        .unwrap();
    let mut engine = hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap();
    capture_all(&db, &mut engine);
    let (dependencies, bytes) =
        crate::test_allocations::measure(|| graph::blockers(&db, "p").unwrap());
    assert_eq!(dependencies[&1], Vec::<i64>::new());
    assert!(
        bytes < 256 * 1024,
        "Dependency projection loaded document bodies: {bytes} bytes"
    );
}

#[test]
fn cutover_does_not_stop_at_a_full_page_of_pending_comment_parents() {
    let fa = Fixture::new();
    let a = fa.db();
    install(&a, "a").unwrap();
    seed(&a);
    a.execute_batch("INSERT INTO artifacts(project_id,id,title,body,created_at,updated_at) VALUES('p','doc','Document','',1,1);").unwrap();
    let fb = Fixture::new();
    let b = fb.db();
    install(&b, "b").unwrap();
    b.execute_batch("UPDATE distributed_meta SET active=1,controller='a'; UPDATE fleet_meta SET role='peer',node='b';").unwrap();
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&fb.0.join("sync.db")).unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    a.execute_batch("INSERT INTO artifact_comments(id,project_id,artifact_id,author,body,created_at) VALUES(2000,'p','doc','human:test','Parent',1);
        WITH RECURSIVE numbers(n) AS (VALUES(1000) UNION ALL SELECT n+1 FROM numbers WHERE n<1129)
        INSERT INTO artifact_comments(id,project_id,artifact_id,parent,author,body,created_at) SELECT n,'p','doc',2000,'human:test','Reply',2 FROM numbers;").unwrap();
    b.execute("UPDATE distributed_meta SET active=0", [])
        .unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    let tx = b.unchecked_transaction().unwrap();
    tx.execute("UPDATE distributed_meta SET active=1", [])
        .unwrap();
    materialize::drain(&tx).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        b.query_row("SELECT count(*) FROM artifact_comments", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        131
    );
    assert_eq!(b.query_row("SELECT count(*) FROM artifact_comments c JOIN artifact_comments p ON c.parent=p.id WHERE p.body='Parent'", [], |r| r.get::<_,i64>(0)).unwrap(), 130);
}

#[test]
fn shadow_cutover_retries_relationships_after_later_task_pages_materialize() {
    let fa = Fixture::new();
    let a = fa.db();
    install(&a, "a").unwrap();
    seed(&a);
    a.execute_batch("WITH RECURSIVE numbers(n) AS (VALUES(2) UNION ALL SELECT n+1 FROM numbers WHERE n<130)
        INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels)
        SELECT 'p',n,'Task','','open','human:test',1,1,1,'[]' FROM numbers;
        INSERT INTO issue_subtasks(project_id,parent_number,child_number,created_by,created_at) VALUES('p',99,98,'human:test',1);").unwrap();
    let fb = Fixture::new();
    let b = fb.db();
    install(&b, "b").unwrap();
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&fb.0.join("sync.db")).unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    let tx = b.unchecked_transaction().unwrap();
    tx.execute("UPDATE distributed_meta SET active=1", [])
        .unwrap();
    materialize::drain(&tx).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        b.query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        130
    );
    assert_eq!(
        b.query_row(
            "SELECT parent_number FROM issue_subtasks WHERE child_number=98",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        99
    );
}

#[test]
fn subtask_received_before_its_parent_is_retried_when_the_task_arrives() {
    use serde_json::json;
    let f = Fixture::new();
    let db = f.db();
    install(&db, "a").unwrap();
    seed(&db);
    let edge = Revision {
        protocol: 1,
        origin: "a".into(),
        clock: 1,
        table: "issue_subtasks".into(),
        before: None,
        after: Some(
            json!({"project_id":"p","parent_number":2,"child_number":1,"created_at":1,"created_by":"human:test"}),
        ),
        resource: None,
        archive: None,
    };
    projection::reduce(&db, &edge).unwrap();
    materialize::apply(&db).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM issue_subtasks", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let mut parent =
        replica::current_row(&db, "issues", &json!({"project_id":"p","number":1})).unwrap();
    parent["number"] = json!(2);
    let allowed = capture::columns(&db, "issues").unwrap();
    parent
        .as_object_mut()
        .unwrap()
        .retain(|k, _| allowed.contains(k));
    projection::reduce(
        &db,
        &Revision {
            protocol: 1,
            origin: "b".into(),
            clock: 2,
            table: "issues".into(),
            before: None,
            after: Some(parent),
            resource: None,
            archive: None,
        },
    )
    .unwrap();
    materialize::apply(&db).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM issue_subtasks", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM distributed_rejections WHERE table_name='issue_subtasks'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn concurrent_dependency_edges_converge_to_an_acyclic_graph() {
    use serde_json::json;
    let first = Fixture::new();
    let a = first.db();
    install(&a, "a").unwrap();
    seed(&a);
    let second = Fixture::new();
    let b = second.db();
    install(&b, "b").unwrap();
    seed(&b);
    for db in [&a, &b] {
        db.execute_batch("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('p',2,'Other','','open','human:test',1,1,1,'[]');").unwrap();
        db.execute("UPDATE distributed_meta SET active=1", [])
            .unwrap();
    }
    let edge = |origin: &str, parent, child| Revision {
        protocol: 1,
        origin: origin.into(),
        clock: 1,
        table: "issue_subtasks".into(),
        before: None,
        after: Some(
            json!({"project_id":"p","parent_number":parent,"child_number":child,"created_at":1,"created_by":"human:test"}),
        ),
        resource: None,
        archive: None,
    };
    let e1 = edge("a", 1, 2);
    let e2 = edge("b", 2, 1);
    for (db, facts) in [(&a, [&e1, &e2]), (&b, [&e2, &e1])] {
        for fact in facts {
            let tx = db.unchecked_transaction().unwrap();
            projection::reduce(&tx, fact).unwrap();
            materialize::apply(&tx).unwrap();
            tx.commit().unwrap();
        }
    }
    let rows = |db: &Connection| {
        replica::rows(
            db,
            "SELECT parent_number,child_number FROM issue_subtasks ORDER BY child_number",
            &[],
        )
        .unwrap()
    };
    assert_eq!(rows(&a), rows(&b));
    assert_eq!(rows(&a).len(), 1);
}

#[test]
fn field_merges_converge_without_losing_independent_edits_or_reviving_deletions() {
    use serde_json::json;
    let first = Fixture::new();
    let a = first.db();
    install(&a, "a").unwrap();
    let second = Fixture::new();
    let b = second.db();
    install(&b, "b").unwrap();
    let initial = json!({"id":"p","name":"Initial","hidden_at":null});
    let renamed = json!({"id":"p","name":"Renamed","hidden_at":null});
    let hidden = json!({"id":"p","name":"Initial","hidden_at":100});
    let fact = |origin: &str, clock, before, after| Revision {
        protocol: 1,
        origin: origin.into(),
        clock,
        table: "projects".into(),
        before,
        after,
        resource: None,
        archive: None,
    };
    let create = fact("seed", 0, None, Some(initial.clone()));
    let rename = fact("a", 1, Some(initial.clone()), Some(renamed.clone()));
    let hide = fact("b", 1, Some(initial.clone()), Some(hidden));
    for f in [&create, &rename, &hide] {
        projection::reduce(&a, f).unwrap();
    }
    for f in [&hide, &rename, &create, &rename] {
        projection::reduce(&b, f).unwrap();
    }
    let key = json!({"id":"p"});
    let expected = json!({"id":"p","name":"Renamed","hidden_at":100});
    assert_eq!(
        projection::row(&a, "projects", &key).unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        projection::row(&b, "projects", &key).unwrap(),
        Some(expected)
    );
    let delete = fact("a", 2, Some(renamed.clone()), None);
    let late_edit = fact(
        "b",
        10,
        Some(renamed),
        Some(json!({"id":"p","name":"Late","hidden_at":null})),
    );
    for f in [&delete, &late_edit] {
        projection::reduce(&a, f).unwrap();
    }
    for f in [&late_edit, &delete] {
        projection::reduce(&b, f).unwrap();
    }
    assert_eq!(projection::row(&a, "projects", &key).unwrap(), None);
    assert_eq!(projection::row(&b, "projects", &key).unwrap(), None);
}

#[test]
fn capture_commits_with_local_edits_and_retries_after_storage_restart() {
    let f = Fixture::new();
    let db = f.db();
    install(&db, "node-a").unwrap();
    db.execute(
        "INSERT INTO projects(id,name,next_number) VALUES('p','Before',1)",
        [],
    )
    .unwrap();
    {
        let tx = db.unchecked_transaction().unwrap();
        tx.execute("UPDATE projects SET activity_at=100", [])
            .unwrap();
    }
    db.execute("UPDATE projects SET activity_at=200", [])
        .unwrap();
    let mut engine = hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap();
    assert_eq!(export(&db, &mut engine).unwrap(), 2);
    assert_eq!(export(&db, &mut engine).unwrap(), 0);
    let facts = engine.pending(0).unwrap();
    assert_eq!(facts.len(), 2);
    let last: Revision = serde_json::from_str(&facts[1].2).unwrap();
    assert_eq!(last.after.unwrap()["activity_at"], 200);
    // Simulate a crash after the engine commit, before the app checkpoint.
    db.execute("UPDATE distributed_meta SET exported=0", [])
        .unwrap();
    drop(engine);
    let mut engine = hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap();
    assert_eq!(export(&db, &mut engine).unwrap(), 2);
    assert_eq!(engine.head().unwrap(), 2);
    assert!(install(&db, "different-node").is_err());
}
