//! A reported task process outlives agent/UI availability. Only reconciled evidence releases it.
use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub(super) const INDEX: &str = "CREATE INDEX IF NOT EXISTS issue_attempt_recovery ON events(project_id,issue_number,id DESC) WHERE action='attempt_reconciled';";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptReport {
    pub attempt_id: String,
    pub owner: String,
    pub pid: u32,
    /// Required when identity cannot be observed; otherwise checked against the live process.
    #[serde(default)]
    pub process_start: Option<String>,
    pub log_path: PathBuf,
    pub worktree: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptEvidence {
    pub attempt_id: String,
    pub process: String,
    pub log_bytes: u64,
    pub log_device: u64,
    pub log_inode: u64,
    pub log_modified_at: (i64, i64),
    pub log_tail_sha256: String,
    pub git_dir: String,
    pub git_head: String,
    pub git_branch: String,
    pub git_status: String,
    pub git_diff_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Hold {
    #[serde(flatten)]
    report: AttemptReport,
    machine: String,
    host: String,
    reported_by: String,
    reported_at: i64,
    worker_run: Option<String>,
    #[serde(default)]
    previous_worker_result: Option<Value>,
    git_dir: String,
    git_head: String,
    git_branch: String,
}

fn read(db: &Connection, project: &str, number: i64) -> Result<Hold> {
    let text: Option<String> = db.query_row(
        "SELECT attempt_hold FROM issues WHERE project_id=?1 AND number=?2",
        params![project, number],
        |r| r.get(0),
    )?;
    serde_json::from_str(
        &text.ok_or_else(|| Error::conflict("This issue has no surviving attempt hold"))?,
    )
    .map_err(Into::into)
}

pub(in crate::issues) fn guard(db: &Connection, project: &str, number: i64) -> Result<()> {
    if held(db, project, number)? {
        return Err(Error::new(
            "attempt_held",
            "A surviving task attempt protects this issue. Inspect and reconcile its terminal process, log and Git outcome before continuing; force and reservation expiry cannot clear this hold.",
        ));
    }
    Ok(())
}
pub(super) fn held(db: &Connection, project: &str, number: i64) -> Result<bool> {
    Ok(db.query_row(
        "SELECT attempt_hold IS NOT NULL FROM issues WHERE project_id=?1 AND number=?2",
        params![project, number],
        |r| r.get(0),
    )?)
}

pub(super) fn enrich(db: &Connection, project: &str, issue: &mut Value) -> Result<()> {
    let Some(number) = issue["number"].as_i64() else {
        return Ok(());
    };
    let data: Option<String> = db.query_row("SELECT data FROM events WHERE project_id=?1 AND issue_number=?2 AND action='attempt_reconciled' ORDER BY id DESC LIMIT 1", params![project,number], |r| r.get(0)).optional()?;
    if let Some(data) = data {
        issue["attempt_recovery"] = serde_json::from_str(&data)?;
    }
    Ok(())
}
impl Store {
    pub(crate) fn worker_attempt_held(&self, job: &super::super::worker::Job) -> Result<bool> {
        held(&self.db, &job.project.id, job.number())
    }
}

fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(Error::conflict(
            "Retained Git source cannot be inspected; the attempt remains protected",
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| Error::invalid("Git evidence must be UTF-8"))
}

fn diff_hash(path: &Path) -> Result<String> {
    let mut child = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(path)
        .args(["diff", "--no-ext-diff", "--no-textconv", "--binary", "HEAD"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut output = child.stdout.take().unwrap();
    let mut hash = Sha256::new();
    let mut buffer = [0; 16_384];
    let result = (|| -> std::io::Result<()> {
        loop {
            let size = output.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            hash.update(&buffer[..size]);
        }
        Ok(())
    })();
    drop(output);
    let status = child.wait()?;
    result?;
    if !status.success() {
        return Err(Error::conflict(
            "Retained Git changes cannot be inspected; hold preserved",
        ));
    }
    Ok(format!("{:x}", hash.finalize()))
}

// Missing inspection permission or app handles are unknown, never proof of exit.
fn process_state(pid: u32, expected: &str) -> &'static str {
    if crate::agent_process::exited(pid).unwrap_or(false) {
        return "terminal";
    }
    match crate::agents::process_identity(pid) {
        Some(actual) if actual != expected => "terminal",
        Some(_) => {
            let zombie = Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "stat="])
                .output()
                .is_ok_and(|o| {
                    o.status.success()
                        && String::from_utf8_lossy(&o.stdout)
                            .trim_start()
                            .starts_with('Z')
                });
            if zombie { "terminal" } else { "live" }
        }
        None => {
            if unsafe { libc::kill(pid as i32, 0) } != 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                "terminal"
            } else {
                "unknown"
            }
        }
    }
}

