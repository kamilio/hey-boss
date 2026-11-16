// Scheduled definitions only. One-off commands, logs and run history stay in the database.
use super::*;
use crate::jobs::{Definition, Operation as Job, Snapshot};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PartialEq)]
struct JobFile {
    enabled: bool,
    #[serde(flatten)]
    definition: Definition,
}

pub(super) struct Jobs(BTreeMap<String, (Snapshot, bool)>);

impl Jobs {
    pub(super) fn load(store: &Store, project: &Project) -> Result<Self> {
        let rows = store.db.query_collect("SELECT snapshot,enabled FROM scheduled_jobs WHERE project_id=?1 AND deleted_at IS NULL AND json_extract(snapshot,'$.definition.execution') IS NULL ORDER BY id", [&project.id], |r| Ok::<_,rusqlite::Error>((r.get::<_,String>(0)?,r.get::<_,bool>(1)?)))?;
        let mut jobs = BTreeMap::new();
        for (raw, enabled) in rows {
            let snapshot: Snapshot = serde_json::from_str(&raw)?;
            crate::jobs::validate_id(&snapshot.job_id)?;
            jobs.insert(snapshot.job_id.clone(), (snapshot, enabled));
        }
        Ok(Self(jobs))
    }

    pub(super) fn files(&self, files: &mut Files) -> Result<()> {
        for (id, (s, enabled)) in &self.0 {
            let yaml = serde_yaml_ng::to_string(&JobFile {
                enabled: *enabled,
                definition: s.definition.clone(),
            })
            .map_err(|e| Error::invalid(e.to_string()))?;
            files.insert(format!("jobs/{id}.yaml"), Some(yaml));
            if s.definition.executable.is_none() {
                files.insert(format!("jobs/{id}.md"), Some(s.instruction_digest.clone()));
            }
        }
        Ok(())
    }

    pub(super) fn apply(
        &self,
        store: &mut Store,
        project: &Project,
        actor: &Actor,
        dir: &Path,
        previous: &Files,
        disk: &Files,
        current: &Files,
    ) -> Result<bool> {
        let mut changed = false;
        for (key, text) in disk {
            let Some(id) = key
                .strip_prefix("jobs/")
                .and_then(|s| s.strip_suffix(".yaml"))
            else {
                continue;
            };
            let md = format!("jobs/{id}.md");
            let content = |map: &Files, key: &str| map.get(key).cloned().flatten();
            let old = self.0.get(id);
            let yaml_edit = text.is_some()
                && text != previous.get(key).unwrap_or(&None)
                && text != current.get(key).unwrap_or(&None);
            let markdown = content(disk, &md);
            let md_edit = markdown.is_some()
                && markdown != content(previous, &md)
                && markdown != content(current, &md);
            if !yaml_edit && !md_edit {
                continue;
            }
            if old.is_none() && content(previous, key).is_some() {
                return Err(Error::conflict(format!(
                    "{id} was deleted while its files were edited"
                )));
            }
            if (yaml_edit
                && content(previous, key).is_some()
                && content(previous, key) != content(current, key))
                || (md_edit
                    && content(previous, &md).is_some()
                    && content(previous, &md) != content(current, &md))
            {
                return Err(Error::conflict(format!(
                    "{id} changed both on disk and in Jobs; reconcile the edits"
                )));
            }
            let text = if yaml_edit {
                text.as_ref()
            } else {
                current.get(key).and_then(Option::as_ref)
            }
            .ok_or_else(|| Error::invalid(format!("Missing {key}")))?;
            let record: JobFile =
                serde_yaml_ng::from_str(text).map_err(|e| Error::invalid(format!("{key}: {e}")))?;
            if record.definition.execution.is_some() {
                return Err(Error::invalid("Job files describe schedules only"));
            }
            let markdown = if md_edit {
                let observed = read(&dir.join(&md))?
                    .ok_or_else(|| Error::conflict("Job instructions disappeared while syncing"))?;
                if Some(crate::jobs::files::digest(&observed)) != markdown {
                    return Err(Error::conflict("Job instructions changed while syncing"));
                }
                Some(observed)
            } else {
                None
            };
            let submit = |store: &mut Store, op: Job| -> Result<Value> {
                let mut req = request(project, actor, Operation::Job { operation: op });
                req.request_id = Some(format!(
                    "config-job-{}",
                    project_config::digest(&serde_json::to_value(&req.operation)?)
                ));
                store.execute(&req)
            };
            if let Some((s, enabled)) = old {
                let mut rev = s.revision;
                if s.definition != record.definition || md_edit {
                    let result = submit(
                        store,
                        Job::Edit {
                            id: id.into(),
                            if_revision: rev,
                            definition: record.definition,
                            markdown,
                        },
                    )?;
                    rev = result["job"]["revision"]
                        .as_i64()
                        .ok_or_else(|| Error::invalid("Missing job revision"))?;
                }
                if *enabled != record.enabled {
                    submit(
                        store,
                        Job::SetEnabled {
                            id: id.into(),
                            if_revision: rev,
                            enabled: record.enabled,
                        },
                    )?;
                }
            } else {
                submit(
                    store,
                    Job::Create {
                        id: id.into(),
                        definition: record.definition,
                        markdown: markdown.unwrap_or_default(),
                        enabled: record.enabled,
                    },
                )?;
            }
            changed = true;
        }
        Ok(changed)
    }
}

