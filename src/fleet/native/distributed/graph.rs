use super::*;
use rusqlite::params;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn reaches(edges: &BTreeMap<String, Vec<String>>, from: &str, target: &str) -> bool {
    let mut pending = vec![from.to_owned()];
    let mut seen = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if node == target {
            return true;
        }
        if seen.insert(node.clone()) {
            pending.extend(edges.get(&node).into_iter().flatten().cloned());
        }
    }
    false
}

fn rows(db: &Connection, table: &str, project: &str) -> Result<Vec<(Value, Value)>> {
    let keys = replica::rows(
        db,
        "SELECT entity FROM distributed_fields WHERE table_name=?1 AND field='@alive' AND json_extract(entity,'$.project_id')=?2 ORDER BY entity",
        &[json!(table), json!(project)],
    )?;
    let mut result = vec![];
    for key in keys {
        let key: Value = serde_json::from_str(key["entity"].as_str().unwrap())?;
        if let Some(row) = projection::row(db, table, &key)? {
            result.push((key, row));
        }
    }
    Ok(result)
}

pub(super) fn blockers(db: &Connection, project: &str) -> Result<BTreeMap<i64, Vec<i64>>> {
    db.execute("DELETE FROM distributed_rejections WHERE table_name='issue_blockers' AND json_extract(entity,'$.project_id')=?1", [project])?;
    let mut edges = BTreeMap::<String, Vec<String>>::new();
    let mut result = BTreeMap::new();
    // Read the covering dependency index, never full documents or the retained
    // requirement snapshots. A cutover can include thousands of archived tasks.
    let mut proposed = replica::rows(db,
        "SELECT f.entity,json_extract(f.value,'$.blockers') AS blockers FROM distributed_fields f WHERE f.table_name='issues' AND f.field='@lifecycle' AND json_extract(f.entity,'$.project_id')=?1 AND EXISTS(SELECT 1 FROM distributed_fields a WHERE a.table_name='issues' AND a.entity=f.entity AND a.field='@alive' AND a.value='true')",
        &[json!(project)])?.into_iter().map(|row| -> Result<(i64, Vec<i64>)> {
        let key: Value = serde_json::from_str(row["entity"].as_str().unwrap())?;
        let number = key["number"]
            .as_i64()
            .ok_or_else(|| replica::invalid("Invalid task graph identity"))?;
        Ok((number, serde_json::from_str(row["blockers"].as_str().unwrap_or("[]"))?))
    }).collect::<Result<Vec<_>>>()?;
    proposed.sort_by_key(|(number, _)| *number);
    for (number, mut blockers) in proposed {
        blockers.sort_unstable();
        blockers.dedup();
        let mut accepted = vec![];
        let mut skipped = vec![];
        for blocker in blockers {
            if blocker > 0 && !reaches(&edges, &blocker.to_string(), &number.to_string()) {
                edges
                    .entry(number.to_string())
                    .or_default()
                    .push(blocker.to_string());
                accepted.push(blocker);
            } else {
                skipped.push(blocker);
            }
        }
        if !skipped.is_empty() {
            db.execute("INSERT INTO distributed_rejections VALUES('issue_blockers',?1,?2)", params![json!({"project_id":project,"number":number}).to_string(),format!("Dependency cycle removed from the active graph; retained proposed blockers: {skipped:?}")])?;
        }
        result.insert(number, accepted);
    }
    Ok(result)
}

pub(super) fn subtasks(db: &Connection, project: &str) -> Result<()> {
    let candidates = rows(db, "issue_subtasks", project)?;
    db.execute("DELETE FROM issue_subtasks WHERE project_id=?1", [project])?;
    db.execute("DELETE FROM distributed_rejections WHERE table_name='issue_subtasks' AND json_extract(entity,'$.project_id')=?1",[project])?;
    for (key, row) in candidates {
        db.execute_batch("SAVEPOINT distributed_edge")?;
        match materialize::write(db, "issue_subtasks", &key, Some(row)) {
            Ok(()) => {
                db.execute_batch("RELEASE distributed_edge")?;
            }
            Err(error) => {
                db.execute_batch("ROLLBACK TO distributed_edge; RELEASE distributed_edge")?;
                db.execute("INSERT INTO distributed_rejections VALUES('issue_subtasks',?1,?2) ON CONFLICT(table_name,entity) DO UPDATE SET reason=excluded.reason",params![key.to_string(),error.to_string()])?;
            }
        }
    }
    // This rebuild handled the complete project, including later dirty pages.
    db.execute("DELETE FROM distributed_dirty WHERE table_name='issue_subtasks' AND json_extract(entity,'$.project_id')=?1",[project])?;
    Ok(())
}

