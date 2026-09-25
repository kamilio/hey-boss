//! Canonical agent skill, shared by the review page and standalone installer.
use std::{fs, io, path::Path};

pub const MARKDOWN: &str = include_str!("../skills/hey-boss/SKILL.md");

pub fn install(home: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    for root in [".codex", ".agents", ".claude"] {
        let directory = home.join(root).join("skills/hey-boss");
        fs::create_dir_all(&directory)?;
        let path = directory.join("SKILL.md");
        if fs::read_to_string(&path).ok().as_deref() != Some(MARKDOWN) {
            let pending = directory.join(format!(".SKILL-{}.tmp", std::process::id()));
            fs::write(&pending, MARKDOWN)?;
            fs::rename(&pending, &path)?;
        }
        paths.push(path);
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    #[test]
    fn installation_is_repeatable_and_keeps_other_skills() {
        let root = crate::admin::Temporary::new().unwrap();
        let other = root.0.join(".agents/skills/other/SKILL.md");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, "Keep this").unwrap();
        let first = super::install(&root.0).unwrap();
        assert_eq!(first, super::install(&root.0).unwrap());
        for path in first {
            assert_eq!(std::fs::read_to_string(path).unwrap(), super::MARKDOWN);
        }
        assert_eq!(std::fs::read_to_string(other).unwrap(), "Keep this");
    }
}
