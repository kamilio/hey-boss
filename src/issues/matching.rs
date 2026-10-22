//! Shared matching for queue moves and drafting, inside the existing batch API.
use super::*;
use crate::issues::{BatchEdit, IssueMatcher, MatchingChange, QueueEnd};

pub(super) fn validate(
    request: &Request,
    matcher: &IssueMatcher,
    edits: &[BatchEdit],
    change: Option<&MatchingChange>,
    version: Option<i64>,
) -> Result<()> {
    if matcher.is_empty()
        || !edits.is_empty()
        || version.is_none_or(|v| v < 0)
        || !matches!(
            change,
            Some(
                MatchingChange::Move { .. }
                    | MatchingChange::Edit { draft: true }
                    | MatchingChange::Delete
            )
        )
        || matcher.numbers.iter().any(|n| *n < 1)
        || matcher.state.as_deref() == Some("deleted")
    {
        return Err(Error::invalid(
            "Matching batches require filters, one move, draft or delete change, and a nonnegative order guard; explicit edits cannot be combined",
        ));
    }
    let mut read = request.clone();
    read.operation = matcher.list();
    read.request_id = None;
    super::validate(&read)
}

pub(super) fn execute(
    db: &Connection,
    project: &Project,
    actor: &Actor,
    matcher: &IssueMatcher,
    change: &MatchingChange,
    expected: Option<i64>,
    now: i64,
) -> Result<Value> {
    let version: i64 = db.query_row(
        "SELECT issue_order_version FROM projects WHERE id=?1",
        [&project.id],
        |r| r.get(0),
    )?;
    if expected != Some(version) {
        return Err(Error::conflict(
            "Issue order changed. Refresh the list and try again.",
        ));
    }
    let Operation::List {
        state,
        assignee,
        unassigned,
        labels,
        search,
        ..
    } = matcher.list()
    else {
        unreachable!()
    };
    let archived =
        super::super::archive::search_bodies(db, &project.id, &state, search.as_deref())?;
    let predicate = list_predicate(search.is_some(), assignee.as_deref(), unassigned);
    let mut query = db.prepare(&format!("SELECT number,sort_order,state,assignee,draft FROM issues WHERE {predicate} AND (?7='[]' OR number IN (SELECT value FROM json_each(?7))) ORDER BY sort_order,number"))?;
    let selected = query
        .query_map(
            params![
                project.id,
                state,
                assignee,
                unassigned,
                search,
                serde_json::to_string(&labels)?,
                serde_json::to_string(&matcher.numbers)?,
                0,
                serde_json::to_string(&archived)?
            ],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, bool>(4)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut skipped = Vec::new();
    let mut changed = 0;
    match change {
        MatchingChange::Move { to } if !selected.is_empty() => {
            let all = db.prepare("SELECT number,sort_order FROM issues WHERE project_id=?1 ORDER BY sort_order,number")?
                .query_map([&project.id], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,i64>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let offset = match to {
                QueueEnd::Top => 0,
                QueueEnd::Bottom => all.len() - selected.len(),
            };
            let already_placed = selected.iter().zip(&all[offset..]).all(|(a, b)| a.0 == b.0);
            if !already_placed {
                let count = i64::try_from(selected.len())
                    .map_err(|_| Error::invalid("Too many matching issues"))?;
                let base = match to {
                    QueueEnd::Top => all.first().unwrap().1.checked_sub(count),
                    QueueEnd::Bottom => all.last().unwrap().1.checked_add(1),
                }
                .filter(|base| base.checked_add(count).is_some())
                .ok_or_else(|| Error::conflict("Issue order range exhausted"))?;
                let mut update = db.prepare("UPDATE issues SET sort_order=?3,updated_at=?4,version=version+1 WHERE project_id=?1 AND number=?2")?;
                for (index, (number, old, ..)) in selected.iter().enumerate() {
                    let target = base + index as i64;
                    update.execute(params![project.id, number, target, now])?;
                    event(
                        db,
                        &project.id,
                        *number,
                        &actor.id,
                        "reordered",
                        now,
                        &json!({"from":old,"to":target,"placement":to}),
                    )?;
                    changed += 1;
                }
                db.execute(
                    "UPDATE projects SET issue_order_version=issue_order_version+1 WHERE id=?1",
                    [&project.id],
                )?;
            }
        }
        MatchingChange::Edit { draft: true } => {
            super::super::drafts::drafts_allowed(db, project)?;
            for (number, _, state, assignee, draft) in &selected {
                if *draft && state == "open" {
                    continue;
                }
                if let Err(error) = super::super::drafts::can_draft(
                    db,
                    project,
                    *number,
                    state,
                    assignee.as_deref(),
                ) {
                    if error.code != "conflict" {
                        return Err(error);
                    }
                    skipped.push(json!({"number":number,"reason":error.message}));
                    continue;
                }
                if state == "blocked" {
                    db.execute("UPDATE worker_runs SET retry_allowed=1 WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NOT NULL", params![project.id,number])?;
                }
                db.execute("UPDATE issues SET draft=1,state='open',manual_blocked=0,version=version+1,updated_at=?3 WHERE project_id=?1 AND number=?2", params![project.id,number,now])?;
                event(
                    db,
                    &project.id,
                    *number,
                    &actor.id,
                    "edited",
                    now,
                    &json!({"before":{"draft":draft,"state":state},"after":{"draft":true,"state":"open"}}),
                )?;
                changed += 1;
            }
        }
        MatchingChange::Delete => {
            for (number, ..) in &selected {
                if let Err(error) = super::super::authority::guard_unowned(db, &project.id, *number)
                    .and_then(|()| attempts::guard(db, &project.id, *number))
                {
                    if !matches!(error.code.as_str(), "conflict" | "attempt_held") {
                        return Err(error);
                    }
                    skipped.push(json!({"number":number,"reason":"Assigned, reserved, or unfinished work was preserved"}));
                    continue;
                }
                db.execute("UPDATE issues SET deleted_at=?3,updated_at=?3,version=version+1 WHERE project_id=?1 AND number=?2", params![project.id,number,now])?;
                event(
                    db,
                    &project.id,
                    *number,
                    &actor.id,
                    "deleted",
                    now,
                    &json!({"previous_assignee":null,"forced":false}),
                )?;
                changed += 1;
            }
        }
        _ => {}
    }
    Ok(
        json!({"ok":true,"project":project,"matched_count":selected.len(),"changed":changed>0,"changed_count":changed,"skipped_count":skipped.len(),"skipped":skipped,"order_version":version+i64::from(changed>0 && matches!(change,MatchingChange::Move {..}))}),
    )
}
