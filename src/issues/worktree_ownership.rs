//! Declared checkout ownership can differ from an agent's process directory.
use super::{Actor, Error, Result, Store};
use crate::database::Connection;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn read(
    db: &Connection,
    presence: impl Fn(&Actor, &str) -> &'static str,
) -> Result<BTreeSet<PathBuf>> {
    let tx = db.read_transaction()?;
    let machine: String =
        tx.query_row("SELECT node FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    let actors = tx
        .prepare(
            "SELECT DISTINCT a.metadata FROM agents a JOIN issues i ON i.assignee=a.id
        WHERE i.state IN ('open','ready') AND i.deleted_at IS NULL LIMIT 10001",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    tx.commit()?;
    if actors.len() > 10000 {
        return Err(Error::invalid(
            "Issue ownership inventory exceeds its limit",
        ));
    }
    let mut roots = BTreeSet::new();
    for raw in actors {
        let actor: Actor = serde_json::from_str(&raw)?;
        if actor.machine != machine || presence(&actor, &machine) == "stale" {
            continue;
        }
        if !actor.cwd.is_absolute() {
            return Err(Error::invalid("Declared issue worktree must be absolute"));
        }
        // Unknown presence includes queued or temporarily uninspectable owners.
        roots.insert(actor.cwd);
    }
    Ok(roots)
}

pub fn declared_worktrees() -> Result<BTreeSet<PathBuf>> {
    let path = super::database_path()?;
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(e.into()),
        Ok(_) => {}
    }
    let db = Store::open_read_connection(&path)?;
    read(&db, super::identity::presence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn fixture() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE fleet_meta(id INTEGER, node TEXT);
            INSERT INTO fleet_meta VALUES(1,'local');
            CREATE TABLE agents(id TEXT PRIMARY KEY, metadata TEXT);
            CREATE TABLE issues(assignee TEXT, state TEXT, deleted_at INTEGER);",
        )
        .unwrap();
        db
    }

    fn claim(db: &Connection, name: &str, machine: &str, state: &str, presence: &str) {
        let actor = serde_json::json!({"id":name,"kind":"codex","session_id":name,
            "machine":machine,"host":"fixture","pid":100,"process_start":presence,
            "cwd":format!("/declared/{name}"),"source":"fixture"});
        db.execute(
            "INSERT INTO agents VALUES(?1,?2)",
            params![name, actor.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO issues VALUES(?1,?2,NULL)",
            params![name, state],
        )
        .unwrap();
    }

    fn presence(actor: &Actor, machine: &str) -> &'static str {
        assert_eq!(machine, "local");
        match actor.process_start.as_deref() {
            Some("running") => "running",
            Some("stale") => "stale",
            _ => "unknown",
        }
    }

    #[test]
    fn declared_checkout_survives_different_process_cwd_and_filters_stale_claims() {
        let db = fixture();
        claim(&db, "active", "local", "open", "running");
        claim(&db, "reused-pid", "local", "open", "stale");
        claim(&db, "foreign", "remote", "open", "running");
        claim(&db, "finished", "local", "closed", "running");
        claim(&db, "deleted", "local", "open", "running");
        db.execute(
            "UPDATE issues SET deleted_at=1 WHERE assignee='deleted'",
            [],
        )
        .unwrap();
        assert_eq!(
            read(&db, presence).unwrap(),
            BTreeSet::from([PathBuf::from("/declared/active")])
        );
    }

    #[test]
    fn unresolved_presence_and_ready_owned_work_are_conservatively_preserved() {
        let db = fixture();
        claim(&db, "pending", "local", "ready", "unknown");
        assert_eq!(
            read(&db, presence).unwrap(),
            BTreeSet::from([PathBuf::from("/declared/pending")])
        );
        db.execute("UPDATE issues SET assignee=NULL", []).unwrap();
        assert!(read(&db, presence).unwrap().is_empty());
    }

    #[test]
    fn invalid_ownership_is_an_error_not_an_empty_success() {
        let db = fixture();
        claim(&db, "active", "local", "open", "running");
        db.execute(
            "UPDATE agents SET metadata=json_set(metadata,'$.cwd','relative')",
            [],
        )
        .unwrap();
        assert!(read(&db, presence).is_err());
    }
}
