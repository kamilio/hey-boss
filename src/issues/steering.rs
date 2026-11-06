//! Messages are delivered by the worker that owns the live session, never by resuming it.
use super::*;

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('agent_steering') WHERE name='issue_body')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let tx = crate::database::Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS agent_steering(request_id TEXT PRIMARY KEY,run_id TEXT NOT NULL REFERENCES worker_runs(id),scope TEXT NOT NULL,text TEXT NOT NULL,issue_body TEXT,state TEXT NOT NULL DEFAULT 'queued',error TEXT,created_at INTEGER NOT NULL);")?;
    if !tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('agent_steering') WHERE name='issue_body')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        tx.execute_batch("ALTER TABLE agent_steering ADD COLUMN issue_body TEXT;")?;
    }
    tx.execute_batch("CREATE INDEX IF NOT EXISTS agent_steering_pending ON agent_steering(run_id,created_at) WHERE state='queued'; CREATE INDEX IF NOT EXISTS agent_steering_history ON agent_steering(run_id,created_at);")?;
    tx.commit()?;
    Ok(())
}
impl Store {
    pub(crate) fn queue_broadcast(&mut self, input: &Value) -> Result<Value> {
        let id = input["request_id"]
            .as_str()
            .ok_or_else(|| Error::invalid("Missing broadcast ID"))?;
        identifier(id, "broadcast ID", 128)?;
        let runs = input["runs"]
            .as_array()
            .ok_or_else(|| Error::invalid("Missing broadcast recipients"))?;
        let text = input["text"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 32_000)
            .ok_or_else(|| Error::invalid("Enter a message of up to 32000 bytes"))?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let runs = serde_json::to_string(runs)?;
        tx.execute("INSERT OR IGNORE INTO agent_steering(request_id,run_id,scope,text,created_at)
            SELECT 'broadcast:'||?1||':'||r.id,r.id,'broadcast',?2,?3 FROM json_each(?4) target
            JOIN worker_runs r ON r.id=target.value JOIN issues i ON i.project_id=r.project_id AND i.number=r.issue_number
            JOIN projects p ON p.id=r.project_id WHERE r.finished_at IS NULL AND r.stop_requested=0
            AND i.state IN ('open','ready') AND i.assignee=r.actor_id AND i.deleted_at IS NULL AND p.hidden_at IS NULL",
            params![id,text,crate::issues::worker::now(),runs])?;
        let queued: i64 = tx.query_row("SELECT count(*) FROM agent_steering WHERE request_id IN
            (SELECT 'broadcast:'||?1||':'||value FROM json_each(?2)) AND scope='broadcast' AND text=?3 AND state IN ('queued','sending','delivered')",
            params![id,runs,text], |r|r.get(0))?;
        tx.commit()?;
        Ok(json!({"ok":true,"queued":queued}))
    }

    pub(crate) fn pending_broadcast(&self, run: &str) -> Result<Option<Value>> {
        pending(&self.db, run)
    }
    pub(crate) fn broadcast_result(
        &self,
        request: &str,
        state: &str,
        error: Option<&str>,
    ) -> Result<bool> {
        let changed = self.db.execute(
            "UPDATE agent_steering SET state=?2,error=?3 WHERE request_id=?1 AND scope='broadcast' AND (?2<>'sending' OR state='queued')",
            params![request, state, error],
        )?;
        Ok(changed != 0)
    }
}

fn pending(db: &Connection, run: &str) -> Result<Option<Value>> {
    Ok(db.query_row("SELECT request_id,text FROM agent_steering WHERE run_id=?1 AND state='queued' AND scope='broadcast' ORDER BY created_at,rowid LIMIT 1", [run], |r| Ok(json!({"request_id":r.get::<_,String>(0)?,"text":r.get::<_,String>(1)?}))).optional()?)
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn legacy_steering_migration_preserves_queued_messages_and_is_repeatable() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE worker_runs(id TEXT PRIMARY KEY);
            INSERT INTO worker_runs VALUES('run');
            CREATE TABLE agent_steering(request_id TEXT PRIMARY KEY,run_id TEXT NOT NULL REFERENCES worker_runs(id),scope TEXT NOT NULL,text TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'queued',error TEXT,created_at INTEGER NOT NULL);
            INSERT INTO agent_steering VALUES('message','run','issue','Keep this instruction','queued',NULL,123);").unwrap();
        migrate(&db).unwrap();
        migrate(&db).unwrap();
        let row = db.query_row("SELECT text,state,created_at,issue_body FROM agent_steering WHERE request_id='message'", [], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<String>>(3)?))).unwrap();
        assert_eq!(
            row,
            ("Keep this instruction".into(), "queued".into(), 123, None)
        );
        db.execute("INSERT INTO agent_steering(request_id,run_id,scope,text,created_at,issue_body) VALUES('new','run','issue','New instruction',124,'Saved issue body')", []).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT issue_body FROM agent_steering WHERE request_id='new'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "Saved issue body"
        );
        let store = Store {
            db,
            attachment_root: std::env::temp_dir(),
        };
        assert!(pending(&store.db, "run").unwrap().is_none());
    }
}
