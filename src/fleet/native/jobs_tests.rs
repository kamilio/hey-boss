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
        let (root, ctx, store) = super::super::context::tests::test_context();
        let db = ctx.db().unwrap();
        db.execute(
            "INSERT INTO projects(id,name,next_number) VALUES(?1,'Scheduled',1)",
            [PROJECT],
        )
        .unwrap();
        replica::install_capture(&db, "controller", &ctx.node).unwrap();
        let spec = json!({"git":"https://example.invalid/scheduled.git","path":root});
        // Use existing checkout configuration/status without a worker registration.
        let key = super::super::context::hash(&json!({"id":PROJECT,"checkout":spec}));
        ctx.atomic_json(
            &ctx.state.join("fleet-main.json"),
            &json!({"workers":[],"projects":{PROJECT:spec}}),
        )
        .unwrap();
        ctx.atomic_json(
            &ctx.state.join("project-checkouts.json"),
            &json!({PROJECT:{"key":key,"path":root}}),
        )
        .unwrap();
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
            },
            markdown: MARKDOWN.into(),
            enabled: false,
        });
        let run = self.op(Operation::RunNow { id: id.into() })["run"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.store.assign_job(&run, &self.ctx.node).unwrap();
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
fn zero_workers_saturated_and_paused_pools_launch_once_without_slots() {
    if isolated(
        "zero_workers_saturated_and_paused_pools_launch_once_without_slots",
        &[],
    ) {
        return;
    }
    let mut f = Fixture::new();
    for mode in ["absent", "saturated", "paused", "stopped"] {
        let d = f.run(mode);
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
            tick(&f.ctx).unwrap();
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
            0
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
    // Companion checkout configuration has no saved workers.
    fs::rename(
        companion.ctx.state.join("fleet-main.json"),
        companion.ctx.state.join("fleet-agent.json"),
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
        .assign_job(&dispatch.run.id, "companion")
        .unwrap();
    dispatch = supervisor
        .store
        .job_dispatches("companion")
        .unwrap()
        .remove(0);
    receive(&companion.ctx, &dispatch, MARKDOWN).unwrap();
    assert!(
        receive(&companion.ctx, &dispatch, "corrupt transfer").is_ok(),
        "identical already-verified dispatch is a no-op"
    );
    let pending = Report::pending(&dispatch);
    execute(&companion.ctx, &dispatch).unwrap();
    let terminal = companion.report(&dispatch);
    assert_eq!(terminal.state, "succeeded");
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
    tick(&f.ctx).unwrap();
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
    assert!(report.reason.unwrap().contains("model"));
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
            .query_row(
                "SELECT state FROM issues WHERE number=?1",
                [dispatch.run.task_number],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "open"
    );
}
