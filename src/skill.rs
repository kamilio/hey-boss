//! Canonical agent skill, cross-agent/fleet skill sync, and skill policy audit.
pub mod manager;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

pub const MARKDOWN: &str = include_str!("../skills/hey-boss/SKILL.md");
pub const SKILL_MAX_LINES: usize = 120;
pub const AGENT_ROOTS: &[(&str, &str)] = &[
    ("codex", ".codex"),
    ("agents", ".agents"),
    ("claude", ".claude"),
];
pub const DEFAULT_SELECTED_SKILLS: &[&str] = &[
    "hey-boss",
    "AGENTS.md",
    "stacked-prs",
    "hey-gh",
    "stop-slop",
];

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillWarning {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: String,
    pub scope: String,
    pub description: String,
    pub selected: bool,
    pub line_count: usize,
    pub max_lines_policy: usize,
    pub agents: BTreeMap<String, bool>,
    pub in_sync: bool,
    pub warnings: Vec<SkillWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SyncConfig {
    selected: Vec<String>,
}

pub fn references() -> Vec<Value> {
    FILES
        .iter()
        .filter(|(path, _)| path.starts_with("references/"))
        .map(|(path, text)| {
            json!({
                "path": path,
                "title": text.lines().next().unwrap_or(path).trim_start_matches("# "),
                "text": text,
                "source": format!("skills/hey-boss/{path}"),
            })
        })
        .collect()
}

fn sync_config_path(home: &Path) -> PathBuf {
    home.join(".hey-boss/skill-sync.json")
}

pub fn selected_skills(home: &Path) -> BTreeSet<String> {
    let path = sync_config_path(home);
    if let Ok(raw) = fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str::<SyncConfig>(&raw)
    {
        let mut set: BTreeSet<String> = cfg.selected.into_iter().collect();
        set.insert("hey-boss".into());
        return set;
    }
    DEFAULT_SELECTED_SKILLS
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

pub fn set_selected_skills(home: &Path, selected: &[String]) -> io::Result<BTreeSet<String>> {
    let mut set: BTreeSet<String> = selected
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| is_valid_skill_name(s))
        .collect();
    set.insert("hey-boss".into());
    let path = sync_config_path(home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let cfg = SyncConfig {
        selected: set.iter().cloned().collect(),
    };
    atomic_write(&path, serde_json::to_string_pretty(&cfg)?.as_bytes())?;
    Ok(set)
}

fn is_valid_skill_name(name: &str) -> bool {
    name == "AGENTS.md"
        || (!name.is_empty()
            && !name.starts_with('.')
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent)?;
    if fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pending = parent.join(format!(".skill-{}-{serial}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)?;
    let result = io::Write::write_all(&mut file, bytes).and_then(|_| fs::rename(&pending, path));
    if result.is_err() {
        let _ = fs::remove_file(&pending);
    }
    result
}

fn install_directory(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for (relative, text) in FILES {
        let path = directory.join(relative);
        atomic_write(&path, text.as_bytes())?;
        paths.push(path);
    }
    Ok(paths)
}

fn extract_description(markdown: &str) -> String {
    let mut in_frontmatter = false;
    for (idx, line) in markdown.lines().enumerate() {
        let trimmed = line.trim();
        if idx == 0 && trimmed == "---" {
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter {
            if trimmed == "---" {
                in_frontmatter = false;
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("description:") {
                return rest.trim().trim_matches('"').trim_matches('\'').to_string();
            }
            continue;
        }
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            return trimmed.chars().take(120).collect();
        }
    }
    String::new()
}

type SkillBundle = Vec<(String, Vec<u8>, bool)>;

fn collect_skill_files(skill_dir: &Path) -> io::Result<SkillBundle> {
    use std::os::unix::fs::PermissionsExt;
    fn walk(root: &Path, dir: &Path, out: &mut SkillBundle, size: &mut u64) -> io::Result<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<io::Result<_>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_symlink() {
                return Err(io::Error::other("Skill bundles cannot contain symlinks"));
            }
            if metadata.is_dir() {
                walk(root, &path, out, size)?;
            } else if metadata.is_file() {
                *size += metadata.len();
                if *size > 4 * 1024 * 1024 || out.len() >= 512 {
                    return Err(io::Error::other("Skill bundle exceeds 4 MiB or 512 files"));
                }
                out.push((
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(&path)?,
                    metadata.permissions().mode() & 0o111 != 0,
                ));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    if !skill_dir.join("SKILL.md").is_file() {
        return Ok(out);
    }
    walk(skill_dir, skill_dir, &mut out, &mut 0)?;
    // Publish the entrypoint after supporting files.
    out.sort_by_key(|(path, _, _)| (path == "SKILL.md", path.clone()));
    Ok(out)
}
fn write_bundle_file(path: &Path, bytes: &[u8], executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    atomic_write(path, bytes)?;
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
    )
}

fn analyze_entry(
    name: &str,
    scope: &str,
    selected: bool,
    agent_contents: &BTreeMap<String, Option<String>>,
) -> SkillEntry {
    let canonical = if name == "hey-boss" && scope == "global" {
        Some(MARKDOWN.to_string())
    } else {
        ["codex", "agents", "claude", "project"]
            .iter()
            .find_map(|k| agent_contents.get(*k).and_then(|v| v.clone()))
    }
    .unwrap_or_default();

    let line_count = canonical.lines().count();
    let description = extract_description(&canonical);

    let mut agents = BTreeMap::new();
    let mut present_agents = Vec::new();
    let mut missing_agents = Vec::new();
    let mut distinct_bodies = BTreeSet::new();

    for &(agent_key, _) in AGENT_ROOTS {
        let content = agent_contents.get(agent_key).and_then(|v| v.as_ref());
        let is_present = content.is_some();
        agents.insert(agent_key.to_string(), is_present);
        if let Some(body) = content {
            present_agents.push(agent_key);
            distinct_bodies.insert(body.trim().to_string());
        } else {
            missing_agents.push(agent_key);
        }
    }

    let in_sync = if scope == "global" {
        missing_agents.is_empty() && distinct_bodies.len() <= 1
    } else {
        distinct_bodies.len() <= 1
    };

    let mut warnings = Vec::new();
    if line_count > SKILL_MAX_LINES {
        warnings.push(SkillWarning {
            kind: "too_long".into(),
            message: format!(
                "Skill is too long ({line_count} lines; policy max is {SKILL_MAX_LINES} lines)"
            ),
        });
    }
    if scope == "global" && !present_agents.is_empty() && !missing_agents.is_empty() {
        warnings.push(SkillWarning {
            kind: "agent_drift".into(),
            message: format!(
                "Present on [{}] but missing on [{}] — sync to align Codex, Claude, and Agents",
                present_agents.join(", "),
                missing_agents.join(", ")
            ),
        });
    } else if distinct_bodies.len() > 1 {
        warnings.push(SkillWarning {
            kind: "content_mismatch".into(),
            message: "SKILL.md content differs across installed coding agents".into(),
        });
    }

    SkillEntry {
        name: name.to_string(),
        scope: scope.to_string(),
        description,
        selected,
        line_count,
        max_lines_policy: SKILL_MAX_LINES,
        agents,
        in_sync,
        warnings,
    }
}

pub fn discover_global_skills(home: &Path) -> Vec<SkillEntry> {
    let selected = selected_skills(home);
    let mut by_name: BTreeMap<String, BTreeMap<String, Option<String>>> = BTreeMap::new();
    by_name.entry("hey-boss".into()).or_default();

    for &(agent_key, root_dir) in AGENT_ROOTS {
        let skills_root = home.join(root_dir).join("skills");
        let Ok(entries) = fs::read_dir(&skills_root) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Some(raw_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if raw_name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if let Ok(text) = fs::read_to_string(&skill_file) {
                    by_name
                        .entry(raw_name.to_string())
                        .or_default()
                        .insert(agent_key.to_string(), Some(text));
                }
            } else if path.is_file()
                && let Some(stem) = raw_name.strip_suffix(".md")
                && is_valid_skill_name(stem)
                && let Ok(text) = fs::read_to_string(&path)
            {
                by_name
                    .entry(stem.to_string())
                    .or_default()
                    .insert(agent_key.to_string(), Some(text));
            }
        }
    }

    by_name
        .into_iter()
        .map(|(name, map)| {
            let is_selected = selected.contains(&name);
            analyze_entry(&name, "global", is_selected, &map)
        })
        .collect()
}

pub fn discover_project_skills(project_dir: &Path) -> Vec<SkillEntry> {
    let mut by_name: BTreeMap<String, BTreeMap<String, Option<String>>> = BTreeMap::new();
    let roots: &[(&str, &str)] = &[
        ("project", "skills"),
        ("codex", ".codex/skills"),
        ("agents", ".agents/skills"),
        ("claude", ".claude/skills"),
    ];

    for &(key, rel) in roots {
        let dir = project_dir.join(rel);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') || !path.is_dir() {
                continue;
            }
            let skill_file = path.join("SKILL.md");
            if let Ok(text) = fs::read_to_string(&skill_file) {
                by_name
                    .entry(name.to_string())
                    .or_default()
                    .insert(key.to_string(), Some(text));
            }
        }
    }

    by_name
        .into_iter()
        .map(|(name, map)| {
            let mut entry = analyze_entry(&name, "project", true, &map);
            let present_in_repo = map.contains_key("project");
            entry.agents.insert("project".into(), present_in_repo);
            entry
        })
        .collect()
}

pub fn audit_report(home: &Path, project_dir: Option<&Path>) -> Value {
    let global_skills = discover_global_skills(home);
    let project_skills = project_dir.map(discover_project_skills).unwrap_or_default();
    let selected: Vec<String> = selected_skills(home).into_iter().collect();
    json!({
        "ok": true,
        "selected": selected,
        "max_lines_policy": SKILL_MAX_LINES,
        "global_skills": global_skills,
        "project_skills": project_skills,
    })
}

fn load_canonical_skill_bundle(home: &Path, name: &str) -> io::Result<Option<SkillBundle>> {
    let managed = home.join(".hey-boss/skills").join(name);
    if managed.join("SKILL.md").is_file() {
        return collect_skill_files(&managed).map(Some);
    }
    if name == "hey-boss" {
        return Ok(Some(
            FILES
                .iter()
                .map(|(rel, text)| ((*rel).to_string(), text.as_bytes().to_vec(), false))
                .collect(),
        ));
    }
    for &(_, root_dir) in AGENT_ROOTS {
        let dir = home.join(root_dir).join("skills").join(name);
        if dir.join("SKILL.md").is_file() {
            let files = collect_skill_files(&dir)?;
            if !files.is_empty() {
                return Ok(Some(files));
            }
        }
        let flat = home
            .join(root_dir)
            .join("skills")
            .join(format!("{name}.md"));
        if flat.is_file() {
            return Ok(Some(vec![("SKILL.md".into(), fs::read(&flat)?, false)]));
        }
    }
    Ok(None)
}

pub fn sync_skills(home: &Path, explicit_names: Option<&[String]>) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let names: BTreeSet<String> = match explicit_names {
        Some(list) if !list.is_empty() => {
            let mut current = selected_skills(home);
            for n in list {
                if is_valid_skill_name(n) {
                    current.insert(n.clone());
                }
            }
            let updated: Vec<String> = current.iter().cloned().collect();
            set_selected_skills(home, &updated)?
        }
        _ => selected_skills(home),
    };

    for name in names {
        let Some(bundle) = load_canonical_skill_bundle(home, &name)? else {
            continue;
        };
        for &(_, root_dir) in AGENT_ROOTS {
            let dest_dir = home.join(root_dir).join("skills").join(&name);
            for (rel, bytes, executable) in &bundle {
                let target = dest_dir.join(rel);
                write_bundle_file(&target, bytes, *executable)?;
                paths.push(target);
            }
        }
    }
    Ok(paths)
}

pub fn install(home: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for name in selected_skills(home) {
        // An unrelated unportable skill must not prevent installing the core integration.
        let Ok(Some(bundle)) = load_canonical_skill_bundle(home, &name) else {
            continue;
        };
        for &(_, root) in AGENT_ROOTS {
            for (relative, bytes, executable) in &bundle {
                let path = home.join(root).join("skills").join(&name).join(relative);
                write_bundle_file(&path, bytes, *executable)?;
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

pub fn archive_for_home(home: Option<&Path>) -> io::Result<Vec<u8>> {
    let temporary = crate::admin::Temporary::new()?;
    install_directory(&temporary.0.join("hey-boss"))?;
    let mut archived_paths = FILES
        .iter()
        .map(|(path, _)| format!("hey-boss/{path}"))
        .collect::<Vec<_>>();

    if let Some(h) = home {
        for name in selected_skills(h) {
            if !is_valid_skill_name(&name) {
                continue;
            }
            if let Ok(Some(bundle)) = load_canonical_skill_bundle(h, &name) {
                for (rel, bytes, executable) in bundle {
                    let staged = temporary.0.join(&name).join(&rel);
                    write_bundle_file(&staged, &bytes, executable)?;
                    let archived = format!("{name}/{rel}");
                    if !archived_paths.contains(&archived) {
                        archived_paths.push(archived);
                    }
                }
            }
        }
    }

    let output = std::process::Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .current_dir(&temporary.0)
        .args(["--no-xattrs", "-cf", "-", "--"])
        .args(&archived_paths)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(String::from_utf8_lossy(&output.stderr)));
    }
    Ok(output.stdout)
}

/// Small, trusted bundle sent over the companion's existing SSH stdin.
pub fn archive() -> io::Result<Vec<u8>> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    archive_for_home(home.as_deref())
}

/// Extract only our generated archive and atomically replace managed files across all agents.
pub fn remote_install_script() -> String {
    r#"set -eu
umask 077
skill_stage=$(mktemp -d "${TMPDIR:-/tmp}/hey-boss-skill.XXXXXX")
trap 'rm -rf "$skill_stage"' EXIT HUP INT TERM
cat > "$skill_stage/bundle.tar"
tar -xf "$skill_stage/bundle.tar" -C "$skill_stage"
rm "$skill_stage/bundle.tar"
for root in .codex .agents .claude; do
  for skill_dir in "$skill_stage"/*; do
    [ -d "$skill_dir" ] || continue
    skill_name=$(basename "$skill_dir")
    (
      cd "$skill_dir"
      find . -type f | while IFS= read -r rel; do
        clean="${rel#./}"
        destination="$HOME/$root/skills/$skill_name/$clean"
        mkdir -p "$(dirname "$destination")"
        if ! cmp -s "$skill_dir/$clean" "$destination"; then
          cp "$skill_dir/$clean" "$destination.new.$$"
          mv "$destination.new.$$" "$destination"
        fi
      done
    )
  done
done
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_version_and_nested_executables_survive_legacy_sync() {
        use std::os::unix::fs::PermissionsExt;
        let home = crate::admin::Temporary::new().unwrap();
        let root = home.0.join(".hey-boss/skills/stacked-prs");
        fs::create_dir_all(root.join("scripts/nested")).unwrap();
        fs::write(root.join("SKILL.md"), "Chosen remote version").unwrap();
        fs::write(root.join("scripts/nested/check"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(
            root.join("scripts/nested/check"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let older = home.0.join(".codex/skills/stacked-prs");
        fs::create_dir_all(&older).unwrap();
        fs::write(older.join("SKILL.md"), "Old local copy").unwrap();
        install(&home.0).unwrap();
        for (_, agent) in AGENT_ROOTS {
            let destination = home.0.join(agent).join("skills/stacked-prs");
            assert_eq!(
                fs::read_to_string(destination.join("SKILL.md")).unwrap(),
                "Chosen remote version"
            );
            assert_ne!(
                fs::metadata(destination.join("scripts/nested/check"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
        }
    }

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
        // Add a selected skill (stacked-prs) in .codex/skills on source home
        let stacked = home.0.join(".codex/skills/stacked-prs/SKILL.md");
        std::fs::create_dir_all(stacked.parent().unwrap()).unwrap();
        std::fs::write(&stacked, "# Stacked PRs\nUse gh stack.\n").unwrap();

        let mut archive = super::archive_for_home(Some(&home.0)).unwrap();
        // Tar readers may stop at the end marker before the sender has written
        // the padded final records. The receiver must consume the entire input.
        archive.resize(archive.len() + 128 * 1024, 0);
        let remote_home = crate::admin::Temporary::new().unwrap();
        let remote_custom = remote_home.0.join(".agents/skills/hey-boss/custom.md");
        std::fs::create_dir_all(remote_custom.parent().unwrap()).unwrap();
        std::fs::write(&remote_custom, "User notes").unwrap();

        for _ in 0..2 {
            let mut child = Command::new("sh")
                .args(["-c", &super::remote_install_script()])
                .env("HOME", &remote_home.0)
                .stdin(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let sent = child.stdin.take().unwrap().write_all(&archive);
            let output = child.wait_with_output().unwrap();
            assert!(
                sent.is_ok() && output.status.success(),
                "Sent {} bytes: {sent:?}; installer {}: {}",
                archive.len(),
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            for root in [".codex", ".agents", ".claude"] {
                for (path, text) in super::FILES {
                    assert_eq!(
                        std::fs::read_to_string(
                            remote_home.0.join(root).join("skills/hey-boss").join(path)
                        )
                        .unwrap(),
                        *text
                    );
                }
                assert_eq!(
                    std::fs::read_to_string(
                        remote_home.0.join(root).join("skills/stacked-prs/SKILL.md")
                    )
                    .unwrap(),
                    "# Stacked PRs\nUse gh stack.\n"
                );
            }
        }
        assert_eq!(
            std::fs::read_to_string(remote_custom).unwrap(),
            "User notes"
        );
    }

    #[test]
    fn skill_audit_detects_agent_drift_length_violations_and_syncs_selected_skills() {
        let home = crate::admin::Temporary::new().unwrap();
        let stacked = home.0.join(".codex/skills/stacked-prs/SKILL.md");
        std::fs::create_dir_all(stacked.parent().unwrap()).unwrap();
        std::fs::write(
            &stacked,
            "---\nname: stacked-prs\ndescription: Native GitHub stacked PRs\n---\n# Stacked PRs\nUse gh stack.\n",
        )
        .unwrap();

        // A long skill only on Claude (unselected garbage by default)
        let bloated = home.0.join(".claude/skills/bloated-skill/SKILL.md");
        std::fs::create_dir_all(bloated.parent().unwrap()).unwrap();
        let long_body = (0..150)
            .map(|i| format!("Line {i} of overly verbose skill instructions"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&bloated, &long_body).unwrap();

        let before = discover_global_skills(&home.0);
        let stacked_entry = before.iter().find(|s| s.name == "stacked-prs").unwrap();
        assert!(stacked_entry.selected);
        assert!(stacked_entry.agents["codex"]);
        assert!(!stacked_entry.agents["claude"]);
        assert!(!stacked_entry.agents["agents"]);
        assert!(
            stacked_entry
                .warnings
                .iter()
                .any(|w| w.kind == "agent_drift")
        );

        let bloated_entry = before.iter().find(|s| s.name == "bloated-skill").unwrap();
        assert!(!bloated_entry.selected);
        assert!(bloated_entry.warnings.iter().any(|w| w.kind == "too_long"));
        assert!(
            bloated_entry
                .warnings
                .iter()
                .any(|w| w.kind == "agent_drift")
        );

        // Sync selected skills: stacked-prs and hey-boss are synced to Codex, Claude, and Agents;
        // unselected bloated-skill is NOT copied to Codex or Agents.
        sync_skills(&home.0, None).unwrap();

        let after = discover_global_skills(&home.0);
        let stacked_after = after.iter().find(|s| s.name == "stacked-prs").unwrap();
        assert!(stacked_after.in_sync);
        assert!(stacked_after.warnings.is_empty());
        assert!(stacked_after.agents["codex"]);
        assert!(stacked_after.agents["claude"]);
        assert!(stacked_after.agents["agents"]);

        let bloated_after = after.iter().find(|s| s.name == "bloated-skill").unwrap();
        assert!(!bloated_after.agents["codex"]);
        assert!(bloated_after.agents["claude"]);
    }
}
