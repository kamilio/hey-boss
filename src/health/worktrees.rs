use super::{Config, Item, Observation, now, output, processes::Table, text, under};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Details {
    pub path: PathBuf,
    pub age_seconds: Option<u64>,
    pub repository: String,
    pub github_url: Option<String>,
}
#[derive(Clone, Copy)]
struct Policy {
    min_age: u64,
    manual: bool,
}

fn github(remote: &str) -> Option<(String, String)> {
    let path = if let Some(path) = remote.trim().strip_prefix("git@github.com:") {
        path
    } else {
        let (scheme, rest) = remote.trim().split_once("://")?;
        if !matches!(scheme, "https" | "http" | "ssh") {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        if !authority
            .rsplit('@')
            .next()?
            .eq_ignore_ascii_case("github.com")
        {
            return None;
        }
        path
    };
    let path = path
        .split(['?', '#'])
        .next()?
        .trim_matches('/')
        .trim_end_matches(".git");
    let pieces: Vec<_> = path.split('/').collect();
    if pieces.len() != 2
        || pieces.iter().any(|p| {
            p.is_empty()
                || matches!(*p, "." | "..")
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
    {
        return None;
    }
    Some((path.into(), format!("https://github.com/{path}")))
}
fn details(w: &Worktree, repository: &str, github_url: &Option<String>, at: u64) -> Details {
    let age_seconds = std::fs::metadata(w.path.join(".git"))
        .ok()
        .and_then(|m| m.created().or_else(|_| m.modified()).ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|t| at.saturating_sub(t.as_secs()));
    Details {
        path: w.path.clone(),
        age_seconds,
        repository: repository.into(),
        github_url: github_url.clone(),
    }
}

#[derive(Clone, Debug)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    pub locked: bool,
    pub bare: bool,
}
fn git(path: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "maintenance.auto=false",
            "-C",
        ])
        .arg(path)
        .args(args);
    c
}
fn git_text(path: &Path, args: &[&str]) -> io::Result<String> {
    text(&mut git(path, args)).map(|s| s.trim().to_owned())
}
pub fn parse_list(bytes: &[u8]) -> io::Result<Vec<Worktree>> {
    use std::os::unix::ffi::OsStringExt;
    let mut result = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in bytes.split(|b| *b == 0) {
        if let Some(path) = field.strip_prefix(b"worktree ") {
            if let Some(w) = current.take() {
                result.push(w);
            }
            current = Some(Worktree {
                path: PathBuf::from(std::ffi::OsString::from_vec(path.to_vec())),
                head: String::new(),
                locked: false,
                bare: false,
            });
        } else if let Some(w) = &mut current {
            if let Some(head) = field.strip_prefix(b"HEAD ") {
                w.head = String::from_utf8(head.to_vec()).map_err(io::Error::other)?;
            }
            if field.starts_with(b"locked") {
                w.locked = true;
            }
            if field == b"bare" {
                w.bare = true;
            }
        }
    }
    if let Some(w) = current {
        result.push(w);
    }
    if result.iter().any(|w| !w.path.is_absolute()) {
        return Err(io::Error::other("Git returned a non-absolute worktree"));
    }
    Ok(result)
}
fn list(repo: &Path) -> io::Result<Vec<Worktree>> {
    let out = output(
        &mut git(repo, &["worktree", "list", "--porcelain", "-z"]),
        Duration::from_secs(15),
    )?;
    if !out.status.success() {
        return Err(io::Error::other("Cannot list Git worktrees"));
    }
    parse_list(&out.stdout)
}

