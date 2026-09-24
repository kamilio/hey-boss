//! Supervisor-owned Chief assignments. Revoke, acknowledge, then hand off.
use crate::{
    database::Connection,
    issues::{Error, Result},
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    if db.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('fleet_chief_ownership','chief_owner_insert','chief_owner_update')", [], |r| r.get::<_,i64>(0))? == 3 {
        return Ok(());
    }
    db.execute_batch("CREATE TABLE IF NOT EXISTS fleet_chief_ownership(project_id TEXT PRIMARY KEY,node TEXT NOT NULL,worker_id TEXT NOT NULL,generation INTEGER NOT NULL,revoking INTEGER NOT NULL DEFAULT 0);")?;
    for (name, operation) in [
        ("chief_owner_insert", "INSERT"),
        (
            "chief_owner_update",
            "UPDATE OF state,owner_pid,owner_start,pid,process_start,last_event,session_id",
        ),
    ] {
        // Enforce ownership for older workers too, while binaries roll forward.
        db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS {name} BEFORE {operation} ON project_chiefs WHEN NEW.state='running' AND (SELECT role FROM fleet_meta WHERE id=1)<>'standalone' AND NOT EXISTS(SELECT 1 FROM fleet_chief_ownership a JOIN fleet_meta m ON m.id=1 AND m.node=a.node WHERE a.project_id=NEW.project_id AND a.worker_id=NEW.worker_id AND a.revoking=0) BEGIN SELECT RAISE(ABORT,'Chief is not assigned to this worker by the supervisor'); END;"))?;
    }
    Ok(())
}

pub(crate) fn allowed(db: &Connection, project: &str, worker: &str) -> Result<bool> {
    Ok(db.query_row("SELECT (SELECT role FROM fleet_meta WHERE id=1)='standalone' OR EXISTS(SELECT 1 FROM fleet_chief_ownership a JOIN fleet_meta m ON m.id=1 AND m.node=a.node WHERE a.project_id=?1 AND a.worker_id=?2 AND a.revoking=0)", params![project,worker], |r| r.get(0))?)
}

