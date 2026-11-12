use anyhow::{Context, Result, ensure};
use poe_agent_rust::context_plugins::parse_import_path;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub fn optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| path.display().to_string()),
    }
}
pub fn settings(home: &Path) -> Result<(PathBuf, PathBuf)> {
    let mut cfg = Value::Null;
    for p in [".hey-boss/config.yaml", ".hey-boss/config.json"] {
        if let Some(s) = optional(&home.join(p))? {
            cfg = serde_yaml_ng::from_str(&s)?;
            break;
        }
    }
    let c = &cfg["conversation"];
    let root = match c.get("root") {
        None => "~/Workspace/root",
        Some(v) => v.as_str().context("Invalid conversation.root")?,
    };
    let root = root
        .strip_prefix("~/")
        .map(|p| home.join(p))
        .unwrap_or_else(|| root.into());
    // GUI launch environments do not necessarily inherit SHELL.
    let shell = match c.get("shell") {
        Some(v) => PathBuf::from(v.as_str().context("Invalid conversation.shell")?),
        None => {
            let entry = unsafe { libc::getpwuid(libc::getuid()) };
            if !entry.is_null() && !unsafe { (*entry).pw_shell }.is_null() {
                PathBuf::from(unsafe { std::ffi::CStr::from_ptr((*entry).pw_shell) }.to_str()?)
            } else {
                std::env::var_os("SHELL")
                    .unwrap_or_else(|| "/bin/sh".into())
                    .into()
            }
        }
    };
    ensure!(
        root.is_absolute() && shell.is_absolute(),
        "Conversation root and shell must be absolute paths."
    );
    Ok((root, shell))
}
fn memory(path: &Path, root: &Path, seen: &mut HashSet<PathBuf>) -> Result<String> {
    let path = path.canonicalize()?;
    ensure!(
        path.starts_with(root),
        "Memory import outside {}",
        root.display()
    );
    ensure!(seen.insert(path.clone()), "Circular AGENTS.md import");
    let mut lines = Vec::new();
    for line in fs::read_to_string(&path)?.lines() {
        let units = line.encode_utf16().collect::<Vec<_>>();
        lines.push(match parse_import_path(&units) {
            Some(p) => memory(
                &path.parent().unwrap().join(String::from_utf16(&p)?),
                root,
                seen,
            )?,
            None => line.into(),
        });
    }
    seen.remove(&path);
    Ok(lines.join("\n"))
}
pub fn prompt(home: &Path, cwd: &Path) -> Result<String> {
    let mut sections = vec![include_str!("../../conversation.md").trim().to_string()];
    for (label, path) in [
        (
            "Project memory",
            cwd.ancestors()
                .map(|p| p.join("AGENTS.md"))
                .find(|p| p.exists()),
        ),
        ("User memory", Some(home.join(".config/poe-code/AGENTS.md"))),
    ] {
        if let Some(p) = path.filter(|p| p.exists()) {
            sections.push(format!(
                "{label}:\n{}",
                memory(
                    &p,
                    &p.parent().unwrap().canonicalize()?,
                    &mut Default::default()
                )?
            ));
        }
    }
    let mut skills = BTreeMap::new();
    for dir in [
        cwd.join(".agents/skills"),
        home.join(".agents/skills"),
        home.join(".config/poe-code/skills"),
    ] {
        let entries = match fs::read_dir(dir) {
            Ok(d) => d,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let dir = entry?.path();
            if !dir.is_dir() {
                continue;
            }
            let path = dir.join("SKILL.md");
            let Some(body) = optional(&path)? else {
                continue;
            };
            let mut lines = body.lines();
            if lines.next() != Some("---") {
                continue;
            }
            let metadata = lines
                .take_while(|l| *l != "---")
                .collect::<Vec<_>>()
                .join("\n");
            let Ok(meta) = serde_yaml_ng::from_str::<Value>(&metadata) else {
                continue;
            };
            if let (Some(n), Some(d)) = (meta["name"].as_str(), meta["description"].as_str()) {
                skills
                    .entry(n.to_string())
                    .or_insert_with(|| format!("- {n}: {d} ({})", path.display()));
            }
        }
    }
    if !skills.is_empty() {
        sections.push(include_str!("../../conversation-skills.md").replace(
            "{catalog}",
            &skills.into_values().collect::<Vec<_>>().join("\n"),
        ));
    }
    sections.push(include_str!("../../conversation-visuals.md").trim().into());
    Ok(sections.join("\n\n"))
}