fn roots(config: &Config) -> Vec<PathBuf> {
    config
        .workspace_roots
        .iter()
        .filter_map(|p| p.canonicalize().ok())
        .collect()
}
fn repositories(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut candidates = BTreeSet::new();
    for root in roots {
        if root.join(".git").exists() {
            candidates.insert(root.clone());
        }
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten().take(1000) {
                if entry.file_type().is_ok_and(|t| t.is_dir()) && entry.path().join(".git").exists()
                {
                    candidates.insert(entry.path());
                }
            }
        }
    }
    let mut common = BTreeSet::new();
    candidates
        .into_iter()
        .filter(|p| {
            git_text(
                p,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .is_ok_and(|dir| common.insert(dir))
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub(crate) fn open_paths() -> io::Result<Vec<PathBuf>> {
    super::linux::open_paths()
}

/// Read-only snapshot of files/cwds owned by this user. Incomplete inspection fails closed.
#[cfg(not(target_os = "linux"))]
pub(crate) fn open_paths() -> io::Result<Vec<PathBuf>> {
    let o = output(
        Command::new("lsof").args([
            "-nP",
            "-a",
            "-u",
            &unsafe { libc::geteuid() }.to_string(),
            "-F",
            "pn",
        ]),
        Duration::from_secs(20),
    )?;
    if !o.status.success() || !o.stderr.is_empty() {
        return Err(io::Error::other(
            "Cannot inspect open files; cleanup preserved",
        ));
    }
    let value = String::from_utf8(o.stdout).map_err(io::Error::other)?;
    Ok(value
        .lines()
        .filter_map(|s| s.strip_prefix("n/"))
        .map(|s| PathBuf::from(format!("/{s}")))
        .collect())
}
fn modified(path: &Path) -> io::Result<u64> {
    Ok(std::fs::symlink_metadata(path)?
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs())
}
fn remote_base(path: &Path) -> Option<String> {
    if let Ok(base) = git_text(
        path,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    ) && base.starts_with("refs/remotes/origin/")
    {
        return Some(base);
    }
    ["refs/remotes/origin/main", "refs/remotes/origin/master"]
        .iter()
        .find_map(|base| {
            git_text(path, &["rev-parse", "--verify", base])
                .ok()
                .map(|_| (*base).to_owned())
        })
}

fn check_submodules(path: &Path) -> Result<(), String> {
    // Gitlinks may exist without .gitmodules. Large monorepo indexes can exceed
    // the ordinary command-output limit; still fail closed on incomplete output.
    let files = super::output_with_limit(
        &mut git(path, &["ls-files", "--stage", "-z"]),
        Duration::from_secs(20),
        64 * 1024 * 1024,
    )
    .map_err(|e| e.to_string())?;
    if !files.status.success() {
        return Err("Cannot inspect submodule entries; preserved".into());
    }
    for entry in files
        .stdout
        .split(|b| *b == 0)
        .filter(|f| f.starts_with(b"160000 "))
    {
        let name = entry
            .splitn(2, |b| *b == b'\t')
            .nth(1)
            .ok_or("Invalid submodule entry; preserved")?;
        let folder = path.join(std::ffi::OsStr::from_bytes(name));
        match std::fs::symlink_metadata(&folder) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
            Ok(meta) if meta.is_dir() => {
                if std::fs::read_dir(&folder)
                    .map_err(|e| e.to_string())?
                    .next()
                    .is_none()
                {
                    continue;
                }
            }
            Ok(_) => {}
        }
        return Err(format!(
            "Populated or symlinked submodule {}; preserved",
            folder.display()
        ));
    }
    Ok(())
}

/// Returns the exact immutable commit used for eligibility; every removal rechecks it.
fn eligible(
    w: &Worktree,
    main: &Path,
    allowed: &[PathBuf],
    active_paths: &[PathBuf],
    table: &Table,
    policy: Policy,
    at: u64,
) -> Result<String, String> {
    if w.path == main {
        return Err("Primary checkout; preserved".into());
    }
    if w.locked || w.bare {
        return Err("Locked or bare worktree; preserved".into());
    }
    if !w.path.is_dir() {
        return Err("Missing checkout; metadata preserved".into());
    }
    let canonical = w.path.canonicalize().map_err(|e| e.to_string())?;
    if canonical != w.path || !policy.manual && !allowed.iter().any(|r| under(&canonical, r)) {
        return Err("Outside configured roots or symlinked; preserved".into());
    }
    let admin = PathBuf::from(
        git_text(&w.path, &["rev-parse", "--absolute-git-dir"]).map_err(|e| e.to_string())?,
    );
    if active_paths
        .iter()
        .any(|p| under(p, &canonical) || under(p, &admin))
        || table
            .values()
            .filter(|p| p.pid != std::process::id())
            .any(|p| {
                p.arguments.contains(&format!("{}/", canonical.display()))
                    || p.arguments
                        .split_whitespace()
                        .any(|s| s == canonical.to_string_lossy())
            })
    {
        return Err("Open in a process or agent; preserved".into());
    }
    for name in [
        "locked",
        "index.lock",
        "HEAD.lock",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_START",
        "rebase-apply",
        "rebase-merge",
        "sequencer",
    ] {
        if admin.join(name).exists() {
            return Err("Git operation or worktree lock present; preserved".into());
        }
    }
    let mut newest = git_text(&w.path, &["log", "-1", "--format=%ct"])
        .map_err(|e| e.to_string())?
        .parse::<u64>()
        .map_err(|e| e.to_string())?;
    for path in [
        w.path.clone(),
        w.path.join(".git"),
        admin.join("HEAD"),
        admin.join("index"),
        admin.join("logs/HEAD"),
    ] {
        if path.exists() {
            newest = newest.max(modified(&path).map_err(|e| e.to_string())?);
        }
    }
    if at.saturating_sub(newest) < policy.min_age.min(3600) {
        return Err("Recently created or changed; preserved".into());
    }
    check_submodules(&w.path)?;
    let status = git_text(
        &w.path,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignored=matching",
        ],
    )
    .map_err(|e| e.to_string())?;
    if !status.is_empty() {
        return Err("Modified, untracked, or ignored files; preserved".into());
    }
    let head = git_text(&w.path, &["rev-parse", "HEAD"]).map_err(|e| e.to_string())?;
    if head != w.head {
        return Err("Checkout changed during inspection; preserved".into());
    }
    let retained_branch = git_text(&w.path, &["symbolic-ref", "--quiet", "HEAD"])
        .ok()
        .filter(|s| s.starts_with("refs/heads/"))
        .is_some_and(|branch| {
            git_text(&w.path, &["rev-parse", "--verify", &branch]).is_ok_and(|value| value == head)
        });
    let merged = remote_base(&w.path).is_some_and(|base| {
        output(
            &mut git(&w.path, &["merge-base", "--is-ancestor", &head, &base]),
            Duration::from_secs(15),
        )
        .is_ok_and(|o| o.status.success())
    });
    if !merged && !retained_branch {
        return Err("Commits not merged into the remote default branch; preserved".into());
    }
    let files = super::output_with_limit(
        &mut git(&w.path, &["ls-files", "-v", "-z"]),
        Duration::from_secs(15),
        64 * 1024 * 1024,
    )
    .map_err(|e| e.to_string())?;
    if !files.status.success() {
        return Err("Cannot inspect tracked files; preserved".into());
    }
    for file in files.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let Some(file) = file.strip_prefix(b"H ") else {
            return Err("Assume-unchanged, sparse, or unusual index flags; preserved".into());
        };
        let path = w.path.join(std::ffi::OsStr::from_bytes(file));
        newest = newest.max(modified(&path).map_err(|e| e.to_string())?);
    }
    let required_age = if merged {
        policy.min_age.min(3600)
    } else {
        policy.min_age
    };
    if at.saturating_sub(newest) < required_age {
        return Err("Recently created or changed; preserved".into());
    }
    Ok(head)
}

