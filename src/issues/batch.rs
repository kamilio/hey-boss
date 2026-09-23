//! One input array is one guarded group in the authoritative SQLite transaction.
use super::*;
use crate::issues::{BatchAssignment, BatchEdit};

pub(super) fn validate(edits: &[BatchEdit]) -> Result<()> {
    if edits.is_empty() || edits.len() > 100 || serde_json::to_vec(edits)?.len() > BODY_LIMIT {
        return Err(Error::invalid(
            "Batch requires 1–100 entries, at most 1 MiB",
        ));
    }
    let mut numbers = BTreeSet::new();
    for edit in edits {
        if edit.number < 1 || edit.if_version < 1 || !numbers.insert(edit.number) {
            return Err(Error::invalid(
                "Batch requires unique positive issue numbers and positive versions",
            ));
        }
        if let Some(owner) = &edit.expected_assignee {
            identifier(owner, "expected assignee", 512)?;
        }
        labels(&edit.add_labels)?;
        labels(&edit.remove_labels)?;
        if edit
            .add_labels
            .iter()
            .any(|l| edit.remove_labels.contains(l))
        {
            return Err(Error::invalid("Cannot add and remove the same label"));
        }
        if edit.add_labels.is_empty()
            && edit.remove_labels.is_empty()
            && matches!(edit.assignment, BatchAssignment::Keep)
        {
            return Err(Error::invalid(
                "Each batch entry must update labels or assignment",
            ));
        }
    }
    Ok(())
}

#[derive(Clone)]
struct TriageState {
    number: i64,
    version: i64,
    assignee: Option<String>,
    labels: Vec<String>,
    state: String,
    draft: bool,
}
fn load(db: &Connection, project: &str, number: i64) -> Result<TriageState> {
    let row = db.query_row("SELECT version,assignee,labels,state,draft FROM issues WHERE project_id=?1 AND number=?2 AND deleted_at IS NULL", params![project,number], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, bool>(4)?))
    }).optional()?.ok_or_else(|| Error::new("not_found", "Issue not found"))?;
    Ok(TriageState {
        number,
        version: row.0,
        assignee: row.1,
        labels: serde_json::from_str(&row.2)?,
        state: row.3,
        draft: row.4,
    })
}
fn summary(issue: &TriageState) -> Value {
    json!({"version":issue.version,"assignee":issue.assignee,"labels":issue.labels})
}

pub(super) fn execute(
    db: &Connection,
    project: &Project,
    actor: Option<&Actor>,
    edits: &[BatchEdit],
    now: i64,
) -> Result<Value> {
    let replica: bool =
        db.query_row("SELECT role='agent' FROM fleet_meta WHERE id=1", [], |r| {
            r.get(0)
        })?;
    if replica {
        return Err(Error::invalid(
            "Issue batches require the authoritative store; use --host SUPERVISOR from fleet companions. Replica replay cannot preserve group atomicity.",
        ));
    }
    let mut results = Vec::with_capacity(edits.len());
    let mut updates = Vec::with_capacity(edits.len());
    let mut accepted = true;
    for edit in edits {
        let prepared = (|| -> Result<(TriageState, TriageState)> {
            // Avoid loading Markdown, PRs or the subtask graph for bulk triage.
            let before = load(db, &project.id, edit.number)?;
            if before.version != edit.if_version || before.assignee != edit.expected_assignee {
                return Err(Error::conflict(format!(
                    "Guard mismatch: current version {}, assignee {}",
                    before.version,
                    before.assignee.as_deref().unwrap_or("unassigned")
                )));
            }
            let mut after = before.clone();
            let mut values: BTreeSet<_> = after.labels.iter().cloned().collect();
            values.extend(edit.add_labels.iter().cloned());
            for value in &edit.remove_labels {
                values.remove(value);
            }
            after.labels = values.into_iter().collect();
            labels(&after.labels)?;
            match edit.assignment {
                BatchAssignment::Keep => {}
                BatchAssignment::Unassign => after.assignee = None,
                BatchAssignment::Boss => {
                    if before.state != "open" || before.draft {
                        return Err(Error::conflict(
                            "Boss assignment requires an open, undrafted issue",
                        ));
                    }
                    after.assignee = Some("human:boss".into());
                }
            }
            // Explicit version + owner guards authorize a handoff of that exact
            // claim, but never override an unclaimed worker reservation.
            if after.assignee != before.assignee {
                let reserved: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL AND claimed_at IS NULL AND reservation_expires>?3)", params![project.id,edit.number,now], |r| r.get(0))?;
                if reserved {
                    return Err(Error::conflict("Issue has an unclaimed worker reservation"));
                }
            }
            if before.labels != after.labels || before.assignee != after.assignee {
                after.version += 1;
            }
            Ok((before, after))
        })();
        match prepared {
            Ok((before, after)) => {
                let changed = before.version != after.version;
                results.push(json!({"number":edit.number,"status":if changed { "changed" } else { "unchanged" },"changed":changed,"before":summary(&before),"after":summary(&after)}));
                updates.push((before, after));
            }
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "conflict" | "not_found" | "invalid_input"
                ) =>
            {
                accepted = false;
                results.push(
                    json!({"number":edit.number,"status":"rejected","changed":false,"error":error}),
                );
            }
            Err(error) => return Err(error),
        }
    }
    if !accepted {
        for result in &mut results {
            if result["status"] != "rejected" {
                result["status"] = json!("blocked");
                result["changed"] = json!(false);
                result.as_object_mut().unwrap().remove("after");
            }
        }
    } else {
        let actor = actor.unwrap();
        for (before, after) in updates {
            if before.version == after.version {
                continue;
            }
            if after.assignee.as_deref() == Some("human:boss") {
                let mut boss = actor.clone();
                boss.id = "human:boss".into();
                boss.kind = "human".into();
                boss.session_id = None;
                boss.pid = None;
                boss.process_start = None;
                boss.source = "Boss assignment".into();
                db.execute("INSERT INTO agents(id,metadata,last_seen) VALUES(?1,?2,?3) ON CONFLICT(id) DO NOTHING", params![boss.id,serde_json::to_string(&boss)?,now])?;
            }
            db.execute("UPDATE issues SET labels=?3,assignee=?4,version=?5,updated_at=?6 WHERE project_id=?1 AND number=?2", params![project.id,after.number,serde_json::to_string(&after.labels)?,after.assignee,after.version,now])?;
            event(
                db,
                &project.id,
                after.number,
                &actor.id,
                "triaged",
                now,
                &json!({"before":summary(&before),"after":summary(&after)}),
            )?;
        }
    }
    let changed = accepted && results.iter().any(|r| r["changed"] == true);
    Ok(
        json!({"ok":true,"project":project,"accepted":accepted,"applied":accepted,"changed":changed,"results":results}),
    )
}
