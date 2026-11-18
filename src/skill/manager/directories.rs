//! Local folder moves and imports. Existing destinations are never replaced.
use super::*;
use std::os::unix::ffi::OsStrExt;

fn directory(home: &Path, input: &str) -> Result<PathBuf> {
    let input = input.trim();
    let path = if input == "~" {
        home.to_path_buf()
    } else if let Some(relative) = input.strip_prefix("~/") {
        home.join(relative)
    } else {
        PathBuf::from(input)
    };
    if !path.is_absolute() || !path.is_dir() {
        return Err(Error::invalid(
            "Enter an existing absolute folder path, or a path starting with ~/",
        ));
    }
    Ok(fs::canonicalize(path)?)
}

fn rename(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let target = std::ffi::CString::new(target.as_os_str().as_bytes())?;
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn copy_folder(source: &Path, target: &Path, size: &mut u64, count: &mut usize) -> Result<()> {
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        let destination = target.join(entry.file_name());
        if metadata.is_dir() {
            *count += 1;
            if *count > 512 {
                return Err(Error::invalid("Skill folder exceeds 512 entries"));
            }
            copy_folder(&entry.path(), &destination, size, count)?;
        } else if metadata.is_file() {
            *size += metadata.len();
            *count += 1;
            if *size > 4 * 1024 * 1024 || *count > 512 {
                return Err(Error::invalid("Skill folder exceeds 4 MiB or 512 entries"));
            }
            fs::copy(entry.path(), destination)?;
        } else {
            return Err(Error::invalid(
                "Skill folders cannot contain symlinks or special files",
            ));
        }
    }
    fs::set_permissions(target, fs::metadata(source)?.permissions())?;
    Ok(())
}

