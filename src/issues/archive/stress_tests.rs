use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "Profiles archival on an explicitly supplied disposable database backup"]
fn profile_private_archive_backup() {
    let path = std::path::PathBuf::from(
        std::env::var_os("HB_ARCHIVE_PROFILE_DB")
            .expect("Set HB_ARCHIVE_PROFILE_DB to a private backup"),
    );
    assert!(
        path.is_file()
            && path
                .parent()
                .and_then(|p| p.file_name())
                .is_some_and(|name| name.to_string_lossy().starts_with("hb-archive-profile.")),
        "Use a disposable hb-archive-profile.* directory"
    );
    let migrate = Instant::now();
    let mut store = Store::open(&path).unwrap();
    eprintln!(
        "private backup migration_ms={}",
        migrate.elapsed().as_millis()
    );
    let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
    let db = crate::database::Connection::connect(&path).unwrap();
    store.replace_connection_for_test(crate::database::Connection::connect(&path).unwrap());
    let requests = db.query_collect("SELECT i.project_id,p.name,i.number FROM issues i JOIN projects p ON p.id=i.project_id WHERE i.state='closed' OR i.deleted_at IS NOT NULL ORDER BY i.updated_at LIMIT 32", [], |r|->rusqlite::Result<_>{Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?))}).unwrap().into_iter().map(|(project,name,number)| serde_json::from_value::<Request>(json!({"version":1,"project":{"id":project,"name":name},"operation":{"action":"view","number":number}})).unwrap()).collect::<Vec<_>>();
    let before: Vec<_> = requests.iter().map(|r| store.execute(r).unwrap()).collect();
    let footprint = |db: &crate::database::Connection| {
        let allocated: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
        let free: i64 = db
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))
            .unwrap();
        let page: i64 = db.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        json!({"allocated_bytes":allocated*page,"used_bytes":(allocated-free)*page,"reusable_bytes":free*page})
    };
    eprintln!("private archive before={}", footprint(&db));
    let started = Instant::now();
    let mut maintenance = Maintenance::default();
    let mut maximum = Duration::ZERO;
    let now = crate::issues::worker::now() + GRACE_MS + 1000;
    let mut idle = false;
    for pass in 0..100_000 {
        let start = Instant::now();
        let work = maintenance.run(&db, now).unwrap();
        maximum = maximum.max(start.elapsed());
        if work == 0 {
            idle = true;
            break;
        }
        if pass % 1000 == 0 {
            eprintln!(
                "private archive passes={pass} elapsed_s={} maximum_pass_ms={}",
                started.elapsed().as_secs(),
                maximum.as_millis()
            );
        }
    }
    assert!(
        idle,
        "Private archival did not drain within its operation budget"
    );
    for (request, expected) in requests.iter().zip(before) {
        assert!(
            store.execute(request).unwrap() == expected,
            "Archival changed a sampled issue view"
        );
    }
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    eprintln!(
        "private archive after={} elapsed_s={} maximum_pass_ms={}",
        footprint(&db),
        started.elapsed().as_secs(),
        maximum.as_millis()
    );
    for table in ["comments", "events", "worker_events", "requests"] {
        let rows: i64 = db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        eprintln!("private archive retained {table}={rows}");
    }
    owner.stop();
}