pub(crate) fn stop_unassigned(db: &Connection) -> Result<()> {
    // A companion enforces revocation even when its worker has not reloaded
    // yet. Retain the reservation until the owning worker records completion.
    let children=db.prepare("SELECT c.pid,c.process_start FROM project_chiefs c WHERE c.state='running' AND c.pid IS NOT NULL AND c.process_start IS NOT NULL AND (SELECT role FROM fleet_meta WHERE id=1)<>'standalone' AND NOT EXISTS(SELECT 1 FROM fleet_chief_ownership a JOIN fleet_meta m ON m.id=1 AND m.node=a.node WHERE a.project_id=c.project_id AND a.worker_id=c.worker_id AND a.revoking=0)")?.query_map([], |r| Ok((r.get::<_,u32>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (pid, start) in children {
        crate::issues::worker::stop_group(pid, &start)?;
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
        let existing: Option<(String,String,i64,bool)> = db.query_row("SELECT node,worker_id,generation,revoking FROM fleet_chief_ownership WHERE project_id=?1", [&a.project_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        if let Some((node, worker, generation, revoking)) = existing {
            if generation > a.generation {
                continue;
            }
            if generation == a.generation {
                if (node, worker, revoking) != (a.node.clone(), a.worker_id.clone(), a.revoking) {
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

fn running(worker: &Value, project: &str) -> bool {
    worker["chiefs"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|c| c["project_id"] == project && c["state"] == "running")
}
fn eligible(machine: &Value, worker: &Value, project: &str) -> bool {
    machine["state"] == "connected"
        && machine["chief_ownership"].is_array()
        && worker["pid"].as_u64().is_some_and(|p| p > 0)
        && worker["config"]["enabled"] == true
        && worker["stop_requested"] != true
        && worker["upgrading"] != true
        && worker["chief_projects"]
            .as_array()
            .is_some_and(|p| p.iter().any(|id| id == project))
        && worker["config"]["projects"]
            .as_array()
            .is_some_and(|p| p.is_empty() || p.iter().any(|id| id == project))
}
fn choose(project: &str, machines: &[Value]) -> Option<(String, String)> {
    let mut candidates = Vec::new();
    for m in machines {
        for w in m["workers"].as_array().into_iter().flatten() {
            if eligible(m, w, project)
                && let (Some(node), Some(worker)) = (m["node"].as_str(), w["id"].as_str())
            {
                candidates.push((!running(w, project), node.to_owned(), worker.to_owned()));
            }
        }
    }
    candidates.sort();
    candidates
        .into_iter()
        .next()
        .map(|(_, node, worker)| (node, worker))
}

fn desired(
    project: &str,
    enabled: bool,
    previous: Option<&Assignment>,
    machines: &[Value],
) -> Option<Assignment> {
    if let Some(old) = previous {
        let owner = machines
            .iter()
            .find(|m| m["node"] == old.node && m["state"] == "connected");
        // A network partition is not proof that the old process stopped.
        let Some(owner) = owner else {
            return Some(old.clone());
        };
        if !old.revoking {
            if enabled
                && owner["workers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|w| w["id"] == old.worker_id && eligible(owner, w, project))
            {
                return Some(old.clone());
            }
            return Some(Assignment {
                revoking: true,
                generation: old.generation + 1,
                ..old.clone()
            });
        }
        let acknowledged = owner["chief_ownership"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|a| {
                serde_json::from_value::<Assignment>(a.clone())
                    .ok()
                    .as_ref()
                    == Some(old)
            });
        let active = owner["workers"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|w| running(w, project));
        if !enabled || !acknowledged || active {
            return Some(old.clone());
        }
    } else if !enabled
        || machines.iter().any(|m| {
            !m["chief_ownership"].is_array()
                && m["workers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|w| running(w, project))
        })
    {
        return None;
    }
    choose(project, machines)
        .map(|(node, worker_id)| Assignment {
            project_id: project.into(),
            node,
            worker_id,
            generation: previous.map_or(1, |a| a.generation + 1),
            revoking: false,
        })
        .or_else(|| previous.cloned())
}

pub(crate) fn reconcile(db: &Connection, machines: &[Value]) -> Result<()> {
    let old = read(db)?;
    let mut projects: BTreeMap<String,bool> = db.prepare("SELECT p.id,coalesce(s.chief_enabled,0)=1 AND p.hidden_at IS NULL FROM projects p LEFT JOIN project_settings s ON s.project_id=p.id")?.query_map([], |r| Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    for a in &old {
        projects.entry(a.project_id.clone()).or_insert(false);
    }
    let next = projects
        .into_iter()
        .filter_map(|(p, enabled)| {
            desired(
                &p,
                enabled,
                old.iter().find(|a| a.project_id == p),
                machines,
            )
        })
        .collect::<Vec<_>>();
    if next == old {
        return Ok(());
    }
    let tx = crate::database::Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    if read(&tx)? == old {
        apply(&tx, &next)?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn machine(node: &str) -> Value {
        json!({"node":node,"state":"connected","chief_ownership":[],"workers":[{"id":format!("worker-{node}"),"pid":123,"config":{"enabled":true,"projects":["project"]},"chief_projects":["project"],"chiefs":[]}]})
    }
    #[test]
    fn supervisor_selects_one_and_retains_it_across_reconnects() {
        let mut machines = vec![machine("b"), machine("a")];
        let owner = desired("project", true, None, &machines).unwrap();
        assert_eq!(owner.node, "a");
        machines.reverse();
        assert_eq!(
            desired("project", true, Some(&owner), &machines),
            Some(owner.clone())
        );
        machines[0]["state"] = json!("disconnected");
        assert_eq!(
            desired("project", true, Some(&owner), &machines),
            Some(owner)
        );
    }
    #[test]
    fn handoff_requires_revocation_acknowledgment_and_stopped_chief() {
        let mut machines = vec![machine("a"), machine("b")];
        let owner = desired("project", true, None, &machines).unwrap();
        machines[0]["workers"][0]["config"]["enabled"] = json!(false);
        let revoked = desired("project", true, Some(&owner), &machines).unwrap();
        assert!(revoked.revoking);
        assert_eq!(
            desired("project", true, Some(&revoked), &machines),
            Some(revoked.clone())
        );
        machines[0]["chief_ownership"] = json!([revoked]);
        machines[0]["workers"][0]["chiefs"] = json!([{"project_id":"project","state":"running"}]);
        assert_eq!(
            desired("project", true, Some(&revoked), &machines),
            Some(revoked.clone())
        );
        machines[0]["workers"][0]["chiefs"] = json!([]);
        let replacement = desired("project", true, Some(&revoked), &machines).unwrap();
        assert_eq!(replacement.node, "b");
        assert!(!replacement.revoking);
        assert!(replacement.generation > revoked.generation);
    }
    #[test]
    fn replicas_fence_other_workers_and_ignore_stale_assignments() {
        let root = std::env::temp_dir().join(format!(
            "hb-chief-owner-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let a_path = root.join("a.db");
        let b_path = root.join("b.db");
        drop(crate::issues::Store::open(&a_path).unwrap());
        drop(crate::issues::Store::open(&b_path).unwrap());
        let a = Connection::open(&a_path).unwrap();
        let b = Connection::open(&b_path).unwrap();
        a.execute("UPDATE fleet_meta SET role='agent',node='a'", [])
            .unwrap();
        b.execute("UPDATE fleet_meta SET role='agent',node='b'", [])
            .unwrap();
        let first = Assignment {
            project_id: "project".into(),
            node: "a".into(),
            worker_id: "selected".into(),
            generation: 1,
            revoking: false,
        };
        apply(&a, std::slice::from_ref(&first)).unwrap();
        apply(&b, std::slice::from_ref(&first)).unwrap();
        assert!(allowed(&a, "project", "selected").unwrap());
        assert!(!allowed(&a, "project", "other").unwrap());
        assert!(!allowed(&b, "project", "selected").unwrap());
        let revoked = Assignment {
            generation: 2,
            revoking: true,
            ..first.clone()
        };
        apply(&a, std::slice::from_ref(&revoked)).unwrap();
        apply(&a, std::slice::from_ref(&first)).unwrap();
        assert!(!allowed(&a, "project", "selected").unwrap());
        assert!(
            apply(
                &a,
                &[Assignment {
                    revoking: false,
                    ..revoked
                }]
            )
            .is_err()
        );
        let next = Assignment {
            node: "b".into(),
            generation: 3,
            ..first
        };
        apply(&a, std::slice::from_ref(&next)).unwrap();
        apply(&b, std::slice::from_ref(&next)).unwrap();
        assert!(!allowed(&a, "project", "selected").unwrap());
        assert!(allowed(&b, "project", "selected").unwrap());
        drop(a);
        drop(b);
        std::fs::remove_dir_all(root).unwrap();
    }
}