pub(super) fn apply(home: &Path, state: &State, action: &Action) -> Result<(String, PathBuf)> {
    let _install_lock = super::super::lock_install(home)?;
    let folder = directory(home, &action.directory)?;
    let (source, target, name) = if action.action == "move" {
        let copy = state
            .machines
            .iter()
            .filter(|m| m["host"] == "local")
            .flat_map(|m| m["copies"].as_array().into_iter().flatten())
            .find(|c| c["name"] == action.skill && c["path"] == action.file_path)
            .ok_or_else(|| Error::conflict("Local skill copy is unavailable. Scan again."))?;
        if matches!(action.skill.as_str(), "AGENTS.md" | "hey-boss")
            || !is_valid_skill_name(&action.skill)
        {
            return Err(Error::invalid("Cannot move this skill"));
        }
        let source = PathBuf::from(copy["path"].as_str().unwrap());
        if !fs::symlink_metadata(&source)?.is_dir() {
            return Err(Error::invalid(
                "Choose a skill folder, not a standalone file or symlink",
            ));
        }
        let source = fs::canonicalize(source)?;
        if folder.starts_with(&source) {
            return Err(Error::invalid(
                "Choose a destination outside the skill folder",
            ));
        }
        (source, folder.join(&action.skill), action.skill.clone())
    } else {
        let name = folder
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_owned();
        if !is_valid_skill_name(&name)
            || matches!(name.as_str(), "AGENTS.md" | "hey-boss")
            || name.len() > 128
        {
            return Err(Error::invalid(
                "Skill folder name must contain 1–128 letters, numbers, hyphens, or underscores",
            ));
        }
        if state.disabled.contains(&name) {
            return Err(Error::conflict(
                "This skill is disabled. Enable it before importing.",
            ));
        }
        let parent = home.join(".hey-boss/skills");
        fs::create_dir_all(&parent)?;
        if fs::canonicalize(&parent)?.starts_with(&folder) {
            return Err(Error::invalid(
                "The source folder cannot contain the saved library",
            ));
        }
        (folder.clone(), parent.join(&name), name)
    };
    if fs::symlink_metadata(&target).is_ok() {
        return Err(Error::conflict(format!(
            "{} already exists. Choose another folder.",
            target.display()
        )));
    }
    if !source.join("SKILL.md").is_file() {
        return Err(Error::invalid("Choose a skill folder containing SKILL.md"));
    }
    fs::read_to_string(source.join("SKILL.md"))?;
    if action.action == "move" {
        rename(&source, &target).map_err(|error| {
            if error.raw_os_error() == Some(libc::EXDEV) {
                Error::invalid("Move requires a destination on the same filesystem. Import the folder to copy it instead.")
            } else { error.into() }
        })?;
    } else {
        let stage = LibraryStage::new(target.parent().unwrap())?;
        let bundle = stage.0.join("bundle");
        copy_folder(&source, &bundle, &mut 0, &mut 0)?;
        // Validate the copy before publishing it to the library.
        fs::read_to_string(bundle.join("SKILL.md"))?;
        rename(&bundle, &target)?;
    }
    Ok((name, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, State, PathBuf) {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let source = root.path().join(".agents/skills/calendar");
        fs::create_dir_all(source.join("references")).unwrap();
        fs::write(source.join("SKILL.md"), "# Calendar\n").unwrap();
        fs::write(source.join("references/guide.md"), "guide").unwrap();
        fs::write(source.join(".hidden"), "keep").unwrap();
        fs::write(source.join("run"), "executable").unwrap();
        fs::set_permissions(source.join("run"), fs::Permissions::from_mode(0o755)).unwrap();
        let state = State {
            machines: vec![
                json!({"host":"local","copies":[{"name":"calendar","path":source,"scope":"global"}]}),
            ],
            ..State::default()
        };
        (root, state, source)
    }
    fn action(kind: &str, source: &Path, directory: &Path) -> Action {
        serde_json::from_value(
            json!({"action":kind,"skill":"calendar","file_path":source,"directory":directory}),
        )
        .unwrap()
    }
    #[test]
    fn moves_complete_folder_and_refuses_collisions() {
        let (root, state, source) = fixture();
        let destination = root.path().join("saved");
        fs::create_dir(&destination).unwrap();
        fs::create_dir(destination.join("calendar")).unwrap();
        assert!(apply(root.path(), &state, &action("move", &source, &destination)).is_err());
        assert!(source.join("SKILL.md").exists());
        fs::remove_dir(destination.join("calendar")).unwrap();
        let (_, target) =
            apply(root.path(), &state, &action("move", &source, &destination)).unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(target.join(".hidden")).unwrap(), "keep");
        assert_eq!(
            fs::read_to_string(target.join("references/guide.md")).unwrap(),
            "guide"
        );
        assert_ne!(
            fs::metadata(target.join("run"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
    }
    #[test]
    fn imports_without_removing_source_or_overwriting_library() {
        let (root, state, source) = fixture();
        let request = action("import", &source, &source);
        let (_, target) = apply(root.path(), &state, &request).unwrap();
        assert!(source.join("SKILL.md").exists());
        assert!(target.join(".hidden").exists());
        assert_ne!(
            fs::metadata(target.join("run"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
        assert!(apply(root.path(), &state, &request).is_err());
    }
    #[test]
    fn rejects_unknown_move_nested_destination_and_unsafe_import() {
        let (root, state, source) = fixture();
        assert!(
            apply(
                root.path(),
                &state,
                &action("move", &source, &source.join("references"))
            )
            .is_err()
        );
        assert!(
            apply(
                root.path(),
                &State::default(),
                &action("move", &source, root.path())
            )
            .is_err()
        );
        std::os::unix::fs::symlink("/etc/passwd", source.join("link")).unwrap();
        assert!(apply(root.path(), &state, &action("import", &source, &source)).is_err());
        assert!(!root.path().join(".hey-boss/skills/calendar").exists());
        assert!(source.exists());
    }
    #[test]
    fn expands_home_and_rejects_relative_or_missing_folder() {
        let (root, _, _) = fixture();
        assert_eq!(
            directory(root.path(), "~/.agents/skills").unwrap(),
            fs::canonicalize(root.path().join(".agents/skills")).unwrap()
        );
        assert!(directory(root.path(), "relative").is_err());
        assert!(directory(root.path(), "~/missing").is_err());
    }
}
