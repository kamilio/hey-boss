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

// Database callers need stored settings only. Skill discovery touches the
// filesystem and belongs to explicit settings responses, not issue transactions.
pub(super) fn read(db: &Connection) -> Result<Value> {
    let (name, version, auto_close, quiet): (String, i64, bool, Option<String>) = db.query_row(
        "SELECT boss_name,version,auto_close_merged_prs,quiet_hours FROM global_settings WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let quiet: crate::quiet_hours::QuietHours = quiet
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_default();
    Ok(
        json!({"ok":true,"scope":"global","quiet_hours":quiet,"boss_name":name,"auto_close_merged_prs":auto_close,"version":version,"boss":{"id":"human:boss","name":name,"version":version}}),
    )
}

pub(super) fn configure(db: &Connection, name: &str, expected: Option<i64>) -> Result<Value> {
    configure_settings(db, Some(name), None, None, expected)
}

fn configure_settings(
    db: &Connection,
    name: Option<&str>,
    auto_close: Option<bool>,
    quiet: Option<&crate::quiet_hours::QuietHours>,
    expected: Option<i64>,
) -> Result<Value> {
    if let Some(quiet) = quiet {
        quiet.validate()?;
    }
    let current = read(db)?;
    let quiet = quiet
        .map(serde_json::to_value)
        .transpose()?
        .unwrap_or_else(|| current["quiet_hours"].clone());
    let name = name.unwrap_or_else(|| current["boss_name"].as_str().unwrap());
    let auto_close =
        auto_close.unwrap_or_else(|| current["auto_close_merged_prs"].as_bool().unwrap());
    identifier(name, "Boss name", 64)?;
    let name = name.trim();
    let version = current["version"].as_i64().unwrap();
    if expected.is_some_and(|expected| expected != version) {
        return Err(Error::conflict(
            "Global settings changed elsewhere; reload before saving",
        ));
    }
    let changed = current["boss_name"] != name
        || current["auto_close_merged_prs"] != auto_close
        || current["quiet_hours"] != quiet;
    if changed {
        db.execute(
            "UPDATE global_settings SET boss_name=?1,auto_close_merged_prs=?2,quiet_hours=?3,version=version+1 WHERE id=1",
            params![name,auto_close,serde_json::to_string(&quiet)?],
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
                    "Request ID was already used for a different global operation",
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
    let mut result = match &request.operation {
        Operation::GlobalSettings => read(db)?,
        Operation::ConfigureGlobal {
            quiet_hours,
            boss_name,
            auto_close_merged_prs,
            selected_skills,
            sync_skills,
            if_version,
        } => {
            if let Some(home_os) = std::env::var_os("HOME") {
                let home = std::path::Path::new(&home_os);
                if let Some(skills) = selected_skills {
                    let _ = crate::skill::set_selected_skills(home, skills);
                }
                if *sync_skills || selected_skills.is_some() {
                    let _ = crate::skill::sync_skills(home, None);
                }
            }
            configure_settings(
                db,
                boss_name.as_deref(),
                *auto_close_merged_prs,
                quiet_hours.as_ref(),
                *if_version,
            )?
        }
        _ => return Err(Error::invalid("Unknown global settings operation")),
    };
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let cwd = std::env::current_dir().ok();
    result["skills"] = home
        .as_deref()
        .map(|h| crate::skill::audit_report(h, cwd.as_deref()))
        .unwrap_or_else(|| json!({"global_skills":[],"project_skills":[],"selected":[],"max_lines_policy":crate::skill::SKILL_MAX_LINES}));
    if let (Some(key), Some(actor)) = (&request.request_id, identity) {
        db.execute("INSERT INTO global_settings_requests(actor,request_id,payload,response) VALUES(?1,?2,?3,?4)",params![actor,key,payload,serde_json::to_string(&result)?])?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiet_hours_validate_and_survive_partial_settings_updates() {
        let root = std::env::temp_dir().join(format!(
            "hb-quiet-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut store = crate::issues::Store::open(&root.join("issues.db")).unwrap();
        let actor = crate::issues::identity::resolve(Some("human:boss"), "test", &root).unwrap();
        let mut run = |operation: Value| {
            store.execute(&Request {
                version: 1,
                project: crate::issues::Project {
                    id: "named:Quiet".into(),
                    name: "Quiet".into(),
                },
                project_override: None,
                actor: Some(actor.clone()),
                operation: serde_json::from_value(operation).unwrap(),
                request_id: None,
            })
        };
        let initial = run(json!({"action":"global_settings"})).unwrap();
        assert_eq!(initial["quiet_hours"]["enabled"], true);
        assert_eq!(initial["quiet_hours"]["start"], "22:00");
        let schedule =
            json!({"enabled":true,"start":"21:30","end":"08:15","time_zone":"America/Chicago"});
        let saved = run(json!({"action":"configure_global","quiet_hours":schedule,"if_version":1}))
            .unwrap();
        assert_eq!(saved["quiet_hours"], schedule);
        assert_eq!(saved["version"], 2);
        assert!(
            run(json!({"action":"configure_global","quiet_hours":schedule,"if_version":1}))
                .is_err()
        );
        let renamed = run(json!({"action":"configure_global","boss_name":"Human"})).unwrap();
        assert_eq!(renamed["quiet_hours"], schedule);
        for (field, value) in [
            ("start", "24:00"),
            ("end", "21:30"),
            ("time_zone", "../etc/passwd"),
        ] {
            let mut invalid = schedule.clone();
            invalid[field] = json!(value);
            assert!(run(json!({"action":"configure_global","quiet_hours":invalid})).is_err());
        }
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ordinary_issue_operations_do_not_scan_skill_files() {
        let root = std::env::temp_dir().join(format!(
            "hb-settings-audits-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut store = crate::issues::Store::open(&root.join("issues.db")).unwrap();
        let actor = crate::issues::identity::resolve(Some("human:boss"), "test", &root).unwrap();
        let request = |operation: Value| Request {
            version: 1,
            project: crate::issues::Project {
                id: "named:Audit".into(),
                name: "Audit".into(),
            },
            project_override: None,
            actor: Some(actor.clone()),
            operation: serde_json::from_value(operation).unwrap(),
            request_id: None,
        };
        crate::database::Connection::open(root.join("issues.db"))
            .unwrap()
            .execute(
                "INSERT INTO projects(id,name,next_number) VALUES('named:Audit','Audit',1)",
                [],
            )
            .unwrap();
        let mut counts = Vec::new();
        for operation in [
            json!({"action":"create","title":"Task","body":"","labels":[],"at_top":false,"draft":false}),
            json!({"action":"view","number":1}),
            json!({"action":"project_settings"}),
        ] {
            crate::skill::AUDIT_COUNT.set(0);
            let result = store.execute(&request(operation.clone())).unwrap();
            assert!(result["ok"] == true);
            counts.push((operation["action"].clone(), crate::skill::AUDIT_COUNT.get()));
        }
        crate::skill::AUDIT_COUNT.set(0);
        assert_eq!(store.close_merged_pull_requests(&actor).unwrap(), 0);
        counts.push((json!("PR completion poll"), crate::skill::AUDIT_COUNT.get()));
        eprintln!("Skill filesystem audits by operation: {counts:?}");
        for operation in [
            json!({"action":"global_settings"}),
            json!({"action":"configure_global","auto_close_merged_prs":false}),
        ] {
            crate::skill::AUDIT_COUNT.set(0);
            let configuring = operation["action"] == "configure_global";
            let mut request = request(operation);
            if configuring {
                request.request_id = Some("audit-settings-request".into());
            }
            let result = store.execute(&request).unwrap();
            assert!(result["skills"]["global_skills"].is_array());
            assert!(result["skills"]["project_skills"].is_array());
            assert!(result["skills"]["selected"].is_array());
            assert_eq!(
                crate::skill::AUDIT_COUNT.get(),
                usize::from(std::env::var_os("HOME").is_some())
            );
            if configuring {
                crate::skill::AUDIT_COUNT.set(0);
                assert_eq!(store.execute(&request).unwrap(), result);
                assert_eq!(crate::skill::AUDIT_COUNT.get(), 0);
            }
        }
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            counts.iter().all(|(_, count)| *count == 0),
            "Hot-path filesystem audits: {counts:?}"
        );
    }

    #[test]
    fn merge_setting_defaults_on_and_legacy_name_edits_preserve_it() {
        let path = std::env::temp_dir().join(format!(
            "hb-merge-setting-{}.db",
            crate::issues::worker::random_id().unwrap()
        ));
        let mut store = crate::issues::Store::open(&path).unwrap();
        let project = crate::issues::Project {
            id: "named:test".into(),
            name: "test".into(),
        };
        let actor =
            crate::issues::identity::resolve(Some("human:boss"), "test", path.parent().unwrap())
                .unwrap();
        let mut run = |operation: Value| {
            store
                .execute(&Request {
                    version: 1,
                    project: project.clone(),
                    project_override: None,
                    actor: Some(actor.clone()),
                    operation: serde_json::from_value(operation).unwrap(),
                    request_id: None,
                })
                .unwrap()
        };
        assert_eq!(
            run(json!({"action":"global_settings"}))["auto_close_merged_prs"],
            true
        );
        assert_eq!(
            run(json!({"action":"configure_global","auto_close_merged_prs":false,"if_version":1}))
                ["version"],
            2
        );
        let result = run(json!({"action":"configure_global","boss_name":"Alex"}));
        assert_eq!(result["auto_close_merged_prs"], false);
        assert_eq!(result["boss_name"], "Alex");
        drop(store);
        let _ = std::fs::remove_file(path);
    }
}
