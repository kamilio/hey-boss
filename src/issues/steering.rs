//! Messages are delivered by the worker that owns the live session, never by resuming it.
use super::*;
use crate::issues::worker::Job;

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='agent_steering' AND type='table')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        db.execute_batch("CREATE TABLE IF NOT EXISTS agent_steering(request_id TEXT PRIMARY KEY,run_id TEXT NOT NULL REFERENCES worker_runs(id),scope TEXT NOT NULL,text TEXT NOT NULL,issue_body TEXT,state TEXT NOT NULL DEFAULT 'queued',error TEXT,created_at INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS agent_steering_pending ON agent_steering(run_id,created_at) WHERE state='queued'; CREATE INDEX IF NOT EXISTS agent_steering_history ON agent_steering(run_id,created_at);")?;
    }
    Ok(())
}
impl Store {
    pub(crate) fn worker_steer(&mut self, run: &str, input: &Value) -> Result<Value> {
        let request = input["request_id"]
            .as_str()
            .ok_or_else(|| Error::invalid("Missing steering request ID"))?;
        identifier(request, "request ID", 128)?;
        let text = input["text"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 32_000)
            .ok_or_else(|| Error::invalid("Instruction must contain 1–32000 bytes"))?;
        let scope = input["scope"]
            .as_str()
            .filter(|s| ["session", "issue", "project"].contains(s))
            .ok_or_else(|| Error::invalid("Choose agent, issue or project scope"))?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved: Option<String> = tx.query_row("SELECT r.job FROM worker_runs r JOIN projects p ON p.id=r.project_id WHERE r.id=?1 AND r.finished_at IS NULL AND r.stop_requested=0 AND p.hidden_at IS NULL", [run], |r| r.get(0)).optional()?;
        let job: Job = serde_json::from_str(&saved.ok_or_else(|| {
            Error::conflict(
                "This agent stopped or is no longer available. Refresh before steering.",
            )
        })?)?;
        let issue = get_issue(&tx, &job.project.id, job.number(), false)?;
        if issue.state != "open" || issue.assignee.as_deref() != Some(&job.actor.id) {
            return Err(Error::conflict(
                "The issue owner changed. Refresh before steering.",
            ));
        }
        let existing: Option<Value> = tx.query_row("SELECT run_id,scope,text,state,error FROM agent_steering WHERE request_id=?1", [request], |r| Ok(json!({"run":r.get::<_,String>(0)?,"scope":r.get::<_,String>(1)?,"text":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"error":r.get::<_,Option<String>>(4)?}))).optional()?;
        if let Some(existing) = existing {
            if existing["run"] != run || existing["scope"] != scope || existing["text"] != text {
                return Err(Error::conflict(
                    "Steering request ID was already used for another instruction",
                ));
            }
            return Ok(
                json!({"ok":true,"request_id":request,"state":existing["state"],"error":existing["error"]}),
            );
        }
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM agent_steering WHERE run_id=?1 AND state IN ('queued','sending')",
            [run],
            |r| r.get(0),
        )?;
        if count >= 16 {
            return Err(Error::conflict(
                "Wait for queued instructions to reach this agent before sending more.",
            ));
        }
        let mut issue_body = None;
        let mut boss = job.actor.clone();
        boss.id = "human:boss".into();
        boss.kind = "human".into();
        boss.session_id = None;
        tx.execute("INSERT INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET metadata=excluded.metadata,last_seen=excluded.last_seen", params![boss.id,serde_json::to_string(&boss)?,crate::issues::worker::now()])?;
        if scope == "issue" {
            let operation: Operation = serde_json::from_value(
                json!({"action":"edit","number":job.number(),"body":format!("{}\n\n{}",issue.body.trim_end(),text),"add_labels":[],"remove_labels":[],"if_version":issue.version}),
            )?;
            mutate(
                &tx,
                &job.project,
                &boss,
                &operation,
                crate::issues::worker::now(),
            )?;
            issue_body = Some(get_issue(&tx, &job.project.id, job.number(), false)?.body);
        } else if scope == "project" {
            let settings = registry::project_settings(&tx, &job.project)?;
            let operation: Operation = serde_json::from_value(
                json!({"action":"configure_project","prompt":format!("{}\n\n{}",settings["prompt"].as_str().unwrap().trim_end(),text),"if_version":settings["version"]}),
            )?;
            registry::execute(&tx, &job.project, &operation, Some(&boss))?;
        }
        tx.execute("INSERT INTO agent_steering(request_id,run_id,scope,text,created_at,issue_body) VALUES(?1,?2,?3,?4,?5,?6)", params![request,run,scope,text,crate::issues::worker::now(),issue_body])?;
        tx.commit()?;
        Ok(json!({"ok":true,"request_id":request,"state":"queued"}))
    }

    pub(crate) fn worker_steering(&self, run: &str) -> Result<Option<Value>> {
        Ok(self.db.query_row("SELECT request_id,text,scope,issue_body FROM agent_steering WHERE run_id=?1 AND state='queued' ORDER BY created_at,rowid LIMIT 1", [run], |r| Ok(json!({"request_id":r.get::<_,String>(0)?,"text":r.get::<_,String>(1)?,"scope":r.get::<_,String>(2)?,"issue_body":r.get::<_,Option<String>>(3)?}))).optional()?)
    }
    pub(crate) fn worker_steering_result(
        &self,
        request: &str,
        state: &str,
        error: Option<&str>,
    ) -> Result<()> {
        self.db.execute(
            "UPDATE agent_steering SET state=?2,error=?3 WHERE request_id=?1",
            params![request, state, error],
        )?;
        Ok(())
    }
}
