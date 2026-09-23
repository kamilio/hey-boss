//! Durable, deduplicated launch history, shared by fleet replicas.
use super::*;

const SCHEMA: &str = "
CREATE TABLE issue_agent_launches(
 run_id TEXT NOT NULL,project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,
 launched_at INTEGER NOT NULL,
 PRIMARY KEY(project_id,issue_number,run_id),
 FOREIGN KEY(project_id,issue_number) REFERENCES issues(project_id,number));
";

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='issue_agent_launches' AND type='table')", [], |r| r.get::<_, bool>(0))? {
        return Ok(());
    }
    let tx = crate::database::Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    // Recheck after taking the lock: simultaneous startups may migrate together.
    if !tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='issue_agent_launches' AND type='table')", [], |r| r.get::<_, bool>(0))? {
        tx.execute_batch(SCHEMA)?;
        tx.execute_batch("INSERT INTO issue_agent_launches SELECT id,project_id,issue_number,started_at FROM worker_runs WHERE pid IS NOT NULL;
        INSERT INTO fleet_outbox(table_name,before_json,after_json,created_at)
        SELECT 'issue_agent_launches',NULL,json_object('run_id',run_id,'project_id',project_id,'issue_number',issue_number,'launched_at',launched_at),launched_at FROM issue_agent_launches
        WHERE (SELECT role FROM fleet_meta WHERE id=1)<>'standalone';")?;
    }
    tx.commit()?;
    Ok(())
}
