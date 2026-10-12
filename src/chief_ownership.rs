//! Revoke legacy scheduler ownership during mixed-version upgrades.
use crate::{
    database::Connection,
    issues::{Error, Result},
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Assignment {
    pub project_id: String,
    pub node: String,
    pub worker_id: String,
    pub generation: i64,
    pub revoking: bool,
}

pub(crate) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('fleet_chief_ownership','chief_retired_insert','chief_retired_update')", [], |r| r.get::<_,i64>(0))? == 3 {
        return Ok(());
    }
    db.execute_batch("CREATE TABLE IF NOT EXISTS fleet_chief_ownership(project_id TEXT PRIMARY KEY,node TEXT NOT NULL,worker_id TEXT NOT NULL,generation INTEGER NOT NULL,revoking INTEGER NOT NULL DEFAULT 0);")?;
    db.execute_batch(
        "DROP TRIGGER IF EXISTS chief_owner_insert; DROP TRIGGER IF EXISTS chief_owner_update;",
    )?;
    for (name, operation) in [
        ("chief_retired_insert", "INSERT"),
        (
            "chief_retired_update",
            "UPDATE OF state,owner_pid,owner_start,pid,process_start,last_event,session_id",
        ),
    ] {
        // Fence every old launch path while binaries roll forward.
        db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS {name} BEFORE {operation} ON project_chiefs WHEN NEW.state='running' BEGIN SELECT RAISE(ABORT,'Chief has been retired; use Jobs'); END;"))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn allowed(db: &Connection, project: &str, worker: &str) -> Result<bool> {
    Ok(db.query_row("SELECT (SELECT role FROM fleet_meta WHERE id=1)='standalone' OR EXISTS(SELECT 1 FROM fleet_chief_ownership a JOIN fleet_meta m ON m.id=1 AND m.node=a.node WHERE a.project_id=?1 AND a.worker_id=?2 AND a.revoking=0)", params![project,worker], |r| r.get(0))?)
}

pub(crate) fn stop_unassigned(db: &Connection) -> Result<()> {
    // These rows are machine-local. Verify process identities before stopping
    // an old scheduler's child; never signal the issue worker or its agents.
    let children = db.query_collect("SELECT pid,process_start FROM project_chiefs WHERE state='running' AND pid IS NOT NULL AND process_start IS NOT NULL", [], |r| -> rusqlite::Result<_> { Ok((r.get::<_,u32>(0)?,r.get::<_,String>(1)?)) })?;
    for (pid, start) in children {
        crate::issues::worker::stop_group(pid, &start)?;
        if crate::agents::process_identity(pid).as_deref() != Some(&start) {
            db.execute("UPDATE project_chiefs SET state='stopped',queued=0,pid=NULL,process_start=NULL,owner_pid=NULL,owner_start=NULL,finished_at=coalesce(finished_at,?3) WHERE pid=?1 AND process_start=?2 AND state='running'", params![pid,start,crate::issues::worker::now()])?;
        }
    }
    Ok(())
}

