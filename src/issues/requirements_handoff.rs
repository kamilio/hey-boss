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

fn external_comments(db: &Connection, project: &str, number: i64, actor: &str) -> Result<i64> {
    Ok(db.query_row(
        "SELECT count(*) FROM comments WHERE project_id=?1 AND issue_number=?2 AND author<>?3",
        params![project, number, actor],
        |r| r.get(0),
    )?)
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
    Ok(
        json!({"run":run,"version":issue.version+1,"sha256":digest(issue)?,
        "external_comments":external_comments(db, project, issue.number, &actor.id)?}),
    )
}

// Consume only the latest lifecycle event. Later versions need an unbroken
// transaction-recorded chain of harmless writes; unexplained drift still fails.
pub(in crate::issues::store) fn current(
    db: &Connection,
    project: &str,
    issue: &Issue,
    actor: &str,
    run: &str,
) -> Result<Option<Acknowledgement>> {
    let event: Option<(i64, String, String, String)> = db.query_row(
        "SELECT id,actor,action,data FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('assigned','claimed','ready','unassigned','closed','reopened','blocked','deleted','restored') ORDER BY id DESC LIMIT 1",
        params![project,issue.number], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
    ).optional()?;
    let Some((id, author, action, data)) = event else {
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
        // Comment rows and their audit events can arrive in different sync
        // batches. Append-only counts are portable across replica-local IDs.
        && (ack.get("external_comments").is_none()
            || ack["external_comments"] == external_comments(db, project, issue.number, actor)?)
        && version_matches(db, project, issue, id, actor, ack)?
        && ack["sha256"].as_str() == Some(digest(issue)?.as_str());
    Ok(Some(Acknowledgement {
        snapshot: ack.clone(),
        valid,
    }))
}

fn version_matches(
    db: &Connection,
    project: &str,
    issue: &Issue,
    event: i64,
    actor: &str,
    ack: &Value,
) -> Result<bool> {
    let Some(mut version) = ack["version"]
        .as_i64()
        .filter(|v| *v > 0 && *v <= issue.version)
    else {
        return Ok(false);
    };
    let (role, node, authority): (String, String, String) = db.query_row(
        "SELECT role,node,coalesce((SELECT origin FROM fleet_row_ids WHERE table_name='events' AND local_id=?1 ORDER BY rowid LIMIT 1),node) FROM fleet_meta WHERE id=1",
        [event], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    )?;
    // Versions are local until the journal is accepted. A peer's coincident
    // number cannot explain a supervisor write, or survive a canonical pull.
    let local_pending = role == "agent" && db.query_row(
        "SELECT EXISTS(SELECT 1 FROM fleet_outbox WHERE table_name IN ('issues','comments','events','issue_status_updates') AND json_extract(coalesce(after_json,before_json),'$.project_id')=?1 AND coalesce(json_extract(coalesce(after_json,before_json),'$.number'),json_extract(coalesce(after_json,before_json),'$.issue_number'))=?2 AND table_name='issues')",
        params![project, issue.number], |r| r.get::<_,bool>(0),
    )?;
    let mut steps = std::collections::BTreeSet::new();
    let mut stmt = db.prepare("SELECT actor,action,CASE WHEN action='requirements_preserved' THEN data ELSE '{}' END,coalesce((SELECT origin FROM fleet_row_ids WHERE table_name='events' AND local_id=events.id ORDER BY rowid LIMIT 1),?4) FROM events WHERE project_id=?1 AND issue_number=?2 AND id>?3 ORDER BY id")?;
    let rows = stmt.query_map(params![project, issue.number, event, node], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (author, action, data, origin) = row?;
        // Prose is never evidence. Only the owning actor's own final notes are
        // harmless; another actor's comment may contain new work or an approval.
        if author == actor && action == "commented" {
            continue;
        }
        if author != actor || action != "requirements_preserved" {
            return Ok(false);
        }
        let data: Value = serde_json::from_str(&data)?;
        let canonical = if role == "agent" { &authority } else { &node };
        if (origin == *canonical || (local_pending && origin == node))
            && data["acknowledgement"] == *ack
            && matches!(data["source"].as_str(), Some("owner_comment" | "replica"))
            && let Some(previous) = data["previous_version"].as_i64()
            && previous.checked_add(1) == data["version"].as_i64()
        {
            steps.insert(previous);
        }
    }
    // Replica-local event IDs/order and duplicate local/canonical steps may
    // differ after a pull. The exact version chain and acknowledgement do not.
    while version < issue.version && steps.contains(&version) {
        version += 1;
    }
    Ok(version == issue.version)
}

impl Store {
    /// Called only before an owner's comment or a field-identical replica merge,
    /// in the same transaction as its single issue-version increment. Never
    /// repairs an already stale acknowledgement or accepts changed requirements.
    pub(crate) fn preserve_requirements_handoff(
        db: &Connection,
        project: &str,
        number: i64,
        commenter: Option<&str>,
        now: i64,
    ) -> Result<()> {
        let latest: Option<(String, String)> = db.query_row(
            "SELECT e.actor,json_extract(e.data,'$.requirements_handoff.run') FROM issues i JOIN events e ON e.project_id=i.project_id AND e.issue_number=i.number WHERE i.project_id=?1 AND i.number=?2 AND i.state='ready' AND e.action IN ('assigned','claimed','ready','unassigned','closed','reopened','blocked','deleted','restored') ORDER BY e.id DESC LIMIT 1",
            params![project, number], |r| Ok((r.get(0)?, r.get::<_,Option<String>>(1)?.unwrap_or_default())),
        ).optional()?;
        let Some((actor, run)) = latest else {
            return Ok(());
        };
        if run.is_empty() || commenter.is_some_and(|commenter| commenter != actor) {
            return Ok(());
        }
        let issue = get_issue(db, project, number, true)?;
        if let Some(ack) = current(db, project, &issue, &actor, &run)?
            && ack.valid
        {
            event(
                db,
                project,
                number,
                &actor,
                "requirements_preserved",
                now,
                &json!({
                    "acknowledgement":ack.snapshot, "previous_version":issue.version,
                    "version":issue.version+1, "source":if commenter.is_some() {"owner_comment"} else {"replica"}
                }),
            )?;
        }
        Ok(())
    }
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
