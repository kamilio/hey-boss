use super::*;
use sha2::{Digest, Sha256};
use std::io::Read;

#[test]
fn reload_after_lost_reply() {
    exercise(false, false, None);
}
#[test]
fn reload_waits_for_job_and_issue() {
    exercise(true, false, None);
}
#[test]
fn busy_job_reaches_rollout_deadline() {
    exercise(false, true, None);
}
#[test]
fn recover_pending_restart() {
    exercise(false, false, Some("pending"));
}
#[test]
fn recover_stopping_restart() {
    exercise(false, false, Some("stopping"));
}
#[test]
fn recover_starting_restart() {
    exercise(false, false, Some("starting"));
}

fn exercise(with_issue: bool, deadline: bool, replay: Option<&str>) {
    let mut f = JobFixture::new();
    if with_issue {
        f.runtime = f.root.join("codex.sh");
        fs::write(&f.runtime, "#!/bin/sh\nif [ -n \"$HEY_BOSS_JOB_RUN\" ]; then exec \"$TEST_JOB_AGENT\" \"$@\"; else exec \"$TEST_ISSUE_AGENT\" \"$@\"; fi\n").unwrap();
        fs::set_permissions(&f.runtime, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(f.checkout.join("mode.txt"), "delay-release").unwrap();
    }
    let mut fleet = f.supervisor();
    let (id, mut worker) = f.start_worker(false);
    let db = f.db();
    f.create_agent();
    let start_job = || {
        f.cli(&["job", "run-now", "agent"])["run"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let run = start_job();
    wait_state(&db, &run, "running");
    wait_for("job prompt", || {
        f.audit.metadata().is_ok_and(|m| m.len() > 0)
    });
    let execution = || {
        let (owner, generation, report): (u32, i64, String) = db
            .query_row(
                "SELECT owner_pid,generation,report FROM local_job_executions WHERE run_id=?1",
                [&run],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        let report: Value = serde_json::from_str(&report).unwrap();
        assert_eq!(owner, worker.0.id());
        assert_eq!(report["cwd"].as_str(), f.checkout.to_str());
        assert_eq!(report["state"], "running");
        (
            generation,
            report["session_id"].as_str().unwrap().to_owned(),
        )
    };
    let before = execution();
    let owner_generation = || {
        db.query_row(
            "SELECT generation FROM scheduled_job_owners WHERE run_id=?1",
            [&run],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(before.0, owner_generation());
    let schedule = || {
        db.query_row("SELECT * FROM scheduled_jobs WHERE id='agent'", [], |r| {
            (0..r.as_ref().column_count())
                .map(|i| r.get::<_, rusqlite::types::Value>(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
    };
    let saved = schedule();
    let counts: (i64, i64) = db
        .query_row(
            "SELECT (SELECT count(*) FROM issues),(SELECT count(*) FROM worker_runs)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(counts, (0, 0));
    let status = || f.cli(&["worker", "status", "--id", &id]);
    let worker_row = |report: &Value| {
        report["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == id)
            .unwrap()
            .clone()
    };
    let initial = status();
    let registration = worker_row(&initial);
    assert_eq!(initial["active"], 0);
    assert_eq!(initial["free"], 1);
    assert_eq!(registration["job_active"], 1);
    let active_issues = || {
        db.query_row(
            "SELECT count(*) FROM worker_runs WHERE claimed_at IS NOT NULL AND finished_at IS NULL",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    if with_issue {
        f.cli(&["issue", "create", "--title", "Fixture issue"]);
        wait_for("claimed issue", || active_issues() == 1);
    }
    let target = registration["build"].as_str().unwrap();
    assert_eq!(
        db.execute(
            "UPDATE issue_worker_builds SET build='previous-build' WHERE worker_id=?1",
            [&id]
        )
        .unwrap(),
        1
    );
    let replacement = f.root.join("hey-boss.new");
    fs::copy(&f.source, &replacement).unwrap();
    fs::rename(replacement, &f.binary).unwrap();
    let rollout = || {
        Service(
            f.command(&f.binary)
                .args(["upgrade", "--reload-workers", target, "--json"])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    };
    let finish = |child: &mut Service, max: Duration, code: i32| {
        let until = Instant::now() + max;
        let exit = loop {
            if let Some(s) = child.0.try_wait().unwrap() {
                break s;
            }
            assert!(Instant::now() < until, "rollout timed out: {}", status());
            thread::sleep(Duration::from_millis(100));
        };
        assert_eq!(exit.code(), Some(code));
        let mut out = String::new();
        child
            .0
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        serde_json::from_str::<Value>(&out).unwrap()
    };
    let still_owned = || {
        assert_eq!(execution(), before);
        let stopping: bool = db
            .query_row(
                "SELECT stop_requested FROM issue_workers WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!stopping);
    };
    let mut helper = rollout();
    if deadline {
        let report = finish(&mut helper, Duration::from_secs(135), 1);
        assert_eq!(report["complete"], false);
        assert_eq!(worker_row(&report)["state"], "waiting_for_idle");
    } else {
        // Allow the helper to inspect the busy worker before simulating its lost reply.
        wait_for("rollout lock", || f.root.join("upgrade.lock").exists());
        thread::sleep(Duration::from_secs(2));
        assert!(helper.0.try_wait().unwrap().is_none());
        helper.terminate();
    }
    still_owned();
    let digest = Sha256::digest(serde_json::to_vec(&id).unwrap());
    let receipt = format!("upgrade-{target}-{}", &format!("{digest:x}")[..16]);
    if !deadline {
        fleet.terminate();
        if let Some(phase) = replay {
            db.execute("INSERT INTO fleet_signals (id,host,worker,signal,state,result,created_at) VALUES(?1,'local',?2,'restart',?3,?4,unixepoch())",rusqlite::params![receipt,id,phase,serde_json::json!({"prior_pid":worker.0.id()}).to_string()]).unwrap();
        }
        fleet = f.supervisor();
    }
    let mut helper = rollout();
    thread::sleep(Duration::from_secs(1));
    assert!(helper.0.try_wait().unwrap().is_none());
    still_owned();
    let release = f.root.join("release");
    fs::write(&release, "").unwrap();
    if with_issue {
        wait_state(&db, &run, "succeeded");
        assert!(helper.0.try_wait().unwrap().is_none());
        assert_eq!(active_issues(), 1);
        fs::write(f.checkout.join("release"), "").unwrap();
    }
    assert_eq!(
        finish(&mut helper, Duration::from_secs(25), 0)["complete"],
        true
    );
    wait_state(&db, &run, "succeeded");
    let after = worker_row(&status());
    assert_eq!(after["pid"], worker.0.id());
    assert_eq!(after["build"], target);
    assert_eq!(after["config"], registration["config"]);
    if replay.is_some() {
        let state: String = db
            .query_row(
                "SELECT state FROM fleet_signals WHERE id=?1",
                [receipt],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "acknowledged");
    }
    assert_eq!(schedule(), saved);
    assert_eq!(owner_generation(), before.0);
    assert_eq!(
        finish(&mut rollout(), Duration::from_secs(20), 0)["complete"],
        true
    );
    let launches = fs::read_to_string(&f.audit).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(launches.trim()).unwrap()["run"],
        run
    );
    fs::remove_file(release).unwrap();
    let second = start_job();
    wait_state(&db, &second, "running");
    f.cli(&["job", "stop", "agent", &second]);
    wait_state(&db, &second, "cancelled");
    wait_state(&db, &run, "succeeded");
    assert_eq!(
        db.query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        i64::from(with_issue)
    );
    worker.terminate();
    fleet.terminate();
}