pub(crate) fn read(db: &Connection) -> Result<Vec<Assignment>> {
    Ok(db.prepare("SELECT project_id,node,worker_id,generation,revoking FROM fleet_chief_ownership ORDER BY project_id")?.query_map([], |r| Ok(Assignment {project_id:r.get(0)?,node:r.get(1)?,worker_id:r.get(2)?,generation:r.get(3)?,revoking:r.get(4)?}))?.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn apply(db: &Connection, assignments: &[Assignment]) -> Result<()> {
    let mut projects = BTreeSet::new();
    for a in assignments {
        if a.project_id.is_empty()
            || a.node.is_empty()
            || a.worker_id.is_empty()
            || a.generation < 1
            || !projects.insert(&a.project_id)
        {
            return Err(Error::invalid("Invalid Chief assignment"));
        }
    }
    if projects.is_empty() {
        return Ok(());
    }
    // Every pull carries ownership, including unchanged projects. Read only
    // this batch's keys once within the caller's synchronization transaction.
    let existing = db
        .prepare(
            "SELECT a.project_id,a.node,a.worker_id,a.generation,a.revoking
         FROM json_each(?1) selected CROSS JOIN fleet_chief_ownership a
         WHERE a.project_id=selected.value",
        )?
        .query_map([serde_json::to_string(&projects)?], |r| {
            let project: String = r.get(0)?;
            Ok((
                project.clone(),
                Assignment {
                    project_id: project,
                    node: r.get(1)?,
                    worker_id: r.get(2)?,
                    generation: r.get(3)?,
                    revoking: r.get(4)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    for a in assignments {
        if let Some(old) = existing.get(&a.project_id) {
            if old.generation > a.generation {
                continue;
            }
            if old.generation == a.generation {
                if old != a {
                    return Err(Error::invalid("Conflicting Chief assignment generation"));
                }
                continue;
            }
        }
        db.execute("INSERT INTO fleet_chief_ownership VALUES(?1,?2,?3,?4,?5) ON CONFLICT(project_id) DO UPDATE SET node=excluded.node,worker_id=excluded.worker_id,generation=excluded.generation,revoking=excluded.revoking",params![a.project_id,a.node,a.worker_id,a.generation,a.revoking])?;
        if !a.revoking {
            db.execute("UPDATE project_chiefs SET next_at=0 WHERE project_id=?1 AND state='blocked' AND owner_pid IS NULL AND EXISTS(SELECT 1 FROM fleet_meta WHERE id=1 AND node=?2)", params![a.project_id,a.node])?;
        }
    }
    Ok(())
}

pub(crate) fn reconcile(db: &Connection) -> Result<()> {
    // Persist revocations so older companions stop their owned pass too.
    // Probe first: healthy heartbeats must not acquire the writer lock.
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM fleet_chief_ownership WHERE revoking=0)",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        db.execute(
            "UPDATE fleet_chief_ownership SET revoking=1,generation=generation+1 WHERE revoking=0",
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::process::CommandExt, process::Command};

    #[test]
    fn retired_ownership_only_revokes_and_unchanged_polls_are_read_only() {
        let root = std::env::temp_dir().join(format!(
            "hb-retired-owner-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("issues.db");
        drop(crate::issues::Store::open(&path).unwrap());
        let db = Connection::open(&path).unwrap();
        db.execute(
            "INSERT INTO fleet_chief_ownership VALUES('project','peer','worker',4,0)",
            [],
        )
        .unwrap();
        reconcile(&db).unwrap();
        let assignments = read(&db).unwrap();
        assert!(assignments[0].revoking);
        assert_eq!(assignments[0].generation, 5);
        db.execute_batch("CREATE TRIGGER no_more_writes BEFORE UPDATE ON fleet_chief_ownership BEGIN SELECT RAISE(ABORT,'unexpected write'); END;").unwrap();
        reconcile(&db).unwrap();
        assert_eq!(read(&db).unwrap(), assignments);
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retired_child_stops_without_terminating_the_issue_worker() {
        let root = std::env::temp_dir().join(format!(
            "hb-retired-child-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("issues.db");
        drop(crate::issues::Store::open(&path).unwrap());
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "DROP TRIGGER chief_retired_insert;
            INSERT INTO projects(id,name,next_number) VALUES('project','Project',1);",
        )
        .unwrap();
        let mut child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let mut unrelated = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let start = crate::agents::process_identity(child.id()).unwrap();
        db.execute("INSERT INTO project_chiefs(project_id,machine,cwd,state,pid,process_start) VALUES('project','local','','running',?1,?2)", params![child.id(),start]).unwrap();
        let stopped = stop_unassigned(&db);
        // Always reap both test processes, including on an assertion failure.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let exited = child.try_wait().unwrap().is_some();
        let unaffected = unrelated.try_wait().unwrap().is_none();
        let _ = child.kill();
        let _ = child.wait();
        let _ = unrelated.kill();
        let _ = unrelated.wait();
        drop(db);
        fs::remove_dir_all(root).unwrap();
        stopped.unwrap();
        assert!(exited);
        assert!(unaffected);
    }
}
