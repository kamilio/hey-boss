//! Issue-backed messages to an existing local session. No worker or claim mutations.
use super::*;
use crate::agents::{Agent, Snapshot};
use std::path::PathBuf;

#[derive(Debug, clap::Args)]
pub struct Coordinate {
    /// Exact current worktree on this device; a shared application PID is insufficient.
    pub worktree: PathBuf,
    /// Link a non-runnable draft to its existing session (the session itself or human:boss).
    #[arg(long, conflicts_with = "comment", requires = "issue")]
    pub link: bool,
    #[arg(long, requires = "issue")]
    pub comment: Option<i64>,
    #[arg(long)]
    pub issue: Option<i64>,
    /// Explicit caller identity, using the same trust boundary as issue commands.
    #[arg(long)]
    pub agent: Option<String>,
    #[arg(long)]
    pub json: bool,
}

fn select<'a>(snapshot: &'a Snapshot, worktree: &Path) -> Result<&'a Agent> {
    if !snapshot.warnings.is_empty() {
        return Err(Error::new(
            "session_unknown",
            "Discovery is incomplete; ownership is unknown",
        ));
    }
    let matches: Vec<_> = snapshot
        .agents
        .iter()
        .filter(|a| {
            a.cwd.as_deref().map(Path::new) == Some(worktree)
                || (a
                    .cwd
                    .as_deref()
                    .is_some_and(|cwd| Path::new(cwd).starts_with(worktree))
                    && a.git
                        .as_ref()
                        .is_some_and(|g| Path::new(&g.worktree) == worktree))
        })
        .collect();
    if matches.len() != 1 {
        return Err(Error::new(
            "session_unknown",
            "Expected exactly one session for this worktree; shared PID matches are unknown",
        ));
    }
    let agent = matches[0];
    if agent.kind != "Codex"
        || agent.session_id.is_none()
        || !matches!(
            agent.cwd_source.as_deref(),
            Some("turn_context" | "live_process_environment")
        )
        || agent.evidence != "Session file held open by this process"
        || agent.state != "Working"
    {
        return Err(Error::new(
            "session_unknown",
            "Current working session is unverified; saved checkout or PID alone is insufficient",
        ));
    }
    Ok(agent)
}

fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS session_issue_links(machine TEXT NOT NULL,session TEXT NOT NULL,project TEXT NOT NULL,number INTEGER NOT NULL,worktree TEXT NOT NULL,actor TEXT NOT NULL,created_at INTEGER NOT NULL,PRIMARY KEY(machine,session),UNIQUE(project,number),FOREIGN KEY(project,number) REFERENCES issues(project_id,number) ON DELETE CASCADE);
        CREATE TABLE IF NOT EXISTS session_comment_receipts(machine TEXT NOT NULL,session TEXT NOT NULL,project TEXT NOT NULL,number INTEGER NOT NULL,comment INTEGER NOT NULL,actor TEXT NOT NULL,state TEXT NOT NULL,error TEXT,created_at INTEGER NOT NULL,PRIMARY KEY(machine,session,comment),FOREIGN KEY(project,number) REFERENCES issues(project_id,number) ON DELETE CASCADE);")?;
    Ok(())
}

