use super::*;
use std::time::Instant;

fn wait(label: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(45);
    while !ready() {
        assert!(Instant::now() < deadline, "Timed out waiting for {label}");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn installed_assignment_deadline_preserves_the_winning_worker() {
    let mut f = installed_fixture();
    f.owner.stop();
    let project = "Assignment deadline QA";
    let root = f.root.path();
    fs::write(root.join("inventory.json"), r#"{"ssh_hosts":[]}"#).unwrap();
    fs::write(root.join("desired.json"), r#"{"machines":{}}"#).unwrap();
    let spawn = |args: &[&str]| {
        Service(
            f.command(args)
                .env("HEY_BOSS_FLEET_CONFIG", root.join("inventory.json"))
                .env("HEY_BOSS_FLEET_DESIRED", root.join("desired.json"))
                .env("HEY_BOSS_CODEX", worker_fixtures::binary("agent-runtime"))
                .env("HEY_BOSS_FIXTURE_PROVIDER", "codex")
                .env("HEY_BOSS_FIXTURE_JOB_HOLD", "1")
                .env("HEY_BOSS_FIXTURE_FRESH_THREADS", "1")
                .env("HEY_BOSS_FLEET_MANAGED", "1")
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let issue = |args: &[&str]| run(f.issue(project, args)).unwrap();
    let mut companion = spawn(&["fleet", "companion"]);
    wait("companion startup", || {
        root.join("fleet-agent.lock").exists()
    });
    f.init(project).unwrap();
    issue(&["create", "--title", "Expired machine assignment"]);
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    let count = |sql| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    let string = |sql| db.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap();
    let machine =
        string("SELECT json_extract(metadata,'$.machine') FROM agents WHERE id='human:qa'");
    let target = format!("machine:{machine}");
    db.execute_batch("CREATE TABLE IF NOT EXISTS distributed_attempts(id TEXT PRIMARY KEY,machine TEXT NOT NULL,project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,state TEXT NOT NULL,started_at INTEGER NOT NULL,finished_at INTEGER,summary TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS distributed_rejections(table_name TEXT NOT NULL,entity TEXT NOT NULL,reason TEXT NOT NULL,PRIMARY KEY(table_name,entity))").unwrap();
    db.execute("UPDATE fleet_meta SET role='peer',node=?1", [&machine])
        .unwrap();
    issue(&["assign", "1", &target]);
    let original = issue(&["view", "1"]);
    assert_eq!(original["issue"]["assignment"]["machine"], machine);
    assert_eq!(string("SELECT assignment_target FROM issues"), target);
    db.execute_batch("UPDATE fleet_allocation_deadlines SET expires_at=0;
        INSERT INTO distributed_attempts VALUES('remote-attempt','remote','named:Assignment deadline QA',1,'running',1,NULL,'','{}')").unwrap();
    let mut workers = ["first", "second"].map(|name| {
        spawn(&[
            "worker",
            "run",
            "--id",
            &managed_worker::register(&f.db),
            "--name",
            name,
            "--project",
            project,
            "--directory",
            root.to_str().unwrap(),
            "--concurrency",
            "1",
            "--no-prs",
            "--no-worktree",
        ])
    });
    let cycle = "SELECT presence_sequence FROM worker_allocation_clock";
    wait("both worker processes", || {
        count("SELECT count(*) FROM issue_workers WHERE owner_pid IS NOT NULL") == 2
    });
    let before = count(cycle);
    wait(
        "allocation maintenance with the remote attempt active",
        || count(cycle) > before,
    );
    assert_eq!(count("SELECT count(*) FROM worker_runs"), 0);
    assert_eq!(count("SELECT count(*) FROM fleet_allocation_deadlines"), 1);
    db.execute(
        "UPDATE distributed_attempts SET state='completed',finished_at=2 WHERE id='remote-attempt'",
        [],
    )
    .unwrap();
    wait("a single agent awaiting its claim", || {
        count(
            "SELECT count(*) FROM worker_runs WHERE state='awaiting_claim' AND session_id IS NOT NULL",
        ) == 1
    });
    assert_eq!(count("SELECT count(*) FROM worker_runs"), 1);
    assert_eq!(count("SELECT count(*) FROM fleet_allocation_deadlines"), 0);
    let queued = issue(&["view", "1"]);
    assert_eq!(queued["issue"]["assignment"]["machine"], machine);
    assert_eq!(queued["issue"]["version"], original["issue"]["version"]);
    let attempt = string("SELECT id FROM worker_runs");
    let actor = string("SELECT actor_id FROM worker_runs");
    issue(&["--agent", &actor, "claim", "1"]);
    let claimed = issue(&["view", "1"]);
    assert_eq!(claimed["issue"]["assignee"], actor);
    assert_eq!(claimed["issue"]["agent_launch_count"], 1);
    db.execute_batch("INSERT OR REPLACE INTO fleet_allocation_deadlines SELECT project_id,issue_number,0 FROM fleet_allocations;
        UPDATE worker_allocation_observations SET deadline_tick=0;
        UPDATE worker_allocation_presence SET seen_tick=0;
        UPDATE worker_allocation_clock SET renewed_tick=0").unwrap();
    let before = count(cycle);
    wait("maintenance after the claim", || count(cycle) >= before + 2);
    let after = issue(&["view", "1"]);
    assert_eq!(after["issue"]["version"], claimed["issue"]["version"]);
    assert_eq!(after["issue"]["assignee"], actor);
    assert_eq!(string("SELECT assignment_target FROM issues"), target);
    assert_eq!(count("SELECT count(*) FROM worker_runs"), 1);
    assert_eq!(
        string("SELECT id FROM worker_runs WHERE state='running' AND finished_at IS NULL"),
        attempt
    );
    assert_eq!(string("SELECT id FROM worker_allocation_winners"), attempt);
    assert!(companion.0.try_wait().unwrap().is_none());
    assert!(
        workers
            .iter_mut()
            .all(|w| w.0.try_wait().unwrap().is_none())
    );
}