fn evidence(hold: &Hold) -> Result<AttemptEvidence> {
    if hold.machine != super::super::identity::machine()? {
        return Err(Error::conflict(format!(
            "Inspect and reconcile on {} ({}) where the retained process and source live",
            hold.host, hold.machine
        )));
    }
    let mut log = fs::File::open(&hold.report.log_path)?;
    let metadata = log.metadata()?;
    if !metadata.is_file() {
        return Err(Error::invalid("Attempt log must be a regular file"));
    }
    log.seek(SeekFrom::Start(metadata.len().saturating_sub(65_536)))?;
    let mut tail = Vec::new();
    log.take(65_537).read_to_end(&mut tail)?;
    if tail.len() > 65_536 {
        return Err(Error::conflict("Attempt log is still changing"));
    }
    let path = &hold.report.worktree;
    let git_dir = git(path, &["rev-parse", "--absolute-git-dir"])?
        .trim()
        .to_owned();
    if git_dir != hold.git_dir {
        return Err(Error::conflict(
            "Retained Git directory changed; hold preserved",
        ));
    }
    Ok(AttemptEvidence {
        attempt_id: hold.report.attempt_id.clone(),
        process: process_state(
            hold.report.pid,
            hold.report.process_start.as_deref().unwrap_or(""),
        )
        .into(),
        log_bytes: metadata.len(),
        log_device: metadata.dev(),
        log_inode: metadata.ino(),
        log_modified_at: (metadata.mtime(), metadata.mtime_nsec()),
        log_tail_sha256: format!("{:x}", Sha256::digest(&tail)),
        git_dir,
        git_head: git(path, &["rev-parse", "HEAD"])?.trim().into(),
        git_branch: git(path, &["rev-parse", "--abbrev-ref", "HEAD"])?
            .trim()
            .into(),
        git_status: git(
            path,
            &["status", "--porcelain=v1", "--untracked-files=normal"],
        )?,
        git_diff_sha256: diff_hash(path)?,
    })
}