fn activity() -> io::Result<(Table, Vec<PathBuf>)> {
    let table = super::processes::inventory()?;
    let mut paths = open_paths()?;
    let agents = crate::agents::scan();
    if !agents.warnings.is_empty() {
        return Err(io::Error::other(
            "Agent activity could not be fully verified; worktree preserved",
        ));
    }
    for agent in agents.agents {
        if let Some(cwd) = agent.cwd {
            paths.push(PathBuf::from(cwd));
        }
        if let Some(git) = agent.git {
            paths.push(PathBuf::from(git.worktree));
        }
    }
    Ok((table, paths))
}

/// Explicit checkout removal bypasses age only. A named branch retains unmerged commits.
pub fn remove_one(path: &Path) -> io::Result<()> {
    if !path.is_absolute() || path.canonicalize()? != path {
        return Err(io::Error::other(
            "Select an absolute, non-symlinked worktree path",
        ));
    }
    let trees = list(path)?;
    let main = trees
        .first()
        .ok_or_else(|| io::Error::other("No registered worktrees"))?
        .path
        .clone();
    let selected = trees
        .iter()
        .find(|w| w.path == path)
        .ok_or_else(|| io::Error::other("Not a registered worktree"))?;
    let policy = Policy {
        min_age: 0,
        manual: true,
    };
    let (table, paths) = activity()?;
    let head =
        eligible(selected, &main, &[], &paths, &table, policy, now()).map_err(io::Error::other)?;
    // Repeat activity and registration checks at the mutation boundary.
    let (table, paths) = activity()?;
    let fresh = list(&main)?;
    let selected = fresh
        .iter()
        .find(|w| w.path == path)
        .ok_or_else(|| io::Error::other("Worktree registration changed; preserved"))?;
    let checked =
        eligible(selected, &main, &[], &paths, &table, policy, now()).map_err(io::Error::other)?;
    if head != checked {
        return Err(io::Error::other("Worktree HEAD changed; preserved"));
    }
    let result = output(
        git(&main, &["worktree", "remove", "--"]).arg(path),
        Duration::from_secs(60),
    )?;
    if !result.status.success() || path.exists() {
        return Err(io::Error::other(format!(
            "Git refused removal; preserved: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    Ok(())
}

pub fn clean(
    config: &Config,
    table: &Table,
    observations: &mut BTreeMap<String, Observation>,
    apply: bool,
) -> io::Result<(Vec<Item>, usize)> {
    let allowed = roots(config);
    if allowed.is_empty() {
        observations.clear();
        return Ok((vec![], 0));
    }
    let mut active_paths = open_paths()?;
    let agents = crate::agents::scan();
    if !agents.warnings.is_empty() {
        return Err(io::Error::other(
            "Agent activity could not be fully verified; worktrees preserved",
        ));
    }
    for agent in agents.agents {
        if let Some(cwd) = agent.cwd {
            active_paths.push(PathBuf::from(cwd));
        }
        if let Some(git) = agent.git {
            active_paths.push(PathBuf::from(git.worktree));
        }
    }
    let at = now();
    let mut items = Vec::new();
    let mut removed = 0;
    let mut retained = BTreeSet::new();
    for repo in repositories(&allowed) {
        let origin = git_text(&repo, &["config", "--get", "remote.origin.url"])
            .ok()
            .and_then(|s| github(&s));
        let repository = origin.as_ref().map(|p| p.0.clone()).unwrap_or_else(|| {
            repo.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        let github_url = origin.map(|p| p.1);
        let trees = list(&repo)?;
        let Some(main) = trees.first().map(|w| w.path.clone()) else {
            continue;
        };
        for w in trees.iter().skip(1) {
            let metadata = details(w, &repository, &github_url, at);
            let name = w.path.display().to_string();
            let check = eligible(
                w,
                &main,
                &allowed,
                &active_paths,
                table,
                Policy {
                    min_age: config.worktree_min_age_days * 86400,
                    manual: false,
                },
                at,
            );
            let head = match check {
                Ok(head) => head,
                Err(detail) => {
                    items.push(Item {
                        name,
                        detail,
                        eligible: false,
                        worktree: Some(metadata),
                    });
                    continue;
                }
            };
            let key = format!("{name}:{head}");
            let first = observations
                .get(&key)
                .filter(|o| at >= o.last_seen && at - o.last_seen <= config.interval_seconds * 3)
                .map_or(at, |o| o.first_seen);
            observations.insert(
                key.clone(),
                Observation {
                    activity_fingerprint: None,
                    first_seen: first,
                    last_seen: at,
                    cpu_seconds: 0.0,
                },
            );
            retained.insert(key.clone());
            let ready = at.saturating_sub(first) >= config.observation_seconds;
            let mut detail = if ready {
                "Clean and unused; merged or old with a retained branch"
            } else {
                "Merged or old with a retained branch; observing before cleanup"
            }
            .to_owned();
            if ready && apply {
                // Re-read registration/locks, process activity and Git state at the mutation boundary.
                let fresh_trees = list(&repo)?;
                let fresh_table = super::processes::inventory()?;
                let mut fresh_paths = open_paths()?;
                fresh_paths.extend(active_paths.iter().cloned());
                let fresh_agents = crate::agents::scan();
                if !fresh_agents.warnings.is_empty() {
                    return Err(io::Error::other(
                        "Agent activity changed; worktrees preserved",
                    ));
                }
                for agent in fresh_agents.agents {
                    if let Some(cwd) = agent.cwd {
                        fresh_paths.push(PathBuf::from(cwd));
                    }
                    if let Some(git) = agent.git {
                        fresh_paths.push(PathBuf::from(git.worktree));
                    }
                }
                if let Some(fresh) = fresh_trees.iter().find(|x| x.path == w.path)
                    && eligible(
                        fresh,
                        &main,
                        &allowed,
                        &fresh_paths,
                        &fresh_table,
                        Policy {
                            min_age: config.worktree_min_age_days * 86400,
                            manual: false,
                        },
                        now(),
                    )
                    .as_deref()
                        == Ok(&head)
                {
                    let mut cmd = git(&repo, &["worktree", "remove", "--"]);
                    cmd.arg(&w.path);
                    let out = output(&mut cmd, Duration::from_secs(30))?;
                    if out.status.success() && !w.path.exists() {
                        removed += 1;
                        detail = "Removed clean merged checkout; branch retained".into();
                        observations.remove(&key);
                    } else {
                        detail = "Git refused removal; preserved".into();
                    }
                } else {
                    detail = "Checkout became active or changed; preserved".into();
                }
            }
            items.push(Item {
                name,
                detail,
                eligible: ready,
                worktree: Some(metadata),
            });
        }
    }
    observations.retain(|k, _| retained.contains(k));
    Ok((items, removed))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_origins_strip_credentials_and_reject_other_hosts() {
        for remote in [
            "git@github.com:owner/repo.git",
            "https://secret@github.com/owner/repo.git",
            "ssh://git@github.com/owner/repo",
            "https://github.com/owner/repo?token=secret",
        ] {
            assert_eq!(
                github(remote),
                Some(("owner/repo".into(), "https://github.com/owner/repo".into()))
            );
        }
        for remote in [
            "https://github.com.evil/owner/repo",
            "https://github.com/../repo",
            "file:///tmp/repo",
            "https://github.com/owner/repo/extra",
        ] {
            assert!(github(remote).is_none());
        }
    }
    #[test]
    fn empty_uninitialized_submodule_can_be_removed_but_local_content_cannot() {
        let root = std::env::temp_dir().join(format!("hb-health-submodule-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let main = root.join("main");
        let work = root.join("work");
        std::fs::create_dir(&main).unwrap();
        git_text(&main, &["init", "-b", "main"]).unwrap();
        git_text(&main, &["config", "user.email", "test@example.invalid"]).unwrap();
        git_text(&main, &["config", "user.name", "Test"]).unwrap();
        std::fs::write(main.join(".gitmodules"), "").unwrap();
        git_text(&main, &["add", "."]).unwrap();
        git_text(&main, &["commit", "-m", "initial"]).unwrap();
        let head = git_text(&main, &["rev-parse", "HEAD"]).unwrap();
        git_text(
            &main,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{head},module"),
            ],
        )
        .unwrap();
        git_text(&main, &["commit", "-m", "uninitialized gitlink"]).unwrap();
        git_text(
            &main,
            &["worktree", "add", "-b", "keep", work.to_str().unwrap()],
        )
        .unwrap();
        std::fs::create_dir_all(work.join("module")).unwrap();
        assert!(check_submodules(&work).is_ok());
        std::fs::write(work.join("module/local-file"), "must survive").unwrap();
        assert!(
            remove_one(&work)
                .unwrap_err()
                .to_string()
                .contains("Populated")
        );
        assert!(work.join("module/local-file").exists());
        // A missing metadata file must not hide populated gitlinks.
        std::fs::remove_file(work.join(".gitmodules")).unwrap();
        assert!(check_submodules(&work).unwrap_err().contains("Populated"));
        std::fs::write(work.join(".gitmodules"), "").unwrap();
        std::fs::remove_file(work.join("module/local-file")).unwrap();
        std::fs::remove_dir(work.join("module")).unwrap();
        std::os::unix::fs::symlink(&main, work.join("module")).unwrap();
        assert!(check_submodules(&work).unwrap_err().contains("symlinked"));
        std::fs::remove_file(work.join("module")).unwrap();
        std::fs::create_dir(work.join("module")).unwrap();
        remove_one(&work).unwrap();
        assert!(!work.exists());
        assert!(git_text(&main, &["rev-parse", "refs/heads/keep"]).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn nul_records_preserve_spaces_newlines_and_locks() {
        let values = parse_list(
            b"worktree /tmp/main\0HEAD abc\0\0worktree /tmp/a\nb c\0HEAD def\0locked reason\0\0",
        )
        .unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[1].path, Path::new("/tmp/a\nb c"));
        assert!(values[1].locked);
    }
    #[test]
    fn protects_dirty_unmerged_active_locked_recent_and_primary_real_worktrees() {
        let root = std::env::temp_dir().join(format!("hb-health-git-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let main = root.join("main");
        let work = root.join("work");
        std::fs::create_dir(&main).unwrap();
        git_text(&main, &["init", "-b", "main"]).unwrap();
        git_text(&main, &["config", "user.email", "test@example.invalid"]).unwrap();
        git_text(&main, &["config", "user.name", "Test"]).unwrap();
        std::fs::write(main.join("file"), "original").unwrap();
        git_text(&main, &["add", "."]).unwrap();
        git_text(&main, &["commit", "-m", "initial"]).unwrap();
        let head = git_text(&main, &["rev-parse", "HEAD"]).unwrap();
        git_text(&main, &["update-ref", "refs/remotes/origin/main", &head]).unwrap();
        git_text(
            &main,
            &["worktree", "add", "-b", "done", work.to_str().unwrap()],
        )
        .unwrap();
        let trees = list(&main).unwrap();
        let w = trees[1].clone();
        let metadata = details(
            &w,
            "owner/repo",
            &Some("https://github.com/owner/repo".into()),
            now(),
        );
        assert_eq!(metadata.path, work);
        assert!(metadata.age_seconds.is_some_and(|s| s < 60));
        let table = Table::new();
        let allowed = vec![root.clone()];
        let check = |w: &Worktree, active: &[PathBuf], at: u64| {
            eligible(
                w,
                &main,
                &allowed,
                active,
                &table,
                Policy {
                    min_age: 86400,
                    manual: false,
                },
                at,
            )
        };
        let future = now() + 30 * 86400;
        assert!(check(&w, &[], future).is_ok());
        assert!(check(&w, &[], now() + 3601).is_ok());
        assert!(
            check(&trees[0], &[], future)
                .unwrap_err()
                .contains("Primary")
        );
        assert!(check(&w, &[], now()).unwrap_err().contains("Recently"));
        assert!(
            check(&w, &[work.join("file")], future)
                .unwrap_err()
                .contains("Open")
        );
        let mut locked = w.clone();
        locked.locked = true;
        assert!(check(&locked, &[], future).unwrap_err().contains("Locked"));
        git_text(&work, &["update-index", "--assume-unchanged", "file"]).unwrap();
        assert!(check(&w, &[], future).unwrap_err().contains("index flags"));
        git_text(&work, &["update-index", "--no-assume-unchanged", "file"]).unwrap();
        git_text(&work, &["update-index", "--skip-worktree", "file"]).unwrap();
        assert!(check(&w, &[], future).unwrap_err().contains("index flags"));
        git_text(&work, &["update-index", "--no-skip-worktree", "file"]).unwrap();
        std::fs::write(work.join("private.env"), "important").unwrap();
        assert!(check(&w, &[], future).unwrap_err().contains("untracked"));
        std::fs::remove_file(work.join("private.env")).unwrap();
        std::fs::write(main.join(".git/info/exclude"), "secret.env\n").unwrap();
        std::fs::write(work.join("secret.env"), "must survive").unwrap();
        assert!(check(&w, &[], future).unwrap_err().contains("ignored"));
        std::fs::remove_file(work.join("secret.env")).unwrap();
        git_text(&main, &["worktree", "lock", work.to_str().unwrap()]).unwrap();
        assert!(
            check(&list(&main).unwrap()[1], &[], future)
                .unwrap_err()
                .contains("Locked")
        );
        git_text(&main, &["worktree", "unlock", work.to_str().unwrap()]).unwrap();
        std::fs::write(work.join("file"), "modified").unwrap();
        assert!(check(&w, &[], future).is_err());
        git_text(&work, &["commit", "-am", "unmerged"]).unwrap();
        let unmerged = list(&main).unwrap()[1].clone();
        // Old named-branch work can be removed without losing its commits.
        assert!(check(&unmerged, &[], future).is_ok());
        assert!(check(&unmerged, &[], now() + 3601).is_err());
        let manual = Policy {
            min_age: 0,
            manual: true,
        };
        assert!(eligible(&unmerged, &main, &[], &[], &table, manual, now()).is_ok());
        git_text(&work, &["checkout", "--detach"]).unwrap();
        assert!(
            check(&list(&main).unwrap()[1], &[], future)
                .unwrap_err()
                .contains("not merged")
        );
        assert!(
            eligible(
                &list(&main).unwrap()[1],
                &main,
                &[],
                &[],
                &table,
                manual,
                now()
            )
            .unwrap_err()
            .contains("not merged")
        );
        git_text(&work, &["checkout", "done"]).unwrap();
        // Manual removal of a young unmerged checkout keeps its named branch.
        remove_one(&work).unwrap();
        assert!(!work.exists());
        assert_eq!(
            git_text(&main, &["rev-parse", "refs/heads/done"]).unwrap(),
            unmerged.head
        );
        git_text(&main, &["worktree", "add", work.to_str().unwrap(), "done"]).unwrap();
        // Once the exact commit is merged, automatic eligibility also succeeds.
        git_text(
            &main,
            &["update-ref", "refs/remotes/origin/main", &unmerged.head],
        )
        .unwrap();
        assert!(check(&unmerged, &[], future).is_ok());
        git_text(&main, &["worktree", "remove", "--", work.to_str().unwrap()]).unwrap();
        assert!(!work.exists() && main.join("file").exists());
        assert_eq!(
            git_text(&main, &["rev-parse", "refs/heads/done"]).unwrap(),
            unmerged.head
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
