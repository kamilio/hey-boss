//! Durable native editing sessions. The editor never writes installed agent copies directly.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
struct Session {
    skill: String,
    file: String,
    digest: String,
    source: Value,
    #[serde(default)]
    choice: Option<String>,
    #[serde(default)]
    completed: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    content: String,
    request_id: String,
}
fn directory(home: &Path, id: &str) -> Result<PathBuf> {
    if id.len() != 64 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::invalid("Invalid skill editing session"));
    }
    Ok(home.join(".hey-boss/skill-edits").join(id))
}
fn store(path: &Path, session: &Session) -> Result<()> {
    atomic_write(&path.join("session.json"), &serde_json::to_vec(session)?)?;
    fs::set_permissions(path.join("session.json"), fs::Permissions::from_mode(0o600))?;
    Ok(())
}
pub(super) fn prepare(home: &Path, state: &State, action: &Action) -> Result<(String, PathBuf)> {
    let source = find_source_copy(state, &action.skill, &action.base_digest)
        .ok_or_else(|| Error::conflict("Scan the skill before editing."))?;
    let file = if action.file_path.is_empty() || action.file_path == "AGENTS.md" {
        "SKILL.md"
    } else {
        &action.file_path
    };
    let rel = Path::new(file);
    if rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || !matches!(
            rel.extension().and_then(|s| s.to_str()),
            Some("md" | "markdown")
        )
    {
        return Err(Error::invalid("Choose a Markdown file in this skill."));
    }
    source["files"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["path"] == file)
        .and_then(|f| f["data"].as_str())
        .ok_or_else(|| Error::invalid("Markdown file is missing from this skill."))?;
    // One window per skill/file/version; reopening cannot reset an unsent draft.
    let id = format!(
        "{:x}",
        Sha256::digest(format!("{}\0{}\0{}", action.skill, file, source["digest"]).as_bytes())
    );
    let dir = directory(home, &id)?;
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let path = dir.join(file);
    if !dir.join("session.json").exists() {
        // Keep relative references and preview images available in the writing window.
        for file in source["files"].as_array().into_iter().flatten() {
            let relative = Path::new(file["path"].as_str().unwrap_or_default());
            if relative.as_os_str().is_empty()
                || relative
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err(Error::invalid("Invalid skill file path"));
            }
            atomic_write(
                &dir.join(relative),
                &STANDARD
                    .decode(file["data"].as_str().unwrap_or_default())
                    .map_err(|_| Error::invalid("Invalid skill file"))?,
            )?;
        }
        store(
            &dir,
            &Session {
                skill: action.skill.clone(),
                file: file.into(),
                digest: source["digest"].as_str().unwrap_or_default().into(),
                source,
                choice: state.choices.get(&action.skill).cloned(),
                completed: BTreeMap::new(),
            },
        )?;
    }
    Ok((id, path))
}