/// Invoked only by the crash test with a private fixture path.
#[test]
fn archive_crash_child() {
    let Some(root) = std::env::var_os("HB_ARCHIVE_CRASH_ROOT") else {
        return;
    };
    let path = std::path::PathBuf::from(root).join("issues.db");
    let _owner = crate::database::Owner::start(&path).unwrap().unwrap();
    let db = crate::database::Connection::connect(&path).unwrap();
    let phase = std::env::var("HB_ARCHIVE_CRASH_PHASE").unwrap();
    let now = GRACE_MS + 100;
    if phase == "copy" {
        db.execute_batch("CREATE TRIGGER fail_archive_publication BEFORE UPDATE ON issues WHEN NEW.archive_key IS NOT NULL BEGIN SELECT RAISE(ABORT,'crash fixture publication'); END;").unwrap();
        assert!(archive_issue(&db, "named:Archive", 1, now).is_err());
    } else {
        assert!(archive_issue(&db, "named:Archive", 1, now).unwrap());
        if phase == "cleanup" {
            assert_eq!(cleanup_history(&db).unwrap(), 16);
        } else if phase == "restore" {
            while cleanup_history(&db).unwrap() > 0 {}
            db.execute_batch("CREATE TRIGGER fail_archive_restore BEFORE INSERT ON comments WHEN NEW.id=40 BEGIN SELECT RAISE(ABORT,'crash fixture restoration'); END;").unwrap();
            assert!(restore_issue(&db, "named:Archive", 1, now).is_err());
            assert!(
                db.query_row(
                    "SELECT archive_restoring=1 FROM issues WHERE number=1",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
            );
        }
    }
    println!("archive-crash-ready");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn killed_database_owner_recovers_every_archive_publication_phase() {
    for phase in ["copy", "published", "cleanup", "restore"] {
        let mut f = Fixture::new();
        f.db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(3) UNION ALL SELECT x+1 FROM n WHERE x<70)
            INSERT INTO comments(id,project_id,issue_number,author,body,created_at) SELECT x,'named:Archive',1,'human:boss','Comment '||x,50 FROM n;").unwrap();
        let operations = [
            json!({"action":"view","number":1}),
            json!({"action":"comments","number":1,"limit":100,"offset":0,"sort":"oldest"}),
            json!({"action":"history","number":1,"limit":100,"offset":0}),
        ];
        let before: Vec<_> = operations.iter().map(|op| f.read(op.clone())).collect();
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "issues::archive::history_tests::stress_tests::archive_crash_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("HB_ARCHIVE_CRASH_ROOT", &f.root)
                .env("HB_ARCHIVE_CRASH_PHASE", phase)
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let output = child.0.stdout.take().unwrap();
        let (ready, waiting) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if line.unwrap().contains("archive-crash-ready") {
                    ready.send(()).unwrap();
                    return;
                }
            }
        });
        waiting.recv_timeout(Duration::from_secs(30)).unwrap();
        child.0.kill().unwrap();
        assert!(!child.0.wait().unwrap().success());
        reader.join().unwrap();
        let _owner = crate::database::Owner::start(&f.root.join("issues.db"))
            .unwrap()
            .unwrap();
        f.db = crate::database::Connection::connect(&f.root.join("issues.db")).unwrap();
        f.store.replace_connection_for_test(
            crate::database::Connection::connect(&f.root.join("issues.db")).unwrap(),
        );
        f.db.execute_batch("DROP TRIGGER IF EXISTS fail_archive_publication; DROP TRIGGER IF EXISTS fail_archive_restore;").unwrap();
        let mut maintenance = Maintenance::default();
        for _ in 0..30 {
            if maintenance.run(&f.db, GRACE_MS + 101).unwrap() == 0 {
                break;
            }
        }
        let after: Vec<_> = operations.iter().map(|op| f.read(op.clone())).collect();
        assert_eq!(before, after, "Crash during {phase}");
        assert_eq!(
            f.db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        eprintln!("Recovered killed database owner after {phase}");
    }
}

