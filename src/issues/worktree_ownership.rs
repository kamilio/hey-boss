//! Declared checkout ownership can differ from an agent's process directory.
use super::{Actor, Error, Result, Store};
use crate::database::Connection;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn read(db: &Connection, home: Option<&Path>) -> Result<BTreeSet<PathBuf>> {
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
    let retained = tx
        .prepare("SELECT attempt_hold FROM issues WHERE attempt_hold IS NOT NULL LIMIT 10001")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // Unassigned fleet allocations schedule future work, not a checkout. Actual
    // pre-claim reservations register an actor and worker_run in one transaction.
    // A known assignee's metadata is validated below and owns the checkout.
    // The issue author's older origin may be absent or belong to another machine.
    // Missing assignee records still require the reservation's checkout hint.
    let reserved = tx
        .prepare(
            "SELECT i.origin FROM fleet_allocations a JOIN issues i
             ON i.project_id=a.project_id AND i.number=a.issue_number
             WHERE a.node=?1 AND i.state IN ('open','ready') AND i.deleted_at IS NULL
             AND i.assignee IS NOT NULL
             AND NOT EXISTS (SELECT 1 FROM agents owner WHERE owner.id=i.assignee)
             LIMIT 10001",
        )?
        .query_map([&machine], |r| r.get::<_, Option<String>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let queued = tx.prepare("SELECT a.metadata FROM worker_runs r LEFT JOIN agents a ON a.id=r.actor_id WHERE r.machine=?1 AND r.finished_at IS NULL LIMIT 10001")?
        .query_map([&machine], |r| r.get::<_, Option<String>>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    tx.commit()?;
    if actors.len() > 10000 {
        return Err(Error::invalid(
            "Issue ownership inventory exceeds its limit",
        ));
    }
    let mut roots = BTreeSet::new();
    if queued.len() > 10000 {
        return Err(Error::invalid(
            "Queued ownership inventory exceeds its limit",
        ));
    }
    let queued = queued
        .into_iter()
        .map(|raw| {
            raw.ok_or_else(|| Error::invalid("Queued work has unknown owner; cleanup preserved"))
        })
        .collect::<Result<Vec<_>>>()?;
    for raw in actors.into_iter().chain(queued) {
        let actor: Actor = serde_json::from_str(&raw)?;
        // A missing process is not an owner release: validation may be queued,
        // detached, or retained for a later session.
        if actor.machine != machine {
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
    if retained.len() > 10000 || reserved.len() > 10000 {
        return Err(Error::invalid(
            "Cleanup ownership inventory exceeds its limit",
        ));
    }
    for raw in retained {
        let hold: serde_json::Value = serde_json::from_str(&raw)?;
        let owner_machine = hold["machine"]
            .as_str()
            .ok_or_else(|| Error::invalid("Retained work has unknown machine"))?;
        if owner_machine == machine {
            let path = hold["worktree"]
                .as_str()
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .ok_or_else(|| Error::invalid("Retained work has unknown checkout"))?;
            roots.insert(path);
        }
    }
    for raw in reserved {
        let origin: serde_json::Value = serde_json::from_str(&raw.ok_or_else(|| {
            Error::invalid("Reserved work has unknown checkout; cleanup preserved")
        })?)?;
        if origin["machine"].as_str() != Some(machine.as_str()) {
            return Err(Error::invalid(
                "Reserved work has ambiguous checkout on this machine; cleanup preserved",
            ));
        }
        let path = origin["cwd"]
            .as_str()
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or_else(|| {
                Error::invalid("Reserved work has unknown checkout; cleanup preserved")
            })?;
        roots.insert(path);
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
    read(&db, home.as_deref())
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
            CREATE TABLE issues(assignee TEXT, state TEXT, deleted_at INTEGER, attempt_hold TEXT, project_id TEXT, number INTEGER, origin TEXT);
            CREATE TABLE fleet_allocations(project_id TEXT, issue_number INTEGER, node TEXT);
            CREATE TABLE worker_runs(actor_id TEXT, machine TEXT, finished_at INTEGER);",
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
            "INSERT INTO issues(assignee,state) VALUES(?1,?2)",
            params![name, state],
        )
        .unwrap();
    }

    #[test]
    fn shared_app_pid_and_stale_presence_do_not_release_claimed_checkouts() {
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
            read(&db, None).unwrap(),
            BTreeSet::from([
                PathBuf::from("/declared/active"),
                PathBuf::from("/declared/reused-pid")
            ])
        );
    }

    #[test]
    fn queued_reservations_and_retained_attempts_survive_without_processes() {
        let db = fixture();
        claim(&db, "queued", "local", "open", "unknown");
        db.execute_batch("UPDATE issues SET assignee=NULL;
            INSERT INTO issues(state,attempt_hold) VALUES('closed','{\"machine\":\"local\",\"worktree\":\"/retained/old\"}');
            INSERT INTO worker_runs VALUES('queued','local',NULL);").unwrap();
        assert_eq!(
            read(&db, None).unwrap(),
            BTreeSet::from([
                PathBuf::from("/retained/old"),
                PathBuf::from("/declared/queued")
            ])
        );
        db.execute("DELETE FROM agents WHERE id='queued'", [])
            .unwrap();
        assert!(read(&db, None).is_err());
    }

    #[test]
    fn unstarted_machine_allocations_do_not_reserve_author_checkouts() {
        let db = fixture();
        for (number, origin) in [
            (1, None),
            (2, Some(r#"{"machine":"foreign","cwd":"/author/work"}"#)),
            (3, Some(r#"{"machine":"local","cwd":"/author/local"}"#)),
        ] {
            db.execute(
                "INSERT INTO issues(state,project_id,number,origin) VALUES('open','project',?1,?2)",
                params![number, origin],
            )
            .unwrap();
            db.execute(
                "INSERT INTO fleet_allocations VALUES('project',?1,'local')",
                [number],
            )
            .unwrap();
        }
        assert!(read(&db, None).unwrap().is_empty());
        // A worker reservation remains authoritative even before issue claim.
        db.execute(
            "INSERT INTO worker_runs VALUES('missing-owner','local',NULL)",
            [],
        )
        .unwrap();
        assert!(read(&db, None).is_err());
    }

    #[test]
    fn unresolved_presence_and_ready_owned_work_are_conservatively_preserved() {
        let db = fixture();
        claim(&db, "pending", "local", "ready", "unknown");
        assert_eq!(
            read(&db, None).unwrap(),
            BTreeSet::from([PathBuf::from("/declared/pending")])
        );
        db.execute("UPDATE issues SET assignee=NULL", []).unwrap();
        assert!(read(&db, None).unwrap().is_empty());
    }

    #[test]
    fn known_assignee_protects_checkout_without_a_valid_allocation_origin() {
        let db = fixture();
        claim(&db, "owner", "local", "open", "unknown");
        db.execute_batch(
            "UPDATE issues SET project_id='project',number=1;
            INSERT INTO fleet_allocations VALUES('project',1,'local');",
        )
        .unwrap();
        let expected = BTreeSet::from([PathBuf::from("/declared/owner")]);
        assert_eq!(read(&db, None).unwrap(), expected);
        db.execute(
            "UPDATE issues SET origin=?1",
            [r#"{"machine":"foreign","cwd":"/author/checkout"}"#],
        )
        .unwrap();
        assert_eq!(read(&db, None).unwrap(), expected);

        db.execute(
            "UPDATE agents SET metadata=json_set(metadata,'$.cwd','relative')",
            [],
        )
        .unwrap();
        assert!(read(&db, None).is_err());
    }

    #[test]
    fn missing_assignee_metadata_does_not_discard_unresolved_reservation() {
        let db = fixture();
        db.execute_batch(
            "INSERT INTO issues(assignee,state,project_id,number)
            VALUES('missing-owner','open','project',1);
            INSERT INTO fleet_allocations VALUES('project',1,'local');",
        )
        .unwrap();
        assert!(read(&db, None).is_err());
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
        assert!(read(&db, None).is_err());
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
        db.execute_batch(
            "UPDATE issues SET project_id='project',number=1 WHERE assignee='boss';
            INSERT INTO fleet_allocations VALUES('project',1,'local');",
        )
        .unwrap();
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
        let ambient = read(&db, Some(&home)).unwrap();
        std::fs::create_dir(home.join(".git")).unwrap();
        let empty_marker = read(&db, Some(&home)).unwrap();
        std::fs::write(home.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let actual_checkout = read(&db, Some(&home)).unwrap();
        std::fs::remove_dir_all(&home).unwrap();
        assert_eq!(ambient, BTreeSet::from([project.clone()]));
        assert_eq!(empty_marker, BTreeSet::from([project.clone()]));
        assert_eq!(actual_checkout, BTreeSet::from([home, project]));
    }
}
