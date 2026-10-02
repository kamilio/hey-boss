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
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reuse_existing: bool,
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
fn clone_failure(stderr: &str) -> &'static str {
    if stderr.contains("Permission denied (publickey)") {
        "Git clone failed: the background service could not authenticate with SSH. Its SSH agent may differ from your terminal’s."
    } else if stderr.contains("No space left on device") {
        "Git clone failed: this machine ran out of disk space."
    } else if stderr.contains("Could not resolve hostname")
        || stderr.contains("Could not resolve host")
    {
        "Git clone failed: this machine could not resolve the repository host."
    } else if stderr.contains("Repository not found") || stderr.contains("repository not found") {
        "Git clone failed: the repository was not found or is not accessible to the background service."
    } else {
        "Git clone failed in the background service. Check the repository URL, checkout path, and access on this machine."
    }
}
fn https_fallback(git: &str, stderr: &str) -> Option<String> {
    if !(git.starts_with("git@github.com:") || git.starts_with("ssh://git@github.com/"))
        || !stderr.contains("Permission denied (publickey)")
    {
        return None;
    }
    Some(format!("https://{}.git", identity(git).ok()?))
}
fn clone_repository(git: &str, target: &Path) -> Result<()> {
    if !target.exists() {
        let parent = target
            .parent()
            .ok_or_else(|| invalid("Checkout needs a parent folder"))?;
        fs::create_dir_all(parent)?;
        let staging = parent.join(format!(".hey-boss-clone-{}", context::id()?));
        let result = (|| -> Result<()> {
            let mut clone_url = git.to_owned();
            loop {
                let mut command = Command::new("git");
                command
                    .args([
                        "-c",
                        "credential.interactive=false",
                        "clone",
                        "--quiet",
                        "--",
                        &clone_url,
                    ])
                    .arg(&staging)
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .env(
                        "GIT_SSH_COMMAND",
                        "ssh -o BatchMode=yes -o ConnectTimeout=15",
                    )
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped());
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
                let mut stderr = child.stderr.take().unwrap();
                let errors = std::thread::spawn(move || {
                    let mut kept = Vec::new();
                    let mut buffer = [0; 4096];
                    while let Ok(n) = stderr.read(&mut buffer) {
                        if n == 0 {
                            break;
                        }
                        let count = n.min(8192usize.saturating_sub(kept.len()));
                        kept.extend_from_slice(&buffer[..count]);
                    }
                    String::from_utf8_lossy(&kept).into_owned()
                });
                let start = Instant::now();
                loop {
                    if let Some(status) = child.try_wait()? {
                        let stderr = errors.join().unwrap_or_default();
                        if !status.success() {
                            if let Some(url) = https_fallback(&clone_url, &stderr) {
                                let _ = fs::remove_dir_all(&staging);
                                clone_url = url;
                                break;
                            }
                            return Err(invalid(clone_failure(&stderr)));
                        }
                        if target.exists() {
                            return Err(invalid(
                                "Checkout path appeared during cloning; existing files were preserved",
                            ));
                        }
                        fs::rename(&staging, target)?;
                        return Ok(());
                    }
                    if start.elapsed() > Duration::from_secs(15 * 60) {
                        unsafe {
                            libc::kill(-(child.id() as i32), libc::SIGKILL);
                        }
                        let _ = child.wait();
                        let _ = errors.join();
                        return Err(invalid(
                            "Git clone timed out; check this machine’s connection and repository access.",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        })();
        let _ = fs::remove_dir_all(&staging);
        result?;
    }
    Ok(())
}
fn matching_checkout(path: &Path, project: &str) -> bool {
    // Cheap existence check avoids starting Git for missing candidates.
    path.join(".git").exists()
        && crate::agents::git_repository_info(&path.to_string_lossy()).is_some_and(|info| {
            info.origin.as_deref() == Some(project)
                && Path::new(&info.repository_root).canonicalize().ok() == path.canonicalize().ok()
        })
}
fn resolve_path(
    ctx: &Context,
    project: &str,
    spec: &Checkout,
    workers: &Value,
    receipt: &Value,
) -> Result<PathBuf> {
    let target = expand(&ctx.home, &spec.path)?;
    if !spec.reuse_existing {
        return Ok(target);
    }
    if let Some(path) = receipt["path"].as_str().map(PathBuf::from)
        && matching_checkout(&path, project)
    {
        return Ok(path);
    }
    // Never bypass an occupied target, including a wrong repository or plain files.
    if target.exists() {
        return Ok(target);
    }
    let name = project.rsplit('/').next().unwrap();
    let mut candidates = BTreeSet::from([
        ctx.home.join(name),
        ctx.home.join("Workspace").join(name),
        ctx.home.join("projects").join(name),
    ]);
    for worker in workers.as_array().into_iter().flatten() {
        let config = &worker["config"];
        if let Some(path) = config["directories"][project].as_str() {
            candidates.insert(expand(&ctx.home, path)?);
        }
        if config["projects"]
            .as_array()
            .is_some_and(|ids| ids.len() == 1 && ids[0] == project)
            && let Some(path) = config["directory"].as_str().filter(|p| !p.is_empty())
        {
            candidates.insert(expand(&ctx.home, path)?);
        }
    }
    let matches: BTreeSet<_> = candidates
        .into_iter()
        .filter(|p| matching_checkout(p, project))
        .filter_map(|p| p.canonicalize().ok())
        .collect();
    match matches.len() {
        0 => Ok(target),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => Err(invalid(
            "Multiple matching checkouts found; choose a checkout path in Edit checkout",
        )),
    }
}

pub(super) fn resolved(ctx: &Context) -> Result<Value> {
    let receipts = ctx.read_json(&ctx.state.join("project-checkouts.json"), json!({}))?;
    Ok(json!(
        receipts
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, r)| r["error"].is_null() && r["path"].is_string())
            .map(|(id, r)| (id, json!({"key":r["key"],"path":r["path"]})))
            .collect::<BTreeMap<_, _>>()
    ))
}

fn checkout_key(id: &str, spec: &Checkout) -> String {
    context::hash(&json!({"id":id,"checkout":spec}))
}

pub(super) fn project_status(projects: &Value, resolved: &Value) -> Value {
    let mut projects = projects.clone();
    for (id, value) in projects.as_object_mut().into_iter().flatten() {
        if let Ok(spec) = serde_json::from_value::<Checkout>(value.clone())
            && resolved[id]["key"] == checkout_key(id, &spec)
        {
            value["resolved_path"] = resolved[id]["path"].clone();
        }
    }
    projects
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

// Called under the worker configuration lock, never while a clone is running.
pub(super) fn retry(ctx: &Context, projects: &Value) -> Result<()> {
    let Some(projects) = projects.as_object().filter(|p| !p.is_empty()) else {
        return Ok(());
    };
    let path = ctx.state.join("project-checkouts.json");
    let mut receipts = ctx.read_json(&path, json!({}))?;
    let previous = receipts.clone();
    for (id, token) in projects {
        if receipts[id]["retry_request"] == *token {
            continue;
        }
        if !receipts[id].is_object() {
            receipts[id] = json!({});
        }
        let receipt = receipts[id].as_object_mut().unwrap();
        receipt.remove("retry_at");
        receipt.insert("retry_request".into(), token.clone());
    }
    if receipts != previous {
        ctx.atomic_json(&path, &receipts)?;
    }
    Ok(())
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
        let key = checkout_key(&id, &spec);
        let prior = if receipts[&id]["key"] == key {
            receipts[&id].clone()
        } else {
            json!({})
        };
        let path = resolve_path(ctx, &id, &spec, workers, &prior)?;
        if receipts[&id]["key"] != key
            || !path.join(".git").exists()
            || receipts[&id]["error"].is_string()
        {
            if receipts[&id]["key"] == key
                && !path.join(".git").exists()
                && receipts[&id]["retry_at"].as_f64().unwrap_or(0.0) > context::now()
            {
                return Err(invalid(
                    receipts[&id]["error"]
                        .as_str()
                        .unwrap_or("Checkout will retry shortly"),
                ));
            }
            let chosen = Checkout {
                git: spec.git.clone(),
                path: path.to_string_lossy().into_owned(),
                reuse_existing: false,
            };
            if let Err(error) = checkout(&ctx.home, &id, &chosen) {
                let error = format!("{id}: {error}");
                receipts[&id] = json!({"key":key,"error":error,"retry_at":context::now()+30.0,"retry_request":receipts[&id]["retry_request"]});
                ctx.atomic_json(&receipt_path, &receipts)?;
                return Err(invalid(&error));
            }
            let detected = crate::issues::Project {
                id: id.clone(),
                name: id.rsplit('/').next().unwrap().into(),
            };
            crate::issues::Store::open(&ctx.path)?.notification_project(&detected, None)?;
            receipts[&id] =
                json!({"key":key,"path":path,"retry_request":receipts[&id]["retry_request"]});
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
    #[test]
    fn automatic_checkout_reuses_matching_home_repo_and_explicit_paths_stay_separate() {
        let (root, ctx, store) = super::super::context::tests::test_context();
        let existing = root.join("right");
        fs::create_dir_all(&existing).unwrap();
        git(&existing, &["init", "--quiet"]);
        git(
            &existing,
            &["remote", "add", "origin", "git@github.com:acme/right.git"],
        );
        fs::write(existing.join("keep.txt"), "local work").unwrap();
        let spec = Checkout {
            git: "https://github.com/acme/right.git".into(),
            path: "~/projects/right".into(),
            reuse_existing: true,
        };
        assert_eq!(
            resolve_path(&ctx, "github.com/acme/right", &spec, &json!([]), &json!({})).unwrap(),
            existing.canonicalize().unwrap()
        );
        let explicit = Checkout {
            reuse_existing: false,
            ..spec
        };
        assert_eq!(
            resolve_path(
                &ctx,
                "github.com/acme/right",
                &explicit,
                &json!([]),
                &json!({})
            )
            .unwrap(),
            root.join("projects/right")
        );
        git(
            &existing,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/other/right.git",
            ],
        );
        let automatic = Checkout {
            reuse_existing: true,
            ..explicit
        };
        assert_eq!(
            resolve_path(
                &ctx,
                "github.com/acme/right",
                &automatic,
                &json!([]),
                &json!({})
            )
            .unwrap(),
            root.join("projects/right")
        );
        assert_eq!(
            fs::read_to_string(existing.join("keep.txt")).unwrap(),
            "local work"
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn automatic_checkout_remembers_resolution_and_rejects_ambiguous_clones() {
        let (root, ctx, store) = super::super::context::tests::test_context();
        for path in [root.join("right"), root.join("Workspace/right")] {
            fs::create_dir_all(&path).unwrap();
            git(&path, &["init", "--quiet"]);
            git(
                &path,
                &["remote", "add", "origin", "git@github.com:acme/right.git"],
            );
        }
        let spec = Checkout {
            git: "https://github.com/acme/right.git".into(),
            path: "~/projects/right".into(),
            reuse_existing: true,
        };
        assert!(
            resolve_path(&ctx, "github.com/acme/right", &spec, &json!([]), &json!({}))
                .unwrap_err()
                .to_string()
                .contains("Multiple")
        );
        let remembered = root.join("right").canonicalize().unwrap();
        assert_eq!(
            resolve_path(
                &ctx,
                "github.com/acme/right",
                &spec,
                &json!([]),
                &json!({"path": remembered})
            )
            .unwrap(),
            remembered
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn github_ssh_auth_failure_can_retry_same_repository_over_https() {
        assert_eq!(
            https_fallback(
                "git@github.com:poe-internal/poe2.git",
                "Permission denied (publickey)."
            ),
            Some("https://github.com/poe-internal/poe2.git".into())
        );
        assert_eq!(
            https_fallback(
                "ssh://git@github.com/poe-internal/poe2.git",
                "Permission denied (publickey)."
            ),
            Some("https://github.com/poe-internal/poe2.git".into())
        );
        assert_eq!(
            https_fallback(
                "git@private.example:acme/repo.git",
                "Permission denied (publickey)."
            ),
            None
        );
        assert_eq!(
            https_fallback(
                "git@github.com:acme/repo.git",
                "Host key verification failed"
            ),
            None
        );
        assert_eq!(
            https_fallback(
                "https://github.com/acme/repo.git",
                "Permission denied (publickey)."
            ),
            None
        );
    }
    #[test]
    fn resolved_checkout_status_is_scoped_to_the_current_saved_settings() {
        let (root, ctx, store) = super::super::context::tests::test_context();
        let target = root.join("right");
        fs::create_dir_all(&target).unwrap();
        git(&target, &["init", "--quiet"]);
        git(
            &target,
            &["remote", "add", "origin", "git@github.com:acme/right.git"],
        );
        let mut projects = json!({"github.com/acme/right":{"git":"https://github.com/acme/right.git","path":"~/projects/right","reuse_existing":true}});
        let workers =
            json!([{ "config": {"projects":["github.com/acme/right"]}, "intent":"pause" }]);
        let prepared = prepare(&ctx, &projects, &workers).unwrap();
        let path = target.canonicalize().unwrap();
        assert_eq!(
            prepared[0]["config"]["directories"]["github.com/acme/right"],
            json!(path)
        );
        assert!(!root.join("projects/right").exists());
        let receipt = resolved(&ctx).unwrap();
        assert_eq!(
            project_status(&projects, &receipt)["github.com/acme/right"]["resolved_path"],
            json!(path)
        );
        projects["github.com/acme/right"]["path"] = json!("~/other/right");
        assert!(
            project_status(&projects, &receipt)["github.com/acme/right"]["resolved_path"].is_null()
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
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
            reuse_existing: false,
        };
        assert!(checkout(&root, "github.com/acme/right", &spec).is_ok());
        assert!(checkout(&root, "github.com/acme/wrong", &spec).is_err());
        let plain = root.join("plain");
        fs::create_dir(&plain).unwrap();
        fs::write(plain.join("keep"), "data").unwrap();
        let spec = Checkout {
            git: spec.git,
            path: plain.to_str().unwrap().into(),
            reuse_existing: false,
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
        let receipt = ctx.state.join("project-checkouts.json");
        let mut saved = ctx.read_json(&receipt, json!({})).unwrap();
        saved["github.com/acme/right"]["error"] = json!("Previous clone failed");
        saved["github.com/acme/right"]["retry_at"] = json!(context::now() + 300.0);
        ctx.atomic_json(&receipt, &saved).unwrap();
        assert_eq!(prepare(&ctx, &projects, &workers).unwrap(), resolved);
        assert!(
            ctx.read_json(&receipt, json!({})).unwrap()["github.com/acme/right"]["error"].is_null()
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn retry_clears_only_selected_backoff() {
        let (root, ctx, store) = super::super::context::tests::test_context();
        let receipt = ctx.state.join("project-checkouts.json");
        ctx.atomic_json(&receipt, &json!({"atlas":{"key":"same","error":"failed","retry_at":context::now()+300.0},"other":{"retry_at":123}})).unwrap();
        retry(&ctx, &json!({"atlas":"attempt-1"})).unwrap();
        let saved = ctx.read_json(&receipt, json!({})).unwrap();
        assert!(saved["atlas"]["retry_at"].is_null());
        assert_eq!(saved["atlas"]["error"], "failed");
        assert_eq!(saved["other"]["retry_at"], 123);
        let mut failed_again = saved.clone();
        failed_again["atlas"]["retry_at"] = json!(9999999999u64);
        ctx.atomic_json(&receipt, &failed_again).unwrap();
        retry(&ctx, &json!({"atlas":"attempt-1"})).unwrap();
        assert_eq!(ctx.read_json(&receipt, json!({})).unwrap(), failed_again);
        retry(&ctx, &json!({"atlas":"attempt-2"})).unwrap();
        assert!(ctx.read_json(&receipt, json!({})).unwrap()["atlas"]["retry_at"].is_null());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checkout_errors_explain_service_failures_without_echoing_git_output() {
        assert!(clone_failure("git@host: Permission denied (publickey).").contains("SSH agent"));
        assert!(clone_failure("fatal: No space left on device").contains("disk space"));
        assert!(clone_failure("Could not resolve hostname github.com").contains("resolve"));
        assert!(!clone_failure("unexpected private output").contains("private output"));
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
