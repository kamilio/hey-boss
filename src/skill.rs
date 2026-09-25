//! Canonical agent skill, shared by the review page and standalone installer.
use std::{fs, io, path::Path};

pub const MARKDOWN: &str = include_str!("../skills/hey-boss/SKILL.md");

// Publish references before the entrypoint so its links already resolve.
pub const FILES: &[(&str, &str)] = &[
    (
        "references/issues.md",
        include_str!("../skills/hey-boss/references/issues.md"),
    ),
    (
        "references/documents.md",
        include_str!("../skills/hey-boss/references/documents.md"),
    ),
    (
        "references/workers.md",
        include_str!("../skills/hey-boss/references/workers.md"),
    ),
    ("SKILL.md", MARKDOWN),
];

pub fn references() -> Vec<serde_json::Value> {
    FILES
        .iter()
        .filter(|(path, _)| path.starts_with("references/"))
        .map(|(path, text)| {
            serde_json::json!({
                "path": path,
                "title": text.lines().next().unwrap_or(path).trim_start_matches("# "),
                "text": text,
                "source": format!("skills/hey-boss/{path}"),
            })
        })
        .collect()
}

fn install_directory(directory: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    for (relative, text) in FILES {
        let path = directory.join(relative);
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent)?;
        if fs::read_to_string(&path).ok().as_deref() != Some(text) {
            static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let pending = parent.join(format!(".skill-{}-{serial}.tmp", std::process::id()));
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&pending)?;
            let result = std::io::Write::write_all(&mut file, text.as_bytes())
                .and_then(|_| fs::rename(&pending, &path));
            if result.is_err() {
                let _ = fs::remove_file(&pending);
            }
            result?;
        }
        paths.push(path);
    }
    Ok(paths)
}

pub fn install(home: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    for root in [".codex", ".agents", ".claude"] {
        let directory = home.join(root).join("skills/hey-boss");
        paths.extend(install_directory(&directory)?);
    }
    Ok(paths)
}

/// Small, trusted bundle sent over the companion's existing SSH stdin.
pub fn archive() -> io::Result<Vec<u8>> {
    let temporary = crate::admin::Temporary::new()?;
    install_directory(&temporary.0)?;
    let output = std::process::Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .current_dir(&temporary.0)
        .args(["-cf", "-", "--"])
        .args(FILES.iter().map(|(path, _)| path))
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(String::from_utf8_lossy(&output.stderr)));
    }
    Ok(output.stdout)
}

/// Extract only our generated archive and atomically replace managed files.
pub fn remote_install_script() -> String {
    let files = FILES
        .iter()
        .map(|(path, _)| *path)
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        r#"set -eu
umask 077
skill_stage=$(mktemp -d "${{TMPDIR:-/tmp}}/hey-boss-skill.XXXXXX")
trap 'rm -rf "$skill_stage"' EXIT HUP INT TERM
tar -xf - -C "$skill_stage"
for root in .codex .agents .claude; do
  for file in {files}; do
    destination="$HOME/$root/skills/hey-boss/$file"
    mkdir -p "$(dirname "$destination")"
    if ! cmp -s "$skill_stage/$file" "$destination"; then
      cp "$skill_stage/$file" "$destination.new.$$"
      mv "$destination.new.$$" "$destination"
    fi
  done
done
"#
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn installation_includes_linked_references() {
        let root = crate::admin::Temporary::new().unwrap();
        super::install(&root.0).unwrap();
        for reference in ["issues.md", "documents.md", "workers.md"] {
            let relative = format!("references/{reference}");
            assert!(super::MARKDOWN.contains(&format!("({relative})")));
            let source = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("skills/hey-boss")
                    .join(&relative),
            )
            .unwrap();
            for directory in [".codex", ".agents", ".claude"] {
                assert_eq!(
                    std::fs::read_to_string(
                        root.0
                            .join(directory)
                            .join("skills/hey-boss")
                            .join(&relative)
                    )
                    .unwrap(),
                    source
                );
            }
        }
    }
    #[test]
    fn installation_is_repeatable_and_keeps_other_skills() {
        let root = crate::admin::Temporary::new().unwrap();
        let other = root.0.join(".agents/skills/other/SKILL.md");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, "Keep this").unwrap();
        let first = super::install(&root.0).unwrap();
        assert_eq!(first, super::install(&root.0).unwrap());
        for path in first {
            let (_, text) = super::FILES
                .iter()
                .find(|(relative, _)| path.ends_with(relative))
                .unwrap();
            assert_eq!(std::fs::read_to_string(path).unwrap(), *text);
        }
        assert_eq!(std::fs::read_to_string(other).unwrap(), "Keep this");
    }

    #[test]
    fn remote_bundle_installs_all_files_and_preserves_unmanaged_content() {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let home = crate::admin::Temporary::new().unwrap();
        let custom = home.0.join(".agents/skills/hey-boss/custom.md");
        std::fs::create_dir_all(custom.parent().unwrap()).unwrap();
        std::fs::write(&custom, "User notes").unwrap();
        let archive = super::archive().unwrap();
        for _ in 0..2 {
            let mut child = Command::new("sh")
                .args(["-c", &super::remote_install_script()])
                .env("HOME", &home.0)
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(&archive).unwrap();
            assert!(child.wait().unwrap().success());
            for root in [".codex", ".agents", ".claude"] {
                for (path, text) in super::FILES {
                    assert_eq!(
                        std::fs::read_to_string(
                            home.0.join(root).join("skills/hey-boss").join(path)
                        )
                        .unwrap(),
                        *text
                    );
                }
            }
        }
        assert_eq!(std::fs::read_to_string(custom).unwrap(), "User notes");
    }
}
