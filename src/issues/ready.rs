//! Ready is a metadata handoff, never a claim or a takeover. All reads and the
//! eventual issue/event/dependency writes share the store's immediate transaction.
use super::*;
use crate::issues::ReadyGuard;
use sha2::{Digest, Sha256};

pub(super) fn snapshot(db: &Connection, project: &str, issue: &Issue) -> Result<ReadyGuard> {
    let allocation: Option<(String, Option<i64>)> = db.query_row(
        "SELECT a.node,d.expires_at FROM fleet_allocations a LEFT JOIN fleet_allocation_deadlines d USING(project_id,issue_number) WHERE a.project_id=?1 AND a.issue_number=?2",
        params![project, issue.number], |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional()?;
    let mut stmt = db.prepare("SELECT id,actor_id,machine,state,claimed_at,reservation_expires FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL ORDER BY id")?;
    let runs = stmt
        .query_map(params![project, issue.number], |r| {
            Ok(json!([
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<i64>>(5)?
            ]))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ReadyGuard {
        if_version: issue.version,
        expected_assignee: issue.assignee.clone(),
        expected_reservation: format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(allocation, runs))?)
        ),
    })
}

pub(super) fn register_boss(db: &Connection, actor: &Actor, now: i64) -> Result<()> {
    let mut boss = actor.clone();
    boss.id = "human:boss".into();
    boss.kind = "human".into();
    boss.session_id = None;
    boss.pid = None;
    boss.process_start = None;
    boss.source = "Boss assignment".into();
    db.execute(
        "INSERT INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3) ON CONFLICT(id) DO NOTHING",
        params![boss.id, serde_json::to_string(&boss)?, now],
    )?;
    Ok(())
}

pub(super) fn handoff(
    db: &Connection,
    project: &Project,
    issue: &mut Issue,
    actor: &Actor,
    guard: Option<&ReadyGuard>,
    clear_manual_hold: bool,
    now: i64,
) -> Result<Option<Value>> {
    if let Some(expected) = guard {
        let current = snapshot(db, &project.id, issue)?;
        if expected.if_version != current.if_version {
            return Err(Error::conflict(format!(
                "Ready version guard mismatch: expected {}, current {}",
                expected.if_version, current.if_version
            )));
        }
        if expected.expected_assignee != current.expected_assignee {
            return Err(Error::conflict(format!(
                "Ready assignee guard mismatch: current {}",
                current.expected_assignee.as_deref().unwrap_or("unassigned")
            )));
        }
        if expected.expected_reservation != current.expected_reservation {
            return Err(Error::conflict(
                "Ready reservation guard mismatch: allocation or worker claim changed; refresh issue view",
            ));
        }
    }
    // Even a matching snapshot or --force cannot release a foreign live attempt.
    // Include expired, unclaimed attempts: expiry is not evidence of safe release.
    let foreign: Option<String> = db.query_row("SELECT id FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL AND (actor_id<>?3 OR claimed_at IS NULL) LIMIT 1", params![project.id,issue.number,actor.id], |r| r.get(0)).optional()?;
    if let Some(run) = foreign {
        return Err(Error::conflict(format!(
            "Ready blocked by unfinished or unclaimed worker attempt {run}; claim and reservation preserved"
        )));
    }
    let allocation: Option<String> = db.query_row("SELECT node FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2 AND node<>?3", params![project.id,issue.number,actor.machine], |r| r.get(0)).optional()?;
    if let Some(machine) = allocation {
        return Err(Error::conflict(format!(
            "Ready blocked by fleet reservation for {machine}; reservation preserved"
        )));
    }
    if issue.draft {
        return Err(Error::conflict("Ready requires an undrafted issue"));
    }
    if issue.manual_blocked && (!clear_manual_hold || guard.is_none()) {
        return Err(Error::conflict(
            "Ready blocked by manual hold; use --clear-manual-hold with version, assignee and reservation guards",
        ));
    }
    if !(matches!(issue.state.as_str(), "open" | "ready")
        || issue.state == "blocked" && issue.manual_blocked && clear_manual_hold)
    {
        return Err(Error::conflict(format!(
            "Ready blocked by issue state {}; reopen closed issues or resolve dependencies",
            issue.state
        )));
    }
    if clear_manual_hold && !issue.manual_blocked {
        return Err(Error::conflict(
            "Ready manual hold guard mismatch: issue has no manual hold",
        ));
    }
    if super::super::blockers::has_dependencies(db, &project.id, issue.number)? {
        return Err(Error::conflict(
            "Ready blocked by unfinished dependencies; dependency links and ownership preserved",
        ));
    }
    if registry::project_settings(db, project)?["prs_enabled"] != true {
        return Err(Error::conflict(
            "Ready requires pull requests enabled for this project",
        ));
    }
    let attached: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM issue_pull_requests WHERE project_id=?1 AND issue_number=?2 AND purpose IN ('fix','unspecified'))", params![project.id,issue.number], |r| r.get(0))?;
    if !attached {
        return Err(Error::conflict(
            "Attach the task's PR before marking it Ready",
        ));
    }
    let own_handoff = issue.assignee.as_deref() == Some("human:boss") && db.query_row(
        "SELECT coalesce((SELECT actor=?3 AND json_extract(data,'$.previous_assignee')=?3 FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('claimed','ready','unassigned','closed','reopened') ORDER BY id DESC LIMIT 1),0)",
        params![project.id,issue.number,actor.id], |r| r.get::<_,bool>(0),
    )?;
    if guard.is_none()
        && !own_handoff
        && !(issue.state == "ready" && issue.assignee.as_deref() == Some("human:boss"))
        && let Some(owner) = &issue.assignee
        && owner != &actor.id
    {
        return Err(Error::conflict(format!(
            "Ready assignee guard required for manual owner {owner}; read issue view and provide the exact version, assignee and reservation guards"
        )));
    }
    if issue.state == "ready" && issue.assignee.as_deref() == Some("human:boss") {
        return Ok(None);
    }
    register_boss(db, actor, now)?;
    let data = json!({"previous_assignee":if own_handoff {Some(actor.id.clone())} else {issue.assignee.clone()},"assignee":"human:boss","previous_state":issue.state,"cleared_manual_hold":issue.manual_blocked,"guard":guard});
    issue.state = "ready".into();
    issue.assignee = Some("human:boss".into());
    issue.manual_blocked = false;
    Ok(Some(data))
}
