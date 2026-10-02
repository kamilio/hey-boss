//! Machine-owned Git checkouts, prepared before worker settings are applied.
use super::{
    Result, configuration,
    context::{self, Context},
    control,
    replica::invalid,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::process::CommandExt,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Checkout {
    pub git: String,
    pub path: String,
}
pub(super) fn default_workspace() -> String {
    "~/Workspace".into()
}
pub(super) fn validate_path(path: &str) -> Result<()> {
    let expanded = path.strip_prefix("~/").unwrap_or(path);
    if path.is_empty()
        || path.len() > 4096
        || path.chars().any(char::is_control)
        || (!path.starts_with("~/") && !Path::new(path).is_absolute())
        || Path::new(expanded)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(invalid(
            "Use an absolute checkout path or ~/folder, without ..",
        ));
    }
    Ok(())
}
pub(super) fn identity(git: &str) -> Result<String> {
    let allowed = git.starts_with("https://")
        || git.starts_with("ssh://")
        || (git.contains('@') && git.contains(':') && !git.contains("://"));
    if !allowed
        || git.len() > 2048
        || git.chars().any(char::is_whitespace)
        || git.contains(['?', '#'])
        || git.starts_with("https://")
            && git
                .trim_start_matches("https://")
                .split('/')
                .next()
                .unwrap_or("")
                .contains('@')
    {
        return Err(invalid(
            "Use an HTTPS or SSH Git URL without embedded credentials",
        ));
    }
    let id = crate::agents::normalize_origin(git)
        .ok_or_else(|| invalid("Invalid Git repository URL"))?;
    if id
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid("Invalid Git repository path"));
    }
    Ok(id)
}
fn expand(home: &Path, path: &str) -> Result<PathBuf> {
    validate_path(path)?;
    Ok(path
        .strip_prefix("~/")
        .map(|p| home.join(p))
        .unwrap_or_else(|| PathBuf::from(path)))
}
pub(super) fn desired(ctx: &Context, host: &str) -> Result<Value> {
    if !configuration::is_yaml(&ctx.desired) {
        return Ok(json!({}));
    }
    Ok(configuration::load(ctx)?["document"]["machines"][host]
        .get("projects")
        .cloned()
        .unwrap_or(json!({})))
}
pub(super) fn revision(node: &str, workers: &Value, projects: &Value) -> String {
    if projects.as_object().is_none_or(|p| p.is_empty()) {
        control::revision(node, workers)
    } else {
        context::hash(&json!({"controller":node,"workers":workers,"projects":projects}))
    }
}
fn clone_repository(git: &str, target: &Path) -> Result<()> {
    if !target.exists() {
        let parent = target
            .parent()
            .ok_or_else(|| invalid("Checkout needs a parent folder"))?;
        fs::create_dir_all(parent)?;
        let staging = parent.join(format!(".hey-boss-clone-{}", context::id()?));
        let result = (|| -> Result<()> {
            let mut command = Command::new("git");
            command
                .args([
                    "-c",
                    "credential.interactive=false",
                    "clone",
                    "--quiet",
                    "--",
                    git,
                ])
                .arg(&staging)
                .env("GIT_TERMINAL_PROMPT", "0")
                .env(
                    "GIT_SSH_COMMAND",
                    "ssh -o BatchMode=yes -o ConnectTimeout=15",
                )
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
            let mut child = command.spawn()?;
            let start = Instant::now();
            loop {
                if let Some(status) = child.try_wait()? {
                    if !status.success() {
                        return Err(invalid(
                            "Git clone failed. Check repository access and Git authentication on this machine.",
                        ));
                    }
                    break;
                }
                if start.elapsed() > Duration::from_secs(60) {
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let _ = child.wait();
                    return Err(invalid(
                        "Git clone timed out; check this machine’s connection and repository access.",
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if target.exists() {
                return Err(invalid(
                    "Checkout path appeared during cloning; existing files were preserved",
                ));
            }
            fs::rename(&staging, target)?;
            Ok(())
        })();
        let _ = fs::remove_dir_all(&staging);
        result?;
    }
    Ok(())
}
fn checkout(home: &Path, project: &str, spec: &Checkout) -> Result<PathBuf> {
    let target = expand(home, &spec.path)?;
    clone_repository(&spec.git, &target)?;
    let info = crate::agents::git_repository_info(
        target
            .to_str()
            .ok_or_else(|| invalid("Checkout path must be UTF-8"))?,
    )
    .ok_or_else(|| {
        invalid("Checkout path exists but is not a Git repository; existing files were preserved")
    })?;
    if info.origin.as_deref() != Some(project)
        || Path::new(&info.repository_root).canonicalize()? != target.canonicalize()?
    {
        return Err(invalid(
            "Checkout path belongs to another repository; existing files were preserved",
        ));
    }
    Ok(target)
}

pub(super) fn prepare(ctx: &Context, projects: &Value, workers: &Value) -> Result<Value> {
    let specs: BTreeMap<String, Checkout> = serde_json::from_value(if projects.is_null() {
        json!({})
    } else {
        projects.clone()
    })?;
    if specs.is_empty() {
        return Ok(workers.clone());
    }
    let receipt_path = ctx.state.join("project-checkouts.json");
    let mut receipts = ctx.read_json(&receipt_path, json!({}))?;
    let previous = receipts.clone();
    let mut paths = BTreeMap::new();
    for (id, spec) in specs {
        if identity(&spec.git)? != id {
            return Err(invalid("Project ID does not match Git repository"));
        }
        let path = expand(&ctx.home, &spec.path)?;
        let key = context::hash(&json!({"id":id,"git":spec.git,"path":path}));
        if receipts[&id]["key"] != key
            || !path.join(".git").exists()
            || receipts[&id]["error"].is_string()
        {
            if receipts[&id]["key"] == key
                && receipts[&id]["retry_at"].as_f64().unwrap_or(0.0) > context::now()
            {
                return Err(invalid(
                    receipts[&id]["error"]
                        .as_str()
                        .unwrap_or("Checkout will retry shortly"),
                ));
            }
            if let Err(error) = checkout(&ctx.home, &id, &spec) {
                let error = format!("{id}: {error}");
                receipts[&id] = json!({"key":key,"error":error,"retry_at":context::now()+30.0});
                ctx.atomic_json(&receipt_path, &receipts)?;
                return Err(invalid(&error));
            }
            let detected = crate::issues::Project {
                id: id.clone(),
                name: id.rsplit('/').next().unwrap().into(),
            };
            crate::issues::Store::open(&ctx.path)?.notification_project(&detected, None)?;
            receipts[&id] = json!({"key":key});
        }
        paths.insert(id, path);
    }
    if receipts != previous {
        ctx.atomic_json(&receipt_path, &receipts)?;
    }
    let mut workers = workers.clone();
    for worker in workers.as_array_mut().into_iter().flatten() {
        if matches!(worker["intent"].as_str(), Some("drain" | "stop")) {
            continue;
        }
        let selected = worker["config"]["projects"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for id in selected.iter().filter_map(Value::as_str) {
            if let Some(path) = paths.get(id) {
                // An explicit worker override wins over the machine default.
                if worker["config"]["directory"]
                    .as_str()
                    .is_none_or(str::is_empty)
                    && worker["config"]["directories"].get(id).is_none()
                {
                    worker["config"]["directories"][id] = json!(path);
                }
            }
        }
    }
    Ok(workers)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(path: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    }
    #[test]
    fn clones_once_and_preserves_existing_files_and_wrong_repositories() {
        let root = std::env::temp_dir().join(format!("hb-checkout-{}", context::id().unwrap()));
        fs::create_dir_all(root.join("source")).unwrap();
        git(&root.join("source"), &["init", "--quiet"]);
        let target = root.join("clone");
        clone_repository(root.join("source").to_str().unwrap(), &target).unwrap();
        assert!(target.join(".git").is_dir());
        fs::write(target.join("keep.txt"), "user work").unwrap();
        clone_repository("https://invalid.invalid/unreachable.git", &target).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("keep.txt")).unwrap(),
            "user work"
        );
        git(
            &target,
            &[
                "remote",
                "set-url",
                "origin",
                "git@github.com:acme/right.git",
            ],
        );
        let spec = Checkout {
            git: "git@github.com:acme/right.git".into(),
            path: target.to_str().unwrap().into(),
        };
        assert!(checkout(&root, "github.com/acme/right", &spec).is_ok());
        assert!(checkout(&root, "github.com/acme/wrong", &spec).is_err());
        let plain = root.join("plain");
        fs::create_dir(&plain).unwrap();
        fs::write(plain.join("keep"), "data").unwrap();
        let spec = Checkout {
            git: spec.git,
            path: plain.to_str().unwrap().into(),
        };
        assert!(checkout(&root, "github.com/acme/right", &spec).is_err());
        assert_eq!(fs::read_to_string(plain.join("keep")).unwrap(), "data");
        assert!(
            clone_repository(root.join("missing").to_str().unwrap(), &root.join("failed")).is_err()
        );
        assert!(!root.join("failed").exists());
        assert!(!fs::read_dir(&root).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".hey-boss-clone-")
        }));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn machine_checkouts_resolve_on_owning_home_and_preserve_worker_overrides() {
        let (root, ctx, store) = super::super::context::tests::test_context();
        let target = root.join("Workspace/right");
        fs::create_dir_all(&target).unwrap();
        git(&target, &["init", "--quiet"]);
        git(
            &target,
            &["remote", "add", "origin", "git@github.com:acme/right.git"],
        );
        let projects = json!({"github.com/acme/right":{"git":"git@github.com:acme/right.git","path":"~/Workspace/right"}});
        let workers = json!([
            {"id":"default","intent":"pause","config":{"projects":["github.com/acme/right"]}},
            {"id":"override","intent":"pause","config":{"projects":["github.com/acme/right"],"directories":{"github.com/acme/right":"/custom/right"}}},
            {"id":"removed","intent":"drain","config":{"projects":["github.com/acme/right"]}}
        ]);
        let resolved = prepare(&ctx, &projects, &workers).unwrap();
        assert_eq!(
            resolved[0]["config"]["directories"]["github.com/acme/right"],
            json!(target)
        );
        assert_eq!(
            resolved[1]["config"]["directories"]["github.com/acme/right"],
            "/custom/right"
        );
        assert!(resolved[2]["config"]["directories"].is_null());
        assert_eq!(prepare(&ctx, &projects, &workers).unwrap(), resolved);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn project_urls_and_home_paths_are_validated() {
        assert_eq!(
            identity("git@github.com:acme/my-project.git").unwrap(),
            "github.com/acme/my-project"
        );
        assert_eq!(
            identity("https://github.com/acme/my-project.git").unwrap(),
            "github.com/acme/my-project"
        );
        for bad in [
            "-x",
            "ext::sh",
            "https://token@github.com/a/b",
            "ssh://host/a/../b",
            "https://host/a/b?secret",
        ] {
            assert!(identity(bad).is_err(), "{bad}");
        }
        assert_eq!(
            expand(Path::new("/home/remote"), "~/Workspace/project").unwrap(),
            Path::new("/home/remote/Workspace/project")
        );
        for bad in ["relative", "~/../outside", "/tmp/../other"] {
            assert!(validate_path(bad).is_err());
        }
        assert_ne!(
            revision("node", &json!([]), &json!({})),
            revision(
                "node",
                &json!([]),
                &json!({"project":{"git":"url","path":"path"}})
            )
        );
    }
}
