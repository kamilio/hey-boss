//! Frontmatter-driven directory renames, staged and rolled back as one machine operation.
use super::manager::{LibraryStage, transport};
use super::{atomic_write, is_valid_skill_name};
use crate::issues::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

pub(super) fn edited_name(name: &str, file: &str, content: &str) -> Result<String> {
    if name == "AGENTS.md" || !matches!(file, "" | "SKILL.md") {
        return Ok(name.into());
    }
    let normalized = content.replace("\r\n", "\n");
    let mut lines = normalized.lines();
    if lines.next() != Some("---") {
        return Ok(name.into());
    }
    let mut front = String::new();
    let mut closed = false;
    for line in lines {
        if line == "---" {
            closed = true;
            break;
        }
        front.push_str(line);
        front.push('\n');
    }
    if !closed {
        return Err(Error::invalid(
            "Close the YAML frontmatter with --- before saving.",
        ));
    }
    let metadata: Value = serde_yaml_ng::from_str(&front)
        .map_err(|e| Error::invalid(format!("Invalid skill frontmatter: {e}")))?;
    let Some(value) = metadata.get("name") else {
        return Ok(name.into());
    };
    let next = value
        .as_str()
        .ok_or_else(|| Error::invalid("Skill name must be text."))?;
    if !is_valid_skill_name(next) || next == "AGENTS.md" || next.len() > 128 {
        return Err(Error::invalid(
            "Skill name must contain 1–128 letters, numbers, hyphens, or underscores.",
        ));
    }
    if next != name && (name == "hey-boss" || next == "hey-boss") {
        return Err(Error::invalid(
            "The bundled hey-boss skill cannot be renamed or replaced.",
        ));
    }
    Ok(next.into())
}

fn digest(files: &Value) -> Result<String> {
    // Match the existing fleet protocol's sorted, ASCII-escaped JSON fingerprint.
    use std::fmt::Write;
    let mut canonical = String::new();
    for ch in serde_json::to_string(files)?.chars() {
        if ch.is_ascii() {
            canonical.push(ch);
        } else {
            for unit in ch.encode_utf16(&mut [0; 2]) {
                write!(canonical, "\\u{unit:04x}").unwrap();
            }
        }
    }
    Ok(format!("{:x}", Sha256::digest(canonical.as_bytes())))
}

pub(super) fn renamed_source(base: &Value, name: &str, content: &str) -> Result<Value> {
    let mut files = base["files"].as_array().cloned().ok_or_else(|| {
        Error::conflict("Skill version is unavailable. Scan again before saving.")
    })?;
    let file = files
        .iter_mut()
        .find(|f| f["path"] == "SKILL.md")
        .ok_or_else(|| Error::invalid("Skill is missing SKILL.md."))?;
    file["data"] = json!(STANDARD.encode(content));
    let files = json!(files);
    Ok(json!({"name": name,"scope":"global","text":content,"digest":digest(&files)?,"files":files}))
}

fn targets(home: &Path, name: &str) -> Vec<PathBuf> {
    [".codex", ".agents", ".claude", ".hey-boss"]
        .into_iter()
        .flat_map(|root| {
            [
                home.join(root).join("skills").join(name),
                home.join(root).join("skills").join(format!("{name}.md")),
            ]
        })
        .collect()
}
fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
fn signatures(copies: &Value, names: &[String]) -> BTreeMap<String, String> {
    copies
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["scope"] == "global" && names.iter().any(|n| c["name"] == *n))
        .filter_map(|c| Some((c["path"].as_str()?.into(), c["digest"].as_str()?.into())))
        .collect()
}