pub(super) fn hold(
    db: &Connection,
    project: &Project,
    actor: &Actor,
    number: i64,
    version: i64,
    report: &AttemptReport,
    now: i64,
) -> Result<Value> {
    let issue = get_issue(db, &project.id, number, false)?;
    if version != issue.version {
        return Err(Error::conflict(
            "Issue changed; refresh before reporting an attempt",
        ));
    }
    guard(db, &project.id, number)?;
    if !matches!(issue.state.as_str(), "open" | "blocked") {
        return Err(Error::conflict(
            "Only unfinished issues can retain an attempt",
        ));
    }
    identifier(&report.attempt_id, "attempt ID", 256)?;
    identifier(&report.owner, "original owner", 256)?;
    if report.pid == 0 || report.pid > i32::MAX as u32 {
        return Err(Error::invalid("PID must be a positive process ID"));
    }
    if !report.log_path.is_absolute() || !report.worktree.is_absolute() {
        return Err(Error::invalid("Log and worktree paths must be absolute"));
    }
    let owner_known: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE project_id=?1 AND issue_number=?2 AND actor=?3 AND action='claimed') OR EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND actor_id=?3)", params![project.id,number,report.owner], |r| r.get(0))?;
    if issue
        .assignee
        .as_deref()
        .is_some_and(|owner| owner != report.owner)
        || (issue.assignee.is_none() && !owner_known)
    {
        return Err(Error::conflict(
            "Original owner does not match this issue's retained attempt",
        ));
    }
    let mut report = report.clone();
    let actual = crate::agents::process_identity(report.pid);
    let expected = report.process_start.clone().or(actual).ok_or_else(|| Error::conflict("Process identity unavailable; supply its recorded process_start before placing a safe hold"))?;
    identifier(&expected, "process start identity", 256)?;
    if process_state(report.pid, &expected) == "terminal" {
        return Err(Error::conflict(
            "The reported process is already terminal; no live-attempt hold was created. Review its log and Git outcome before ordinary continuation.",
        ));
    }
    report.process_start = Some(expected);
    report.worktree = report.worktree.canonicalize()?;
    report.log_path = report.log_path.canonicalize()?;
    if !report.log_path.is_file() {
        return Err(Error::invalid("Attempt log must be a regular file"));
    }
    let path = &report.worktree;
    let other: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND actor_id<>?3 AND finished_at IS NULL)", params![project.id,number,report.owner], |r| r.get(0))?;
    if other {
        return Err(Error::conflict(
            "Another attempt already reserved this issue; inspect its ownership before reporting a surviving process",
        ));
    }
    let previous: Option<(String,String,Option<i64>)> = db.query_row("SELECT id,state,finished_at FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND actor_id=?3 ORDER BY started_at DESC,id DESC LIMIT 1", params![project.id,number,report.owner], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let worker_run = previous.as_ref().map(|(id, _, _)| id.clone());
    let previous_worker_result = previous
        .as_ref()
        .map(|(id, state, finished)| json!({"id":id,"state":state,"finished_at":finished}));
    if let Some((id, _, Some(_))) = &previous {
        db.execute("UPDATE worker_runs SET state='attempt_held',finished_at=NULL,updated_at=?2,retry_at=NULL WHERE id=?1",params![id,now])?;
    }
    let hold = Hold {
        git_dir: git(path, &["rev-parse", "--absolute-git-dir"])?
            .trim()
            .into(),
        git_head: git(path, &["rev-parse", "HEAD"])?.trim().into(),
        git_branch: git(path, &["rev-parse", "--abbrev-ref", "HEAD"])?
            .trim()
            .into(),
        report,
        machine: super::super::identity::machine()?,
        host: super::super::identity::host(),
        reported_by: actor.id.clone(),
        reported_at: now,
        worker_run,
        previous_worker_result,
    };
    db.execute("UPDATE issues SET attempt_hold=?3,version=version+1,updated_at=?4 WHERE project_id=?1 AND number=?2", params![project.id,number,serde_json::to_string(&hold)?,now])?;
    event(
        db,
        &project.id,
        number,
        &actor.id,
        "attempt_held",
        now,
        &json!(hold),
    )?;
    Ok(json!({"ok":true,"project":project,"issue":get_issue(db,&project.id,number,false)?}))
}

