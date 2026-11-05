//! Local working copies; paths and reconciliation journals stay on this machine.
use super::{
    Actor, Error, Operation, Project, Request, Result, Store, identity, project_config, worker,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

type Files = BTreeMap<String, Option<String>>;

fn read(path: &Path) -> Result<Option<String>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Error::invalid(format!(
                "Cannot read {}: {e}",
                path.display()
            )));
        }
    };
    if !file.metadata()?.is_file() {
        return Err(Error::invalid(format!(
            "{} must be a regular file",
            path.display()
        )));
    }
    let mut text = String::new();
    file.take(64_001).read_to_string(&mut text)?;
    if text.len() > 64_000 {
        return Err(Error::invalid(format!("{} exceeds 64 KB", path.display())));
    }
    Ok(Some(text))
}

fn directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(Error::invalid(format!(
            "{} must be a directory, not a link or file",
            path.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(path)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn write(path: &Path, text: &str) -> Result<()> {
    let temporary = path.with_file_name(format!(
        ".{}.{}.new",
        path.file_name().unwrap().to_string_lossy(),
        worker::random_id()?
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(temporary);
    result
}

fn root(cwd: &Path) -> Result<PathBuf> {
    let cwd = cwd.canonicalize()?;
    if let Some(git) = crate::agents::git_repository_info(&cwd.to_string_lossy()) {
        return Ok(PathBuf::from(git.worktree));
    }
    Ok(cwd
        .ancestors()
        .find(|p| p.join(".hey-boss").is_file() || p.join(".hey-boss/project.json").is_file())
        .unwrap_or(&cwd)
        .to_owned())
}

fn materialize_identity(root: &Path, project: &Project) -> Result<PathBuf> {
    let dir = root.join(".hey-boss");
    let backup = root.join(".hey-boss.identity-backup");
    if fs::symlink_metadata(&dir).is_ok_and(|m| m.is_file()) {
        let saved: Project = serde_json::from_str(&read(&dir)?.unwrap())?;
        if saved.id != project.id {
            return Err(Error::conflict("Checkout belongs to another project"));
        }
        if backup.exists() {
            return Err(Error::conflict(
                "An unfinished project identity migration needs recovery",
            ));
        }
        fs::rename(&dir, &backup)?;
    }
    directory(&dir)?;
    if let Some(text) = read(&backup)? {
        let saved: Project = serde_json::from_str(&text)?;
        if saved.id != project.id {
            return Err(Error::conflict("Migration belongs to another project"));
        }
        if let Some(existing) = read(&dir.join("project.json"))? {
            if serde_json::from_str::<Project>(&existing)?.id != saved.id {
                return Err(Error::conflict("Project identity changed during migration"));
            }
        } else {
            write(&dir.join("project.json"), &text)?;
        }
        fs::remove_file(backup)?;
    }
    if let Some(text) = read(&dir.join("project.json"))? {
        let saved: Project = serde_json::from_str(&text)?;
        if saved.id != project.id {
            return Err(Error::conflict("Checkout belongs to another project"));
        }
        if saved.name != project.name {
            write(
                &dir.join("project.json"),
                &(serde_json::to_string_pretty(project)? + "\n"),
            )?;
        }
    } else {
        write(
            &dir.join("project.json"),
            &(serde_json::to_string_pretty(project)? + "\n"),
        )?;
    }
    directory(&dir.join("prompts"))?;
    Ok(dir)
}

fn config(settings: &Value) -> Value {
    let mut value = project_config::snapshot(settings);
    value.as_object_mut().unwrap().remove("prompt");
    value.as_object_mut().unwrap().remove("prompt_overrides");
    value
}
fn parse_config(text: &str) -> Result<Value> {
    let value: Value =
        serde_yaml_ng::from_str(text).map_err(|e| Error::invalid(format!("config.yaml: {e}")))?;
    let expected = config(&json!({}));
    let object = value
        .as_object()
        .ok_or_else(|| Error::invalid("config.yaml must contain settings"))?;
    if object.keys().ne(expected.as_object().unwrap().keys()) {
        return Err(Error::invalid(
            "config.yaml must retain its settings fields; prompt text belongs in prompts/*.md",
        ));
    }
    Ok(value)
}
fn desired(settings: &Value, existing: &Files) -> Result<Files> {
    let mut files = Files::new();
    let config = config(settings);
    let yaml = existing
        .get("config.yaml")
        .and_then(|v| v.as_ref())
        .filter(|text| parse_config(text).ok().as_ref() == Some(&config))
        .cloned()
        .unwrap_or(serde_yaml_ng::to_string(&config).map_err(|e| Error::invalid(e.to_string()))?);
    files.insert("config.yaml".into(), Some(yaml));
    files.insert(
        "prompts/worker.md".into(),
        Some(settings["prompt"].as_str().unwrap().into()),
    );
    for (key, default) in worker::prompt_defaults().as_object().unwrap() {
        files.insert(
            format!("prompts/{key}.md"),
            Some(
                settings["prompt_overrides"][key]
                    .as_str()
                    .unwrap_or(default.as_str().unwrap())
                    .into(),
            ),
        );
    }
    Ok(files)
}
fn observed(dir: &Path, template: &Files) -> Result<Files> {
    template
        .keys()
        .map(|key| Ok((key.clone(), read(&dir.join(key))?)))
        .collect()
}
fn edit(before: &Files, after: &Files) -> Result<Value> {
    let mut changes = serde_json::Map::new();
    for (key, text) in after {
        if before.get(key) == Some(text) {
            continue;
        }
        if key == "config.yaml" {
            let next = parse_config(text.as_deref().ok_or_else(|| {
                Error::invalid("config.yaml is missing; restore it before syncing")
            })?)?;
            let previous = before
                .get(key)
                .and_then(|v| v.as_deref())
                .map(parse_config)
                .transpose()?
                .unwrap_or(json!({}));
            for (key, value) in next.as_object().unwrap() {
                if previous[key] != *value {
                    changes.insert(key.clone(), value.clone());
                }
            }
        } else if key == "prompts/worker.md" {
            changes.insert(
                "prompt".into(),
                json!(text.as_deref().unwrap_or(worker::DEFAULT_PROMPT)),
            );
        } else {
            let section = key
                .strip_prefix("prompts/")
                .unwrap()
                .strip_suffix(".md")
                .unwrap();
            changes.insert(format!("prompt_overrides.{section}"), json!(text));
        }
    }
    Ok(Value::Object(changes))
}

fn request(project: &Project, actor: &Actor, operation: Operation) -> Request {
    Request {
        version: 1,
        project: project.clone(),
        project_override: Some(project.id.clone()),
        actor: Some(actor.clone()),
        operation,
        request_id: None,
    }
}

/// Registers this checkout and reconciles it. One lock covers manual and service
/// syncs; a durable materialization intent makes interrupted writes recoverable.
pub fn sync(path: &Path, cwd: &Path, project: &Project) -> Result<Value> {
    sync_root(path, &root(cwd)?, project)
}

fn sync_root(path: &Path, root: &Path, project: &Project) -> Result<Value> {
    let machine = identity::machine()?;
    let mut author = identity::resolve_inspection(
        Some(&format!(
            "filesystem:{}",
            std::env::var("USER").unwrap_or_else(|_| "local".into())
        )),
        &machine,
        root,
    )?;
    author.source = "filesystem".into();
    let actor = &author;
    let lock_path = path.with_file_name(format!(
        "config-{}.lock",
        project_config::digest(&json!(root))
    ));
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock_path)?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut store = Store::open(path)?;
    let mut settings = store.execute(&request(project, actor, Operation::ProjectSettings))?;
    let dir = materialize_identity(root, project)?;
    let saved: Option<(String, String, Option<String>)> = store
        .db
        .query_row(
            "SELECT project_id,base,error FROM project_config_checkouts WHERE path=?1",
            [root.to_string_lossy().as_ref()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if saved.as_ref().is_some_and(|(id, _, _)| id != &project.id) {
        return Err(Error::conflict(
            "This working copy is registered to another project",
        ));
    }
    let template = desired(&settings, &Files::new())?;
    let mut disk = observed(&dir, &template)?;
    // Recreate an emptied working copy from the saved configuration, including
    // after an interrupted directory recreation.
    let saved = saved.filter(|_| disk.values().any(Option::is_some));
    let initial = saved.is_none();
    let had_error = saved.as_ref().is_some_and(|(_, _, error)| error.is_some());
    let mut base = match saved {
        Some((_, raw, _)) => serde_json::from_str::<Value>(&raw)?,
        None => {
            // Existing files are user edits. Missing files on initial setup are
            // populated from the database rather than resetting saved overrides.
            let files: Files = template
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        if disk[key].is_some() {
                            value.clone()
                        } else {
                            None
                        },
                    )
                })
                .collect();
            json!({"files":files,"revision":null})
        }
    };
    let recovering = !base["pending"].is_null();
    if recovering {
        let previous: Files = serde_json::from_value(base["files"].clone())?;
        let pending: Files = serde_json::from_value(base["pending"].clone())?;
        for (key, text) in &pending {
            if disk.get(key) == previous.get(key) && disk.get(key) != Some(text) {
                if let Some(text) = text {
                    write(&dir.join(key), text)?;
                }
                disk.insert(key.clone(), text.clone());
            }
        }
        base["files"] = json!(pending);
        base.as_object_mut().unwrap().remove("pending");
    }
    let previous: Files = serde_json::from_value(base["files"].clone())?;
    let changes = edit(&previous, &disk)?;
    let changed = !changes.as_object().unwrap().is_empty();
    if changed {
        let mut req = request(
            project,
            actor,
            Operation::ProjectConfig {
                operation: project_config::Operation::Apply {
                    changes,
                    base: base["revision"].as_str().map(str::to_owned),
                },
            },
        );
        req.request_id = Some(format!(
            "config-files-{}",
            project_config::digest(&json!({"root":root,"base":base,"operation":req.operation}))
        ));
        store.execute(&req)?;
        settings = store.execute(&request(project, actor, Operation::ProjectSettings))?;
    }
    let next = desired(&settings, &disk)?;
    let revision: Option<String> = store.db.query_row("SELECT id FROM project_config_revisions WHERE project_id=?1 ORDER BY sequence DESC,id DESC LIMIT 1", [&project.id], |r| r.get(0)).optional()?;
    let pending = json!({"files":disk,"pending":next,"revision":revision});
    let root_string = root.to_string_lossy();
    if disk != next
        || base["revision"] != json!(revision)
        || initial
        || recovering
        || base["files"] != json!(next)
    {
        store.db.execute("INSERT INTO project_config_checkouts(path,project_id,base,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(path) DO UPDATE SET base=excluded.base,error=NULL,updated_at=excluded.updated_at", params![root_string,project.id,pending.to_string(),worker::now()])?;
        for (key, text) in &next {
            if disk.get(key) != Some(text) && read(&dir.join(key))? == disk[key] {
                if let Some(text) = text {
                    write(&dir.join(key), text)?;
                }
            }
        }
        store.db.execute(
            "UPDATE project_config_checkouts SET base=?2,error=NULL WHERE path=?1",
            params![
                root_string,
                json!({"files":next,"revision":revision}).to_string()
            ],
        )?;
    } else if had_error {
        store.db.execute(
            "UPDATE project_config_checkouts SET error=NULL WHERE path=?1 AND error IS NOT NULL",
            [&*root_string],
        )?;
    }
    Ok(json!({"ok":true,"project":project,"directory":dir,"revision":revision,"changed":changed}))
}

pub(crate) fn reconcile(path: &Path) -> Result<()> {
    let store = Store::open(path)?;
    let mut rows = store.db.prepare("SELECT c.path,p.id,p.name,c.error FROM project_config_checkouts c JOIN projects p ON p.id=c.project_id")?
        .query_map([], |r| Ok((PathBuf::from(r.get::<_,String>(0)?), (Project {id:r.get(1)?,name:r.get(2)?},r.get::<_,Option<String>>(3)?))))?.collect::<rusqlite::Result<BTreeMap<_,_>>>()?;
    let configs = path.parent().unwrap().join("project-configs");
    let projects = store.db.prepare("SELECT p.id,p.name FROM projects p JOIN project_name_keys k ON k.project_id=p.id WHERE p.hidden_at IS NULL")?
        .query_map([], |r| Ok(Project {id:r.get(0)?,name:r.get(1)?}))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for project in projects {
        let slug = project
            .id
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let slug = &slug[..slug.len().min(80)];
        let hash = project_config::digest(&json!(project.id));
        let root = configs.join(format!("{slug}-{}", &hash[..16]));
        rows.entry(root).or_insert((project, None));
    }
    for (root, (project, previous_error)) in rows {
        let result = (|| -> Result<()> {
            if root.parent() == Some(&configs) {
                directory(&configs)?;
                directory(&root)?;
            } else if !root.is_dir() {
                return Ok(());
            }
            sync_root(path, &root, &project)?;
            Ok(())
        })();
        if let Err(error) = result {
            if previous_error.as_deref() == Some(&error.to_string()) {
                continue;
            }
            let recorded = store.db.execute(
                "UPDATE project_config_checkouts SET error=?2 WHERE path=?1 AND error IS NOT ?2",
                params![root.to_string_lossy(), error.to_string()],
            )?;
            if recorded == 0 {
                eprintln!("Project configuration {}: {error}", root.display());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_materializes_projects_without_workers_or_checkouts() {
        let root = std::env::current_dir()
            .unwrap()
            .join(format!(".config-host-{}", worker::random_id().unwrap()));
        fs::create_dir(&root).unwrap();
        let path = root.join("issues.db");
        let mut store = Store::open(&path).unwrap();
        let project = Project {
            id: "named:../../Host config".into(),
            name: "Host config".into(),
        };
        let actor = identity::resolve_inspection(Some("human:test"), "test", &root).unwrap();
        let init = || {
            serde_json::from_value(json!({"action":"project_init","settings":{"prs_enabled":false,"worktree_enabled":false,"if_version":0}})).unwrap()
        };
        store.execute(&request(&project, &actor, init())).unwrap();
        reconcile(&path).unwrap();
        let directories = fs::read_dir(root.join("project-configs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(directories.len(), 1);
        let copy = &directories[0];
        let yaml = copy.join(".hey-boss/config.yaml");
        let original = fs::read_to_string(&yaml).unwrap();
        let identity: Project =
            serde_json::from_str(&fs::read_to_string(copy.join(".hey-boss/project.json")).unwrap())
                .unwrap();
        assert_eq!(identity.id, project.id);
        assert!(copy.join(".hey-boss/prompts/worker.md").is_file());
        assert_eq!(
            store
                .db
                .query_row("SELECT count(*) FROM project_config_revisions", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .db
                .query_row("SELECT count(*) FROM issue_workers", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        fs::write(
            &yaml,
            original.replace("prs_enabled: false", "prs_enabled: true"),
        )
        .unwrap();
        reconcile(&path).unwrap();
        assert_eq!(
            store
                .execute(&request(&project, &actor, Operation::ProjectSettings))
                .unwrap()["prs_enabled"],
            true
        );
        let change =
            serde_json::from_value(json!({"action":"configure_project","worktree_enabled":true}))
                .unwrap();
        store.execute(&request(&project, &actor, change)).unwrap();
        reconcile(&path).unwrap();
        assert!(
            fs::read_to_string(&yaml)
                .unwrap()
                .contains("worktree_enabled: true")
        );
        let saved = fs::read_to_string(&yaml).unwrap();
        let revisions: i64 = store
            .db
            .query_row("SELECT count(*) FROM project_config_revisions", [], |r| {
                r.get(0)
            })
            .unwrap();
        fs::remove_dir_all(copy).unwrap();
        reconcile(&path).unwrap();
        assert_eq!(fs::read_to_string(&yaml).unwrap(), saved);
        assert_eq!(
            store
                .db
                .query_row("SELECT count(*) FROM project_config_revisions", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            revisions
        );
        // A broken copy must not stop a newly registered project materializing.
        fs::write(&yaml, "invalid: [").unwrap();
        let second = Project {
            id: "named:Second".into(),
            name: "Second".into(),
        };
        store.execute(&request(&second, &actor, init())).unwrap();
        reconcile(&path).unwrap();
        assert_eq!(
            fs::read_dir(root.join("project-configs")).unwrap().count(),
            2
        );
        assert_eq!(
            store
                .db
                .query_row(
                    "SELECT count(*) FROM project_config_checkouts WHERE error IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        fs::write(&yaml, original).unwrap();
        reconcile(&path).unwrap();
        let before: i64 = store
            .db
            .query_row("PRAGMA data_version", [], |r| r.get(0))
            .unwrap();
        reconcile(&path).unwrap();
        assert_eq!(
            store
                .db
                .query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn service_sync_recovers_invalid_files_and_keeps_idle_checks_read_only() {
        let root = std::env::current_dir()
            .unwrap()
            .join(format!(".config-service-{}", worker::random_id().unwrap()));
        fs::create_dir(&root).unwrap();
        let path = root.join("issues.db");
        let mut store = Store::open(&path).unwrap();
        let project = Project {
            id: "named:File sync".into(),
            name: "File sync".into(),
        };
        let actor = identity::resolve_inspection(Some("human:test"), "test", &root).unwrap();
        let init:Operation=serde_json::from_value(json!({"action":"project_init","settings":{"prs_enabled":false,"worktree_enabled":false,"if_version":0}})).unwrap();
        store.execute(&request(&project, &actor, init)).unwrap();
        sync_root(&path, &root, &project).unwrap();
        let yaml = root.join(".hey-boss/config.yaml");
        let original = fs::read_to_string(&yaml).unwrap();
        fs::write(&yaml, "invalid: [").unwrap();
        reconcile(&path).unwrap();
        let error: Option<String> = store
            .db
            .query_row(
                "SELECT error FROM project_config_checkouts WHERE path=?1",
                [root.to_str().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(error.unwrap().contains("config.yaml"));
        fs::write(
            &yaml,
            original.replace("prs_enabled: false", "prs_enabled: true"),
        )
        .unwrap();
        reconcile(&path).unwrap();
        let settings = store
            .execute(&request(&project, &actor, Operation::ProjectSettings))
            .unwrap();
        assert_eq!(settings["prs_enabled"], true);
        assert_eq!(
            store
                .db
                .query_row(
                    "SELECT error IS NULL FROM project_config_checkouts WHERE path=?1",
                    [root.to_str().unwrap()],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap(),
            true
        );
        let before: i64 = store
            .db
            .query_row("PRAGMA data_version", [], |r| r.get(0))
            .unwrap();
        reconcile(&path).unwrap();
        assert_eq!(
            store
                .db
                .query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before,
            "An unchanged sync must not acquire a writer or create history"
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
