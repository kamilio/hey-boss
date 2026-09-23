//! Names select one destination; old storage IDs remain readable for history.
use super::*;

fn empty_git_metadata(db: &Connection, id: &str) -> Result<bool> {
    if !super::super::identity::is_git_metadata_project(id) {
        return Ok(false);
    }
    Ok(!db.query_row(
        "SELECT EXISTS(SELECT 1 FROM issues WHERE project_id=?1)
        OR EXISTS(SELECT 1 FROM artifacts WHERE project_id=?1)
        OR EXISTS(SELECT 1 FROM mindmap_nodes WHERE project_id=?1)
        OR EXISTS(SELECT 1 FROM project_settings WHERE project_id=?1)
        OR EXISTS(SELECT 1 FROM project_workers WHERE project_id=?1)",
        [id],
        |r| r.get::<_, bool>(0),
    )?)
}

/// Release only empty metadata identities' name slots. Keep every storage row,
/// and restore a slot if explicit full-ID access later saves work there.
pub(super) fn reconcile_git_metadata(db: &Connection) -> Result<()> {
    let projects = db.prepare("SELECT id,name FROM projects WHERE id LIKE 'local:%/.git' OR id LIKE 'local:%/.git/%' ORDER BY created_at,id")?
        .query_map([], |r| Ok(Project { id:r.get(0)?, name:r.get(1)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut repair_needed = false;
    for p in &projects {
        let key: Option<String> = db
            .query_row(
                "SELECT project_id FROM project_name_keys WHERE name=?1",
                [&p.name],
                |r| r.get(0),
            )
            .optional()?;
        let empty = empty_git_metadata(db, &p.id)?;
        if (empty && key.as_deref() == Some(&p.id)) || (!empty && key.is_none()) {
            repair_needed = true;
            break;
        }
    }
    if !repair_needed {
        return Ok(());
    }
    let tx =
        crate::database::Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)?;
    let mut released_names = Vec::new();
    for p in &projects {
        if empty_git_metadata(&tx, &p.id)?
            && tx.execute("DELETE FROM project_name_keys WHERE project_id=?1", [&p.id])? > 0
        {
            released_names.push(p.name.clone());
        }
    }
    for name in released_names {
        let candidates = tx
            .prepare(
                "SELECT p.id,p.name FROM projects p WHERE p.name=?1 COLLATE NOCASE ORDER BY
            (SELECT count(*) FROM issues i WHERE i.project_id=p.id AND i.deleted_at IS NULL) DESC,
            (SELECT count(*) FROM artifacts a WHERE a.project_id=p.id) DESC,
            (SELECT count(*) FROM mindmap_nodes m WHERE m.project_id=p.id) DESC,p.created_at,p.id",
            )?
            .query_map([name], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for p in candidates {
            if !empty_git_metadata(&tx, &p.id)? {
                tx.execute(
                    "INSERT OR IGNORE INTO project_name_keys(name,project_id) VALUES(?1,?2)",
                    params![p.name, p.id],
                )?;
                break;
            }
        }
    }
    for p in &projects {
        if !empty_git_metadata(&tx, &p.id)? {
            tx.execute(
                "INSERT OR IGNORE INTO project_name_keys(name,project_id) VALUES(?1,?2)",
                params![p.name, p.id],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='project_name_keys')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        // Old clients recorded every harmless name reuse. Keep legacy history
        // identities for compatibility; new aliases stay silent.
        if db.query_row(
            "SELECT EXISTS(SELECT 1 FROM project_name_collisions WHERE legacy=0)",
            [],
            |r| r.get::<_, bool>(0),
        )? {
            db.execute("DELETE FROM project_name_collisions WHERE legacy=0", [])?;
        }
        return Ok(());
    }
    db.execute_batch("BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS project_name_keys(name TEXT PRIMARY KEY COLLATE NOCASE,project_id TEXT NOT NULL UNIQUE REFERENCES projects(id));
        CREATE TABLE IF NOT EXISTS project_name_collisions(rejected_id TEXT PRIMARY KEY,name TEXT NOT NULL,project_id TEXT NOT NULL REFERENCES projects(id),legacy INTEGER NOT NULL DEFAULT 0);
        INSERT OR IGNORE INTO project_name_keys SELECT name,id FROM (
            SELECT p.name,p.id,row_number() OVER (PARTITION BY p.name COLLATE NOCASE ORDER BY
                (SELECT count(*) FROM issues i WHERE i.project_id=p.id AND i.deleted_at IS NULL) DESC,
                (SELECT count(*) FROM artifacts a WHERE a.project_id=p.id) DESC,
                (SELECT count(*) FROM mindmap_nodes m WHERE m.project_id=p.id) DESC,
                p.created_at,p.id) AS rank FROM projects p) WHERE rank=1;
        INSERT OR IGNORE INTO project_name_collisions SELECT p.id,p.name,k.project_id,1 FROM projects p JOIN project_name_keys k ON k.name=p.name WHERE p.id<>k.project_id;
        CREATE TRIGGER IF NOT EXISTS project_name_guard BEFORE INSERT ON projects
        WHEN NOT EXISTS(SELECT 1 FROM projects WHERE id=NEW.id)
            AND EXISTS(SELECT 1 FROM project_name_keys WHERE name=NEW.name)
            AND (SELECT syncing FROM fleet_meta WHERE id=1)=0
        BEGIN SELECT RAISE(ABORT,'Project name already exists; use the registered project name'); END;
        CREATE TRIGGER IF NOT EXISTS project_name_register AFTER INSERT ON projects
        BEGIN INSERT OR IGNORE INTO project_name_keys VALUES(NEW.name,NEW.id);
            INSERT OR IGNORE INTO project_name_collisions SELECT NEW.id,NEW.name,project_id,1 FROM project_name_keys WHERE name=NEW.name AND project_id<>NEW.id;
        END;
        CREATE TRIGGER IF NOT EXISTS project_name_update_guard BEFORE UPDATE OF name ON projects
        WHEN NEW.name<>OLD.name
        BEGIN SELECT RAISE(ABORT,'Project names are stable identifiers'); END;
        COMMIT;")?;
    Ok(())
}

pub(super) fn by_name(db: &Connection, name: &str) -> Result<Option<Project>> {
    let project = db.query_row("SELECT p.id,p.name FROM project_name_keys k JOIN projects p ON p.id=k.project_id WHERE k.name=?1", [name], |r| Ok(Project { id:r.get(0)?,name:r.get(1)? })).optional()?;
    match project {
        Some(p) if empty_git_metadata(db, &p.id)? => Ok(None),
        other => Ok(other),
    }
}

pub(super) fn canonical(db: &Connection, project: Project) -> Result<Project> {
    Ok(by_name(db, &project.name)?.unwrap_or(project))
}

pub(super) fn warnings(db: &Connection) -> Result<Value> {
    let mut stmt = db.prepare("SELECT name,project_id,rejected_id,legacy FROM project_name_collisions ORDER BY name COLLATE NOCASE,rejected_id")?;
    let rows = stmt.query_map([], |r| {
        let name: String = r.get(0)?;
        let legacy: bool = r.get(3)?;
        Ok(json!({"name":name,"project_id":r.get::<_,String>(1)?,"rejected_id":r.get::<_,String>(2)?,"legacy":r.get::<_,bool>(3)?,
            "message":if legacy { format!("Project {name} had multiple identities. One destination is listed; saved history is preserved.") } else { format!("Project {name} already exists. Another identity was detected; no new project was created.") }}))
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut visible = Vec::with_capacity(rows.len());
    for w in rows {
        if !empty_git_metadata(db, w["rejected_id"].as_str().unwrap())?
            && !empty_git_metadata(db, w["project_id"].as_str().unwrap())?
            && !super::super::identity::is_home_project(&Project {
                id: w["rejected_id"].as_str().unwrap().into(),
                name: w["name"].as_str().unwrap().into(),
            })
        {
            visible.push(w);
        }
    }
    Ok(json!(visible))
}