fn linked(db: &Connection, machine: &str, session: &str) -> Result<Option<(String, i64)>> {
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='session_issue_links')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(None);
    }
    Ok(db
        .query_row(
            "SELECT project,number FROM session_issue_links WHERE machine=?1 AND session=?2",
            params![machine, session],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

fn issues(db: &Connection, machine: &str, session: &str) -> Result<Vec<Value>> {
    let owner = format!("codex:{session}");
    let mut rows = db.prepare("SELECT project_id,number,title,state FROM issues WHERE assignee=?1 AND deleted_at IS NULL AND state IN ('open','ready') ORDER BY project_id,number")?
        .query_map([owner], |r| Ok(json!({"project":r.get::<_,String>(0)?,"number":r.get::<_,i64>(1)?,"title":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"association":"assignment"})))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if let Some((project, number)) = linked(db, machine, session)? {
        let row = db.query_row("SELECT title,state FROM issues WHERE project_id=?1 AND number=?2 AND deleted_at IS NULL",params![project,number], |r| Ok(json!({"project":project,"number":number,"title":r.get::<_,String>(0)?,"state":r.get::<_,String>(1)?,"association":"session_link"}))).optional()?;
        if let Some(row) = row {
            rows.push(row);
        }
    }
    Ok(rows)
}

fn link(
    db: &Connection,
    actor: &Actor,
    session: &str,
    project: &Project,
    number: i64,
    worktree: &Path,
) -> Result<()> {
    if actor.id != format!("codex:{session}") && actor.id != "human:boss" {
        return Err(Error::new(
            "unauthorized",
            "Only the existing session or human:boss may approve its issue link",
        ));
    }
    let issue = get_issue(db, &project.id, number, false)?;
    if !issue.draft || issue.assignee.is_some() || issue.state != "open"
        || db.query_row("SELECT EXISTS(SELECT 1 FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2) OR EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL)",params![project.id,number],|r|r.get::<_,bool>(0))?
    {
        return Err(Error::conflict("Session links require an open, unassigned, unreserved draft without a running worker"));
    }
    if let Some(existing) = linked(db, &actor.machine, session)? {
        if existing == (project.id.clone(), number) {
            return Ok(());
        }
        return Err(Error::conflict(
            "This session already has a coordination issue; reuse it",
        ));
    }
    db.execute(
        "INSERT OR IGNORE INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3)",
        params![
            actor.id,
            serde_json::to_string(actor)?,
            crate::issues::worker::now()
        ],
    )?;
    db.execute(
        "INSERT INTO session_issue_links VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            actor.machine,
            session,
            project.id,
            number,
            worktree.to_string_lossy(),
            actor.id,
            crate::issues::worker::now()
        ],
    )?;
    event(
        db,
        &project.id,
        number,
        &actor.id,
        "session_linked",
        crate::issues::worker::now(),
        &json!({"machine":actor.machine,"session":session,"worktree":worktree}),
    )?;
    Ok(())
}

fn message(
    db: &Connection,
    actor: &Actor,
    session: &str,
    project: &Project,
    number: i64,
    comment: i64,
) -> Result<String> {
    let issue = get_issue(db, &project.id, number, false)?;
    let is_linked = linked(db, &actor.machine, session)? == Some((project.id.clone(), number));
    if issue.state != "open"
        || (issue.assignee.as_deref() != Some(&format!("codex:{session}"))
            && !(is_linked && issue.draft && issue.assignee.is_none()))
    {
        return Err(Error::new(
            "unauthorized",
            "Issue is not associated with this current session",
        ));
    }
    let (author, body): (String, String) = db
        .query_row(
            "SELECT author,body FROM comments WHERE project_id=?1 AND issue_number=?2 AND id=?3",
            params![project.id, number, comment],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| Error::new("not_found", "Comment not found on this issue"))?;
    if actor.id != author && actor.id != "human:boss" {
        return Err(Error::new(
            "unauthorized",
            "Only the comment author or human:boss may deliver it",
        ));
    }
    if body.trim().is_empty() || body.len() > 32_000 {
        return Err(Error::invalid("Comment must contain 1–32000 bytes"));
    }
    Ok(body)
}

pub fn coordinate(options: &Coordinate) -> Result<Value> {
    if std::env::var_os("HEY_BOSS_ISSUE_HOST").is_some() {
        return Err(Error::invalid(
            "Run agent coordinate on the session's owning device; remote issue overrides are unsupported",
        ));
    }
    let worktree = options.worktree.canonicalize()?;
    let mut snapshot = crate::agents::scan();
    crate::agents::verify_worktree(&mut snapshot, &worktree)?;
    let agent = select(&snapshot, &worktree)?;
    let session = agent.session_id.as_deref().unwrap();
    let machine = super::super::identity::machine()?;
    let path = super::super::database_path()?;
    let db = Store::open_read_connection(&path)?;
    let associated = issues(&db, &machine, session)?;
    let mut report = json!({"ok":true,"owner":format!("codex:{session}"),"session":session,"pid":agent.pid,"worktree":worktree,"evidence":agent.cwd_source,"issues":associated,"issue_absent":associated.is_empty(),"delivery":"not_requested"});
    if !options.link && options.comment.is_none() {
        return Ok(report);
    }
    drop(db);
    let actor = super::super::identity::resolve(
        options.agent.as_deref(),
        &machine,
        &std::env::current_dir()?,
    )?;
    let project = super::super::identity::project(&worktree, &machine)?;
    let number = options
        .issue
        .ok_or_else(|| Error::invalid("Choose an issue"))?;
    let mut store = Store::open(&path)?;
    let tx = store
        .db
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    migrate(&tx)?;
    if options.link {
        link(&tx, &actor, session, &project, number, &worktree)?;
        report["issues"] = json!(issues(&tx, &machine, session)?);
        report["issue_absent"] = json!(false);
        tx.commit()?;
        return Ok(report);
    }
    tx.commit()?;
    let outcome = deliver(
        &mut store,
        Delivery {
            actor: &actor,
            session,
            project: &project,
            number,
            comment: options.comment.unwrap(),
        },
        || {
            let mut fresh = crate::agents::scan();
            crate::agents::verify_worktree(&mut fresh, &worktree)?;
            if select(&fresh, &worktree)?.session_id.as_deref() != Some(session) {
                return Err(Error::conflict("Worktree owner changed"));
            }
            Ok(())
        },
        |action, input| crate::agent_control::run(session, action, input),
    )?;
    report
        .as_object_mut()
        .unwrap()
        .extend(outcome.as_object().unwrap().clone());
    Ok(report)
}

struct Delivery<'a> {
    actor: &'a Actor,
    session: &'a str,
    project: &'a Project,
    number: i64,
    comment: i64,
}

