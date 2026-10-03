//! Requirements acknowledgement is explicit, attempt-scoped, and independent
//! of GitHub evidence. Event metadata travels through the existing fleet journal.
use super::*;

pub(in crate::issues::store) struct Acknowledgement {
    pub snapshot: Value,
    pub valid: bool,
}

fn digest(issue: &Issue) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            &issue.title,
            &issue.body,
            &issue.labels,
        ))?)
    ))
}

pub(in crate::issues::store) fn capture(
    db: &Connection,
    project: &str,
    issue: &Issue,
    actor: &Actor,
) -> Result<Value> {
    let run =
        assignments::live_issue_run(db, project, issue.number, &actor.id)?.ok_or_else(|| {
            Error::conflict("Acknowledging requirements requires an active owning worker run")
        })?;
    let delegated = issue.state == "ready" && issue.assignee.as_deref() == Some("human:boss")
        && db.query_row("SELECT coalesce((SELECT action='ready' AND actor=?3 AND json_extract(data,'$.previous_assignee')=?3 AND json_extract(data,'$.requirements_handoff.run')=?4 FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('assigned','claimed','ready','unassigned','closed','reopened','blocked','deleted','restored') ORDER BY id DESC LIMIT 1),0)", params![project,issue.number,actor.id,run], |r| r.get::<_,bool>(0))?;
    if issue.assignee.as_deref() != Some(&actor.id) && !delegated {
        return Err(Error::conflict(
            "Only the owning worker run can acknowledge requirements",
        ));
    }
    Ok(json!({"run":run,"version":issue.version+1,"sha256":digest(issue)?}))
}

// Consume only the latest lifecycle event, at its exact committed version.
// Comments and unrelated metadata edits also require a fresh acknowledgement.
pub(in crate::issues::store) fn current(
    db: &Connection,
    project: &str,
    issue: &Issue,
    actor: &str,
    run: &str,
) -> Result<Option<Acknowledgement>> {
    let event: Option<(String, String, String)> = db.query_row(
        "SELECT actor,action,data FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('assigned','claimed','ready','unassigned','closed','reopened','blocked','deleted','restored') ORDER BY id DESC LIMIT 1",
        params![project,issue.number], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?;
    let Some((author, action, data)) = event else {
        return Ok(None);
    };
    let data: Value = serde_json::from_str(&data)?;
    let ack = &data["requirements_handoff"];
    if ack.is_null() {
        return Ok(None);
    }
    let owner_matches = issue.assignee.as_deref() == Some(actor)
        || (action == "ready" && issue.assignee.as_deref() == Some("human:boss"));
    let valid = issue.state == "ready"
        && issue.deleted_at.is_none()
        && !issue.draft
        && author == actor
        && owner_matches
        && ((action == "ready" && data["previous_assignee"] == actor)
            || (action == "assigned"
                && data["target"] == "github"
                && data["github_handoff"]["run"] == run
                && (data["previous_assignee"] == actor
                    || data["previous_assignee"] == "human:boss")))
        && ack["run"] == run
        && ack["version"] == issue.version
        && ack["sha256"].as_str() == Some(digest(issue)?.as_str());
    Ok(Some(Acknowledgement {
        snapshot: ack.clone(),
        valid,
    }))
}

pub(in crate::issues::store) fn matches(
    db: &Connection,
    job: &crate::issues::worker::Job,
    issue: &Issue,
) -> Result<bool> {
    // Acknowledgement never converts an implementation task into artifact work.
    if crate::issues::worker::artifact_task(&json!({"labels":issue.labels}))
        != crate::issues::worker::artifact_task(&job.issue)
    {
        return Ok(false);
    }
    if let Some(ack) = current(db, &job.project.id, issue, &job.actor.id, &job.id)? {
        return Ok(ack.valid);
    }
    Ok(issue.title == job.issue["title"] && issue.body == job.issue["body"])
}
