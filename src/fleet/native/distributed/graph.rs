use super::*;
use rusqlite::params;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

// Keep graph reads separate from lifecycle records, which retain full task bodies.
const BLOCKERS: &str = "SELECT entity,blockers FROM distributed_issue_graph WHERE project_id=?1 AND alive=1 AND blockers IS NOT NULL";

pub(super) fn install(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_issue_graph')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS distributed_issue_graph(entity TEXT PRIMARY KEY,project_id TEXT,blockers TEXT,alive INTEGER) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS distributed_issue_graph_project ON distributed_issue_graph(project_id,entity,blockers) WHERE alive=1 AND blockers IS NOT NULL;")?;
    for operation in ["INSERT", "UPDATE"] {
        tx.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS distributed_issue_graph_{operation} AFTER {operation} ON distributed_fields
            WHEN NEW.table_name='issues' AND NEW.field IN ('@alive','@lifecycle') BEGIN
            INSERT INTO distributed_issue_graph VALUES(NEW.entity,json_extract(NEW.entity,'$.project_id'),
                CASE WHEN NEW.field='@lifecycle' THEN coalesce(json_extract(NEW.value,'$.blockers'),'[]') END,
                CASE WHEN NEW.field='@alive' THEN NEW.value='true' END)
            ON CONFLICT(entity) DO UPDATE SET blockers=coalesce(excluded.blockers,blockers),alive=coalesce(excluded.alive,alive);
            END;"))?;
    }
    tx.execute_batch("CREATE TRIGGER IF NOT EXISTS distributed_issue_graph_DELETE AFTER DELETE ON distributed_fields
        WHEN OLD.table_name='issues' AND OLD.field IN ('@alive','@lifecycle') BEGIN
        UPDATE distributed_issue_graph SET blockers=CASE WHEN OLD.field='@lifecycle' THEN NULL ELSE blockers END,
            alive=CASE WHEN OLD.field='@alive' THEN NULL ELSE alive END WHERE entity=OLD.entity;
        END;
        INSERT INTO distributed_issue_graph
        SELECT entity,json_extract(entity,'$.project_id'),
            CASE WHEN field='@lifecycle' THEN coalesce(json_extract(value,'$.blockers'),'[]') END,
            CASE WHEN field='@alive' THEN value='true' END
        FROM distributed_fields WHERE table_name='issues' AND field IN ('@alive','@lifecycle')
        ON CONFLICT(entity) DO UPDATE SET blockers=coalesce(excluded.blockers,blockers),alive=coalesce(excluded.alive,alive);")?;
    tx.commit()?;
    Ok(())
}

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
    let mut proposed = replica::rows(db, BLOCKERS, &[json!(project)])?
        .into_iter()
        .map(|row| -> Result<(i64, Vec<i64>)> {
            let key: Value = serde_json::from_str(row["entity"].as_str().unwrap())?;
            let number = key["number"]
                .as_i64()
                .ok_or_else(|| replica::invalid("Invalid task graph identity"))?;
            Ok((
                number,
                serde_json::from_str(row["blockers"].as_str().unwrap_or("[]"))?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_scan_keeps_alive_checks_out_of_document_pages() {
        let root = crate::admin::Temporary::new().unwrap();
        let path = root.0.join("graph.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE fleet_row_ids(origin TEXT,table_name TEXT,local_id TEXT,origin_id TEXT)",
        )
        .unwrap();
        db.execute_batch(super::super::SCHEMA).unwrap();
        install(&db).unwrap();
        db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<2400),
            fields(field,value) AS (VALUES('@alive','true'),('@lifecycle','{\"blockers\":\"[]\"}'),('@requirements',json_quote(printf('%.*c',16384,'x'))))
            INSERT INTO distributed_fields SELECT 'issues',json_object('project_id','p','number',x),field,1,'a',value FROM n CROSS JOIN fields;
            PRAGMA cache_size=-64; PRAGMA shrink_memory;").unwrap();
        let mut pages = 0;
        let mut high = 0;
        unsafe {
            assert_eq!(
                rusqlite::ffi::sqlite3_db_status(
                    db.handle(),
                    rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS,
                    &mut pages,
                    &mut high,
                    1
                ),
                rusqlite::ffi::SQLITE_OK
            );
        }
        let dependencies = blockers(&db, "p").unwrap();
        unsafe {
            assert_eq!(
                rusqlite::ffi::sqlite3_db_status(
                    db.handle(),
                    rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS,
                    &mut pages,
                    &mut high,
                    0
                ),
                rusqlite::ffi::SQLITE_OK
            );
        }
        drop(db);
        std::fs::remove_file(path).unwrap();
        assert_eq!(dependencies.len(), 2400);
        assert!(dependencies.values().all(Vec::is_empty));
        eprintln!("Dependency graph cold page reads: {pages}");
        assert!(
            pages < 100,
            "Dependency graph read document-tree pages: {pages}"
        );
    }

    #[test]
    fn dependency_scan_is_scoped_to_project_without_statistics() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE fleet_row_ids(origin TEXT,table_name TEXT,local_id TEXT,origin_id TEXT)",
        )
        .unwrap();
        db.execute_batch(super::super::SCHEMA).unwrap();
        install(&db).unwrap();
        // Replicas do not require ANALYZE. Unrelated tasks and requirement
        // documents must not determine how long this writer transaction lasts.
        db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000),
            fields(field,value) AS (VALUES('@alive','true'),('@lifecycle','{\"blockers\":\"[]\"}'),('@requirements',json_quote(printf('%.*c',4096,'x'))))
            INSERT INTO distributed_fields SELECT 'issues',json_object('project_id','other','number',x),field,1,'a',value FROM n CROSS JOIN fields;
            INSERT INTO distributed_fields VALUES
            ('issues','{\"project_id\":\"p\",\"number\":1}','@alive',1,'a','true'),
            ('issues','{\"project_id\":\"p\",\"number\":1}','@lifecycle',1,'a','{\"blockers\":\"[2,2]\"}'),
            ('issues','{\"project_id\":\"p\",\"number\":2}','@alive',1,'a','true'),
            ('issues','{\"project_id\":\"p\",\"number\":2}','@lifecycle',1,'a','{\"blockers\":\"[1]\"}'),
            ('issues','{\"project_id\":\"p\",\"number\":3}','@alive',1,'a','false'),
            ('issues','{\"project_id\":\"p\",\"number\":3}','@lifecycle',1,'a','{\"blockers\":\"[1]\"}');").unwrap();
        let mut statement = db.prepare(BLOCKERS).unwrap();
        let rows = statement
            .query_map(["p"], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        let steps = statement.get_status(rusqlite::StatementStatus::VmStep);
        assert!(
            steps < 1000,
            "Dependency scan visited unrelated history: {steps} VM steps"
        );
        let dependencies = blockers(&db, "p").unwrap();
        assert_eq!(dependencies, BTreeMap::from([(1, vec![2]), (2, vec![])]));
        assert_eq!(
            db.query_row("SELECT count(*) FROM distributed_rejections", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let tx = db.unchecked_transaction().unwrap();
        tx.execute("UPDATE distributed_fields SET value='false' WHERE table_name='issues' AND field='@alive' AND json_extract(entity,'$.project_id')='p' AND json_extract(entity,'$.number')=1", []).unwrap();
        assert!(!blockers(&tx, "p").unwrap().contains_key(&1));
        tx.rollback().unwrap();
        assert_eq!(blockers(&db, "p").unwrap(), dependencies);
    }
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
