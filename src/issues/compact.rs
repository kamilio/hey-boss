//! Allowlisted read projections. Keep history and Markdown out of the SQL result,
//! not merely out of the final serializer.
use super::*;

pub(super) const COLUMNS: &str = "json_object('number',number,'title',title,'state',state,'assignee',assignee,'assignment_target',assignment_target,'version',version,'labels',json(labels),'draft',json(CASE WHEN draft THEN 'true' ELSE 'false' END),'manual_blocked',json(CASE WHEN manual_blocked THEN 'true' ELSE 'false' END),'blocker_numbers',json(blockers),'created_at',created_at,'updated_at',updated_at,'closed_at',closed_at,'deleted_at',deleted_at,'sort_order',sort_order,'agent_launch_count',(SELECT count(*) FROM issue_agent_launches l WHERE l.project_id=issues.project_id AND l.issue_number=issues.number),'status',(SELECT json_object('level',level,'created_at',created_at) FROM issue_status_updates s WHERE s.project_id=issues.project_id AND s.issue_number=issues.number ORDER BY created_at DESC,id DESC LIMIT 1),'attempt_hold',CASE WHEN attempt_hold IS NULL THEN NULL ELSE json_object('active',json('true')) END) AS summary";

pub(super) fn row(row: &crate::database::Row<'_>) -> rusqlite::Result<Value> {
    serde_json::from_str(&row.get::<_, String>("summary")?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

pub(super) fn metadata(db: &Connection, result: &mut Value) -> Result<()> {
    let (role, machine): (String, String) =
        db.query_row("SELECT role,node FROM fleet_meta WHERE id=1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    result["projection"] = json!("compact");
    result["projection_version"] = json!(1);
    result["omitted"] = json!([
        "bodies",
        "rendered_bodies",
        "commits",
        "provenance",
        "resource_histories",
        "github_evidence"
    ]);
    result["health"] = json!({"authoritative":role!="agent","store_role":role,"store_machine":machine,
        "freshness":"stored_snapshot","omitted_data":"not_loaded"});
    Ok(())
}

pub(super) fn issues(db: &Connection, project: &str, numbers: &[i64]) -> Result<Vec<Value>> {
    let mut issues = db.query_collect::<_, _, rusqlite::Error>(
        &format!("SELECT {COLUMNS} FROM json_each(?2) selected CROSS JOIN issues WHERE project_id=?1 AND number=selected.value AND deleted_at IS NULL"),
        params![project, serde_json::to_string(numbers)?], row,
    )?;
    enrich(db, project, &mut issues)?;
    Ok(issues)
}

pub(super) fn enrich(db: &Connection, project: &str, issues: &mut [Value]) -> Result<()> {
    let numbers: Vec<_> = issues.iter().filter_map(|i| i["number"].as_i64()).collect();
    if numbers.is_empty() {
        return Ok(());
    }
    let selected = serde_json::to_string(&numbers)?;
    let mut prs = std::collections::BTreeMap::<i64, Vec<Value>>::new();
    for (number, pr) in db.query_collect::<_, _, rusqlite::Error>(
        "SELECT issue_number,url,purpose,status,checked_at,error FROM json_each(?2) selected CROSS JOIN issue_pull_requests WHERE project_id=?1 AND issue_number=selected.value ORDER BY issue_number,created_at,url",
        params![project, selected], |r| Ok((r.get::<_, i64>(0)?, json!({"url":r.get::<_,String>(1)?,"purpose":r.get::<_,String>(2)?,"status":r.get::<_,String>(3)?,"checked_at":r.get::<_,Option<i64>>(4)?,"error":r.get::<_,Option<String>>(5)?}))),
    )? { prs.entry(number).or_default().push(pr); }
    let mut parents = std::collections::BTreeMap::new();
    let mut children = std::collections::BTreeMap::<i64, Vec<i64>>::new();
    for (parent, child) in db.query_collect::<_, _, rusqlite::Error>(
        "SELECT parent_number,child_number FROM issue_subtasks WHERE project_id=?1 AND (parent_number IN (SELECT value FROM json_each(?2)) OR child_number IN (SELECT value FROM json_each(?2))) ORDER BY parent_number,child_number",
        params![project, selected], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?)),
    )? { parents.insert(child,parent); children.entry(parent).or_default().push(child); }
    let (role, machine): (String, String) =
        db.query_row("SELECT role,node FROM fleet_meta WHERE id=1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    let now = super::super::worker::now();
    let mut allocations = std::collections::BTreeMap::new();
    for (number, reserved, expires, worker) in db.query_collect::<_, _, rusqlite::Error>(
        "SELECT i.number,a.node,CASE WHEN i.assignee IS NULL THEN d.expires_at END,
        (SELECT json_object('run_id',id,'actor_id',actor_id,'machine',machine,'claimed_at',claimed_at,'expires_at',reservation_expires,
        'state',CASE WHEN claimed_at IS NOT NULL THEN 'claimed' WHEN reservation_expires<=?3 THEN 'expired' WHEN reservation_expires IS NOT NULL THEN 'awaiting_claim' ELSE 'active' END)
        FROM worker_runs w WHERE w.project_id=i.project_id AND w.issue_number=i.number AND w.finished_at IS NULL ORDER BY w.started_at DESC,w.id DESC LIMIT 1)
        FROM json_each(?2) selected CROSS JOIN issues i
        LEFT JOIN fleet_allocations a ON a.project_id=i.project_id AND a.issue_number=i.number
        LEFT JOIN fleet_allocation_deadlines d ON d.project_id=i.project_id AND d.issue_number=i.number
        WHERE i.project_id=?1 AND i.number=selected.value",
        params![project, selected, now], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<i64>>(2)?,r.get::<_,Option<String>>(3)?)),
    )? {
        let reason = match reserved.as_deref() {
            Some(_) if expires.is_some_and(|v| v<=now) => "allocation_expired",
            Some(owner) if owner != machine => "reserved_elsewhere",
            Some(_) => "allocated_here",
            None if role == "agent" => "allocation_missing",
            None => "unallocated",
        };
        allocations.insert(number,json!({"reason":reason,"role":role,"store_machine":machine,"reserved_machine":reserved,"expires_at":expires,
            "authoritative":role!="agent","worker_reservation":worker.map(|s|serde_json::from_str::<Value>(&s)).transpose()?}));
    }
    for issue in issues {
        let number = issue["number"].as_i64().unwrap();
        issue["pull_requests"] = json!(prs.remove(&number).unwrap_or_default());
        issue["parent_number"] = json!(parents.get(&number));
        issue["child_numbers"] = json!(children.remove(&number).unwrap_or_default());
        issue["allocation"] = allocations
            .remove(&number)
            .ok_or_else(|| Error::new("not_found", "Selected issue disappeared"))?;
    }
    Ok(())
}
