//! Complete, reversible archives of global skill directories.
use crate::issues::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

fn database(home: &Path) -> PathBuf {
    home.join(".hey-boss/disabled-skills.db")
}

pub fn is_disabled(home: &Path, name: &str) -> Result<bool> {
    if !database(home).exists() {
        return Ok(false);
    }
    let db =
        Connection::open_with_flags(database(home), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM skills WHERE name=?1)",
        [name],
        |r| r.get(0),
    )?)
}

fn paths(name: &str) -> Vec<PathBuf> {
    [".codex", ".agents", ".claude", ".hey-boss"]
        .into_iter()
        .flat_map(|root| {
            [
                Path::new(root).join("skills").join(name),
                Path::new(root).join("skills").join(format!("{name}.md")),
            ]
        })
        .collect()
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn tar(args: &[&str], archive: &Path, directory: &Path, paths: &[PathBuf]) -> Result<()> {
    let output = Command::new("tar")
        .args(args)
        .arg(archive)
        .arg("-C")
        .arg(directory)
        .args(paths)
        .output()?;
    if !output.status.success() {
        return Err(Error::invalid(format!(
            "Could not archive skill: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

/// Move whole folders out of discovery before archiving, so omitted inventory files are safe too.
/// A failed/interrupted archive leaves its folders in a durable staging directory for retry.
pub fn apply(home: &Path, name: &str, enable: bool) -> Result<()> {
    if !super::is_valid_skill_name(name)
        || matches!(name, "hey-boss" | "AGENTS.md")
        || name.len() > 128
    {
        return Err(Error::invalid("Cannot disable this skill"));
    }
    fs::create_dir_all(home.join(".hey-boss"))?;
    let _lock = super::lock_install(home)?;
    let db = Connection::open(database(home))?;
    fs::set_permissions(database(home), fs::Permissions::from_mode(0o600))?;
    db.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS skills (name TEXT PRIMARY KEY, archive BLOB, selected INTEGER NOT NULL);")?;
    let saved: Option<(bool, bool)> = db
        .query_row(
            "SELECT archive IS NOT NULL, selected FROM skills WHERE name=?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let stage = home.join(".hey-boss/skill-disable-pending").join(name);
    let targets = paths(name);
    // Do not write through redirected skill roots when restoring or moving folders.
    for relative in &targets {
        let mut parent = relative.parent();
        while let Some(path) = parent.filter(|p| !p.as_os_str().is_empty()) {
            if fs::symlink_metadata(home.join(path)).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(Error::invalid(format!(
                    "Skill root is a symlink: {}",
                    path.display()
                )));
            }
            parent = path.parent();
        }
    }
    if enable {
        let Some((archived, selected)) = saved else {
            return Ok(());
        };
        if targets
            .iter()
            .any(|p| exists(&home.join(p)) && (archived || exists(&stage.join(p))))
        {
            return Err(Error::conflict(
                "A skill folder already exists. Move it aside before enabling; the archive is safe.",
            ));
        }
        let temp = crate::admin::Temporary::new()?;
        let restored = if archived {
            let bytes: Vec<u8> =
                db.query_row("SELECT archive FROM skills WHERE name=?1", [name], |r| {
                    r.get(0)
                })?;
            fs::write(temp.0.join("skill.tar.gz"), bytes)?;
            fs::create_dir(temp.0.join("files"))?;
            tar(
                &["-xpf"],
                &temp.0.join("skill.tar.gz"),
                &temp.0.join("files"),
                &[],
            )?;
            temp.0.join("files")
        } else {
            stage.clone()
        };
        let mut moved = Vec::new();
        let result = (|| -> Result<()> {
            for relative in &targets {
                if exists(&restored.join(relative)) {
                    fs::create_dir_all(home.join(relative).parent().unwrap())?;
                    fs::rename(restored.join(relative), home.join(relative))?;
                    moved.push(relative.clone());
                }
            }
            let mut selection = super::selected_skills(home);
            if selected {
                selection.insert(name.into());
            }
            super::set_selected_skills(home, &selection.into_iter().collect::<Vec<_>>())?;
            db.execute("DELETE FROM skills WHERE name=?1", [name])?;
            Ok(())
        })();
        if result.is_err() {
            for relative in moved.iter().rev() {
                fs::rename(home.join(relative), restored.join(relative))?;
            }
        }
        result?;
        if stage.exists() {
            fs::remove_dir_all(stage)?;
        }
        return Ok(());
    }
    if saved.is_some_and(|(archived, _)| archived) {
        if targets.iter().any(|p| exists(&home.join(p))) {
            return Err(Error::conflict(
                "New files appeared for a disabled skill. Move them aside; the original archive is safe.",
            ));
        }
        if stage.exists() {
            fs::remove_dir_all(stage)?;
        }
        return Ok(());
    }
    let mut selection = super::selected_skills(home);
    db.execute(
        "INSERT OR IGNORE INTO skills(name, selected) VALUES (?1, ?2)",
        params![name, selection.contains(name)],
    )?;
    selection.remove(name);
    super::set_selected_skills(home, &selection.into_iter().collect::<Vec<_>>())?;
    fs::create_dir_all(&stage)?;
    fs::set_permissions(stage.parent().unwrap(), fs::Permissions::from_mode(0o700))?;
    for relative in &targets {
        if exists(&home.join(relative)) {
            if exists(&stage.join(relative)) {
                return Err(Error::conflict(
                    "New files conflict with an interrupted archive. Both copies have been preserved.",
                ));
            }
            fs::create_dir_all(stage.join(relative).parent().unwrap())?;
            fs::rename(home.join(relative), stage.join(relative))?;
        }
    }
    let temp = crate::admin::Temporary::new()?;
    let archive = temp.0.join("skill.tar.gz");
    tar(&["-czpf"], &archive, &stage, &[PathBuf::from(".")])?;
    db.execute(
        "UPDATE skills SET archive=?2 WHERE name=?1",
        params![name, fs::read(archive)?],
    )?;
    fs::remove_dir_all(stage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    #[test]
    fn failed_database_write_keeps_complete_folders_for_retry_or_enable() {
        for retry in [true, false] {
            let temp = crate::admin::Temporary::new().unwrap();
            let home = &temp.0;
            let dir = home.join(".codex/skills/example");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), "original").unwrap();
            fs::create_dir_all(home.join(".hey-boss")).unwrap();
            let db = Connection::open(database(home)).unwrap();
            db.execute_batch("CREATE TABLE skills(name TEXT PRIMARY KEY, archive BLOB, selected INTEGER NOT NULL); CREATE TRIGGER fail_archive BEFORE UPDATE ON skills BEGIN SELECT RAISE(FAIL, 'storage failure'); END;").unwrap();
            assert!(apply(home, "example", false).is_err());
            assert!(is_disabled(home, "example").unwrap());
            assert_eq!(
                fs::read_to_string(home.join(
                    ".hey-boss/skill-disable-pending/example/.codex/skills/example/SKILL.md"
                ))
                .unwrap(),
                "original"
            );
            db.execute_batch("DROP TRIGGER fail_archive").unwrap();
            if retry {
                apply(home, "example", false).unwrap();
            }
            apply(home, "example", true).unwrap();
            assert_eq!(
                fs::read_to_string(dir.join("SKILL.md")).unwrap(),
                "original"
            );
        }
    }

    #[test]
    fn disable_preserves_whole_directories_and_enable_restores_them() {
        let temp = crate::admin::Temporary::new().unwrap();
        let home = &temp.0;
        for root in [".codex", ".agents", ".claude", ".hey-boss"] {
            let dir = home.join(root).join("skills/example");
            fs::create_dir_all(dir.join("empty")).unwrap();
            fs::write(dir.join("SKILL.md"), root).unwrap();
            fs::write(dir.join(".hidden"), [0, 128, 255]).unwrap();
            fs::write(dir.join("script"), b"executable").unwrap();
            fs::set_permissions(dir.join("script"), fs::Permissions::from_mode(0o751)).unwrap();
            symlink(".hidden", dir.join("link")).unwrap();
        }
        super::super::set_selected_skills(home, &["example".into()]).unwrap();
        apply(home, "example", false).unwrap();
        apply(home, "example", false).unwrap();
        assert!(is_disabled(home, "example").unwrap());
        assert!(!super::super::selected_skills(home).contains("example"));
        super::super::sync_skills(home, Some(&["example".into()])).unwrap();
        super::super::install(home).unwrap();
        for root in [".codex", ".agents", ".claude", ".hey-boss"] {
            assert!(!home.join(root).join("skills/example").exists());
        }
        apply(home, "example", true).unwrap();
        apply(home, "example", true).unwrap();
        assert!(!is_disabled(home, "example").unwrap());
        for root in [".codex", ".agents", ".claude", ".hey-boss"] {
            let dir = home.join(root).join("skills/example");
            assert_eq!(fs::read_to_string(dir.join("SKILL.md")).unwrap(), root);
            assert_eq!(fs::read(dir.join(".hidden")).unwrap(), [0, 128, 255]);
            assert_eq!(
                fs::read_link(dir.join("link")).unwrap().to_str(),
                Some(".hidden")
            );
            assert!(dir.join("empty").is_dir());
            assert_eq!(
                fs::metadata(dir.join("script"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o751
            );
        }
    }

    #[test]
    fn rejects_core_names_and_restore_collisions_without_losing_archive() {
        let temp = crate::admin::Temporary::new().unwrap();
        let home = &temp.0;
        for name in ["../escape", "hey-boss", "AGENTS.md", ""] {
            assert!(apply(home, name, false).is_err());
        }
        let dir = home.join(".codex/skills/example");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "original").unwrap();
        apply(home, "example", false).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "new copy").unwrap();
        assert!(apply(home, "example", true).is_err());
        assert_eq!(
            fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            "new copy"
        );
        assert!(is_disabled(home, "example").unwrap());
        assert!(apply(home, "example", false).is_err());
        fs::remove_dir_all(dir).unwrap();
        apply(home, "example", true).unwrap();
        assert_eq!(
            fs::read_to_string(home.join(".codex/skills/example/SKILL.md")).unwrap(),
            "original"
        );
    }
}
