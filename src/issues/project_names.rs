//! Names select one destination; old storage IDs remain readable for history.
use super::*;

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='project_name_keys')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
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
    Ok(db.query_row("SELECT p.id,p.name FROM project_name_keys k JOIN projects p ON p.id=k.project_id WHERE k.name=?1", [name], |r| Ok(Project { id:r.get(0)?,name:r.get(1)? })).optional()?)
}

pub(super) fn canonical(db: &Connection, project: Project) -> Result<Project> {
    Ok(by_name(db, &project.name)?.unwrap_or(project))
}

pub(super) fn record(db: &Connection, incoming: &Project, resolved: &Project) -> Result<()> {
    if incoming.id != resolved.id && incoming.name.eq_ignore_ascii_case(&resolved.name) {
        db.execute("INSERT OR IGNORE INTO project_name_collisions(rejected_id,name,project_id,legacy) VALUES(?1,?2,?3,EXISTS(SELECT 1 FROM projects WHERE id=?1))", params![incoming.id,resolved.name,resolved.id])?;
    }
    Ok(())
}

pub(super) fn record_override(
    db: &Connection,
    value: Option<&str>,
    resolved: &Project,
) -> Result<()> {
    if let Some(value) =
        value.filter(|v| v.contains('/') || v.starts_with("named:") || v.starts_with("local:"))
    {
        let name = value.rsplit('/').next().unwrap_or(value);
        record(
            db,
            &Project {
                id: value.into(),
                name: name.strip_prefix("named:").unwrap_or(name).into(),
            },
            resolved,
        )?;
    }
    Ok(())
}

pub(super) fn warnings(db: &Connection) -> Result<Value> {
    let mut stmt = db.prepare("SELECT name,project_id,rejected_id,legacy FROM project_name_collisions ORDER BY name COLLATE NOCASE,rejected_id")?;
    let rows = stmt.query_map([], |r| {
        let name: String = r.get(0)?;
        let legacy: bool = r.get(3)?;
        Ok(json!({"name":name,"project_id":r.get::<_,String>(1)?,"rejected_id":r.get::<_,String>(2)?,"legacy":r.get::<_,bool>(3)?,
            "message":if legacy { format!("Project {name} had multiple identities. One destination is listed; saved history is preserved.") } else { format!("Project {name} already exists. Another identity was detected; no new project was created.") }}))
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!(
        rows.into_iter()
            .filter(|w| !super::super::identity::is_home_project(&Project {
                id: w["rejected_id"].as_str().unwrap().into(),
                name: w["name"].as_str().unwrap().into()
            }))
            .collect::<Vec<_>>()
    ))
}