pub(super) fn mindmap(db: &Connection, project: &str) -> Result<()> {
    // Mindmap primary keys do not contain project_id; scope from field values.
    let keys = replica::rows(
        db,
        "SELECT entity FROM distributed_fields WHERE table_name='mindmap_nodes' AND field='project_id' AND value=?1 ORDER BY entity",
        &[json!(json!(project).to_string())],
    )?;
    let mut candidates = vec![];
    for key in keys {
        let key: Value = serde_json::from_str(key["entity"].as_str().unwrap())?;
        if let Some(row) = projection::row(db, "mindmap_nodes", &key)? {
            candidates.push((key, row));
        }
    }
    let ids = candidates
        .iter()
        .filter_map(|(_, r)| r["id"].as_str().map(str::to_owned))
        .collect::<BTreeSet<_>>();
    let mut aliases = BTreeSet::new();
    let mut resources = BTreeSet::new();
    let mut edges = BTreeMap::<String, Vec<String>>::new();
    db.execute("DELETE FROM distributed_rejections WHERE table_name='mindmap_nodes' AND entity IN (SELECT entity FROM distributed_fields WHERE table_name='mindmap_nodes' AND field='project_id' AND value=?1)",[json!(project).to_string()])?;
    let mut conflicts = BTreeMap::<String, Vec<String>>::new();
    db.execute("DELETE FROM mindmap_nodes WHERE project_id=?1", [project])?;
    for (key, original) in &candidates {
        let mut row = original.clone();
        row["parent_id"] = Value::Null;
        if let Some(alias) = row["alias"].as_str()
            && !aliases.insert(alias.to_owned())
        {
            conflicts.entry(key.to_string()).or_default().push(
                "Duplicate alias retained in sync history; displayed without the alias".into(),
            );
            row["alias"] = Value::Null;
        }
        if !row["reference"].is_null()
            && !resources.insert(
                json!([row["kind"], row["reference_project"], row["reference"]]).to_string(),
            )
        {
            conflicts.entry(key.to_string()).or_default().push(
                "Duplicate resource reference retained in sync history; displayed as a note".into(),
            );
            row["reference"] = Value::Null;
            row["reference_project"] = Value::Null;
            row["kind"] = json!("markdown");
        }
        materialize::write(db, "mindmap_nodes", key, Some(row))?;
    }
    for (key, row) in candidates {
        let id = row["id"]
            .as_str()
            .ok_or_else(|| replica::invalid("Invalid map identity"))?;
        if let Some(parent) = row["parent_id"].as_str() {
            if !ids.contains(parent) || reaches(&edges, parent, id) {
                conflicts.entry(key.to_string()).or_default().push("Parent is missing or would create a cycle; proposed parent retained in sync history".into());
                continue;
            }
            edges.entry(id.into()).or_default().push(parent.into());
            let too_deep = ids.iter().any(|start| {
                let mut current = start.as_str();
                for _ in 0..32 {
                    match edges.get(current).and_then(|v| v.first()) {
                        Some(parent) => current = parent,
                        None => return false,
                    }
                }
                true
            });
            if too_deep {
                edges.remove(id);
                conflicts.entry(key.to_string()).or_default().push("Parent would exceed the 32-level outline limit; proposed parent retained in sync history".into());
                continue;
            }
            db.execute(
                "UPDATE mindmap_nodes SET parent_id=?2 WHERE id=?1",
                [id, parent],
            )?;
        }
    }
    for (key, reasons) in conflicts {
        db.execute(
            "INSERT INTO distributed_rejections VALUES('mindmap_nodes',?1,?2)",
            params![key, reasons.join("; ")],
        )?;
    }
    db.execute("INSERT OR IGNORE INTO distributed_dirty SELECT table_name,entity FROM distributed_fields WHERE table_name='mindmap_links' AND field='@alive'",[])?;
    db.execute("DELETE FROM distributed_dirty WHERE table_name='mindmap_nodes' AND entity IN (SELECT entity FROM distributed_fields WHERE table_name='mindmap_nodes' AND field='project_id' AND value=?1)",[json!(project).to_string()])?;
    Ok(())
}
