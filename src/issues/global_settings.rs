//! Human profile shared by every project in the authoritative issue store.
use super::{Error, Operation, Request, Result, identifier};
use crate::database::Connection;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

pub(super) const SCHEMA: &str = "
CREATE TABLE global_settings(id INTEGER PRIMARY KEY CHECK(id=1),boss_name TEXT NOT NULL,version INTEGER NOT NULL);
INSERT INTO global_settings VALUES(1,coalesce((SELECT trim(s.boss_name) FROM project_settings s JOIN projects p ON p.id=s.project_id WHERE trim(s.boss_name)<>'' AND s.boss_name<>'Boss' ORDER BY p.activity_at DESC,p.id LIMIT 1),'Boss'),1);
CREATE TABLE global_settings_requests(actor TEXT NOT NULL,request_id TEXT NOT NULL,payload TEXT NOT NULL,response TEXT NOT NULL,PRIMARY KEY(actor,request_id));
";

pub(super) fn read(db: &Connection) -> Result<Value> {
    let (name, version): (String, i64) = db.query_row(
        "SELECT boss_name,version FROM global_settings WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(
        json!({"ok":true,"scope":"global","boss_name":name,"version":version,"boss":{"id":"human:boss","name":name,"version":version}}),
    )
}

pub(super) fn configure(db: &Connection, name: &str, expected: Option<i64>) -> Result<Value> {
    identifier(name, "Boss name", 64)?;
    let name = name.trim();
    let current = read(db)?;
    let version = current["version"].as_i64().unwrap();
    if expected.is_some_and(|expected| expected != version) {
        return Err(Error::conflict(
            "Global settings changed elsewhere; reload before saving",
        ));
    }
    let changed = current["boss_name"] != name;
    if changed {
        db.execute(
            "UPDATE global_settings SET boss_name=?1,version=version+1 WHERE id=1",
            [name],
        )?;
    }
    let mut result = read(db)?;
    result["changed"] = json!(changed);
    Ok(result)
}

pub(super) fn cached_response(
    db: &Connection,
    request: &Request,
    payload: &str,
) -> Result<Option<Value>> {
    let identity = request.actor.as_ref().map(|actor| actor.id.as_str());
    if let (Some(key), Some(actor)) = (&request.request_id, identity) {
        let previous: Option<(String,String)> = db.query_row("SELECT payload,response FROM global_settings_requests WHERE actor=?1 AND request_id=?2", params![actor,key], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
        if let Some((old, response)) = previous {
            if old != payload {
                return Err(Error::conflict(
                    "Request ID was already used for a different global settings operation",
                ));
            }
            return Ok(Some(serde_json::from_str(&response)?));
        }
    }
    Ok(None)
}

pub(super) fn execute(db: &Connection, request: &Request) -> Result<Value> {
    let payload = serde_json::to_string(&request.operation)?;
    if let Some(response) = cached_response(db, request, &payload)? {
        return Ok(response);
    }
    let identity = request.actor.as_ref().map(|actor| actor.id.as_str());
    let result = match &request.operation {
        Operation::GlobalSettings => read(db)?,
        Operation::ConfigureGlobal {
            boss_name,
            if_version,
        } => configure(db, boss_name, *if_version)?,
        _ => return Err(Error::invalid("Unknown global settings operation")),
    };
    if let (Some(key), Some(actor)) = (&request.request_id, identity) {
        db.execute("INSERT INTO global_settings_requests(actor,request_id,payload,response) VALUES(?1,?2,?3,?4)",params![actor,key,payload,serde_json::to_string(&result)?])?;
    }
    Ok(result)
}
