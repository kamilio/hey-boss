use super::*;
use crate::jobs::{Definition, Operation};
use std::{fs, process::Command, time::Instant};
const PROJECT: &str = "named:Scheduled";
const MARKDOWN: &str = include_str!("../../jobs/fixtures/original.md");

fn isolated(name: &str, extra: &[(&str, &str)]) -> bool {
    if std::env::var("HEY_BOSS_JOB_TEST").as_deref() == Ok(name) {
        return false;
    }
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agent-runtime.mjs");
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("fleet::native::jobs::tests::{name}"),
            "--nocapture",
        ])
        .env("HEY_BOSS_JOB_TEST", name)
        .env("HEY_BOSS_CODEX", &fixture)
        .env("HEY_BOSS_CLAUDE", &fixture)
        .env("HEY_BOSS_PI", &fixture)
        .env("HEY_BOSS_FIXTURE_PROVIDER", "codex")
        .env("HEY_BOSS_FIXTURE_EXPECT_MODEL", "exact-model")
        .envs(extra.iter().copied())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}
struct Fixture {
    root: PathBuf,
    ctx: Context,
    store: Store,
}
impl Fixture {
    fn new() -> Self {
        let (root, ctx, mut store) = super::super::context::tests::test_context();
        let db = ctx.db().unwrap();
        db.execute(
            "INSERT INTO projects(id,name,next_number) VALUES(?1,'Scheduled',1)",
            [PROJECT],
        )
        .unwrap();
        replica::install_capture(&db, "controller", &ctx.node).unwrap();
        store
            .register_worker(
                Some("pool"),
                &crate::issues::worker::Settings {
                    directory: root.to_string_lossy().into_owned(),
                    projects: vec![PROJECT.into()],
                    enabled: true,
                    ..Default::default()
                },
                &ctx.node,
            )
            .unwrap();
        advertise_worker(&ctx, "pool").unwrap();
        Self { root, ctx, store }
    }
    fn op(&mut self, operation: Operation) -> Value {
        let request=serde_json::from_value(json!({"version":1,"project":{"id":PROJECT,"name":"Scheduled"},"actor":{"id":"human:test","kind":"human","machine":"test","host":"test","cwd":self.root,"source":"test"},"request_id":operation.writes().then(||super::super::context::id().unwrap()),"operation":{"action":"job","operation":operation}})).unwrap();
        self.store.execute(&request).unwrap()
    }
    fn run(&mut self, id: &str) -> Dispatch {
        self.op(Operation::Create {
            id: id.into(),
            definition: Definition {
                name: id.into(),
                cron: "* * * * *".into(),
                timezone: "UTC".into(),
                harness: "codex".into(),
                model: "exact-model".into(),
                machines: Vec::new(),
                all_machines: false,
            },
            markdown: MARKDOWN.into(),
            enabled: false,
        });
        let run = self.op(Operation::RunNow { id: id.into() })["run"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.store.assign_job(&run, &self.ctx.node, "pool").unwrap();
        self.store
            .job_dispatches(&self.ctx.node)
            .unwrap()
            .into_iter()
            .find(|d| d.run.id == run)
            .unwrap()
    }
    fn machines(&self) -> Vec<Value> {
        vec![
            json!({"node":self.ctx.node,"state":"connected","heartbeat":super::super::context::now(),"jobs":capability(&self.ctx).unwrap()}),
        ]
    }
    fn report(&self, d: &Dispatch) -> Report {
        serde_json::from_str(
            &self
                .ctx
                .db()
                .unwrap()
                .query_row(
                    "SELECT report FROM local_job_executions WHERE run_id=?1",
                    [&d.run.id],
                    |r| r.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.ctx
            .stop
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn wait(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn checkout_without_a_running_worker_keeps_job_pending() {
    let mut f = Fixture::new();
    let d = f.run("no-worker");
    f.store.unregister_worker("pool").unwrap();
    f.ctx
        .db()
        .unwrap()
        .execute(
            "UPDATE scheduled_job_owners SET node=NULL WHERE run_id=?1",
            [&d.run.id],
        )
        .unwrap();
    schedule(&f.ctx, &f.machines()).unwrap();
    assert!(f.store.job_owner(&d.run.id).unwrap().is_none());
    let result = f.op(Operation::Run {
        id: "no-worker".into(),
        run_id: d.run.id,
    });
    assert!(result["run"]["reason"].as_str().unwrap().contains("worker"));
}

#[test]
fn paused_stopped_and_other_project_workers_cannot_receive_jobs() {
    let mut f = Fixture::new();
    let d = f.run("waiting");
    f.ctx
        .db()
        .unwrap()
        .execute(
            "UPDATE scheduled_job_owners SET node=NULL WHERE run_id=?1",
            [&d.run.id],
        )
        .unwrap();
    for sql in [
        "UPDATE issue_workers SET config=json_set(config,'$.enabled',json('false'))",
        "UPDATE issue_workers SET config=json_set(config,'$.enabled',json('true')),stop_requested=1",
        "UPDATE issue_workers SET stop_requested=0,config=json_set(config,'$.projects',json('[\"named:Other\"]'))",
    ] {
        f.ctx.db().unwrap().execute_batch(sql).unwrap();
        schedule(&f.ctx, &f.machines()).unwrap();
        assert!(f.store.job_owner(&d.run.id).unwrap().is_none());
    }
}

#[test]
fn worker_jobs_survive_fleet_shutdown_and_use_isolated_worktrees() {
    if isolated(
        "worker_jobs_survive_fleet_shutdown_and_use_isolated_worktrees",
        &[("HEY_BOSS_FIXTURE_JOB_HOLD", "1")],
    ) {
        return;
    }
    let mut f = Fixture::new();
    let checkout = f.root.join("checkout");
    fs::create_dir(&checkout).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(&checkout)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q", "--initial-branch=legacy"]);
    fs::write(checkout.join("file"), "committed").unwrap();
    git(&["add", "file"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "-c",
        "core.hooksPath=/dev/null",
        "commit",
        "-qm",
        "fixture",
    ]);
    fs::write(checkout.join("file"), "uncommitted").unwrap();
    let config = crate::issues::worker::Settings {
        directory: checkout.to_string_lossy().into_owned(),
        projects: vec![PROJECT.into()],
        enabled: true,
        worktree_enabled: Some(true),
        ..Default::default()
    };
    f.ctx
        .db()
        .unwrap()
        .execute(
            "UPDATE issue_workers SET config=?1 WHERE id='pool'",
            [serde_json::to_string(&config).unwrap()],
        )
        .unwrap();
    f.ctx.db().unwrap().execute("INSERT INTO project_settings(project_id,prompt,prs_enabled,version,worktree_enabled) VALUES(?1,?2,0,1,1)", params![PROJECT,MARKDOWN]).unwrap();
    let d = f.run("worktree");
    receive(&f.ctx, &d, MARKDOWN).unwrap();
    let mut worker_ctx = f.ctx.clone();
    worker_ctx.stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut jobs = WorkerJobs {
        ctx: worker_ctx,
        worker: "pool".into(),
        handles: vec![],
    };
    jobs.tick().unwrap();
    wait(|| f.report(&d).state == "running");
    let workspace = PathBuf::from(f.report(&d).cwd.unwrap());
    assert_ne!(workspace, checkout);
    assert_eq!(
        fs::read_to_string(workspace.join("file")).unwrap(),
        "committed"
    );
    assert_eq!(
        fs::read_to_string(checkout.join("file")).unwrap(),
        "uncommitted"
    );
    assert!(!jobs.idle(), "worker upgrade must wait for the job");
    f.ctx.stop.store(true, std::sync::atomic::Ordering::Release);
    reconcile(&f.ctx).unwrap();
    assert_eq!(
        f.report(&d).state,
        "running",
        "fleet shutdown does not own the agent"
    );
    f.store
        .revoke_job(&d.run.id, &f.ctx.node, d.generation)
        .unwrap();
    receive(
        &f.ctx,
        &f.store.job_dispatches(&f.ctx.node).unwrap().remove(0),
        MARKDOWN,
    )
    .unwrap();
    wait(|| f.report(&d).state == "cancelled");
    wait(|| jobs.idle());
    let mut disabled = f
        .store
        .job_worker_projects("pool")
        .unwrap()
        .remove(PROJECT)
        .unwrap();
    disabled.worktree_enabled = false;
    assert_eq!(job_workspace(&disabled, "existing").unwrap(), checkout);
    f.ctx
        .db()
        .unwrap()
        .execute("UPDATE project_settings SET worktree_enabled=0", [])
        .unwrap();
    assert!(!f.store.job_worker_projects("pool").unwrap()[PROJECT].worktree_enabled);
}

#[test]
fn saturated_worker_launches_jobs_once_without_using_task_slots() {
    if isolated(
        "saturated_worker_launches_jobs_once_without_using_task_slots",
        &[],
    ) {
        return;
    }
    let mut f = Fixture::new();
    for mode in ["saturated"] {
        let d = f.run(mode);
        let db = f.ctx.db().unwrap();
        db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES(?1,1,'Task','', 'open','human:test',1,1,1,'[]')",[PROJECT]).unwrap();
        db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id) VALUES('ordinary',?1,1,'{}','human:test','running',?2,'fixture',?3,1,1,'pool')",params![PROJECT,std::process::id(),f.ctx.node]).unwrap();
        f.ctx
            .db()
            .unwrap()
            .execute(
                "UPDATE scheduled_job_owners SET node=NULL WHERE run_id=?1",
                [&d.run.id],
            )
            .unwrap();
        // The scheduler is given adversarial pool reports, never a free slot.
        let mut machines = f.machines();
        machines[0]["workers"] = match mode {
            "absent" => json!([]),
            _ => {
                json!([{"id":"pool","state":mode,"active":10,"free":0,"config":{"enabled":false,"concurrency":10}}])
            }
        };
        schedule(&f.ctx, &machines).unwrap();
        assert_eq!(f.store.job_owner(&d.run.id).unwrap().unwrap().0, f.ctx.node);
        let ctx = f.ctx.clone();
        let first = d.clone();
        let lock = ctx
            .lock(&format!("job-{}.lock", first.run.id), false)
            .unwrap()
            .unwrap();
        let thread = std::thread::spawn(move || {
            let _lock = lock;
            execute(&ctx, &first).unwrap()
        });
        // Racing delivery and ticks cannot overwrite custody or launch twice.
        for _ in 0..5 {
            receive(&f.ctx, &d, MARKDOWN).unwrap();
            for handle in tick_worker(&f.ctx, "pool").unwrap() {
                drop(handle);
            }
        }
        thread.join().unwrap();
        wait(|| f.report(&d).state == "succeeded");
        schedule(&f.ctx, &machines).unwrap();
        assert_eq!(
            f.ctx
                .db()
                .unwrap()
                .query_row("SELECT count(*) FROM worker_runs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(
            f.ctx
                .db()
                .unwrap()
                .query_row(
                    "SELECT NOT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE number=?1)",
                    [d.run.task_number],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
        );
    }
}

#[test]
fn companion_sync_preserves_snapshot_exact_model_and_terminal_ack() {
    if isolated(
        "companion_sync_preserves_snapshot_exact_model_and_terminal_ack",
        &[],
    ) {
        return;
    }
    let mut supervisor = Fixture::new();
    let mut companion = Fixture::new();
    companion.ctx.node = "companion".into();
    replica::install_capture(&companion.ctx.db().unwrap(), "agent", "companion").unwrap();
    companion
        .ctx
        .db()
        .unwrap()
        .execute(
            "UPDATE issue_workers SET machine='companion' WHERE id='pool'",
            [],
        )
        .unwrap();
    let mut dispatch = supervisor.run("remote");
    supervisor
        .store
        .revoke_job(&dispatch.run.id, "test", dispatch.generation)
        .unwrap();
    let mut ack = Report::pending(&dispatch);
    ack.state = "released".into();
    ack.finished_at = Some(now());
    supervisor.store.accept_job_report("test", &ack).unwrap();
    supervisor
        .store
        .assign_job(&dispatch.run.id, "companion", "pool")
        .unwrap();
    dispatch = supervisor
        .store
        .job_dispatches("companion")
        .unwrap()
        .remove(0);
    replica::apply_pull(
        &companion.ctx.db().unwrap(),
        "companion",
        &replica::snapshot(&supervisor.ctx.db().unwrap(), "companion").unwrap(),
        &[],
    )
    .unwrap();
    assert!(dispatch.run.task_number.is_none());
    receive(&companion.ctx, &dispatch, MARKDOWN).unwrap();
    assert!(
        receive(&companion.ctx, &dispatch, "corrupt transfer").is_ok(),
        "identical already-verified dispatch is a no-op"
    );
    let pending = Report::pending(&dispatch);
    execute(&companion.ctx, &dispatch).unwrap();
    let terminal = companion.report(&dispatch);
    assert_eq!(terminal.state, "succeeded", "{terminal:?}");
    assert!(terminal.session_id.is_some());
    acknowledge(&companion.ctx, &pending).unwrap();
    assert_eq!(
        reports(&companion.ctx).unwrap().len(),
        1,
        "stale pending acknowledgement must not hide completion"
    );
    supervisor
        .store
        .accept_job_report("companion", &terminal)
        .unwrap();
    replica::apply_pull(
        &companion.ctx.db().unwrap(),
        "companion",
        &replica::snapshot(&supervisor.ctx.db().unwrap(), "companion").unwrap(),
        &[],
    )
    .unwrap();
    assert_eq!(
        supervisor.op(Operation::Run {
            id: "remote".into(),
            run_id: dispatch.run.id.clone()
        })["run"]["state"],
        "succeeded"
    );
    assert_eq!(
        companion
            .ctx
            .db()
            .unwrap()
            .query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    acknowledge(&companion.ctx, &terminal).unwrap();
    assert!(reports(&companion.ctx).unwrap().is_empty());
    receive(&companion.ctx, &dispatch, MARKDOWN).unwrap();
    execute(&companion.ctx, &dispatch).unwrap();
    assert_eq!(
        companion.report(&dispatch),
        terminal,
        "a retried completed assignment is a tombstone, never a new session"
    );
    assert_eq!(
        companion
            .store
            .job_instructions(&dispatch.run.snapshot)
            .unwrap(),
        MARKDOWN
    );
}

#[test]
fn stop_cancels_only_its_process_and_schedule_deletion_retains_execution() {
    if isolated(
        "stop_cancels_only_its_process_and_schedule_deletion_retains_execution",
        &[("HEY_BOSS_FIXTURE_JOB_HOLD", "1")],
    ) {
        return;
    }
    let mut f = Fixture::new();
    let first = f.run("first");
    let second = f.run("second");
    for d in [&first, &second] {
        receive(&f.ctx, d, MARKDOWN).unwrap();
    }
    for handle in tick_worker(&f.ctx, "pool").unwrap() {
        drop(handle);
    }
    wait(|| f.report(&first).state == "running" && f.report(&second).state == "running");
    f.op(Operation::Delete {
        id: "second".into(),
        if_revision: 1,
    });
    f.op(Operation::Stop {
        id: "first".into(),
        run_id: first.run.id.clone(),
    });
    schedule(&f.ctx, &f.machines()).unwrap();
    wait(|| f.report(&first).state == "cancelled");
    assert_eq!(
        f.report(&second).state,
        "running",
        "deleting a schedule does not cancel its session"
    );
    f.op(Operation::Stop {
        id: "second".into(),
        run_id: second.run.id.clone(),
    });
    schedule(&f.ctx, &f.machines()).unwrap();
    wait(|| f.report(&second).state == "cancelled");
    for d in [&first, &second] {
        let pid = f
            .ctx
            .db()
            .unwrap()
            .query_row(
                "SELECT pid FROM local_job_executions WHERE run_id=?1",
                [&d.run.id],
                |r| r.get::<_, u32>(0),
            )
            .unwrap();
        assert!(group_empty(pid).unwrap());
    }
}

#[test]
fn crash_executor() {
    let Some(root) = std::env::var_os("HEY_BOSS_JOB_CRASH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let ctx = Context {
        home: root.clone(),
        state: root.join("state"),
        desired: root.join("fleet.json"),
        binary: std::env::current_exe().unwrap(),
        path: root.join("issues.db"),
        node: "test".into(),
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let dispatch: Dispatch = serde_json::from_str(
        &ctx.db()
            .unwrap()
            .query_row(
                "SELECT dispatch FROM local_job_executions LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
    )
    .unwrap();
    let _lock = ctx
        .lock(&format!("job-{}.lock", dispatch.run.id), false)
        .unwrap()
        .unwrap();
    execute(&ctx, &dispatch).unwrap();
}

#[test]
fn service_crashes_before_and_after_launch_ack_never_replay_instructions() {
    if isolated(
        "service_crashes_before_and_after_launch_ack_never_replay_instructions",
        &[],
    ) {
        return;
    }
    for phase in ["initializing", "ack_lost", "acknowledged"] {
        let mut f = Fixture::new();
        let dispatch = f.run(phase);
        receive(&f.ctx, &dispatch, MARKDOWN).unwrap();
        let audit = f.root.join("launches.jsonl");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "fleet::native::jobs::tests::crash_executor",
                "--nocapture",
            ])
            .env("HEY_BOSS_JOB_CRASH_ROOT", &f.root)
            .env("HEY_BOSS_FIXTURE_JOB_AUDIT", &audit)
            .env("HEY_BOSS_FIXTURE_JOB_HOLD", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if phase == "initializing" {
            command.env("HEY_BOSS_FIXTURE_JOB_HOLD_INIT", "1");
        }
        if phase == "ack_lost" {
            command.env("HEY_BOSS_FIXTURE_JOB_ACK_LOST", "1");
        }
        let mut child = command.spawn().unwrap();
        wait(|| {
            if phase == "initializing" {
                f.ctx
                    .db()
                    .unwrap()
                    .query_row(
                        "SELECT pid IS NOT NULL FROM local_job_executions",
                        [],
                        |r| r.get::<_, bool>(0),
                    )
                    .unwrap()
            } else {
                audit.exists()
            }
        });
        let pid = f
            .ctx
            .db()
            .unwrap()
            .query_row("SELECT pid FROM local_job_executions", [], |r| {
                r.get::<_, u32>(0)
            })
            .unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        execute(&f.ctx, &dispatch).unwrap();
        assert_eq!(f.report(&dispatch).state, "failed");
        assert!(
            group_empty(pid).unwrap(),
            "surviving runtime must be conclusively stopped"
        );
        receive(&f.ctx, &dispatch, MARKDOWN).unwrap();
        execute(&f.ctx, &dispatch).unwrap();
        assert_eq!(
            fs::read_to_string(&audit)
                .unwrap_or_default()
                .lines()
                .count(),
            usize::from(phase != "initializing")
        );
    }
}

#[test]
fn unavailable_runtime_or_model_is_visible_without_false_success() {
    if isolated(
        "unavailable_runtime_or_model_is_visible_without_false_success",
        &[("HEY_BOSS_FIXTURE_MODEL_FAILURE", "ignore")],
    ) {
        return;
    }
    let mut f = Fixture::new();
    let dispatch = f.run("model_failure");
    receive(&f.ctx, &dispatch, MARKDOWN).unwrap();
    execute(&f.ctx, &dispatch).unwrap();
    let report = f.report(&dispatch);
    assert_eq!(report.state, "failed");
    assert!(
        report.reason.as_deref().unwrap().contains("model"),
        "{report:?}"
    );
    assert!(report.started_at.is_none());
    schedule(&f.ctx, &f.machines()).unwrap();
    let view = f.op(Operation::Run {
        id: "model_failure".into(),
        run_id: dispatch.run.id.clone(),
    });
    assert_eq!(view["run"]["state"], "failed");
    assert_eq!(
        f.ctx
            .db()
            .unwrap()
            .query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn ordered_machine_targets_control_dispatch_and_fallback() {
    for (targets, failure, expected) in [
        (vec![], "", Some("other")),
        (vec!["preferred", "other"], "", Some("preferred")),
        (vec!["preferred", "other"], "offline", Some("other")),
        (vec!["preferred", "other"], "stale", Some("other")),
        (vec!["preferred", "other"], "runtime", Some("other")),
        (vec!["preferred", "other"], "worker", Some("other")),
        (vec!["preferred"], "offline", None),
        (vec!["missing"], "", None),
    ] {
        let mut f = Fixture::new();
        f.op(Operation::Create {
            id: "targeted".into(),
            definition: serde_json::from_value(json!({"name":"Targeted","cron":"* * * * *","timezone":"UTC","harness":"codex","model":"exact-model","machines":targets})).unwrap(),
            markdown: MARKDOWN.into(),
            enabled: false,
        });
        let run = f.op(Operation::RunNow {
            id: "targeted".into(),
        })["run"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut machines = vec![
            json!({"node":"other","state":"connected","heartbeat":super::super::context::now(),"jobs":{"available":true,"workers":[{"id":"other-worker","projects":[PROJECT]}],"runtimes":{"codex":{"available":true}}}}),
        ];
        let mut preferred = machines[0].clone();
        preferred["node"] = json!("preferred");
        match failure {
            "offline" => preferred["state"] = json!("disconnected"),
            "stale" => preferred["heartbeat"] = json!(0),
            "runtime" => preferred["jobs"]["runtimes"]["codex"]["available"] = json!(false),
            "worker" => preferred["jobs"]["workers"] = json!([]),
            _ => {}
        }
        machines.push(preferred);
        schedule(&f.ctx, &machines).unwrap();
        assert_eq!(
            f.store.job_owner(&run).unwrap().map(|o| o.0),
            expected.map(str::to_owned),
            "targets={targets:?}, failure={failure}"
        );
    }
}

#[test]
fn all_machines_fans_out_once_pins_offline_targets_and_stops_as_a_group() {
    let mut f = Fixture::new();
    f.op(Operation::Create {
        id: "maintenance".into(),
        definition: serde_json::from_value(json!({"name":"Maintenance","cron":"* * * * *","timezone":"UTC","harness":"codex","model":"exact-model","all_machines":true})).unwrap(),
        markdown: MARKDOWN.into(), enabled: false,
    });
    let root = f.op(Operation::RunNow {
        id: "maintenance".into(),
    })["run"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut machines = f.machines();
    let mut remote = machines[0].clone();
    remote["node"] = json!("offline");
    remote["state"] = json!("disconnected");
    machines.push(remote);
    schedule(&f.ctx, &machines).unwrap();
    schedule(&f.ctx, &machines).unwrap();
    let history = f.op(Operation::History {
        id: "maintenance".into(),
        before: None,
        limit: 100,
    });
    let runs = history["runs"].as_array().unwrap();
    assert_eq!(
        runs.len(),
        3,
        "one occurrence plus two machine executions; ticks cannot duplicate them"
    );
    let children: Vec<_> = runs.iter().filter(|r| r["parent_run_id"] == root).collect();
    assert_eq!(children.len(), 2);
    let offline = children
        .iter()
        .find(|r| r["target_machine"] == "offline")
        .unwrap();
    assert!(
        f.store
            .job_owner(offline["id"].as_str().unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        children
            .iter()
            .filter(|r| f
                .store
                .job_owner(r["id"].as_str().unwrap())
                .unwrap()
                .is_some())
            .count(),
        1
    );
    let overlap = f.op(Operation::RunNow {
        id: "maintenance".into(),
    });
    assert_eq!(overlap["run"]["reason"], "overlap");
    let mut newcomer = machines[0].clone();
    newcomer["node"] = json!("newcomer");
    machines.push(newcomer);
    machines[1]["state"] = json!("connected");
    schedule(&f.ctx, &machines).unwrap();
    assert_eq!(
        f.store
            .job_owner(offline["id"].as_str().unwrap())
            .unwrap()
            .unwrap()
            .0,
        "offline"
    );
    let history = f.op(Operation::History {
        id: "maintenance".into(),
        before: None,
        limit: 100,
    });
    assert_eq!(
        history["runs"].as_array().unwrap().len(),
        4,
        "new machines do not join an existing occurrence"
    );
    f.op(Operation::Stop {
        id: "maintenance".into(),
        run_id: root.clone(),
    });
    for child in children {
        let id = child["id"].as_str().unwrap();
        let (node, generation, revoking) = f.store.job_owner(id).unwrap().unwrap();
        assert!(revoking);
        f.store
            .accept_job_report(
                &node,
                &Report {
                    run_id: id.into(),
                    generation,
                    state: "released".into(),
                    session_id: None,
                    cwd: None,
                    started_at: None,
                    finished_at: Some(now()),
                    reason: None,
                },
            )
            .unwrap();
    }
    schedule(&f.ctx, &machines).unwrap();
    let run = f.op(Operation::Run {
        id: "maintenance".into(),
        run_id: root,
    });
    assert_eq!(run["run"]["state"], "cancelled");
}

#[test]
fn all_machines_aggregates_independent_outcomes_and_preserves_execution_snapshots() {
    for failure in [false, true] {
        let mut f = Fixture::new();
        let definition:Definition=serde_json::from_value(json!({"name":"Maintenance","cron":"* * * * *","timezone":"UTC","harness":"codex","model":"exact-model","all_machines":true})).unwrap();
        f.op(Operation::Create {
            id: "all".into(),
            definition: definition.clone(),
            markdown: MARKDOWN.into(),
            enabled: false,
        });
        let root = f.op(Operation::RunNow { id: "all".into() })["run"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        schedule(&f.ctx, &[]).unwrap();
        let mut machines = f.machines();
        let mut other = machines[0].clone();
        other["node"] = json!("other");
        machines.push(other);
        schedule(&f.ctx, &machines).unwrap();
        f.op(Operation::Edit {
            id: "all".into(),
            if_revision: 1,
            definition: Definition {
                all_machines: false,
                ..definition
            },
            markdown: None,
        });
        let children = f
            .store
            .active_job_runs()
            .unwrap()
            .into_iter()
            .filter(|r| r.parent_run_id.as_deref() == Some(&root))
            .collect::<Vec<_>>();
        assert_eq!(children.len(), 2);
        for (i, child) in children.iter().enumerate() {
            assert!(child.snapshot.definition.all_machines);
            let (node, generation, _) = f.store.job_owner(&child.id).unwrap().unwrap();
            f.store
                .accept_job_report(
                    &node,
                    &Report {
                        run_id: child.id.clone(),
                        generation,
                        state: if failure && i == 0 {
                            "failed"
                        } else {
                            "succeeded"
                        }
                        .into(),
                        session_id: Some(format!("session-{i}")),
                        cwd: None,
                        started_at: Some(now()),
                        finished_at: Some(now() + 1),
                        reason: None,
                    },
                )
                .unwrap();
            if i == 0 {
                f.store.reconcile_job_groups().unwrap();
                assert_eq!(
                    f.op(Operation::Run {
                        id: "all".into(),
                        run_id: root.clone()
                    })["run"]["state"],
                    "running"
                );
            }
        }
        f.store.reconcile_job_groups().unwrap();
        let job = f.op(Operation::View { id: "all".into() });
        assert!(job["job"]["active_run"].is_null());
        assert_eq!(job["job"]["last_run"]["id"], root);
        assert_eq!(
            job["job"]["last_run"]["state"],
            if failure { "failed" } else { "succeeded" }
        );
        assert_eq!(job["job"]["last_run"]["reason"], "2 of 2 machines finished");
    }
}

#[test]
fn stopping_all_machines_before_dispatch_cancels_every_waiting_execution() {
    let mut f = Fixture::new();
    f.op(Operation::Create{id:"waiting-all".into(),definition:serde_json::from_value(json!({"name":"Maintenance","cron":"* * * * *","timezone":"UTC","harness":"codex","model":"exact-model","all_machines":true})).unwrap(),markdown:MARKDOWN.into(),enabled:false});
    let root = f.op(Operation::RunNow {
        id: "waiting-all".into(),
    })["run"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.store
        .expand_job_machines(&root, &["down".into(), "down".into(), "also-down".into()])
        .unwrap();
    let stopped = f.op(Operation::Stop {
        id: "waiting-all".into(),
        run_id: root,
    });
    assert_eq!(stopped["run"]["state"], "cancelled");
    let history = f.op(Operation::History {
        id: "waiting-all".into(),
        before: None,
        limit: 100,
    });
    assert_eq!(history["runs"].as_array().unwrap().len(), 3);
    assert!(
        history["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["state"] == "cancelled")
    );
}

#[test]
fn all_machines_launches_one_real_session_per_machine_and_never_replays() {
    if isolated(
        "all_machines_launches_one_real_session_per_machine_and_never_replays",
        &[],
    ) {
        return;
    }
    let mut supervisor = Fixture::new();
    let mut companion = Fixture::new();
    companion.ctx.node = "other".into();
    companion
        .ctx
        .db()
        .unwrap()
        .execute(
            "UPDATE issue_workers SET machine='other' WHERE id='pool'",
            [],
        )
        .unwrap();
    supervisor.op(Operation::Create{id:"fanout-runtime".into(),definition:serde_json::from_value(json!({"name":"Maintenance","cron":"* * * * *","timezone":"UTC","harness":"codex","model":"exact-model","all_machines":true})).unwrap(),markdown:MARKDOWN.into(),enabled:false});
    let root = supervisor.op(Operation::RunNow {
        id: "fanout-runtime".into(),
    })["run"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut machines = supervisor.machines();
    machines.extend(companion.machines());
    schedule(&supervisor.ctx, &machines).unwrap();
    for ctx in [&supervisor.ctx, &companion.ctx] {
        let dispatch = supervisor
            .store
            .job_dispatches(&ctx.node)
            .unwrap()
            .remove(0);
        assert_eq!(dispatch.run.parent_run_id.as_deref(), Some(root.as_str()));
        assert_eq!(
            dispatch.run.target_machine.as_deref(),
            Some(ctx.node.as_str())
        );
        receive(ctx, &dispatch, MARKDOWN).unwrap();
        execute(ctx, &dispatch).unwrap();
        let saved = reports(ctx).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].state, "succeeded");
        assert!(saved[0].session_id.is_some());
        let before = saved[0].clone();
        execute(ctx, &dispatch).unwrap();
        assert_eq!(reports(ctx).unwrap()[0], before);
        supervisor
            .store
            .accept_job_report(&ctx.node, &before)
            .unwrap();
        acknowledge(ctx, &before).unwrap();
    }
    schedule(&supervisor.ctx, &machines).unwrap();
    assert_eq!(
        supervisor.op(Operation::Run {
            id: "fanout-runtime".into(),
            run_id: root
        })["run"]["state"],
        "succeeded"
    );
    assert!(supervisor.store.active_job_runs().unwrap().is_empty());
}
