//! Declared checkout ownership can differ from an agent's process directory.
use super::{Actor, Error, Result, Store};
use crate::database::Connection;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn read(
    db: &Connection,
    presence: impl Fn(&Actor, &str) -> &'static str,
    home: Option<&Path>,
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
        // Human/UI assignments can inherit HOME as their default cwd. That is
        // not a reservation of every project and cache below the home directory.
        if home == Some(actor.cwd.as_path()) && !checkout_marker(&actor.cwd)? {
            continue;
        }
        // Unknown presence includes queued or temporarily uninspectable owners.
        roots.insert(actor.cwd);
    }
    Ok(roots)
}

fn checkout_marker(path: &Path) -> Result<bool> {
    let marker = path.join(".git");
    match std::fs::symlink_metadata(&marker) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
        Ok(m) if m.is_dir() => Ok(std::fs::read_dir(marker)?.next().transpose()?.is_some()),
        // Retain linked checkouts and unresolved/symlinked ownership markers.
        Ok(_) => Ok(true),
    }
}

pub fn declared_worktrees() -> Result<BTreeSet<PathBuf>> {
    let path = super::database_path()?;
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(e.into()),
        Ok(_) => {}
    }
    let db = Store::open_read_connection(&path)?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    read(&db, super::identity::presence, home.as_deref())
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
            read(&db, presence, None).unwrap(),
            BTreeSet::from([PathBuf::from("/declared/active")])
        );
    }

    #[test]
    fn unresolved_presence_and_ready_owned_work_are_conservatively_preserved() {
        let db = fixture();
        claim(&db, "pending", "local", "ready", "unknown");
        assert_eq!(
            read(&db, presence, None).unwrap(),
            BTreeSet::from([PathBuf::from("/declared/pending")])
        );
        db.execute("UPDATE issues SET assignee=NULL", []).unwrap();
        assert!(read(&db, presence, None).unwrap().is_empty());
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
        assert!(read(&db, presence, None).is_err());
    }

    #[test]
    fn default_home_cwd_does_not_reserve_every_project_but_real_home_checkout_does() {
        let home = std::env::temp_dir().join(format!("hb-owned-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let home = home.canonicalize().unwrap();
        let project = home.join("project");
        let db = fixture();
        claim(&db, "boss", "local", "ready", "unknown");
        claim(&db, "unbound-agent", "local", "open", "unknown");
        claim(&db, "queued-project", "local", "open", "unknown");
        db.execute(
            "UPDATE agents SET metadata=json_set(metadata,'$.cwd',?1) WHERE id IN ('boss','unbound-agent')",
            [home.to_str().unwrap()],
        ).unwrap();
        db.execute(
            "UPDATE agents SET metadata=json_set(metadata,'$.kind','explicit') WHERE id='boss'",
            [],
        )
        .unwrap();
        db.execute(
            "UPDATE agents SET metadata=json_set(metadata,'$.cwd',?1) WHERE id='queued-project'",
            [project.to_str().unwrap()],
        )
        .unwrap();
        let ambient = read(&db, presence, Some(&home)).unwrap();
        std::fs::create_dir(home.join(".git")).unwrap();
        let empty_marker = read(&db, presence, Some(&home)).unwrap();
        std::fs::write(home.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let actual_checkout = read(&db, presence, Some(&home)).unwrap();
        std::fs::remove_dir_all(&home).unwrap();
        assert_eq!(ambient, BTreeSet::from([project.clone()]));
        assert_eq!(empty_marker, BTreeSet::from([project.clone()]));
        assert_eq!(actual_checkout, BTreeSet::from([home, project]));
    }
}