pub fn save(id: &str, bytes: &[u8]) -> Result<Value> {
    let request: Save = serde_json::from_slice(bytes)?;
    if request.content.len() > 4 * 1024 * 1024
        || request.request_id.is_empty()
        || request.request_id.len() > 128
    {
        return Err(Error::invalid("Invalid skill save request"));
    }
    let home = home()?;
    let dir = directory(&home, id)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("save.lock"))?;
    use std::os::fd::AsRawFd;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(Error::new(
            "sync_pending",
            "Skill save is still running; retrying…",
        ));
    }
    let mut session: Session = serde_json::from_slice(&fs::read(dir.join("session.json"))?)?;
    let fingerprint = format!("{:x}", Sha256::digest(request.content.as_bytes()));
    if let Some(previous) = session.completed.get(&request.request_id) {
        if previous != &fingerprint {
            return Err(Error::conflict(
                "Save request was reused with different text.",
            ));
        }
        return Ok(json!({"ok":true}));
    }
    report()?;
    {
        let current = state().lock().unwrap();
        // A process can stop after committing a rename but before updating this journal.
        let next = super::super::manager_rename::edited_name(
            &session.skill,
            &session.file,
            &request.content,
        )?;
        if next != session.skill {
            let expected = super::super::manager_rename::renamed_source(
                &session.source,
                &next,
                &request.content,
            )?;
            if let Some(saved) = find_source_copy(
                &current,
                &next,
                expected["digest"].as_str().unwrap_or_default(),
            ) && saved["digest"] == expected["digest"]
            {
                session.skill = next;
                session.digest = saved["digest"].as_str().unwrap_or_default().into();
                session.source = saved;
                session.choice = current.choices.get(&session.skill).cloned();
            }
        }
        // Other open files may have saved since this window opened. Keep those edits.
        if current.choices.get(&session.skill) != session.choice.as_ref()
            && let Some(source) = find_source_copy(&current, &session.skill, "")
        {
            session.digest = source["digest"].as_str().unwrap_or_default().into();
            session.source = source;
        }
    }
    let mut action: Action = serde_json::from_value(
        json!({"action":"save_file","skill":session.skill,"file_path":session.file,"content":request.content,"base_digest":session.digest,"distribute":true}),
    )?;
    action.source = Some(session.source.clone());
    let initial = act(action).map_err(|error| {
        if error.message.contains("already running") || error.message.contains("Retry shortly") {
            Error::new("sync_pending", error.message)
        } else {
            error
        }
    })?;
    let saved_name = initial["saved_skill"]
        .as_str()
        .unwrap_or(&session.skill)
        .to_owned();
    // Record the local result before waiting on the network, including a rename.
    session.skill = saved_name;
    session.digest = initial["choices"][&session.skill]
        .as_str()
        .unwrap_or_default()
        .into();
    session.choice = Some(session.digest.clone());
    {
        let current = state().lock().unwrap();
        session.source = find_source_copy(&current, &session.skill, &session.digest)
            .ok_or_else(|| Error::invalid("Saved skill is missing"))?;
    }
    atomic_write(&dir.join(&session.file), request.content.as_bytes())?;
    store(&dir, &session)?;
    let report = wait_until_idle()?;
    let failures: Vec<_> = hosts(&home)?
        .into_iter()
        .filter(|host| {
            !report["machines"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|m| m["host"] == *host && m["state"] == "synced" && !m["error"].is_string())
        })
        .collect();
    if !failures.is_empty() {
        return Err(Error::new(
            "sync_pending",
            format!(
                "Saved locally · sync pending on {}. Retrying…",
                failures.join(", ")
            ),
        ));
    }
    session.completed.insert(request.request_id, fingerprint);
    store(&dir, &session)?;
    Ok(json!({"ok":true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_keeps_installed_files_and_references_and_reuses_pending_session() {
        let temp = crate::admin::Temporary::new().unwrap();
        let installed = temp.0.join(".codex/skills/example/SKILL.md");
        atomic_write(&installed, b"---\nname: example\n---\nOriginal\n").unwrap();
        atomic_write(
            &installed.parent().unwrap().join("references/notes.md"),
            b"# Notes",
        )
        .unwrap();
        let mut machine = transport(&temp.0, "local", &json!({"action":"scan"})).unwrap();
        machine["host"] = json!("local");
        let state = State {
            machines: vec![machine],
            ..State::default()
        };
        let action: Action = serde_json::from_value(
            json!({"action":"open_editor","skill":"example","file_path":"SKILL.md"}),
        )
        .unwrap();
        let (id, file) = prepare(&temp.0, &state, &action).unwrap();
        assert_ne!(file, installed);
        assert_eq!(
            fs::read(file.parent().unwrap().join("references/notes.md")).unwrap(),
            b"# Notes"
        );
        fs::write(&file, "Pending draft").unwrap();
        assert_eq!(prepare(&temp.0, &state, &action).unwrap().0, id);
        assert_eq!(fs::read_to_string(&file).unwrap(), "Pending draft");
        assert!(fs::read_to_string(installed).unwrap().contains("Original"));
        let invalid: Action = serde_json::from_value(
            json!({"action":"open_editor","skill":"example","file_path":"../escape.md"}),
        )
        .unwrap();
        assert!(prepare(&temp.0, &state, &invalid).is_err());
        assert!(directory(&temp.0, "../escape").is_err());
    }
}