fn deliver(
    store: &mut Store,
    delivery: Delivery<'_>,
    verify: impl FnOnce() -> Result<()>,
    mut control: impl FnMut(&str, &Value) -> std::io::Result<Value>,
) -> Result<Value> {
    let Delivery {
        actor,
        session,
        project,
        number,
        comment,
    } = delivery;
    let machine = &actor.machine;
    let tx = store
        .db
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    migrate(&tx)?;
    let body = message(&tx, actor, session, project, number, comment)?;
    if let Some(state) = tx.query_row("SELECT state FROM session_comment_receipts WHERE machine=?1 AND session=?2 AND comment=?3", params![machine,session,comment],|r|r.get::<_,String>(0)).optional()? {
        return Ok(json!({"delivery":state}));
    }
    tx.commit()?;
    let controls = control("inspect", &json!({}))
        .map_err(|e| Error::new("control_unavailable", e.to_string()))?;
    if controls["canSteer"] != true {
        return Err(Error::conflict("Existing session cannot accept input now"));
    }
    // Revalidate after network I/O, then reserve once before sending. A crash or lost
    // acknowledgement stays unknown; retrying cannot duplicate an accepted comment.
    verify()?;
    let tx = store
        .db
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if message(&tx, actor, session, project, number, comment)? != body {
        return Err(Error::conflict("Comment changed"));
    }
    let reserved = tx.execute("INSERT OR IGNORE INTO session_comment_receipts(machine,session,project,number,comment,actor,state,created_at) VALUES(?1,?2,?3,?4,?5,?6,'unknown',?7)",params![machine,session,project.id,number,comment,actor.id,crate::issues::worker::now()])?;
    tx.commit()?;
    if reserved == 0 {
        return Ok(json!({"delivery":"unknown"}));
    }
    let result = control(
        "steer",
        &json!({"text":body,"expectedTurnId":controls["turnId"]}),
    );
    let (state, error) = match result {
        Ok(_) => ("delivered", None),
        Err(e) => ("unknown", Some(e.to_string())),
    };
    let tx = store
        .db
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("UPDATE session_comment_receipts SET state=?4,error=?5 WHERE machine=?1 AND session=?2 AND comment=?3",params![machine,session,comment,state,error])?;
    event(
        &tx,
        &project.id,
        number,
        &actor.id,
        "session_comment_delivery",
        crate::issues::worker::now(),
        &json!({"session":session,"comment":comment,"state":state,"error":error}),
    )?;
    tx.commit()?;
    Ok(json!({"delivery":state,"error":error}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot {
        serde_json::from_value(json!({"host":"local","observed_at":1,"warnings":[],"agents":[
            {"id":"first","pid":42,"kind":"Codex","session_id":"first","cwd":"/worktrees/first","cwd_source":"turn_context","state":"Working","evidence":"Session file held open by this process"},
            {"id":"second","pid":42,"kind":"Codex","session_id":"second","cwd":"/worktrees/second","cwd_source":"turn_context","state":"Working","evidence":"Session file held open by this process"}
        ]})).unwrap()
    }

    #[test]
    fn shared_pid_requires_exact_current_worktree_and_rejects_ambiguity() {
        let mut s = snapshot();
        assert_eq!(
            select(&s, Path::new("/worktrees/second"))
                .unwrap()
                .session_id
                .as_deref(),
            Some("second")
        );
        assert!(select(&s, Path::new("/main")).is_err());
        s.agents[0].cwd = s.agents[1].cwd.clone();
        assert_eq!(
            select(&s, Path::new("/worktrees/second")).unwrap_err().code,
            "session_unknown"
        );
        s.agents.remove(0);
        s.agents[0].cwd_source = Some("session_meta".into());
        assert!(select(&s, Path::new("/worktrees/second")).is_err());
        s.agents[0].cwd_source = Some("turn_context".into());
        s.agents[0].evidence = "Previously observed transcript".into();
        assert!(select(&s, Path::new("/worktrees/second")).is_err());
    }

    struct Fixture {
        store: Store,
        request: Request,
        root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "hb-coordinate-{}",
                crate::issues::worker::random_id().unwrap()
            ));
            let store = Store::open(&root.join("issues.db")).unwrap();
            let actor =
                crate::issues::identity::resolve(Some("codex:first"), "local", &root).unwrap();
            let request = Request {
                version: 1,
                project: Project {
                    id: "named:test".into(),
                    name: "test".into(),
                },
                project_override: None,
                actor: Some(actor),
                operation: Operation::View { number: 1 },
                request_id: None,
            };
            let mut f = Self {
                store,
                request,
                root,
            };
            f.call(json!({"action":"create","title":"Coordination","body":"","draft":true,"labels":[]}));
            migrate(&f.store.db).unwrap();
            f
        }
        fn call(&mut self, op: Value) -> Value {
            self.request.operation = serde_json::from_value(op).unwrap();
            self.store.execute(&self.request).unwrap()
        }
        fn link(&self, actor: &Actor, session: &str, number: i64) -> Result<()> {
            link(
                &self.store.db,
                actor,
                session,
                &self.request.project,
                number,
                Path::new("/worktrees/first"),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn untracked_session_link_is_authorized_audited_and_idempotent_without_claims() {
        let mut f = Fixture::new();
        let actor = f.request.actor.clone().unwrap();
        assert!(issues(&f.store.db, "local", "first").unwrap().is_empty());
        assert_eq!(
            f.link(&actor, "second", 1).unwrap_err().code,
            "unauthorized"
        );
        f.link(&actor, "first", 1).unwrap();
        f.link(&actor, "first", 1).unwrap();
        assert_eq!(issues(&f.store.db, "local", "first").unwrap().len(), 1);
        assert!(
            issues(&f.store.db, "other-device", "first")
                .unwrap()
                .is_empty()
        );
        f.call(
            json!({"action":"create","title":"Another draft","body":"","draft":true,"labels":[]}),
        );
        assert!(f.link(&actor, "first", 2).is_err());
        let issue = get_issue(&f.store.db, "named:test", 1, false).unwrap();
        assert!(issue.draft && issue.assignee.is_none());
        assert_eq!(
            f.store
                .db
                .query_row("SELECT count(*) FROM worker_runs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            f.store
                .db
                .query_row(
                    "SELECT count(*) FROM events WHERE action='session_linked'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        let mut boss = actor.clone();
        boss.id = "human:boss".into();
        boss.kind = "explicit".into();
        f.link(&boss, "second", 2).unwrap();
    }

    #[test]
    fn only_associated_issue_and_authorized_saved_comment_can_be_delivered() {
        let mut f = Fixture::new();
        let actor = f.request.actor.clone().unwrap();
        f.call(json!({"action":"comment","number":1,"body":"Saved coordination note"}));
        let id = f
            .store
            .db
            .query_row("SELECT max(id) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap();
        assert!(message(&f.store.db, &actor, "first", &f.request.project, 1, id).is_err());
        f.link(&actor, "first", 1).unwrap();
        assert_eq!(
            message(&f.store.db, &actor, "first", &f.request.project, 1, id).unwrap(),
            "Saved coordination note"
        );
        let mut other = actor.clone();
        other.id = "codex:other".into();
        assert_eq!(
            message(&f.store.db, &other, "first", &f.request.project, 1, id)
                .unwrap_err()
                .code,
            "unauthorized"
        );
        assert!(message(&f.store.db, &actor, "second", &f.request.project, 1, id).is_err());
        f.store
            .db
            .execute("UPDATE issues SET draft=0 WHERE number=1", [])
            .unwrap();
        assert!(message(&f.store.db, &actor, "first", &f.request.project, 1, id).is_err());
    }

    #[test]
    fn runnable_or_reserved_issues_cannot_be_linked() {
        let f = Fixture::new();
        let actor = f.request.actor.clone().unwrap();
        f.store.db.execute("UPDATE issues SET draft=0", []).unwrap();
        assert!(f.link(&actor, "first", 1).is_err());
        f.store.db.execute("UPDATE issues SET draft=1", []).unwrap();
        f.store.db.execute("INSERT INTO fleet_allocations(project_id,issue_number,node) VALUES('named:test',1,'local')",[]).unwrap();
        assert!(f.link(&actor, "first", 1).is_err());
    }

    #[test]
    fn delivery_preserves_exact_comment_and_never_retries_uncertain_or_accepted_send() {
        for acknowledged in [false, true] {
            let mut f = Fixture::new();
            let actor = f.request.actor.clone().unwrap();
            let project = f.request.project.clone();
            f.link(&actor, "first", 1).unwrap();
            f.call(json!({"action":"comment","number":1,"body":"Exact saved note\nSecond line."}));
            let comment = f
                .store
                .db
                .query_row("SELECT max(id) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap();
            let mut sends = 0;
            for _ in 0..2 {
                let result = deliver(
                    &mut f.store,
                    Delivery {
                        actor: &actor,
                        session: "first",
                        project: &project,
                        number: 1,
                        comment,
                    },
                    || Ok(()),
                    |action, input| {
                        if action == "inspect" {
                            return Ok(json!({"canSteer":true,"turnId":"active-turn"}));
                        }
                        sends += 1;
                        assert_eq!(input["text"], "Exact saved note\nSecond line.");
                        assert_eq!(input["expectedTurnId"], "active-turn");
                        if acknowledged {
                            Ok(json!({"ok":true}))
                        } else {
                            Err(std::io::Error::other("Acknowledgement lost"))
                        }
                    },
                )
                .unwrap();
                assert_eq!(
                    result["delivery"],
                    if acknowledged { "delivered" } else { "unknown" }
                );
            }
            assert_eq!(sends, 1);
            assert_eq!(
                f.store
                    .db
                    .query_row(
                        "SELECT count(*) FROM events WHERE action='session_comment_delivery'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                1
            );
            assert!(
                get_issue(&f.store.db, &project.id, 1, false)
                    .unwrap()
                    .assignee
                    .is_none()
            );
        }
    }

    #[test]
    fn unavailable_control_or_changed_owner_never_sends_or_reserves_delivery() {
        let mut f = Fixture::new();
        let actor = f.request.actor.clone().unwrap();
        let project = f.request.project.clone();
        f.link(&actor, "first", 1).unwrap();
        f.call(json!({"action":"comment","number":1,"body":"Saved note"}));
        let comment = f
            .store
            .db
            .query_row("SELECT max(id) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap();
        for available in [false, true] {
            let result = deliver(
                &mut f.store,
                Delivery {
                    actor: &actor,
                    session: "first",
                    project: &project,
                    number: 1,
                    comment,
                },
                || Err(Error::conflict("Owner changed")),
                |action, _| {
                    assert_eq!(action, "inspect");
                    if available {
                        Ok(json!({"canSteer":true}))
                    } else {
                        Err(std::io::Error::other("No endpoint"))
                    }
                },
            );
            assert!(result.is_err());
            assert_eq!(
                f.store
                    .db
                    .query_row("SELECT count(*) FROM session_comment_receipts", [], |r| r
                        .get::<_, i64>(
                        0
                    ))
                    .unwrap(),
                0
            );
        }
    }
}