pub(super) fn discover(dir: &Path, keys: &mut Files) -> Result<()> {
    for entry in fs::read_dir(dir.join("jobs"))? {
        let name = entry?.file_name();
        let Some((id, ext)) = name.to_str().and_then(|s| s.rsplit_once('.')) else {
            continue;
        };
        if ext == "yaml" || ext == "md" {
            crate::jobs::validate_id(id)?;
            keys.entry(format!("jobs/{id}.{ext}")).or_default();
        }
    }
    let mut spellings = std::collections::HashMap::new();
    for key in keys.keys().filter_map(|k| k.strip_prefix("jobs/")) {
        if let Some((id, _)) = key.rsplit_once('.')
            && spellings
                .insert(id.to_ascii_lowercase(), id)
                .is_some_and(|prior| prior != id)
        {
            return Err(Error::conflict(format!(
                "Two job IDs differ only by case: {id}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkout_and_database_share_jobs_without_leaking_private_files_to_git() {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        };
        assert!(git(&["init", "-q"]));
        let global = temp.path().join("ignore");
        fs::write(&global, ".hey-boss\n").unwrap();
        assert!(git(&[
            "config",
            "core.excludesFile",
            global.to_str().unwrap()
        ]));
        let db = temp.path().join("state.db");
        let p = Project {
            id: "named:job-files".into(),
            name: "job-files".into(),
        };
        let mut store = Store::open(&db).unwrap();
        store
            .db
            .execute(
                "INSERT INTO projects(id,name,next_number) VALUES(?1,?2,1)",
                params![p.id, p.name],
            )
            .unwrap();
        let sync = || sync_root(&db, &root, &p);
        sync().unwrap();
        let dir = root.join(".hey-boss");
        let yaml = dir.join("jobs/inbox.yaml");
        let md = dir.join("jobs/inbox.md");
        fs::write(
            &yaml,
            "enabled: false\nname: Inbox\ncron: '5 * * * *'\ntimezone: UTC\nharness: codex\n",
        )
        .unwrap();
        let instructions = include_str!("../jobs/fixtures/original.md");
        fs::write(&md, instructions).unwrap();
        sync().unwrap();
        let get = || {
            store
                .db
                .query_row(
                    "SELECT enabled,snapshot FROM scheduled_jobs WHERE id='inbox'",
                    [],
                    |r| Ok((r.get::<_, bool>(0)?, r.get::<_, String>(1)?)),
                )
                .unwrap()
        };
        let (enabled, json) = get();
        assert!(!enabled);
        let s: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(store.job_instructions(&s).unwrap(), instructions);
        assert!(git(&["check-ignore", "-q", ".hey-boss/jobs/inbox.yaml"]));
        let config = dir.join("config.yaml");
        fs::write(
            &config,
            fs::read_to_string(&config)
                .unwrap()
                .replace("track_config: false", "track_config: true"),
        )
        .unwrap();
        sync().unwrap();
        assert!(!git(&["check-ignore", "-q", ".hey-boss/jobs/inbox.yaml"]));
        assert!(!git(&["check-ignore", "-q", ".hey-boss/jobs/inbox.md"]));
        assert!(git(&["check-ignore", "-q", ".hey-boss/project.json"]));
        assert!(git(&["check-ignore", "-q", ".hey-boss/secret"]));
        fs::remove_file(&md).unwrap();
        sync().unwrap();
        assert_eq!(fs::read_to_string(&md).unwrap(), instructions);
        let mut def = s.definition;
        def.name = "Changed from Jobs".into();
        let actor = identity::resolve_inspection(Some("human:test"), "test", &root).unwrap();
        let mut req = request(
            &p,
            &actor,
            Operation::Job {
                operation: Job::Edit {
                    id: "inbox".into(),
                    if_revision: s.revision,
                    definition: def,
                    markdown: Some(include_str!("../jobs/fixtures/revised.md").into()),
                },
            },
        );
        req.request_id = Some("edit-inbox".into());
        store.execute(&req).unwrap();
        sync().unwrap();
        assert!(
            fs::read_to_string(&yaml)
                .unwrap()
                .contains("Changed from Jobs")
        );
        assert_eq!(
            fs::read_to_string(&md).unwrap(),
            include_str!("../jobs/fixtures/revised.md")
        );
        fs::write(
            &yaml,
            fs::read_to_string(&yaml)
                .unwrap()
                .replace("enabled: false", "enabled: true")
                .replace("Changed from Jobs", "Edited locally"),
        )
        .unwrap();
        sync().unwrap();
        assert!(
            store
                .db
                .query_row(
                    "SELECT enabled FROM scheduled_jobs WHERE id='inbox'",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
        );
        assert_eq!(sync().unwrap()["changed"], false);
    }
}