pub(super) fn inspect(db: &Connection, project: &Project, number: i64) -> Result<Value> {
    let hold = read(db, &project.id, number)?;
    Ok(
        json!({"ok":true,"project":project,"issue":get_issue(db,&project.id,number,false)?,"evidence":evidence(&hold)?}),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn reconcile(
    db: &Connection,
    project: &Project,
    actor: &Actor,
    number: i64,
    version: i64,
    reviewed: &AttemptEvidence,
    outcome: &str,
    now: i64,
) -> Result<Value> {
    let issue = get_issue(db, &project.id, number, false)?;
    if issue.version != version {
        return Err(Error::conflict(
            "Issue changed; inspect the attempt again before reconciliation",
        ));
    }
    body(outcome, true)?;
    let hold = read(db, &project.id, number)?;
    let current = evidence(&hold)?;
    if current.process != "terminal" {
        return Err(Error::conflict(
            "The retained process is live or unknown; no hold was released",
        ));
    }
    if &current != reviewed {
        return Err(Error::conflict(
            "Process, log or Git evidence changed; review the current outcome before releasing the hold",
        ));
    }
    if let Some(run) = &hold.worker_run {
        let state: Option<String> = db
            .query_row(
                "SELECT state FROM worker_runs WHERE id=?1 AND finished_at IS NULL",
                [run],
                |r| r.get(0),
            )
            .optional()?;
        if state.as_deref().is_some_and(|s| s != "attempt_held") {
            return Err(Error::conflict(
                "The original agent is still working; wait for its blocked/interrupted result before reconciliation",
            ));
        }
        if let Some((Some(pid), Some(start))) = db.query_row(
            "SELECT pid,process_start FROM worker_runs WHERE id=?1 AND state='attempt_held' AND finished_at IS NULL",
            [run], |r| Ok((r.get::<_,Option<u32>>(0)?,r.get::<_,Option<String>>(1)?)),
        ).optional()? {
            // Only after reviewing terminal task evidence may its retained app-server group stop.
            super::super::worker::stop_group(pid, &start)?;
        }
        db.execute("UPDATE worker_runs SET state='interrupted',finished_at=?2,updated_at=?2,retry_allowed=1,retry_at=NULL,summary=?3 WHERE id=?1 AND state='attempt_held' AND finished_at IS NULL", params![run,now,outcome])?;
    }
    // Preserve user state, dependencies and destination. Only this attempt's claim is released.
    db.execute("UPDATE issues SET attempt_hold=NULL,assignee=CASE WHEN assignee=?3 THEN NULL ELSE assignee END,version=version+1,updated_at=?4 WHERE project_id=?1 AND number=?2", params![project.id,number,hold.report.owner,now])?;
    db.execute("UPDATE worker_runs SET retry_allowed=1,retry_at=NULL WHERE project_id=?1 AND issue_number=?2 AND actor_id=?3 AND finished_at IS NOT NULL", params![project.id,number,hold.report.owner])?;
    event(
        db,
        &project.id,
        number,
        &actor.id,
        "attempt_reconciled",
        now,
        &json!({"hold":hold,"evidence":current,"outcome":outcome}),
    )?;
    Ok(json!({"ok":true,"project":project,"issue":get_issue(db,&project.id,number,false)?}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Stdio};

    struct Fixture {
        root: PathBuf,
        store: Store,
        actor: Actor,
        project: Project,
        child: Child,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "hb-attempt-{}",
                super::super::super::worker::random_id().unwrap()
            ));
            fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            for args in [
                vec!["init", "-q"],
                vec![
                    "-c",
                    "user.name=QA",
                    "-c",
                    "user.email=qa@example.test",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "Fixture",
                ],
            ] {
                assert!(
                    Command::new("git")
                        .arg("-C")
                        .arg(&root)
                        .args(args)
                        .status()
                        .unwrap()
                        .success()
                );
            }
            fs::write(root.join("task.log"), "validation started\n").unwrap();
            let actor = Actor {
                id: "codex:attempt-owner".into(),
                kind: "codex".into(),
                session_id: Some("attempt-owner".into()),
                machine: super::super::super::identity::machine().unwrap(),
                host: "test".into(),
                pid: None,
                process_start: None,
                cwd: root.clone(),
                source: "test".into(),
                invocation: None,
                creation_run: None,
                model: None,
            };
            let project = Project {
                id: "named:Attempt QA".into(),
                name: "Attempt QA".into(),
            };
            // Keep changing SQLite files outside the retained Git worktree.
            fs::write(
                root.join(".gitignore"),
                "issues.db*\ntask.log\n.gitignore\n",
            )
            .unwrap();
            let store = Store::open(&root.join("issues.db")).unwrap();
            let child = Command::new("/bin/sh")
                .args(["-c", "read outcome; exit \"$outcome\""])
                .process_group(0)
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            let mut f = Self {
                root,
                store,
                actor,
                project,
                child,
            };
            f.apply(Operation::Create {
                draft: false,
                title: "Task".into(),
                body: "Requirements".into(),
                labels: vec![],
                at_top: false,
                blockers: vec![],
                then_titles: vec![],
            })
            .unwrap();
            f.apply(Operation::Claim {
                number: 1,
                force: false,
            })
            .unwrap();
            f
        }
        fn apply(&mut self, operation: Operation) -> Result<Value> {
            self.store.execute(&Request {
                version: 1,
                project: self.project.clone(),
                project_override: None,
                actor: Some(self.actor.clone()),
                operation,
                request_id: None,
            })
        }
        fn version(&self) -> i64 {
            get_issue(&self.store.db, &self.project.id, 1, false)
                .unwrap()
                .version
        }
        fn report(&self) -> AttemptReport {
            AttemptReport {
                attempt_id: "validation-1".into(),
                owner: self.actor.id.clone(),
                pid: self.child.id(),
                process_start: None,
                log_path: self.root.join("task.log"),
                worktree: self.root.clone(),
            }
        }
        fn hold(&mut self) {
            self.apply(Operation::HoldAttempt {
                number: 1,
                if_version: self.version(),
                report: self.report(),
            })
            .unwrap();
        }
        fn finish(&mut self, code: i32) {
            writeln!(self.child.stdin.as_mut().unwrap(), "{code}").unwrap();
            assert_eq!(self.child.wait().unwrap().code(), Some(code));
            fs::write(
                self.root.join("task.log"),
                format!("validation exited {code}\n"),
            )
            .unwrap();
        }
        fn inspect(&mut self) -> AttemptEvidence {
            serde_json::from_value(
                self.apply(Operation::InspectAttempt { number: 1 }).unwrap()["evidence"].clone(),
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn attempt_hold_survives_expiry_and_only_matching_terminal_evidence_releases_once() {
        for code in [0, 7] {
            let mut f = Fixture::new();
            f.store
                .db
                .execute(
                    "INSERT INTO fleet_allocations VALUES(?1,1,'old-machine')",
                    [&f.project.id],
                )
                .unwrap();
            f.store
                .db
                .execute("UPDATE fleet_allocation_deadlines SET expires_at=0", [])
                .unwrap();
            let old = f.version();
            f.hold();
            assert!(
                f.apply(Operation::HoldAttempt {
                    number: 1,
                    if_version: old,
                    report: f.report()
                })
                .is_err()
            );
            f.apply(Operation::Unassign {
                number: 1,
                force: false,
            })
            .unwrap();
            for _ in 0..8 {
                assert!(
                    !f.store
                        .db
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE number=1)",
                            [],
                            |r| r.get::<_, bool>(0)
                        )
                        .unwrap()
                );
                assert!(
                    f.apply(Operation::Claim {
                        number: 1,
                        force: true
                    })
                    .is_err()
                );
                assert!(
                    f.apply(Operation::Reopen {
                        number: 1,
                        if_version: None,
                        clear_manual_hold: false
                    })
                    .is_err()
                );
                f.store
                    .release_stale_claims(&f.actor.machine, i64::MAX / 2)
                    .unwrap();
                assert!(
                    f.child.try_wait().unwrap().is_none(),
                    "Scheduler must not signal a retained process"
                );
            }
            let live = f.inspect();
            assert_eq!(live.process, "live");
            assert!(
                f.apply(Operation::ReconcileAttempt {
                    number: 1,
                    if_version: f.version(),
                    evidence: live,
                    outcome: "Reviewed".into()
                })
                .is_err()
            );
            f.finish(code);
            let terminal = f.inspect();
            assert_eq!(terminal.process, "terminal");
            assert!(
                f.apply(Operation::ReconcileAttempt {
                    number: 1,
                    if_version: old,
                    evidence: terminal.clone(),
                    outcome: "Reviewed".into()
                })
                .is_err()
            );
            fs::write(f.root.join("task.log"), "new outcome\n").unwrap();
            assert!(
                f.apply(Operation::ReconcileAttempt {
                    number: 1,
                    if_version: f.version(),
                    evidence: terminal,
                    outcome: "Reviewed".into()
                })
                .is_err()
            );
            let terminal = f.inspect();
            let version = f.version();
            f.apply(Operation::ReconcileAttempt {
                number: 1,
                if_version: version,
                evidence: terminal.clone(),
                outcome: format!("Reviewed exit {code}; source retained, no commit"),
            })
            .unwrap();
            assert!(
                f.apply(Operation::ReconcileAttempt {
                    number: 1,
                    if_version: version,
                    evidence: terminal,
                    outcome: "Duplicate".into()
                })
                .is_err()
            );
            assert!(
                f.store
                    .db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE number=1)",
                        [],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap()
            );
            let history: String = f
                .store
                .db
                .query_row(
                    "SELECT data FROM events WHERE action='attempt_reconciled'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let history: Value = serde_json::from_str(&history).unwrap();
            assert_eq!(history["hold"]["owner"], f.actor.id);
            assert_eq!(
                history["hold"]["worktree"],
                f.root.to_string_lossy().as_ref()
            );
            f.store
                .db
                .execute("DELETE FROM fleet_allocations", [])
                .unwrap();
            f.apply(Operation::Claim {
                number: 1,
                force: false,
            })
            .unwrap();
            let mut other = f.actor.clone();
            other.id = "codex:other".into();
            assert!(
                f.store
                    .execute(&Request {
                        version: 1,
                        project: f.project.clone(),
                        project_override: None,
                        actor: Some(other),
                        operation: Operation::Claim {
                            number: 1,
                            force: false
                        },
                        request_id: None
                    })
                    .is_err()
            );
        }
    }

    #[test]
    fn already_terminal_and_reused_children_do_not_create_indefinite_holds() {
        let mut f = Fixture::new();
        let mut report = f.report();
        report.process_start = crate::agents::process_identity(f.child.id());
        f.finish(0);
        assert!(
            f.apply(Operation::HoldAttempt {
                number: 1,
                if_version: f.version(),
                report
            })
            .is_err()
        );
        assert!(!held(&f.store.db, &f.project.id, 1).unwrap());
        assert_eq!(
            process_state(std::process::id(), "old PID identity"),
            "terminal"
        );
    }

    #[test]
    fn detached_child_keeps_protection_after_its_parent_and_app_handle_disappear() {
        struct Detached(u32);
        impl Drop for Detached {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(self.0 as i32, libc::SIGTERM);
                }
            }
        }
        let mut f = Fixture::new();
        let output = Command::new("/bin/sh")
            .args(["-c", "sleep 30 >/dev/null 2>&1 & echo $!"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let child = Detached(
            String::from_utf8(output.stdout)
                .unwrap()
                .trim()
                .parse()
                .unwrap(),
        );
        let identity = crate::agents::process_identity(child.0).unwrap();
        let mut report = f.report();
        report.pid = child.0;
        report.process_start = Some(identity.clone());
        f.apply(Operation::HoldAttempt {
            number: 1,
            if_version: f.version(),
            report,
        })
        .unwrap();
        f.apply(Operation::Unassign {
            number: 1,
            force: false,
        })
        .unwrap();
        for _ in 0..5 {
            f.store
                .release_stale_claims(&f.actor.machine, i64::MAX / 2)
                .unwrap();
            assert!(
                !f.store
                    .db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE number=1)",
                        [],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap()
            );
            assert_eq!(process_state(child.0, &identity), "live");
        }
        let saved = f.inspect();
        assert_eq!(saved.process, "live");
        assert_eq!(saved.attempt_id, "validation-1");
    }

    #[test]
    fn process_and_git_guards_preserve_hold_across_store_reopen() {
        let mut f = Fixture::new();
        f.hold();
        f.finish(7);
        let evidence = f.inspect();
        fs::write(f.root.join("changed.txt"), "retained edits").unwrap();
        f.store = Store::open(&f.root.join("issues.db")).unwrap();
        assert!(
            f.apply(Operation::ReconcileAttempt {
                number: 1,
                if_version: f.version(),
                evidence,
                outcome: "Reviewed".into()
            })
            .is_err()
        );
        assert!(held(&f.store.db, &f.project.id, 1).unwrap());
    }

    #[test]
    fn reporting_after_claim_release_restores_the_original_attempt_slot() {
        let mut f = Fixture::new();
        f.store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at) VALUES('original-run',?1,1,'{}',?2,'blocked',1,'old',?3,1,2,2)",params![f.project.id,f.actor.id,f.actor.machine]).unwrap();
        f.apply(Operation::Unassign {
            number: 1,
            force: false,
        })
        .unwrap();
        f.hold();
        assert_eq!(f.store.db.query_row("SELECT count(*) FROM worker_runs WHERE finished_at IS NULL AND state='attempt_held'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        f.finish(0);
        let evidence = f.inspect();
        let result = f
            .apply(Operation::ReconcileAttempt {
                number: 1,
                if_version: f.version(),
                evidence,
                outcome: "Reviewed successful validation; source retained".into(),
            })
            .unwrap();
        assert_eq!(
            result["issue"]["attempt_recovery"]["hold"]["worker_run"],
            "original-run"
        );
        assert_eq!(
            result["issue"]["attempt_recovery"]["hold"]["previous_worker_result"]["state"],
            "blocked"
        );
        let continuation =
            super::super::subtasks::worker_issue(&f.store.db, &f.project.id, 1).unwrap();
        assert_eq!(
            continuation["attempt_recovery"]["hold"]["owner"],
            f.actor.id
        );
        assert_eq!(
            continuation["attempt_recovery"]["hold"]["worktree"],
            f.root.to_string_lossy().as_ref()
        );
        assert_eq!(
            f.store
                .db
                .query_row(
                    "SELECT count(*) FROM worker_runs WHERE finished_at IS NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}