#[test]
#[ignore = "Sustained isolated archive/read/edit/backup load; HB_ARCHIVE_SOAK_SECONDS sets duration"]
fn archive_mixed_load_soak() {
    let seconds: u64 = std::env::var("HB_ARCHIVE_SOAK_SECONDS")
        .unwrap_or_else(|_| "60".into())
        .parse()
        .unwrap();
    assert!(seconds > 0);
    let started = Instant::now();
    let mut iterations = 0usize;
    let mut reads = 0usize;
    let mut backups = 0usize;
    let mut max_read = Duration::ZERO;
    let mut random = 0x198cb780feu64;
    while started.elapsed() < Duration::from_secs(seconds) {
        let mut f = Fixture::new();
        let path = f.root.join("issues.db");
        let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
        f.db = crate::database::Connection::connect(&path).unwrap();
        f.store
            .replace_connection_for_test(crate::database::Connection::connect(&path).unwrap());
        f.db.execute_batch("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at,expanded_prompt)
            VALUES('soak-run','named:Archive',1,'{\"issue\":{\"number\":1,\"title\":\"Old issue\"}}','human:boss','completed',1,'old','test',1,100,100,'Saved prompt');
            WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<64) INSERT INTO worker_events(run_id,created_at,text) SELECT 'soak-run',50,'Original event '||x FROM n;").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let reading = stop.clone();
        let reader = std::thread::spawn(move || {
            let mut store = Store::open(&path).unwrap();
            store.replace_connection_for_test(crate::database::Connection::connect(&path).unwrap());
            let request: Request = serde_json::from_value(json!({"version":1,"project":{"id":"named:Archive","name":"Archive"},"operation":{"action":"view","number":1}})).unwrap();
            let mut count = 0;
            let mut slowest = Duration::ZERO;
            while !reading.load(Ordering::Acquire) {
                let start = Instant::now();
                let response = store.execute(&request).unwrap();
                assert!(!response["issue"]["body"].as_str().unwrap().is_empty());
                assert_eq!(response["issue"]["number"], 1);
                slowest = slowest.max(start.elapsed());
                count += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            (count, slowest)
        });
        let mut maintenance = Maintenance::default();
        for cycle in 0..64 {
            if started.elapsed() >= Duration::from_secs(seconds) {
                break;
            }
            let current = f.read(json!({"action":"view","number":1}));
            if !current["issue"]["deleted_at"].is_null() {
                f.write(json!({"action":"restore","number":1}));
            }
            if current["issue"]["state"] == "closed" {
                f.write(json!({"action":"reopen","number":1}));
            }
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let operation = match random % 4 {
                0 => {
                    json!({"action":"comment","number":1,"body":format!("Soak comment {iterations}: 🦀")})
                }
                1 => {
                    json!({"action":"edit","number":1,"body":format!("Iteration {iterations}: {}", "日本語 🦀\n".repeat(32)),"add_labels":[],"remove_labels":[]})
                }
                2 => {
                    json!({"action":"resolve_comment","number":1,"comment_id":1,"resolved":cycle%2==0})
                }
                _ => {
                    json!({"action":"status","number":1,"level":"green","comment":format!("Progress {iterations}")})
                }
            };
            f.write(operation);
            f.write(json!({"action":"close","number":1,"force":true}));
            if cycle % 9 == 0 {
                f.write(json!({"action":"delete","number":1,"force":true}));
            }
            if cycle % 7 == 0 {
                f.db.execute(
                    "INSERT INTO worker_events(run_id,created_at,text) VALUES('soak-run',?1,?2)",
                    params![
                        crate::issues::worker::now(),
                        format!("Late event {iterations}")
                    ],
                )
                .unwrap();
            }
            let before = f.read(json!({"action":"view","number":1}));
            let tail = worker_event_tails(&f.db, &["soak-run"]).unwrap();
            let now = crate::issues::worker::now() + GRACE_MS + 1000;
            for _ in 0..32 {
                if maintenance.run(&f.db, now).unwrap() == 0 {
                    break;
                }
            }
            assert_eq!(f.read(json!({"action":"view","number":1})), before);
            assert_eq!(worker_event_tails(&f.db, &["soak-run"]).unwrap(), tail);
            if cycle % 16 == 0 {
                let destination = f.root.join(format!("backup-{cycle}.db"));
                backup_store(&f.db, &destination).unwrap();
                let mut backup = Store::open(&destination).unwrap();
                let request: Request = serde_json::from_value(json!({"version":1,"project":{"id":"named:Archive","name":"Archive"},"operation":{"action":"view","number":1}})).unwrap();
                assert_eq!(backup.execute(&request).unwrap(), before);
                backups += 1;
            }
            iterations += 1;
            std::thread::sleep(Duration::from_millis(150));
        }
        stop.store(true, Ordering::Release);
        let (count, slowest) = reader.join().unwrap();
        reads += count;
        max_read = max_read.max(slowest);
        assert_eq!(
            f.db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            worker_payload(&f.db, "soak-run", "named:Archive")
                .unwrap()
                .0,
            "Saved prompt"
        );
        owner.stop();
        eprintln!(
            "archive soak: elapsed={}s iterations={iterations} concurrent_reads={reads} backups={backups} max_read_ms={}",
            started.elapsed().as_secs(),
            max_read.as_millis()
        );
    }
    assert!(iterations > 0 && reads > 0 && backups > 0);
}
