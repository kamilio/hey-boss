use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Fixture {
    root: PathBuf,
    db: PathBuf,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let root = (if mode == "offline-updates" {
            PathBuf::from("/tmp")
        } else {
            std::env::temp_dir()
        })
        .join(format!("hey-boss-workers-{}-{}", std::process::id(), mode));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("mode.txt"), mode).unwrap();
        let db = root.join("issues.db");
        Self { root, db }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        c.current_dir(&self.root)
            .env("HEY_BOSS_ISSUE_DB", &self.db)
            .env("HEY_BOSS_INBOX_SOCKET", self.root.join("absent-inbox.sock"))
            .env(
                "HEY_BOSS_CODEX",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-worker.py"),
            )
            .env("HEY_BOSS_TEST_CLI", self.notification_cli())
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args([
                "issue",
                "--project",
                "Worker fixture",
                "--agent",
                "human:worker-test",
                "--json",
            ])
            .args(args);
        c
    }
    fn cli(&self, args: &[&str]) -> Value {
        let o = self.command(args).output().unwrap();
        assert!(
            o.status.success(),
            "{} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn setup(&self, extra: &[&str]) {
        self.cli(&[
            "create",
            "--title",
            "Fixture issue",
            "--body",
            "## Requirements\nCheck {{title}} stays literal.",
        ]);
        fs::write(
            self.root.join("worker-args.json"),
            serde_json::to_vec(extra).unwrap(),
        )
        .unwrap();
    }
    fn notification_cli(&self) -> PathBuf {
        let copied = self.root.join("bin/hey-boss");
        if copied.exists() {
            copied
        } else {
            env!("CARGO_BIN_EXE_hey-boss").into()
        }
    }
    fn worker(&self) -> Worker {
        self.worker_binary(std::path::Path::new(env!("CARGO_BIN_EXE_hey-boss")))
    }
    fn worker_binary(&self, binary: &std::path::Path) -> Worker {
        let extra: Vec<String> =
            serde_json::from_slice(&fs::read(self.root.join("worker-args.json")).unwrap()).unwrap();
        Worker(
            Command::new(binary)
                .current_dir(&self.root)
                .env("HEY_BOSS_ISSUE_DB", &self.db)
                .env("HEY_BOSS_INBOX_SOCKET", self.root.join("absent-inbox.sock"))
                .env("HEY_BOSS_TEST_CLI", self.notification_cli())
                .env(
                    "HEY_BOSS_CODEX",
                    if self.root.join("codex.sh").exists() {
                        self.root.join("codex.sh")
                    } else {
                        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                            .join("tests/fixtures/codex-worker.py")
                    },
                )
                .env_remove("HEY_BOSS_ISSUE_HOST")
                .args([
                    "worker",
                    "--project",
                    "Worker fixture",
                    "--directory",
                    self.root.to_str().unwrap(),
                ])
                .args(extra)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }
    fn control(&self, command: &str, run: &str) {
        let s = self.cli(&["worker", "status"]);
        let worker = s["worker_id"].as_str().unwrap();
        let db = rusqlite::Connection::open(&self.db).unwrap();
        if command == "stop" {
            db.execute(
                "UPDATE worker_runs SET stop_requested=1 WHERE id=?1 AND worker_id=?2",
                [run, worker],
            )
            .unwrap();
        } else {
            db.execute(
                "UPDATE worker_runs SET retry_allowed=1 WHERE id=?1 AND worker_id=?2",
                [run, worker],
            )
            .unwrap();
        }
    }
    fn wait(&self, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let status = self.cli(&["worker", "status"]);
            if predicate(&status) {
                return status;
            }
            assert!(Instant::now() < deadline, "Timed out: {status}");
            thread::sleep(Duration::from_millis(50));
        }
    }
    fn transcript(&self) -> Vec<Value> {
        fs::read_to_string(self.root.join("protocol.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Worker(Child);

#[test]
fn saved_prompt_edits_steer_the_owned_turn_without_restarting_the_agent() {
    live_prompt_scenario("direct");
}

#[test]
fn saved_prompt_edits_survive_a_turn_completion_race() {
    live_prompt_scenario("race");
}

#[test]
fn saved_prompt_edits_retry_rejected_steering_without_losing_the_claim() {
    live_prompt_scenario("reject");
}

#[test]
fn saved_prompt_edits_respect_worker_overrides_and_active_branches() {
    live_prompt_scenario("override");
}

fn live_prompt_scenario(mode: &str) {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(&format!("live-prompt-{mode}"));
    f.setup(if mode == "override" {
        &["--prompt", "Worker instructions for {{number}}"]
    } else {
        &[]
    });
    fs::write(f.root.join("prompt-mode.txt"), mode).unwrap();
    let script = f.root.join("codex.sh");
    fs::write(&script, r#"#!/usr/bin/env node
const fs = require('node:fs');
const {spawnSync} = require('node:child_process');
const send = value => process.stdout.write(JSON.stringify(value)+'\n');
const mode=fs.readFileSync('prompt-mode.txt','utf8');let turns=0,steers=0;
require('node:readline').createInterface({input:process.stdin}).on('line', line => {
 const v=JSON.parse(line);fs.appendFileSync('protocol.jsonl',line+'\n');
 if(v.method==='initialize')send({id:v.id,result:{}});
 if(v.method==='thread/start')send({id:v.id,result:{thread:{id:'live-prompt-session'}}});
 if(v.method==='turn/start'){
  turns++;
  const claim=spawnSync(process.env.HEY_BOSS_TEST_CLI,['issue','--project','Worker fixture','--agent','codex:live-prompt-session','claim','1','--json']);
  if(claim.status)process.exit(1);
  send({id:v.id,result:{turn:{id:turns===1?'owned-turn':'updated-turn'}}});
  send({method:'item/started',params:{threadId:'live-prompt-session',item:{type:'agentMessage'}}});
 }
 if(v.method==='turn/steer'){
  steers++;
  if(mode==='race'){
   send({method:'item/completed',params:{threadId:'live-prompt-session',item:{type:'agentMessage',text:'{"status":"completed","summary":"Old instructions done"}'}}});
   send({method:'turn/completed',params:{threadId:'live-prompt-session',turn:{id:'owned-turn',status:'completed'}}});
   send({id:v.id,error:{message:'Turn has already completed'}});
  }else if(mode==='reject'&&steers===1)send({id:v.id,error:{message:'Temporary steering failure'}});
  else send({id:v.id,result:{turnId:v.params.expectedTurnId}});
 }
});
"#).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let mut worker = f.worker();
    // Cold CLI/mock startup can be slow alongside the rest of the integration
    // suite. Wait for the claim before measuring instruction delivery.
    let startup_deadline = Instant::now() + Duration::from_secs(60);
    let active = loop {
        let status = f.cli(&["worker", "status"]);
        if status["runs"][0]["claimed_at"].is_number() {
            break status;
        }
        assert!(
            Instant::now() < startup_deadline,
            "Agent startup timed out: {status}"
        );
        thread::sleep(Duration::from_millis(50));
    };
    let run = active["runs"][0]["id"].as_str().unwrap();
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute_batch("INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES('named:Unrelated','Unrelated',1,0,0);
        INSERT INTO project_settings(project_id,prompt,version) VALUES('named:Unrelated','Never send this to another project',1);").unwrap();
    db.execute("INSERT INTO project_settings(project_id,prompt,version,prompt_overrides,prs_enabled,worktree_enabled) SELECT id,?1,1,'{\"worktree\":\"Inactive branch change\"}',1,1 FROM projects WHERE name='Worker fixture'", [if mode == "override" { "Project prompt is overridden" } else { "Claim and implement `{{issue_command}}`." }]).unwrap();
    thread::sleep(Duration::from_millis(2300));
    assert!(
        !f.transcript().iter().any(|v| v["method"] == "turn/steer"),
        "Unrelated projects, inactive branches and overridden prompts must not steer"
    );
    if mode == "override" {
        db.execute("UPDATE project_settings SET prompt_overrides='{\"main\":\"Updated instructions for {{number}}: preserve the current task.\"}',version=2 WHERE project_id<>'named:Unrelated'", []).unwrap();
    } else {
        db.execute("UPDATE project_settings SET prompt='Updated instructions for {{number}}: preserve the current task.',version=2 WHERE project_id<>'named:Unrelated'", []).unwrap();
    }
    // A steering RPC allows 45 seconds for acknowledgement in production.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let saved: String = db
            .query_row(
                "SELECT expanded_prompt FROM worker_runs WHERE id=?1",
                [run],
                |r| r.get(0),
            )
            .unwrap();
        if saved.contains("Updated instructions for 1") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Saved prompt never reached the running agent"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let t = f.transcript();
    let steering = t.iter().find(|v| v["method"] == "turn/steer").unwrap();
    assert_eq!(steering["params"]["threadId"], "live-prompt-session");
    assert_eq!(steering["params"]["expectedTurnId"], "owned-turn");
    let text = steering["params"]["input"][0]["text"].as_str().unwrap();
    assert!(text.contains("Updated instructions for 1"), "{text}");
    assert!(text.contains("current task"));
    assert!(!text.contains("Never send this"));
    assert!(!text.contains("Inactive branch change"));
    assert!(text.contains("project's existing checkout"));
    assert!(!text.contains("PR handoff"));
    if mode == "override" {
        assert!(text.contains("Worker instructions for 1"));
        assert!(!text.contains("Project prompt is overridden"));
    }
    thread::sleep(Duration::from_millis(2200));
    let t = f.transcript();
    assert_eq!(
        t.iter().filter(|v| v["method"] == "turn/steer").count(),
        if mode == "reject" { 2 } else { 1 },
        "Unchanged prompts must not be sent repeatedly"
    );
    assert_eq!(
        t.iter().filter(|v| v["method"] == "turn/start").count(),
        if mode == "race" { 2 } else { 1 }
    );
    if mode == "race" {
        let followup = t.iter().rfind(|v| v["method"] == "turn/start").unwrap();
        assert_eq!(followup["params"]["threadId"], "live-prompt-session");
        assert!(
            followup["params"]["input"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Updated instructions for 1")
        );
        assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "open");
    }
    assert_eq!(
        t.iter().filter(|v| v["method"] == "thread/start").count(),
        1
    );
    let saved: String = db
        .query_row(
            "SELECT expanded_prompt FROM worker_runs WHERE id=?1",
            [run],
            |r| r.get(0),
        )
        .unwrap();
    assert!(saved.contains("Updated instructions for 1"));
    let config: String = db
        .query_row(
            "SELECT json_extract(job,'$.config') FROM worker_runs WHERE id=?1",
            [run],
            |r| r.get(0),
        )
        .unwrap();
    let config: Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config["prs_enabled"], false);
    assert_eq!(config["worktree_enabled"], false);
    assert_eq!(
        f.cli(&["view", "1"])["issue"]["assignee"],
        "codex:live-prompt-session"
    );
    worker.stop();
}

impl Worker {
    fn stop(&mut self) {
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "Scheduler failed to stop");
            thread::sleep(Duration::from_millis(25));
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn repeatable_checkouts_pick_only_selected_projects_and_survive_restart() {
    let f = Fixture::new("multi-checkout");
    f.setup(&[]); // This issue is outside the selected checkouts.
    let mut projects = Vec::new();
    for name in ["atlas checkout", "beacon"] {
        let path = f.root.join(name);
        fs::create_dir(&path).unwrap();
        fs::write(path.join("mode.txt"), "delay-unclaimed").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&path)
            .env("HEY_BOSS_ISSUE_DB", &f.db)
            .env("HEY_BOSS_INBOX_SOCKET", f.root.join("absent-inbox.sock"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args([
                "issue",
                "--agent",
                "human:worker-test",
                "create",
                "--title",
                name,
                "--json",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        projects.push((
            value["project"]["id"].as_str().unwrap().to_owned(),
            path.canonicalize().unwrap(),
        ));
    }
    let start = |args: &[&str]| {
        Worker(
            Command::new(env!("CARGO_BIN_EXE_hey-boss"))
                .current_dir(&f.root)
                .env("HEY_BOSS_ISSUE_DB", &f.db)
                .env("HEY_BOSS_INBOX_SOCKET", f.root.join("absent-inbox.sock"))
                .env(
                    "HEY_BOSS_CODEX",
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/codex-worker.py"),
                )
                .env_remove("HEY_BOSS_ISSUE_HOST")
                .args(["worker", "--json"])
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    };
    let mut worker = start(&[
        "-C",
        "atlas checkout",
        "--cwd",
        "beacon",
        "--concurrency",
        "2",
    ]);
    let status = f.wait(|s| {
        s["active"] == 2
            && s["runs"].as_array().is_some_and(|runs| {
                runs.iter()
                    .all(|r| r["state"] == "awaiting_claim" && r["session_id"].is_string())
            })
    });
    let id = status["worker_id"].as_str().unwrap().to_owned();
    assert_eq!(
        status["config"]["directories"].as_object().unwrap().len(),
        2
    );
    for (project, path) in &projects {
        assert_eq!(
            status["config"]["directories"][project],
            path.to_str().unwrap()
        );
        assert!(
            status["runs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["project_id"] == *project)
        );
        let protocol = fs::read_to_string(path.join("protocol.jsonl")).unwrap();
        let messages: Vec<Value> = protocol
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let thread = messages
            .iter()
            .find(|m| m["method"] == "thread/start")
            .unwrap();
        assert_eq!(thread["params"]["cwd"], path.to_str().unwrap());
    }
    worker.stop();
    let mut restarted = start(&["--id", &id]);
    let restored = f.wait(|s| {
        s["active"] == 2
            && s["runs"].as_array().is_some_and(|runs| {
                runs.iter()
                    .filter(|r| r["finished_at"].is_null())
                    .all(|r| r["state"] == "awaiting_claim" && r["session_id"].is_string())
            })
    });
    assert_eq!(restored["config"]["projects"], status["config"]["projects"]);
    assert_eq!(
        restored["config"]["directories"],
        status["config"]["directories"]
    );
    restarted.stop();
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
}
#[test]
fn completed_pr_worker_keeps_fix_open_and_hands_it_to_boss() {
    let f = Fixture::new("pr-handoff");
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.setup(&["--prs"]);
    f.cli(&[
        "pr",
        "add",
        "1",
        "https://github.com/example/repo/pull/50",
        "--purpose",
        "fix",
    ]);
    f.cli(&["comment", "1", "--body", "Preserve existing history"]);
    let mut worker = f.worker();
    let status = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(status["runs"][0]["state"], "completed");
    assert_eq!(status["eligible"], 0);
    let view = f.cli(&["view", "1"]);
    assert_eq!(view["issue"]["state"], "open");
    assert_eq!(view["issue"]["assignee"], "human:boss");
    assert!(view["issue"]["closed_at"].is_null());
    assert_eq!(view["comments"].as_array().unwrap().len(), 2);
    assert_eq!(
        f.cli(&["pr", "list", "1"])["pull_requests"][0]["purpose"],
        "fix"
    );
    let transcript = f.transcript();
    let turn = transcript
        .iter()
        .find(|v| v["method"] == "turn/start")
        .unwrap();
    let text = turn["params"]["input"][0]["text"].as_str().unwrap();
    assert!(text.contains("Keep the issue open until the actual fix PR is merged"));
    assert!(text.contains("hey-boss issue assign-to-boss 1"));
    worker.stop();
    assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "open");
}

#[test]
fn codex_protocol_goal_completion_and_prompt_variables() {
    let f = Fixture::new("completed");
    f.setup(&["--prompt", "/goal"]);
    assert_eq!(f.cli(&["view", "1"])["issue"]["agent_launch_count"], 0);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["runs"][0]["state"], "completed");
    assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "closed");
    assert_eq!(f.cli(&["view", "1"])["issue"]["agent_launch_count"], 1);
    assert_eq!(
        f.cli(&["list", "--state", "all"])["issues"][0]["agent_launch_count"],
        1
    );
    let transcript = f.transcript();
    let goals: Vec<_> = transcript
        .iter()
        .filter(|v| v["method"] == "thread/goal/set")
        .collect();
    assert_eq!(goals.len(), 2);
    assert_eq!(goals[0]["params"]["status"], "active");
    assert_eq!(goals[1]["params"]["status"], "complete");
    assert!(goals[1]["params"].get("objective").is_none());
    let turn = transcript
        .iter()
        .find(|v| v["method"] == "turn/start")
        .unwrap();
    let text = turn["params"]["input"][0]["text"].as_str().unwrap();
    assert_eq!(
        text,
        "Claim and implement `hey-boss issue view 1`.\n\nWork in the project's existing checkout.\n\nCommit your changes. If a Git remote is configured, push to main."
    );
    w.stop();
}

#[test]
fn artifact_task_claim_and_worker_completion_skip_pr_handoff() {
    let f = Fixture::new("completed-artifact-task");
    // The fixture completes successfully for the ordinary completed mode.
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.setup(&[
        "--prs",
        "--worktree",
        "--prompt",
        "/goal Implement and deploy",
    ]);
    f.cli(&["edit", "1", "--label", "task:research"]);
    let mut worker = f.worker();
    let status = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(status["runs"][0]["state"], "completed", "{status}");
    assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "closed");
    let protocol = f.transcript();
    let turn = protocol
        .iter()
        .find(|v| v["method"] == "turn/start")
        .unwrap();
    let text = turn["params"]["input"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("Claim and research"), "{text}");
    assert!(text.contains("hey-boss artifact create"));
    assert!(!text.contains("pull request"));
    assert!(!text.contains("Implement and deploy"));
    assert!(protocol.iter().any(|v| v["method"] == "thread/goal/set"));
    worker.stop();
}
#[test]
fn custom_prompt_slash_goal_preserves_all_lines() {
    let f = Fixture::new("blocked");
    f.setup(&[
        "--prompt",
        "/goal Fix {{title}}\nRetrieve {{issue_command}}. {{body}}",
    ]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["runs"][0]["state"], "blocked");
    assert_eq!(s["eligible"], 0);
    let issue = f.cli(&["view", "1"]);
    assert_eq!(issue["issue"]["state"], "open");
    assert!(issue["issue"]["assignee"].is_null());
    let t = f.transcript();
    let goal = t.iter().find(|v| v["method"] == "thread/goal/set").unwrap();
    assert_eq!(
        goal["params"]["objective"],
        "Fix Fixture issue\nRetrieve hey-boss issue view 1. ## Requirements\nCheck {{title}} stays literal.\n\nWork in the project's existing checkout.\n\nCommit your changes. If a Git remote is configured, push to main."
    );
    let turn = t.iter().find(|v| v["method"] == "turn/start").unwrap();
    assert!(
        turn["params"]["input"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Fix Fixture issue\nRetrieve hey-boss issue view")
    );
    w.stop();
}
#[test]
fn codex_disconnection_releases_capacity_and_retains_failure() {
    let f = Fixture::new("disconnect");
    f.setup(&[]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert!(
        !f.transcript()
            .iter()
            .any(|v| v["method"] == "thread/goal/set"),
        "Plain prompts must not create a native goal"
    );
    assert_eq!(s["runs"][0]["state"], "failed");
    assert_eq!(s["active"], 0);
    assert_eq!(s["eligible"], 0);
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
    w.stop();
}
#[test]
fn unfinished_unassigned_issues_are_reserved_again_after_retry_delay() {
    let f = Fixture::new("automatic-retry");
    fs::write(f.root.join("mode.txt"), "disconnect").unwrap();
    f.setup(&[]);
    let mut first = f.worker();
    f.wait(|s| s["runs"][0]["finished_at"].is_number());
    first.stop();
    for state in [
        "failed",
        "cancelled",
        "blocked",
        "interrupted",
        "claim_timeout",
    ] {
        let db = rusqlite::Connection::open(&f.db).unwrap();
        db.execute(
            "UPDATE worker_runs SET state=?1,finished_at=0,retry_allowed=0",
            [state],
        )
        .unwrap();
        assert_eq!(
            f.cli(&["worker", "status"])["eligible"],
            1,
            "{state} must not exclude an open unassigned issue permanently"
        );
        fs::write(f.root.join("mode.txt"), "delay").unwrap();
        let mut retry = f.worker();
        f.wait(|s| s["active"] == 1 && s["runs"][0]["state"] == "running");
        assert!(!f.cli(&["view", "1"])["issue"]["assignee"].is_null());
        retry.stop();
    }
}
#[test]
fn stopping_a_claimed_agent_releases_unfinished_work_for_immediate_pickup() {
    let f = Fixture::new("stop-releases-claim");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let first = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    let session = first["runs"][0]["session_id"].clone();
    worker.stop();
    let issue = f.cli(&["view", "1"])["issue"].clone();
    assert_eq!(issue["state"], "open");
    assert!(issue["assignee"].is_null());
    assert_eq!(f.cli(&["worker", "status"])["eligible"], 1);
    f.cli(&["claim", "1"]);
    let mut replacement = f.worker();
    f.wait(|s| s["worker_id"] != first["worker_id"] && s["active"] == 0);
    assert_eq!(
        f.cli(&["view", "1"])["issue"]["assignee"],
        "human:worker-test"
    );
    f.cli(&["unassign", "1"]);
    let resumed =
        f.wait(|s| s["runs"][0]["finished_at"].is_null() && s["runs"][0]["claimed_at"].is_number());
    assert_eq!(resumed["runs"][0]["session_id"], session);
    assert_eq!(
        f.cli(&["view", "1"])["issue"]["assignee"],
        format!("codex:{}", session.as_str().unwrap())
    );
    let protocol = f.transcript();
    assert_eq!(
        protocol
            .iter()
            .filter(|v| v["method"] == "thread/start")
            .count(),
        1
    );
    let resume = protocol
        .iter()
        .find(|v| v["method"] == "thread/resume")
        .unwrap();
    assert_eq!(resume["params"]["threadId"], session);
    for thread in protocol
        .iter()
        .filter(|v| matches!(v["method"].as_str(), Some("thread/start" | "thread/resume")))
    {
        assert_eq!(thread["params"]["approvalPolicy"], "on-request");
        assert_eq!(thread["params"]["approvalsReviewer"], "auto_review");
        assert_eq!(thread["params"]["sandbox"], "workspace-write");
    }
    let prompts: Vec<_> = protocol
        .iter()
        .filter(|v| v["method"] == "turn/start")
        .map(|v| v["params"]["input"][0]["text"].clone())
        .collect();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[0], prompts[1]);
    replacement.stop();
}

#[test]
fn timed_out_unassigned_work_resumes_the_saved_session_and_claims_again() {
    let f = Fixture::new("timeout-resume");
    fs::write(f.root.join("mode.txt"), "delay-unclaimed").unwrap();
    f.setup(&["--claim-timeout", "5"]);
    let mut worker = f.worker();
    let timed_out = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(timed_out["runs"][0]["state"], "claim_timeout");
    let session = timed_out["runs"][0]["session_id"].clone();
    assert_eq!(f.cli(&["view", "1"])["issue"]["agent_launch_count"], 1);
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
    worker.stop();
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute("UPDATE worker_runs SET finished_at=0", [])
        .unwrap();
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    let mut replacement = f.worker();
    let resumed = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    assert_eq!(resumed["runs"][0]["session_id"], session);
    assert_eq!(f.cli(&["view", "1"])["issue"]["agent_launch_count"], 2);
    assert_eq!(
        f.cli(&["view", "1"])["issue"]["assignee"],
        format!("codex:{}", session.as_str().unwrap())
    );
    replacement.stop();
}

#[test]
fn a_completed_session_is_not_resumed_when_the_issue_is_reopened() {
    let f = Fixture::new("completed-session-reopen");
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let completed = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(completed["runs"][0]["state"], "completed");
    let session = completed["runs"][0]["session_id"].clone();
    worker.stop();
    f.cli(&["reopen", "1"]);
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    let mut replacement = f.worker();
    let reopened =
        f.wait(|s| s["runs"][0]["finished_at"].is_null() && s["runs"][0]["claimed_at"].is_number());
    assert_ne!(reopened["runs"][0]["session_id"], session);
    assert!(
        !f.transcript()
            .iter()
            .any(|v| v["method"] == "thread/resume")
    );
    replacement.stop();
}

#[test]
fn saved_sessions_are_not_reused_on_another_machine_or_checkout() {
    for mismatch in ["machine", "checkout"] {
        let f = Fixture::new(&format!("resume-{mismatch}-mismatch"));
        fs::write(f.root.join("mode.txt"), "delay").unwrap();
        f.setup(&[]);
        let mut worker = f.worker();
        let first = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
        let session = first["runs"][0]["session_id"].clone();
        worker.stop();
        let db = rusqlite::Connection::open(&f.db).unwrap();
        if mismatch == "machine" {
            db.execute("UPDATE worker_runs SET machine='another-machine'", [])
                .unwrap();
        } else {
            db.execute(
                "UPDATE worker_runs SET job=json_set(job,'$.config.cwd','/another-checkout')",
                [],
            )
            .unwrap();
        }
        let mut replacement = f.worker();
        let current = f.wait(|s| {
            s["runs"][0]["finished_at"].is_null() && s["runs"][0]["claimed_at"].is_number()
        });
        assert_ne!(current["runs"][0]["session_id"], session, "{mismatch}");
        assert!(
            !f.transcript()
                .iter()
                .any(|v| v["method"] == "thread/resume")
        );
        replacement.stop();
    }
}

#[test]
fn large_saved_sessions_resume_without_loading_history_into_the_worker() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("large-resume");
    f.setup(&[]);
    let db = rusqlite::Connection::open(&f.db).unwrap();
    let machine: String = db
        .query_row(
            "SELECT json_extract(metadata,'$.machine') FROM agents LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let job = serde_json::json!({"config":{"cwd":f.root.canonicalize().unwrap()}});
    db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at,session_id,retry_allowed) VALUES('previous',(SELECT id FROM projects LIMIT 1),1,?1,'old-agent','cancelled',1,'old-start',?2,0,0,1,'saved-session',1)", rusqlite::params![job.to_string(), machine]).unwrap();
    let script = f.root.join("codex.sh");
    fs::write(&script, r#"#!/bin/sh
while IFS= read -r message; do
    printf '%s\n' "$message" >> protocol.jsonl
    case "$message" in
        *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
        *'"method":"thread/resume"'*)
            case "$message" in
                *'"excludeTurns":true'*) printf '%s\n' '{"id":2,"result":{"thread":{"id":"saved-session","turns":[]}}}' ;;
                *)
                    printf '%s' '{"id":2,"result":{"thread":{"id":"saved-session","turns":[{"text":"'
                    head -c 9000000 /dev/zero | tr '\000' x
                    printf '%s\n' '"}]}}}' ;;
            esac ;;
        *'"method":"turn/start"'*)
            "$HEY_BOSS_TEST_CLI" issue --project 'Worker fixture' --agent codex:saved-session claim 1 --json >/dev/null || exit 1
            printf '%s\n' '{"id":3,"result":{"turn":{"id":"turn"}}}'
            printf '%s\n' '{"method":"item/completed","params":{"threadId":"saved-session","item":{"type":"agentMessage","text":"{\"status\":\"completed\",\"summary\":\"Resumed without transferring history\"}"}}}'
            printf '%s\n' '{"method":"turn/completed","params":{"threadId":"saved-session","turn":{"id":"turn","status":"completed"}}}' ;;
    esac
done
"#).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let mut worker = f.worker();
    let finished =
        f.wait(|s| s["runs"][0]["id"] != "previous" && s["runs"][0]["finished_at"].is_number());
    assert_eq!(finished["runs"][0]["state"], "completed", "{finished}");
    assert_eq!(finished["runs"][0]["session_id"], "saved-session");
    assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "closed");
    worker.stop();
}

#[test]
fn a_resume_error_preserves_the_saved_session_for_retry() {
    let f = Fixture::new("resume-error");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let first = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    let session = first["runs"][0]["session_id"].clone();
    worker.stop();
    fs::write(f.root.join("mode.txt"), "resume-unavailable").unwrap();
    let mut replacement = f.worker();
    let failed =
        f.wait(|s| s["runs"][0]["state"] == "failed" && s["runs"][0]["finished_at"].is_number());
    assert_eq!(failed["runs"][0]["session_id"], session);
    assert!(
        failed["runs"][0]["summary"]
            .as_str()
            .unwrap()
            .contains("locked by another writer")
    );
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
    assert_eq!(
        f.transcript()
            .iter()
            .filter(|v| v["method"] == "thread/start")
            .count(),
        1
    );
    replacement.stop();
}

#[test]
fn stopping_a_killed_worker_recovers_its_orphaned_agent_and_claim() {
    let f = Fixture::new("killed-worker-release");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let running = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    let id = running["worker_id"].as_str().unwrap();
    worker.0.kill().unwrap();
    worker.0.wait().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .current_dir(&f.root)
        .env("HEY_BOSS_ISSUE_DB", &f.db)
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .args(["worker", "--json", "stop", id])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
    let status = f.cli(&["worker", "status"]);
    assert_eq!(status["active"], 0);
    assert_eq!(status["eligible"], 1);
}

#[test]
fn web_monitor_recovers_a_killed_worker_without_another_running_worker() {
    let f = Fixture::new("web-orphan-recovery");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    worker.0.kill().unwrap();
    worker.0.wait().unwrap();
    let _web = Worker(
        f.command(&["web", "--port", "0"])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let recovered = f.wait(|s| s["active"] == 0);
    assert_eq!(recovered["runs"][0]["state"], "interrupted");
    assert_eq!(recovered["eligible"], 1);
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
}

#[test]
fn worker_queue_distinguishes_open_issues_from_pickup_eligibility() {
    let f = Fixture::new("queue-counts");
    f.setup(&[]);
    f.cli(&["create", "--title", "Already claimed"]);
    f.cli(&["claim", "2"]);
    f.cli(&["create", "--title", "Boss work"]);
    f.cli(&["assign-to-boss", "3"]);
    let status = f.cli(&["worker", "status"]);
    assert_eq!(
        status["queue"],
        serde_json::json!({
            "open": 3, "assigned": 2, "tag_filtered": 0, "waiting": 0, "eligible": 1
        })
    );
    assert_eq!(status["eligible"], 1);

    f.cli(&["claim", "1"]);
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .current_dir(&f.root)
        .env("HEY_BOSS_ISSUE_DB", &f.db)
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .args([
            "issue",
            "--project",
            "Worker fixture",
            "--agent",
            "human:worker-test",
            "worker",
            "status",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("3 open · 3 assigned · 0 eligible"), "{text}");
    assert!(!text.contains("Issues on another machine"), "{text}");
    assert!(
        !text.contains("worker --host HOST --directory PATH"),
        "{text}"
    );
}

#[test]
fn worker_queue_accounts_for_tags_and_waiting_subtasks() {
    let f = Fixture::new("queue-filters");
    f.setup(&[]);
    f.cli(&["edit", "1", "--label", "ready"]);
    f.cli(&[
        "subtask", "create", "1", "--title", "Child", "--label", "ready",
    ]);
    f.cli(&["create", "--title", "Missing tag"]);
    f.cli(&["create", "--title", "Assigned without tag"]);
    f.cli(&["claim", "4"]);
    let project = f.cli(&["projects"])["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let config = hey_boss::issues::worker::Settings {
        projects: vec![project],
        tags: vec!["ready".into()],
        ..Default::default()
    };
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute(
        "INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES('queue-worker','managed',?1,1,0)",
        [serde_json::to_string(&config).unwrap()],
    ).unwrap();
    let status = f.cli(&["worker", "status"]);
    assert_eq!(
        status["queue"],
        serde_json::json!({
            "open": 4, "assigned": 1, "tag_filtered": 1, "waiting": 1, "eligible": 1
        })
    );
    db.execute(
        "UPDATE issue_workers SET config=json_set(config,'$.projects',json('[\"named:Other\"]'))",
        [],
    )
    .unwrap();
    assert_eq!(
        f.cli(&["worker", "status"])["queue"],
        serde_json::json!({
            "open": 0, "assigned": 0, "tag_filtered": 0, "waiting": 0, "eligible": 0
        })
    );
}

#[test]
fn approval_hold_is_not_retried_automatically_even_after_delay() {
    let f = Fixture::new("approval-hold");
    fs::write(f.root.join("mode.txt"), "approval").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let first = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(first["runs"][0]["state"], "blocked", "{first}");
    let id = first["runs"][0]["id"].as_str().unwrap().to_owned();
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute("UPDATE worker_runs SET finished_at=0", [])
        .unwrap();
    let status = f.cli(&["worker", "status"]);
    assert_eq!(status["eligible"], 0, "{status}");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.control("retry", &id);
    f.wait(|s| s["active"] == 1 && s["runs"][0]["state"] == "running");
    worker.stop();
}
#[test]
fn private_issue_database_does_not_modify_default_fleet_configuration() {
    let f = Fixture::new("private-fleet-scope");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let home = f.root.join("home");
    let config = home.join(".local/share/hey-boss/fleet-main.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = br#"{"role":"controller","revision":"sentinel","workers":[]}"#;
    fs::write(&config, original).unwrap();
    let mut worker = Worker(
        Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&f.root)
            .env("HOME", &home)
            .env("HEY_BOSS_ISSUE_DB", &f.db)
            .env(
                "HEY_BOSS_CODEX",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-worker.py"),
            )
            .env("HEY_BOSS_TEST_CLI", env!("CARGO_BIN_EXE_hey-boss"))
            .env_remove("HEY_BOSS_FLEET_STATE")
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args([
                "worker",
                "--project",
                "Worker fixture",
                "--directory",
                f.root.to_str().unwrap(),
                "--json",
            ])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    worker.stop();
    assert_eq!(fs::read(config).unwrap(), original);
}

#[test]
fn legacy_upgrade_state_migrates_without_stopping_active_sessions() {
    let f = Fixture::new("legacy-upgrade");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let before = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    let id = before["worker_id"].as_str().unwrap();
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute("UPDATE issue_workers SET config=json_set(config,'$.enabled',json('false'),'$.upgrading',json('true')) WHERE id=?1", [id]).unwrap();
    let after = f.cli(&["worker", "status"]);
    assert_eq!(after["active"], 1);
    assert_eq!(after["config"]["enabled"], true);
    assert_eq!(after["upgrading"], true);
    assert_eq!(
        after["runs"][0]["session_id"],
        before["runs"][0]["session_id"]
    );
    assert!(
        db.query_row(
            "SELECT json_type(config,'$.upgrading') IS NULL FROM issue_workers WHERE id=?1",
            [id],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    worker.stop();
}

#[test]
fn offline_replica_launches_only_work_allocated_to_its_machine() {
    let f = Fixture::new("offline-replica-pickup");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let actor = f.cli(&["whoami"])["agent"].clone();
    let node = actor["machine"].as_str().unwrap();
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute("UPDATE fleet_meta SET role='agent',node=?1", [node])
        .unwrap();
    assert_eq!(f.cli(&["worker", "status"])["eligible"], 0);
    assert_eq!(
        f.command(&["claim", "1"]).output().unwrap().status.code(),
        Some(4)
    );
    let project = f.cli(&["projects"])["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    db.execute(
        "INSERT INTO fleet_allocations VALUES(?1,1,'another-machine')",
        [&project],
    )
    .unwrap();
    assert_eq!(f.cli(&["worker", "status"])["eligible"], 0);
    db.execute("UPDATE fleet_allocations SET node=?1", [node])
        .unwrap();
    assert_eq!(f.cli(&["worker", "status"])["eligible"], 1);
    let mut worker = f.worker();
    f.wait(|s| s["active"] == 1 && s["runs"][0]["state"] == "running");
    worker.stop();
}
#[test]
fn emergency_cli_update_drains_active_sessions_then_restores_same_worker() {
    cli_update_idle_handoff(true);
}
#[test]
fn routine_cli_update_reloads_when_idle_with_the_same_worker_settings() {
    cli_update_idle_handoff(false);
}
fn cli_update_idle_handoff(emergency: bool) {
    let f = Fixture::new(if emergency {
        "emergency-upgrade-handoff"
    } else {
        "routine-upgrade-handoff"
    });
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&["--concurrency", "2"]);
    let directory = f.root.join("bin");
    fs::create_dir_all(&directory).unwrap();
    let binary = directory.join("hey-boss");
    fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &binary).unwrap();
    let mut worker = f.worker_binary(&binary);
    let initial = f.wait(|s| s["active"] == 1 && s["runs"][0]["state"] == "running");
    let id = initial["worker_id"].as_str().unwrap().to_owned();
    let run = initial["runs"][0]["id"].as_str().unwrap().to_owned();
    if emergency {
        fs::write(f.db.with_added_extension("drain-for-update"), "").unwrap();
        thread::sleep(Duration::from_secs(1));
        assert_eq!(f.cli(&["worker", "status"])["upgrading"], false);
    }
    let replacement = directory.join("hey-boss.new");
    fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &replacement).unwrap();
    fs::rename(replacement, binary).unwrap();
    let still_running = if emergency {
        f.wait(|status| status["upgrading"] == true)
    } else {
        thread::sleep(Duration::from_secs(2));
        f.cli(&["worker", "status"])
    };
    assert_eq!(still_running["active"], 1);
    assert_eq!(still_running["runs"][0]["id"], run);
    assert_eq!(still_running["version"], initial["version"]);
    assert_eq!(still_running["upgrading"], emergency);
    assert_eq!(still_running["workers"][0]["upgrading"], emergency);
    if emergency {
        f.cli(&["create", "--title", "Queued during emergency drain"]);
        thread::sleep(Duration::from_secs(1));
        let waiting = f.cli(&["worker", "status"]);
        assert_eq!(waiting["active"], 1);
        assert_eq!(waiting["runs"].as_array().unwrap().len(), 1);
    }
    // Unfinished work is eligible immediately after stop. Configure the next
    // attempt before reload can pick it up, rather than racing its startup.
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.control("stop", &run);
    let restored = f.wait(|s| {
        s["version"].as_i64().unwrap() > initial["version"].as_i64().unwrap()
            && s["workers"][0]["pid"].is_number()
    });
    assert_eq!(restored["worker_id"], id);
    assert_eq!(restored["upgrading"], false);
    assert_eq!(restored["config"], initial["config"]);
    f.wait(|s| s["runs"][0]["state"] == "completed");
    worker.stop();
}
#[test]
fn routine_cli_update_keeps_picking_up_work_and_emergency_drain_is_reversible() {
    let f = Fixture::new("upgrade-continued-pickup");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&["--concurrency", "2"]);
    let directory = f.root.join("bin");
    fs::create_dir_all(&directory).unwrap();
    let binary = directory.join("hey-boss");
    fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &binary).unwrap();
    let mut worker = f.worker_binary(&binary);
    let initial = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    let replacement = directory.join("hey-boss.new");
    fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &replacement).unwrap();
    fs::rename(replacement, binary).unwrap();
    // Give the scheduler time to notice the replacement before adding work.
    thread::sleep(Duration::from_secs(2));
    assert_eq!(f.cli(&["worker", "status"])["upgrading"], false);
    f.cli(&["create", "--title", "Work after routine update"]);
    let busy = f.wait(|s| {
        s["active"] == 2
            && s["runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["claimed_at"].is_number())
    });
    assert_eq!(busy["version"], initial["version"]);
    assert_eq!(busy["workers"][0]["pid"], initial["workers"][0]["pid"]);
    assert!(
        busy["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == initial["runs"][0]["id"]
                && r["session_id"] == initial["runs"][0]["session_id"])
    );
    let marker = f.db.with_added_extension("drain-for-update");
    fs::write(&marker, "").unwrap();
    let draining = f.wait(|s| s["upgrading"] == true);
    assert_eq!(draining["config"]["enabled"], true);
    assert_eq!(draining["version"], initial["version"]);
    fs::remove_file(marker).unwrap();
    let resumed = f.wait(|s| s["upgrading"] == false);
    assert_eq!(resumed["active"], 2);
    assert_eq!(resumed["version"], initial["version"]);
    worker.stop();
}
#[test]
fn terminal_history_is_separate_bounded_and_can_be_hidden() {
    let f = Fixture::new("display-history");
    fs::write(f.root.join("mode.txt"), "disconnect").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    f.wait(|s| s["runs"][0]["finished_at"].is_number());
    worker.stop();
    let db = rusqlite::Connection::open(&f.db).unwrap();
    for n in 1..=5 {
        db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id,finished_at) SELECT ?1,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,worker_id,finished_at FROM worker_runs LIMIT 1", [format!("historical-{n}")]).unwrap();
    }
    for (limit, expected) in [(2, 2), (0, 0)] {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&f.root)
            .env("HEY_BOSS_ISSUE_DB", &f.db)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args(["worker", "--history", &limit.to_string(), "status"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("Active agents (0)"));
        assert_eq!(
            text.lines()
                .filter(|line| line.contains(" · failed · "))
                .count(),
            expected
        );
        assert_eq!(
            text.contains("Recent attempts (history; these do not use slots)"),
            limit > 0
        );
    }
    assert_eq!(
        f.cli(&["worker", "status"])["runs"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
}
#[test]
fn approval_blocks_without_auto_approving() {
    let f = Fixture::new("approval");
    f.setup(&[]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["runs"][0]["state"], "blocked");
    assert!(
        s["runs"][0]["summary"]
            .as_str()
            .unwrap()
            .contains("approval")
    );
    w.stop();
}
#[test]
fn stop_and_shutdown_reap_codex_before_releasing_claim() {
    let f = Fixture::new("delay");
    f.setup(&[
        "--prompt",
        "/goal Assign and implement `{{issue_command}}`. {{commit_instruction}}",
    ]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["state"] == "running" && s["runs"][0]["goal"].is_object());
    let id = s["runs"][0]["id"].as_str().unwrap();
    let pid = s["runs"][0]["pid"].as_u64().unwrap() as i32;
    f.control("stop", id);
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["runs"][0]["state"], "cancelled");
    assert_eq!(s["runs"][0]["goal"]["status"], "paused");
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
    f.control("retry", id);
    f.wait(|s| s["active"] == 1 && s["runs"][0]["goal"].is_object());
    w.stop();
    let s = f.cli(&["worker", "status"]);
    assert_eq!(s["active"], 0);
    assert_eq!(s["runs"][0]["state"], "cancelled");
    assert_eq!(s["runs"][0]["goal"]["status"], "paused");
}
#[test]
fn orphan_recovery_stops_process_and_preserves_session() {
    let f = Fixture::new("delay-orphan");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["state"] == "running");
    let pid = s["runs"][0]["pid"].as_u64().unwrap() as i32;
    let session = s["runs"][0]["session_id"].clone();
    let old_run = s["runs"][0]["id"].clone();
    let _ = w.0.kill();
    let _ = w.0.wait();
    let mut replacement = f.worker();
    f.wait(|s| {
        s["workers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["pid"] == replacement.0.id() && w["active"] == 1)
            && f.cli(&["view", "1"])["issue"]["assignee"]
                == format!("codex:{}", session.as_str().unwrap())
    });
    let db = rusqlite::Connection::open(&f.db).unwrap();
    let old_state: String = db
        .query_row(
            "SELECT state FROM worker_runs WHERE id=?1",
            [old_run.as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(old_state, "interrupted");
    let active_session: String = db.query_row("SELECT session_id FROM worker_runs WHERE owner_pid=?1 AND claimed_at IS NOT NULL AND finished_at IS NULL", [replacement.0.id()], |r| r.get(0)).unwrap();
    assert_eq!(active_session, session.as_str().unwrap());
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    replacement.stop();
}

#[test]
fn unclaimed_completion_never_closes_issue() {
    let f = Fixture::new("unclaimed");
    f.setup(&[]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["runs"][0]["state"], "blocked");
    assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "open");
    w.stop();
}
#[test]
fn missed_manual_claim_deadline_stops_codex_and_frees_slot() {
    let f = Fixture::new("delay-unclaimed");
    f.setup(&["--claim-timeout", "5"]);
    let mut w = f.worker();
    let s = f.wait(|s| s["runs"][0]["state"] == "awaiting_claim");
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
    let pid = s["runs"][0]["pid"].as_u64().unwrap() as i32;
    let s = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(s["active"], 0);
    assert_eq!(s["free"], 1);
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "open");
    w.stop();
}

#[test]
fn independent_workers_have_separate_capacity_tags_and_atomic_reservations() {
    let f = Fixture::new("parallel");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&["--concurrency", "2", "--tag", "ready"]);
    f.cli(&["edit", "1", "--label", "ready"]);
    f.cli(&["create", "--title", "Ready two", "--label", "ready"]);
    f.cli(&["create", "--title", "Backlog one", "--label", "backlog"]);
    f.cli(&["create", "--title", "Backlog two", "--label", "backlog"]);
    let mut first = f.worker();
    f.wait(|s| {
        s["active"] == 2
            && s["runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["claimed_at"].is_number())
    });
    fs::write(
        f.root.join("worker-args.json"),
        serde_json::to_vec(&["--concurrency", "2", "--tag", "backlog"]).unwrap(),
    )
    .unwrap();
    let mut second = f.worker();
    let s = f.wait(|s| {
        s["workers"].as_array().unwrap().len() == 2
            && s["workers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|w| w["active"] == 2)
    });
    assert_eq!(s["config"]["tags"][0], "backlog");
    assert_eq!(s["free"], 0);
    let db = rusqlite::Connection::open(&f.db).unwrap();
    let distinct: i64 = db
        .query_row(
            "SELECT count(DISTINCT issue_number) FROM worker_runs WHERE finished_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        distinct, 4,
        "No shared project/global cap and no duplicate reservation"
    );
    f.cli(&["create", "--title", "Unrestricted fifth"]);
    fs::write(f.root.join("worker-args.json"), "[]").unwrap();
    let mut third = f.worker();
    f.wait(|s| s["workers"].as_array().unwrap().len() == 3 && s["active"] == 1);
    let s = f.cli(&["worker", "status"]);
    assert!(s["config"]["tags"].as_array().unwrap().is_empty());
    assert_eq!(s["runs"][0]["number"], 5);
    let text = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .env("HEY_BOSS_ISSUE_DB", &f.db)
        .args(["worker", "status"])
        .output()
        .unwrap();
    let text = String::from_utf8(text.stdout).unwrap();
    for expected in [
        "Slots:",
        "free",
        "busy",
        "Pipeline:",
        "manual claim",
        "Codex",
        "m0",
    ] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
    third.stop();
    second.stop();
    first.stop();
}

#[test]
fn reservation_blocks_other_agents_until_expiry_and_preserves_takeover() {
    let f = Fixture::new("lock");
    fs::write(f.root.join("mode.txt"), "delay-unclaimed").unwrap();
    f.setup(&["--claim-timeout", "5"]);
    let mut w = f.worker();
    f.wait(|s| s["runs"][0]["state"] == "awaiting_claim");
    let result = f.command(&["claim", "1"]).output().unwrap();
    assert_eq!(result.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&result.stdout).contains("reserved"));
    let boss = f.command(&["assign-to-boss", "1"]).output().unwrap();
    assert_eq!(boss.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&boss.stdout).contains("reserved"));
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute(
        "UPDATE worker_runs SET reservation_expires=0 WHERE finished_at IS NULL",
        [],
    )
    .unwrap();
    assert_eq!(
        f.cli(&["claim", "1"])["issue"]["assignee"],
        "human:worker-test"
    );
    f.wait(|s| s["active"] == 0);
    assert_eq!(
        f.cli(&["view", "1"])["issue"]["assignee"],
        "human:worker-test"
    );
    w.stop();
}

#[test]
fn disconnected_mac_does_not_block_issue_completion_pickup_or_worker_stop_and_updates_replay() {
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    let f = Fixture::new("offline-updates");
    let state = f.root.join("companion-state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("bridge-protocol"), "1").unwrap();
    let executable = f.root.join("bin/hey-boss");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &executable).unwrap();
    fs::write(executable.with_extension("state"), state.to_str().unwrap()).unwrap();
    let broker_start = || {
        let broker = Worker(
            Command::new(env!("CARGO_BIN_EXE_hey-boss"))
                .args(["companion", "serve", "--state"])
                .arg(&state)
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(state.join("daemon.sock")).is_err() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        broker
    };
    let broker = broker_start();
    f.setup(&["--concurrency", "1"]);
    f.cli(&["create", "--title", "Second offline issue"]);
    f.cli(&["create", "--title", "Third offline issue"]);
    let mut worker = f.worker();
    let status = f.wait(|s| {
        s["runs"].as_array().is_some_and(|runs| {
            runs.len() == 3 && runs.iter().all(|r| r["finished_at"].is_number())
        })
    });
    for run in status["runs"].as_array().unwrap() {
        assert_eq!(run["state"], "completed");
    }
    for number in ["1", "2", "3"] {
        assert_eq!(f.cli(&["view", number])["issue"]["state"], "closed");
    }
    let queued = || {
        fs::read_dir(state.join("queue"))
            .unwrap()
            .map(|p| p.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect::<Vec<_>>()
    };
    let paths = queued();
    assert_eq!(paths.len(), 3, "Every offline update was saved");
    for path in &paths {
        let entry: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(entry["upstream"].is_null());
        assert_eq!(entry["request"]["command"], "update");
    }
    let began = Instant::now();
    worker.stop();
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "Stopping must not wait for queue delivery"
    );
    assert_eq!(queued().len(), 3);
    drop(broker);
    fs::remove_file(state.join("daemon.sock")).unwrap();
    let bridge = UnixListener::bind(state.join("bridge.sock")).unwrap();
    bridge.set_nonblocking(true).unwrap();
    let restarted = broker_start();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut delivered = Vec::new();
    while delivered.len() < 3 {
        match bridge.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                if request["command"] == "update" {
                    assert_eq!(request["project"], "Offline worker QA");
                    delivered.push(request["question"].as_str().unwrap().to_owned());
                }
                stream
                    .write_all(br#"{"task_id":"replayed-offline-update","status":"pending"}"#)
                    .unwrap();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("Bridge failed: {e}"),
        }
        assert!(
            Instant::now() < deadline,
            "Queue did not replay all updates: {delivered:?}"
        );
    }
    for number in [1, 2, 3] {
        assert!(
            delivered
                .iter()
                .any(|text| text.contains(&format!("issue view {number}`"))),
            "Lost update for issue {number}"
        );
    }
    drop(restarted);
}

#[test]
fn worker_refreshes_order_before_each_reservation_and_preserves_tag_filters() {
    let f = Fixture::new("order-live");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&["--tag", "ready"]);
    f.cli(&["edit", "1", "--label", "ready"]);
    f.cli(&["create", "--title", "Second ready", "--label", "ready"]);
    f.cli(&["create", "--title", "Third ready", "--label", "ready"]);
    f.cli(&["create", "--title", "Unready backlog"]);
    let mut worker = f.worker();
    let running = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    let run = running["runs"][0]["id"].as_str().unwrap();
    f.cli(&["move", "3", "--before", "2"]);
    f.cli(&["move", "4", "--before", "3"]);
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.control("stop", run);
    let result = f.wait(|s| {
        s["runs"]
            .as_array()
            .is_some_and(|r| r.len() == 4 && r.iter().all(|r| r["finished_at"].is_number()))
    });
    let turns: Vec<_> = f
        .transcript()
        .into_iter()
        .filter(|v| v["method"] == "turn/start")
        .map(|v| v["params"]["input"][0]["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(turns.len(), 4, "{result}");
    // Stopped unfinished work stays first and is eligible immediately.
    for (text, number) in turns.iter().zip([1, 1, 3, 2]) {
        assert!(text.contains(&format!("issue view {number}`")), "{turns:?}");
    }
    assert_eq!(f.cli(&["view", "4"])["issue"]["state"], "open");
    assert_eq!(f.cli(&["view", "3"])["issue"]["state"], "closed");
    assert_eq!(f.cli(&["view", "2"])["issue"]["state"], "closed");
    worker.stop();
}

#[test]
fn worker_starts_with_saved_order_instead_of_creation_order() {
    let f = Fixture::new("order-start");
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.setup(&[]);
    f.cli(&["create", "--title", "Two"]);
    f.cli(&["create", "--title", "Three"]);
    f.cli(&["move", "3", "--before", "1"]);
    let mut worker = f.worker();
    f.wait(|s| {
        s["runs"]
            .as_array()
            .is_some_and(|r| r.len() == 3 && r.iter().all(|r| r["state"] == "completed"))
    });
    let turns: Vec<_> = f
        .transcript()
        .into_iter()
        .filter(|v| v["method"] == "turn/start")
        .map(|v| v["params"]["input"][0]["text"].as_str().unwrap().to_owned())
        .collect();
    for (text, number) in turns.iter().zip([3, 1, 2]) {
        assert!(text.contains(&format!("issue view {number}`")), "{turns:?}");
    }
    worker.stop();
}

#[test]
fn worker_skips_boss_assignment_until_human_releases_it() {
    let f = Fixture::new("boss-skip");
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.setup(&[]);
    f.cli(&["assign-to-boss", "1"]);
    f.cli(&[
        "create",
        "--title",
        "Agent work",
        "--body",
        "Pick this instead",
    ]);
    let mut w = f.worker();
    let s = f.wait(|s| {
        s["runs"].as_array().is_some_and(|runs| {
            runs.iter()
                .any(|r| r["number"] == 2 && r["finished_at"].is_number())
        })
    });
    assert!(
        s["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["number"] != 1)
    );
    assert_eq!(f.cli(&["view", "1"])["issue"]["assignee"], "human:boss");
    f.cli(&["unassign", "1", "--force"]);
    f.wait(|s| {
        s["runs"].as_array().is_some_and(|runs| {
            runs.iter()
                .any(|r| r["number"] == 1 && r["finished_at"].is_number())
        })
    });
    assert_eq!(f.cli(&["view", "1"])["issue"]["state"], "closed");
    w.stop();
}

#[test]
fn parallel_worker_refreshes_subtask_readiness_before_reserving_parents() {
    let f = Fixture::new("subtasks-completed");
    f.setup(&["--concurrency", "2", "--prompt", "/goal"]);
    f.cli(&["subtask", "create", "1", "--title", "Intermediate"]);
    f.cli(&["subtask", "create", "2", "--title", "Nested leaf"]);
    f.cli(&["create", "--title", "Independent"]);
    f.cli(&["subtask", "create", "1", "--title", "Sibling leaf"]);
    let mut worker = f.worker();
    let status = f.wait(|s| {
        s["runs"].as_array().is_some_and(|runs| {
            runs.len() == 5 && runs.iter().all(|r| r["finished_at"].is_number())
        })
    });
    let runs = status["runs"].as_array().unwrap();
    assert!(runs.iter().all(|r| r["state"] == "completed"), "{status}");
    for (parent, child) in [(1, 2), (2, 3), (1, 5)] {
        let p = runs.iter().find(|r| r["number"] == parent).unwrap();
        let c = runs.iter().find(|r| r["number"] == child).unwrap();
        assert!(
            p["started_at"].as_i64().unwrap() >= c["finished_at"].as_i64().unwrap(),
            "Parent {parent} was reserved before child {child} completed: {status}"
        );
    }
    for number in 1..=5 {
        assert_eq!(
            f.cli(&["view", &number.to_string()])["issue"]["state"],
            "closed"
        );
    }
    let transcript = f.transcript();
    let starts: Vec<_> = transcript
        .iter()
        .filter(|v| v["method"] == "turn/start")
        .collect();
    assert_eq!(starts.len(), 5);
    let first: Vec<_> = starts
        .iter()
        .take(2)
        .map(|v| v["params"]["input"][0]["text"].as_str().unwrap())
        .collect();
    assert!(
        first.iter().any(|s| s.contains("issue view 3"))
            && first.iter().any(|s| s.contains("issue view 4")),
        "Fresh queue must reserve the first two ready issues: {first:?}"
    );
    assert_eq!(
        transcript
            .iter()
            .filter(|v| v["method"] == "thread/goal/set" && v["params"]["status"] == "complete")
            .count(),
        5
    );
    worker.stop();
}

#[test]
fn model_queue_wait_does_not_consume_manual_claim_deadline() {
    let f = Fixture::new("delay-model-start");
    f.setup(&["--claim-timeout", "5"]);
    let mut worker = f.worker();
    f.wait(|s| s["runs"][0]["state"] == "awaiting_model");
    thread::sleep(Duration::from_secs(5));
    let status = f.cli(&["worker", "status"]);
    assert!(status["runs"][0]["finished_at"].is_null());
    f.wait(|s| s["runs"][0]["state"] == "awaiting_claim");
    let status = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(status["runs"][0]["state"], "claim_timeout");
    worker.stop();
}

#[test]
fn default_claim_window_survives_past_two_minute_reasoning_timeout() {
    let f = Fixture::new("past-two-minute-timeout");
    fs::write(f.root.join("mode.txt"), "delay-unclaimed").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let status = f.wait(|s| s["runs"][0]["state"] == "awaiting_claim");
    let run = &status["runs"][0];
    let db = rusqlite::Connection::open(&f.db).unwrap();
    // Replay 130 seconds of pre-claim reasoning without a long wall-clock test.
    db.execute(
        "UPDATE worker_runs SET reservation_expires=reservation_expires-130000 WHERE id=?1",
        [run["id"].as_str().unwrap()],
    )
    .unwrap();
    thread::sleep(Duration::from_millis(600));
    let current = f.cli(&["worker", "status"]);
    assert!(current["runs"][0]["finished_at"].is_null(), "{current}");
    let mut claim = f.command(&["claim", "1"]);
    claim.args([
        "--agent",
        &format!("codex:{}", run["session_id"].as_str().unwrap()),
    ]);
    let result = claim.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    worker.stop();
}

#[test]
fn default_claim_window_still_expires_after_ten_minutes() {
    let f = Fixture::new("ten-minute-timeout");
    fs::write(f.root.join("mode.txt"), "delay-unclaimed").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let status = f.wait(|s| s["runs"][0]["state"] == "awaiting_claim");
    assert_eq!(status["config"]["reservation_seconds"], 600);
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute(
        "UPDATE worker_runs SET reservation_expires=reservation_expires-601000 WHERE id=?1",
        [status["runs"][0]["id"].as_str().unwrap()],
    )
    .unwrap();
    let expired = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(expired["runs"][0]["state"], "claim_timeout");
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
    worker.stop();
}

#[test]
fn worker_startup_waits_for_transient_writer_contention() {
    let f = Fixture::new("startup-writer-contention");
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.setup(&["--concurrency", "1"]);
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut worker = f.worker();
    thread::sleep(Duration::from_secs(12));
    assert!(
        worker.0.try_wait().unwrap().is_none(),
        "Worker exited before the startup write lock was released"
    );
    db.execute_batch("ROLLBACK").unwrap();
    let status = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(status["runs"][0]["state"], "completed");
    worker.stop();
}

#[test]
fn codex_sqlite_startup_contention_retries_the_same_reservation() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("codex-sqlite-startup");
    fs::write(f.root.join("mode.txt"), "completed").unwrap();
    f.setup(&["--concurrency", "1"]);
    let mock = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-worker.py");
    let wrapper = f.root.join("codex.sh");
    fs::write(&wrapper, format!("#!/bin/sh\nif [ ! -f startup-retried ]; then\n touch startup-retried\n printf 'Error: failed to initialize sqlite state runtime: database is locked\\n' >&2\n exit 1\nfi\nexec '{}' \"$@\"\n", mock.display())).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let mut worker = f.worker();
    let status = f.wait(|s| s["runs"][0]["finished_at"].is_number());
    assert_eq!(status["runs"][0]["state"], "completed", "{status}");
    assert_eq!(
        status["runs"].as_array().unwrap().len(),
        1,
        "Startup retry created an issue attempt"
    );
    worker.stop();
}

#[test]
fn stopping_worker_cancels_codex_sqlite_startup_retry() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("codex-sqlite-startup-stop");
    f.setup(&["--concurrency", "1"]);
    let wrapper = f.root.join("codex.sh");
    fs::write(&wrapper, "#!/bin/sh\nprintf 'Error: failed to initialize sqlite state runtime: database is locked\\n' >&2\nexit 1\n").unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let mut worker = f.worker();
    f.wait(|s| {
        s["runs"][0]["last_event"]
            .as_str()
            .is_some_and(|event| event.contains("retrying startup"))
    });
    let started = Instant::now();
    worker.stop();
    assert!(started.elapsed() < Duration::from_secs(3));
    let status = f.cli(&["worker", "status"]);
    assert_eq!(status["runs"][0]["state"], "cancelled", "{status}");
    assert!(f.cli(&["view", "1"])["issue"]["assignee"].is_null());
}

#[test]
fn worker_shutdown_waits_for_writer_and_exits_successfully() {
    let f = Fixture::new("shutdown-writer-contention");
    fs::write(f.root.join("mode.txt"), "delay-unclaimed").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    f.wait(|s| s["runs"][0]["state"] == "awaiting_claim");
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    unsafe {
        libc::kill(worker.0.id() as i32, libc::SIGTERM);
    }
    thread::sleep(Duration::from_secs(12));
    db.execute_batch("ROLLBACK").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = worker.0.try_wait().unwrap() {
            assert!(
                status.success(),
                "Worker failed to shut down cleanly: {status}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Worker failed to finish shutdown"
        );
        thread::sleep(Duration::from_millis(25));
    }
    let status = f.cli(&["worker", "status"]);
    assert!(status["runs"][0]["finished_at"].is_number());
    assert_eq!(status["runs"][0]["state"], "cancelled");
    assert!(!status["config"]["enabled"].as_bool().unwrap());
}

#[test]
fn writer_contention_does_not_exit_worker_or_kill_claimed_session() {
    let f = Fixture::new("writer-contention");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    let mut worker = f.worker();
    let running = f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute_batch("BEGIN IMMEDIATE; UPDATE issues SET title=title WHERE number=1")
        .unwrap();
    thread::sleep(Duration::from_secs(12));
    assert!(
        worker.0.try_wait().unwrap().is_none(),
        "Worker exited during transient writer contention"
    );
    let current = f.cli(&["worker", "status"]);
    assert_eq!(current["runs"][0]["pid"], running["runs"][0]["pid"]);
    assert!(current["runs"][0]["finished_at"].is_null());
    db.execute_batch("ROLLBACK").unwrap();
    worker.stop();
}

#[test]
fn worker_status_reports_supervisor_connectivity_from_fleet_heartbeat() {
    let f = Fixture::new("supervisor-connectivity");
    f.cli(&["create", "--title", "Connectivity fixture"]);
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute("UPDATE fleet_meta SET role='agent' WHERE id=1", [])
        .unwrap();
    let state = f.root.join("fleet-state");
    fs::create_dir_all(&state).unwrap();
    let read_status = || {
        let output = f
            .command(&["worker", "status"])
            .env("HEY_BOSS_FLEET_STATE", &state)
            .output()
            .unwrap();
        assert!(output.status.success());
        let fleet = serde_json::from_slice::<Value>(&output.stdout).unwrap()["fleet"].clone();
        assert_eq!(
            fleet["supervisor_connection"],
            fleet["controller_connection"]
        );
        fleet["supervisor_connection"].clone()
    };
    assert_eq!(read_status()["state"], "unknown");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as f64;
    fs::write(
        state.join("fleet-agent-status.json"),
        serde_json::json!({"connected_at":timestamp,"last_sync":timestamp}).to_string(),
    )
    .unwrap();
    assert_eq!(read_status()["state"], "connected");
    assert_eq!(read_status()["last_sync"], timestamp);
    fs::write(
        state.join("fleet-agent-status.json"),
        r#"{"connected_at":1,"last_sync":1}"#,
    )
    .unwrap();
    assert_eq!(read_status()["state"], "disconnected");
    fs::write(state.join("fleet-agent-status.json"), "broken").unwrap();
    assert_eq!(read_status()["state"], "unknown");
    db.execute("UPDATE fleet_meta SET role='controller' WHERE id=1", [])
        .unwrap();
    assert_eq!(read_status()["state"], "local");
    db.execute("UPDATE fleet_meta SET role='standalone' WHERE id=1", [])
        .unwrap();
    assert_eq!(read_status()["state"], "standalone");
}

#[test]
fn fleet_supervisor_command_accepts_the_old_name() {
    let help = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args(["fleet", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("supervisor"));
    for (name, canonical) in [
        ("supervisor", "supervisor"),
        ("controller", "supervisor"),
        ("companion", "companion"),
        ("agent", "companion"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(["fleet", name, "--help"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{name}: {:?}", output.stderr);
        assert!(String::from_utf8_lossy(&output.stdout).contains(canonical));
    }
}

#[test]
fn worker_does_not_reserve_drafts_and_picks_up_after_undrafting() {
    let f = Fixture::new("draft-pickup");
    fs::write(f.root.join("mode.txt"), "delay").unwrap();
    f.setup(&[]);
    f.cli(&["edit", "1", "--draft"]);
    let mut worker = f.worker();
    let status = f.wait(|s| s["config"]["enabled"] == true && s["active"] == 0);
    assert_eq!(status["eligible"], 0);
    assert!(status["runs"].as_array().unwrap().is_empty());
    f.cli(&["undraft", "1"]);
    f.wait(|s| s["runs"][0]["claimed_at"].is_number());
    let result = f.command(&["edit", "1", "--draft"]).output().unwrap();
    assert_eq!(result.status.code(), Some(4));
    worker.stop();
}

#[test]
fn worker_startup_repairs_missing_draft_schema_before_pickup() {
    for version in [10, 12] {
        let f = Fixture::new(&format!("repair-draft-schema-{version}"));
        fs::write(f.root.join("mode.txt"), "completed").unwrap();
        f.setup(&[]);
        f.cli(&["comment", "1", "--body", "Preserved history"]);
        let db = rusqlite::Connection::open(&f.db).unwrap();
        db.busy_timeout(Duration::from_secs(10)).unwrap();
        db.execute_batch("ALTER TABLE issues DROP COLUMN draft;")
            .unwrap();
        if version == 10 {
            db.execute_batch(
                "ALTER TABLE issues DROP COLUMN plan;
                ALTER TABLE project_settings DROP COLUMN drafts_enabled;
                ALTER TABLE project_settings DROP COLUMN plan_template;",
            )
            .unwrap();
        }
        db.pragma_update(None, "user_version", version).unwrap();

        let mut worker = f.worker();
        // Read SQLite directly: a status CLI would repair the store itself and
        // could conceal a worker that queried i.draft before migrating.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                worker.0.try_wait().unwrap().is_none(),
                "Worker exited before pickup (schema {version})"
            );
            let completed: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM worker_runs WHERE issue_number=1 AND state='completed' AND finished_at IS NOT NULL)",
                [], |row| row.get(0),
            ).unwrap();
            if completed {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Worker did not complete pickup (schema {version})"
            );
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            13
        );
        let issue = f.cli(&["view", "1"]);
        assert_eq!(issue["issue"]["state"], "closed");
        assert_eq!(issue["issue"]["draft"], false);
        assert!(issue["issue"]["plan"].is_null());
        assert_eq!(issue["comments"][0]["body"], "Preserved history");
        worker.stop();
    }
}

#[test]
fn worker_worktree_flags_override_project_choices() {
    for (mode, project_enabled, flag, expected) in [
        (
            "worktree-flag",
            false,
            "--worktree",
            "dedicated Git worktree",
        ),
        ("checkout-flag", true, "--no-worktree", "existing checkout"),
    ] {
        let f = Fixture::new(mode);
        fs::write(f.root.join("mode.txt"), "completed").unwrap();
        f.setup(&[flag]);
        f.cli(&[
            "settings",
            "set",
            if project_enabled {
                "--worktree"
            } else {
                "--no-worktree"
            },
        ]);
        let mut worker = f.worker();
        let status = f.wait(|s| s["runs"][0]["finished_at"].is_number());
        assert_eq!(status["config"]["worktree_enabled"], !project_enabled);
        let transcript = f.transcript();
        let turn = transcript
            .iter()
            .find(|v| v["method"] == "turn/start")
            .unwrap();
        assert!(
            turn["params"]["input"][0]["text"]
                .as_str()
                .unwrap()
                .contains(expected)
        );
        worker.stop();
    }
}