/// Internal fleet protocol. Callers supply the observed copies, never arbitrary filesystem paths.
pub fn apply(home: &Path, input: &Value) -> Result<Value> {
    use std::os::fd::AsRawFd;
    fs::create_dir_all(home.join(".hey-boss"))?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join(".hey-boss/skill-install.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let source = &input["source"];
    let name = source["name"]
        .as_str()
        .ok_or_else(|| Error::invalid("Missing skill name"))?;
    let previous: Vec<String> = serde_json::from_value(input["previous_names"].clone())?;
    if previous.is_empty()
        || previous.len() > 128
        || source["scope"] != "global"
        || std::iter::once(name)
            .chain(previous.iter().map(String::as_str))
            .any(|n| {
                !is_valid_skill_name(n) || matches!(n, "AGENTS.md" | "hey-boss") || n.len() > 128
            })
        || previous.iter().any(|n| n == name)
    {
        return Err(Error::invalid("Invalid skill rename"));
    }
    let files = source["files"]
        .as_array()
        .ok_or_else(|| Error::invalid("Missing skill files"))?;
    if files.len() > 512 || digest(&source["files"])? != source["digest"] {
        return Err(Error::invalid("Invalid skill bundle"));
    }
    let mut decoded = BTreeMap::new();
    let mut size = 0;
    for file in files {
        let rel = file["path"]
            .as_str()
            .ok_or_else(|| Error::invalid("Missing file path"))?;
        if rel.is_empty() || Path::new(rel).components().any(|p| !matches!(p, std::path::Component::Normal(n) if !n.to_string_lossy().starts_with('.'))) {
            return Err(Error::invalid("Invalid skill file path"));
        }
        let bytes = STANDARD
            .decode(file["data"].as_str().unwrap_or(""))
            .map_err(|_| Error::invalid("Invalid skill file contents"))?;
        size += bytes.len();
        if size > 4 * 1024 * 1024
            || decoded
                .insert(rel, (bytes, file["executable"] == true))
                .is_some()
        {
            return Err(Error::invalid("Invalid or oversized skill bundle"));
        }
    }
    let markdown = decoded
        .get("SKILL.md")
        .and_then(|(b, _)| std::str::from_utf8(b).ok())
        .ok_or_else(|| Error::invalid("Missing SKILL.md"))?;
    if edited_name(name, "SKILL.md", markdown)? != name {
        return Err(Error::invalid("Skill name does not match its frontmatter"));
    }
    let current = transport(home, "local", &json!({"action":"scan"}))?;
    let old_targets: Vec<_> = previous
        .iter()
        .flat_map(|n| targets(home, n))
        .filter(|p| exists(p))
        .collect();
    let destinations: Vec<_> = targets(home, name).into_iter().step_by(2).collect();
    // A lost response may replay a completed rename. Only accept the exact installed version.
    if old_targets.is_empty()
        && destinations.iter().all(|p| {
            current["copies"].as_array().unwrap().iter().any(|c| {
                c["path"] == p.to_string_lossy().as_ref() && c["digest"] == source["digest"]
            })
        })
    {
        return Ok(current);
    }
    if let Some(path) = targets(home, name).iter().find(|p| exists(p)) {
        return Err(Error::conflict(format!(
            "Cannot rename to {name}: {} already exists. Choose another name.",
            path.display()
        )));
    }
    if signatures(&current["copies"], &previous) != signatures(&input["expected"], &previous) {
        return Err(Error::conflict(
            "Skill changed since scanning. Scan again and review before renaming.",
        ));
    }
    for path in &old_targets {
        if fs::symlink_metadata(path)?.file_type().is_symlink()
            || !current["copies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["path"] == path.to_string_lossy().as_ref())
        {
            return Err(Error::conflict(format!(
                "Cannot safely inspect {}. Resolve its scan warning before renaming.",
                path.display()
            )));
        }
    }
    // Stage before touching any installed copy. Keep originals in the standard backup folder.
    let mut stages = Vec::new();
    for destination in &destinations {
        let parent = destination.parent().unwrap();
        fs::create_dir_all(parent)?;
        let stage = LibraryStage::new(parent)?;
        let bundle = stage.0.join("bundle");
        fs::create_dir(&bundle)?;
        for (rel, (bytes, executable)) in &decoded {
            let path = bundle.join(rel);
            atomic_write(&path, bytes)?;
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(if *executable { 0o755 } else { 0o644 }),
            )?;
        }
        stages.push(stage);
    }
    let backup = home.join(".hey-boss/skill-backups").join(format!(
        "rename-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut moved = Vec::new();
    let mut installed = Vec::new();
    let operation = (|| -> Result<()> {
        for old in &old_targets {
            let archived = backup.join(old.strip_prefix(home).unwrap());
            fs::create_dir_all(archived.parent().unwrap())?;
            fs::rename(old, &archived)?;
            moved.push((old, archived));
        }
        for (destination, stage) in destinations.iter().zip(&stages) {
            if exists(destination) {
                return Err(Error::conflict(
                    "Rename destination appeared while saving. Retry after scanning.",
                ));
            }
            fs::rename(stage.0.join("bundle"), destination)?;
            installed.push(destination);
        }
        let mut selected = super::selected_skills(home);
        for old in &previous {
            selected.remove(old);
        }
        selected.insert(name.into());
        super::set_selected_skills(home, &selected.into_iter().collect::<Vec<_>>())?;
        Ok(())
    })();
    if let Err(error) = operation {
        for path in installed.iter().rev() {
            let _ = fs::remove_dir_all(path);
        }
        for (old, archived) in moved.iter().rev() {
            let _ = fs::rename(archived, old);
        }
        return Err(error);
    }
    transport(home, "local", &json!({"action":"scan"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (crate::admin::Temporary, Value, Value) {
        let temp = crate::admin::Temporary::new().unwrap();
        let dir = temp.0.join(".codex/skills/before");
        fs::create_dir_all(dir.join("references")).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "---\nname: before\ndescription: Fixture\n---\n",
        )
        .unwrap();
        fs::write(dir.join("references/żółć.md"), "Reference").unwrap();
        fs::write(dir.join("helper"), [0, 128, 255]).unwrap();
        fs::set_permissions(dir.join("helper"), fs::Permissions::from_mode(0o755)).unwrap();
        let scan = transport(&temp.0, "local", &json!({"action":"scan"})).unwrap();
        let source = renamed_source(
            &scan["copies"][0],
            "after",
            "---\nname: after\ndescription: Fixture\n---\n",
        )
        .unwrap();
        let request =
            json!({"source":source,"previous_names":["before"],"expected":scan["copies"]});
        (temp, scan, request)
    }
    #[test]
    fn frontmatter_names_are_validated_only_for_the_skill_entrypoint() {
        assert_eq!(
            edited_name(
                "old",
                "SKILL.md",
                "---\r\nname: 'new-name' # comment\r\n---\r\n"
            )
            .unwrap(),
            "new-name"
        );
        for name in [
            "../escape",
            "AGENTS.md",
            "hey-boss",
            "",
            "bad name",
            "/tmp/path",
        ] {
            assert!(
                edited_name("old", "SKILL.md", &format!("---\nname: '{name}'\n---\n")).is_err(),
                "{name}"
            );
        }
        assert!(edited_name("hey-boss", "SKILL.md", "---\nname: new\n---\n").is_err());
        assert!(edited_name("old", "SKILL.md", "---\nname: [broken\n---\n").is_err());
        assert!(edited_name("old", "SKILL.md", "---\nname: new\n").is_err());
        assert_eq!(
            edited_name("old", "references/notes.md", "---\nname: new\n---\n").unwrap(),
            "old"
        );
        assert_eq!(
            edited_name("AGENTS.md", "AGENTS.md", "---\nname: new\n---\n").unwrap(),
            "AGENTS.md"
        );
    }
    #[test]
    fn rename_keeps_unicode_references_binary_modes_and_is_retryable() {
        let (temp, _, request) = fixture();
        let result = apply(&temp.0, &request).unwrap();
        assert_eq!(result["copies"].as_array().unwrap().len(), 4);
        assert!(
            result["copies"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["name"] == "after" && c["digest"] == request["source"]["digest"])
        );
        for root in [".codex", ".agents", ".claude", ".hey-boss"] {
            let dir = temp.0.join(root).join("skills/after");
            assert_eq!(fs::read(dir.join("helper")).unwrap(), [0, 128, 255]);
            assert_ne!(
                fs::metadata(dir.join("helper"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
            assert_eq!(
                fs::read_to_string(dir.join("references/żółć.md")).unwrap(),
                "Reference"
            );
            assert!(!temp.0.join(root).join("skills/before").exists());
        }
        assert!(super::super::selected_skills(&temp.0).contains("after"));
        assert!(apply(&temp.0, &request).is_ok());
    }
    #[test]
    fn collision_stale_edits_and_symlinks_leave_originals_untouched() {
        let (temp, _, request) = fixture();
        let collision = temp.0.join(".agents/skills/after.md");
        fs::create_dir_all(collision.parent().unwrap()).unwrap();
        fs::write(&collision, "Unrelated").unwrap();
        assert!(
            apply(&temp.0, &request)
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        assert_eq!(fs::read_to_string(&collision).unwrap(), "Unrelated");
        fs::remove_file(&collision).unwrap();
        std::os::unix::fs::symlink("missing", &collision).unwrap();
        assert!(
            apply(&temp.0, &request)
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        fs::remove_file(collision).unwrap();
        let original = temp.0.join(".codex/skills/before/SKILL.md");
        fs::write(&original, "Changed elsewhere").unwrap();
        assert!(
            apply(&temp.0, &request)
                .unwrap_err()
                .to_string()
                .contains("changed since scanning")
        );
        assert_eq!(fs::read_to_string(original).unwrap(), "Changed elsewhere");
        assert!(!temp.0.join(".codex/skills/after").exists());
    }
    #[test]
    fn failed_config_write_rolls_back_every_directory() {
        let (temp, _, request) = fixture();
        fs::create_dir_all(temp.0.join(".hey-boss/skill-sync.json")).unwrap();
        assert!(apply(&temp.0, &request).is_err());
        assert!(temp.0.join(".codex/skills/before/SKILL.md").exists());
        for root in [".codex", ".agents", ".claude", ".hey-boss"] {
            assert!(!temp.0.join(root).join("skills/after").exists());
        }
    }
}
