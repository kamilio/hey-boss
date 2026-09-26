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
    discard_ignored: bool,
}

fn ineligible_item(name: String, refusal: io::Error, metadata: Details) -> Item {
    let detail = refusal.to_string();
    let error = (!super::is_preserved(&refusal)).then(|| detail.clone());
    Item {
        name,
        detail,
        eligible: false,
        worktree: Some(metadata),
        error,
    }
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
    pub lock_reason: Option<String>,
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
                lock_reason: None,
                bare: false,
            });
        } else if let Some(w) = &mut current {
            if let Some(head) = field.strip_prefix(b"HEAD ") {
                w.head = String::from_utf8(head.to_vec()).map_err(io::Error::other)?;
            }
            if field == b"locked" || field.starts_with(b"locked ") {
                w.locked = true;
                w.lock_reason = field
                    .strip_prefix(b"locked ")
                    .map(|reason| String::from_utf8_lossy(reason).into_owned());
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
fn lock_detail(reason: Option<&str>) -> String {
    match reason.filter(|s| !s.trim().is_empty()) {
        Some(reason) => format!("Locked worktree; preserved — {reason}"),
        None => "Locked worktree; preserved".into(),
    }
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
        .filter(|p| common_directory(p).is_ok_and(|dir| common.insert(dir)))
        .collect()
}

// A checkout's Git pointer and commondir contain everything discovery needs.
// Spawning Git for every linked checkout made each scheduled scan take minutes.
fn common_directory(repo: &Path) -> io::Result<PathBuf> {
    let pointer = repo.join(".git");
    let admin = if pointer.is_dir() {
        pointer
    } else {
        let value = std::fs::read_to_string(pointer)?;
        let target = value
            .trim()
            .strip_prefix("gitdir: ")
            .ok_or_else(|| io::Error::other("Invalid Git pointer"))?;
        repo.join(target)
    };
    if !std::fs::metadata(admin.join("HEAD"))?.is_file() {
        return Err(io::Error::other("Git HEAD is not a file"));
    }
    match std::fs::read_to_string(admin.join("commondir")) {
        Ok(relative) => admin.join(relative.trim()).canonicalize(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => admin.canonicalize(),
        Err(e) => Err(e),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn open_paths() -> io::Result<Vec<PathBuf>> {
    super::linux::open_paths()
}

/// Read-only snapshot of files/cwds owned by this user. Incomplete inspection fails closed.
#[cfg(not(target_os = "linux"))]
pub(crate) fn open_paths() -> io::Result<Vec<PathBuf>> {
    let o = super::output_with_limit(
        Command::new("lsof").args([
            "-nP",
            "-a",
            "-u",
            &unsafe { libc::geteuid() }.to_string(),
            "-F",
            "pn",
        ]),
        // The same inventory protects cache deletion. Busy machines with many
        // test descriptors need the process inventory's 90-second allowance.
        Duration::from_secs(90),
        // Mapped browser libraries repeat for every helper. The global inventory
        // exceeded the ordinary command cap on a real 16 GiB Mac, disabling all
        // cache and checkout cleanup. Still fail closed if this larger cap is hit.
        64 * 1024 * 1024,
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
        .collect::<BTreeSet<_>>()
        .into_iter()
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

fn check_submodules(path: &Path) -> io::Result<()> {
    // Gitlinks may exist without .gitmodules. Large monorepo indexes can exceed
    // the ordinary command-output limit; still fail closed on incomplete output.
    let files = super::output_with_limit(
        &mut git(path, &["ls-files", "--stage", "-z"]),
        Duration::from_secs(20),
        64 * 1024 * 1024,
    )?;
    if !files.status.success() {
        return Err(io::Error::other(
            "Cannot inspect submodule entries; preserved",
        ));
    }
    for entry in files
        .stdout
        .split(|b| *b == 0)
        .filter(|f| f.starts_with(b"160000 "))
    {
        let name = entry
            .splitn(2, |b| *b == b'\t')
            .nth(1)
            .ok_or_else(|| io::Error::other("Invalid submodule entry; preserved"))?;
        let folder = path.join(std::ffi::OsStr::from_bytes(name));
        match std::fs::symlink_metadata(&folder) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
            Ok(meta) if meta.is_dir() => {
                if std::fs::read_dir(&folder)?.next().is_none() {
                    continue;
                }
            }
            Ok(_) => {}
        }
        return Err(super::preserved(format!(
            "Populated or symlinked submodule {}; preserved",
            folder.display()
        )));
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
) -> io::Result<String> {
    if w.path == main {
        return Err(super::preserved("Primary checkout; preserved"));
    }
    if w.locked {
        return Err(super::preserved(lock_detail(w.lock_reason.as_deref())));
    }
    if w.bare {
        return Err(super::preserved("Bare worktree; preserved"));
    }
    if !w.path.is_dir() {
        return Err(super::preserved("Missing checkout; metadata preserved"));
    }
    let canonical = w.path.canonicalize()?;
    if canonical != w.path || !policy.manual && !allowed.iter().any(|r| under(&canonical, r)) {
        return Err(super::preserved(
            "Outside configured roots or symlinked; preserved",
        ));
    }
    let admin = PathBuf::from(git_text(&w.path, &["rev-parse", "--absolute-git-dir"])?);
    // A lock may have been acquired after the registration snapshot. Preserve
    // its reason too; failed reads still fail closed in the checks below.
    if let Ok(reason) = std::fs::read_to_string(admin.join("locked")) {
        return Err(super::preserved(lock_detail(Some(reason.trim_end()))));
    }
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
        return Err(super::preserved("Open in a process or agent; preserved"));
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
            return Err(super::preserved(
                "Git operation or worktree lock present; preserved",
            ));
        }
    }
    let mut newest = git_text(&w.path, &["log", "-1", "--format=%ct"])?
        .parse::<u64>()
        .map_err(io::Error::other)?;
    for path in [
        w.path.clone(),
        w.path.join(".git"),
        admin.join("HEAD"),
        admin.join("index"),
        admin.join("logs/HEAD"),
    ] {
        if path.exists() {
            newest = newest.max(modified(&path)?);
        }
    }
    if at.saturating_sub(newest) < policy.min_age.min(3600) {
        return Err(super::preserved("Recently created or changed; preserved"));
    }
    check_submodules(&w.path)?;
    // One status record is enough to preserve the checkout. A clean result
    // still requires complete output and a successful inspector exit.
    let clean = super::visit_nul_output(
        &mut git(
            &w.path,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                if policy.discard_ignored {
                    "--ignored=no"
                } else {
                    "--ignored=matching"
                },
            ],
        ),
        Duration::from_secs(15),
        |_| Ok(false),
    )?;
    if !clean {
        return Err(super::preserved(
            "Modified, untracked, or ignored files; preserved",
        ));
    }
    let head = git_text(&w.path, &["rev-parse", "HEAD"])?;
    if head != w.head {
        return Err(super::preserved(
            "Checkout changed during inspection; preserved",
        ));
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
        return Err(super::preserved(
            "Commits not merged into the remote default branch; preserved",
        ));
    }
    let files = super::output_with_limit(
        &mut git(&w.path, &["ls-files", "-v", "-z"]),
        Duration::from_secs(15),
        64 * 1024 * 1024,
    )?;
    if !files.status.success() {
        return Err(io::Error::other("Cannot inspect tracked files; preserved"));
    }
    for file in files.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let Some(file) = file.strip_prefix(b"H ") else {
            return Err(super::preserved(
                "Assume-unchanged, sparse, or unusual index flags; preserved",
            ));
        };
        let path = w.path.join(std::ffi::OsStr::from_bytes(file));
        newest = newest.max(modified(&path)?);
    }
    let required_age = if merged {
        policy.min_age.min(3600)
    } else {
        policy.min_age
    };
    if at.saturating_sub(newest) < required_age {
        return Err(super::preserved("Recently created or changed; preserved"));
    }
    Ok(head)
}

fn activity() -> io::Result<(Table, Vec<PathBuf>)> {
    let table = super::processes::inventory()?;
    let mut paths = open_paths()?;
    paths.extend(super::workload_ownership::declared_roots()?);
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
    remove_one_with_policy(path, false)
}

fn remove_one_with_policy(path: &Path, discard_ignored: bool) -> io::Result<()> {
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
    // Persistent ownership needs no process scan to reject removal. In
    // particular, a queued/interrupted agent can be outside this directory.
    if selected.locked {
        return Err(io::Error::other(lock_detail(
            selected.lock_reason.as_deref(),
        )));
    }
    let policy = Policy {
        min_age: 0,
        manual: true,
        discard_ignored,
    };
    let (table, paths) = activity()?;
    let head = eligible(selected, &main, &[], &paths, &table, policy, now())?;
    // Repeat activity and registration checks at the mutation boundary.
    let (table, paths) = activity()?;
    let fresh = list(&main)?;
    let selected = fresh
        .iter()
        .find(|w| w.path == path)
        .ok_or_else(|| io::Error::other("Worktree registration changed; preserved"))?;
    let checked = eligible(selected, &main, &[], &paths, &table, policy, now())?;
    if head != checked {
        return Err(io::Error::other("Worktree HEAD changed; preserved"));
    }
    remove_checkout_with_policy(path, discard_ignored)?;
    Ok(())
}

pub fn clean(
    config: &Config,
    table: &Table,
    observations: &mut BTreeMap<String, Observation>,
    apply: bool,
) -> io::Result<(Vec<Item>, usize)> {
    if config.aggressive {
        return aggressive_clean(config, table, observations, apply);
    }
    let allowed = roots(config);
    if allowed.is_empty() {
        observations.clear();
        return Ok((vec![], 0));
    }
    let mut active_paths = open_paths()?;
    active_paths.extend(super::workload_ownership::declared_roots()?);
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
                    discard_ignored: false,
                },
                at,
            );
            let head = match check {
                Ok(head) => head,
                Err(detail) => {
                    items.push(ineligible_item(name, detail, metadata));
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
                if let Some(fresh) = fresh_trees.iter().find(|x| x.path == w.path) {
                    let fresh_head = match eligible(
                        fresh,
                        &main,
                        &allowed,
                        &fresh_paths,
                        &fresh_table,
                        Policy {
                            min_age: config.worktree_min_age_days * 86400,
                            manual: false,
                            discard_ignored: false,
                        },
                        now(),
                    ) {
                        Ok(head) => head,
                        Err(refusal) => {
                            items.push(ineligible_item(name, refusal, metadata));
                            continue;
                        }
                    };
                    if fresh_head == head {
                        match remove_checkout(&w.path) {
                            Ok(()) if !w.path.exists() => {
                                removed += 1;
                                detail = "Removed clean merged checkout; branch retained".into();
                                observations.remove(&key);
                            }
                            Ok(()) => detail = "Checkout reappeared; preserved".into(),
                            Err(refusal) => {
                                items.push(ineligible_item(name, refusal, metadata));
                                continue;
                            }
                        }
                    } else {
                        detail = "Checkout became active or changed; preserved".into();
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
                error: None,
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
    fn broken_checkout_inspection_is_reported_without_removal() {
        let root =
            std::env::temp_dir().join(format!("harvester-inspection-error-{}", std::process::id()));
        std::fs::create_dir_all(root.join("main")).unwrap();
        let root = root.canonicalize().unwrap();
        let main = root.join("main");
        let work = root.join("work");
        git_text(&main, &["init", "-b", "main"]).unwrap();
        git_text(&main, &["config", "user.email", "test@example.invalid"]).unwrap();
        git_text(&main, &["config", "user.name", "Test"]).unwrap();
        std::fs::write(main.join("file"), "committed").unwrap();
        git_text(&main, &["add", "."]).unwrap();
        git_text(&main, &["commit", "-m", "fixture"]).unwrap();
        git_text(
            &main,
            &["worktree", "add", "-b", "work", work.to_str().unwrap()],
        )
        .unwrap();
        let w = list(&main).unwrap().remove(1);
        std::fs::write(work.join("receipt"), "retain me").unwrap();
        std::fs::rename(work.join(".git"), root.join("saved-git-pointer")).unwrap();
        let mut outcomes = Vec::new();
        for aggressive in [false, true] {
            let inspect = |w: &Worktree| {
                if aggressive {
                    aggressive_eligible(
                        w,
                        &main,
                        std::slice::from_ref(&root),
                        &[],
                        &Table::new(),
                        now(),
                    )
                } else {
                    eligible(
                        w,
                        &main,
                        std::slice::from_ref(&root),
                        &[],
                        &Table::new(),
                        Policy {
                            min_age: 0,
                            manual: false,
                            discard_ignored: false,
                        },
                        now(),
                    )
                }
            };
            let item = ineligible_item(
                work.display().to_string(),
                inspect(&w).unwrap_err(),
                details(&w, "", &None, now()),
            );
            assert!(!item.eligible);
            assert!(item.detail.contains("not a git repository"));
            let mut locked = w.clone();
            locked.locked = true;
            locked.lock_reason = Some("queued owner; keep".into());
            let protected = ineligible_item(
                work.display().to_string(),
                inspect(&locked).unwrap_err(),
                details(&locked, "", &None, now()),
            );
            assert!(protected.detail.contains("queued owner; keep"));
            assert!(
                protected.error.is_none(),
                "ownership refusals are not failures"
            );
            assert_eq!(
                std::fs::read_to_string(work.join("receipt")).unwrap(),
                "retain me"
            );
            assert!(root.join("saved-git-pointer").exists());
            outcomes.push((aggressive, item.error));
        }
        std::fs::remove_dir_all(root).unwrap();
        for (aggressive, error) in outcomes {
            assert!(
                error.is_some_and(|e| e.contains("not a git repository")),
                "inspection failure missing from error count (aggressive={aggressive})"
            );
        }
    }

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
        assert!(
            check_submodules(&work)
                .unwrap_err()
                .to_string()
                .contains("Populated")
        );
        std::fs::write(work.join(".gitmodules"), "").unwrap();
        std::fs::remove_file(work.join("module/local-file")).unwrap();
        std::fs::remove_dir(work.join("module")).unwrap();
        std::os::unix::fs::symlink(&main, work.join("module")).unwrap();
        assert!(
            check_submodules(&work)
                .unwrap_err()
                .to_string()
                .contains("symlinked")
        );
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
        assert_eq!(values[1].lock_reason.as_deref(), Some("reason"));
    }
    #[test]
    fn ownership_lock_survives_queued_active_and_interrupted_use() {
        let root = std::env::temp_dir().join(format!("hb-owned-worktree-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let main = root.join("main");
        let work = root.join("owned");
        std::fs::create_dir(&main).unwrap();
        git_text(&main, &["init", "-b", "main"]).unwrap();
        git_text(&main, &["config", "user.email", "test@example.invalid"]).unwrap();
        git_text(&main, &["config", "user.name", "Test"]).unwrap();
        std::fs::write(main.join("file"), "committed").unwrap();
        git_text(&main, &["add", "file"]).unwrap();
        git_text(&main, &["commit", "-m", "fixture"]).unwrap();
        let head = git_text(&main, &["rev-parse", "HEAD"]).unwrap();
        git_text(&main, &["update-ref", "refs/remotes/origin/main", &head]).unwrap();
        let reason = "issue 147; owner fixture-session; queued validation";
        git_text(
            &main,
            &[
                "worktree",
                "add",
                "--lock",
                "--reason",
                reason,
                "-b",
                "owned",
                work.to_str().unwrap(),
            ],
        )
        .unwrap();
        let admin = PathBuf::from(git_text(&work, &["rev-parse", "--absolute-git-dir"]).unwrap());
        // Evidence lives outside the removable checkout. Simulate an owner waiting
        // elsewhere: no open cwd/file or agent inventory is needed for protection.
        let receipt = root.join("receipt");
        std::fs::write(&receipt, "validation pending; no completed task graph").unwrap();
        for active in [vec![], vec![work.join("file")], vec![]] {
            let w = list(&main)
                .unwrap()
                .into_iter()
                .find(|w| w.path == work)
                .unwrap();
            let error = eligible(
                &w,
                &main,
                std::slice::from_ref(&root),
                &active,
                &Table::new(),
                Policy {
                    min_age: 0,
                    manual: false,
                    discard_ignored: false,
                },
                now() + 86400,
            )
            .unwrap_err();
            assert!(error.to_string().contains(reason), "{error}");
            assert!(remove_one(&work).unwrap_err().to_string().contains(reason));
            // Exercise the actual Git removal path; a lock protects even if an
            // unrelated caller bypasses the health eligibility check.
            assert!(
                git_text(&main, &["worktree", "remove", "--", work.to_str().unwrap()]).is_err()
            );
            assert!(admin.is_dir() && work.join("file").is_file());
        }
        // Resuming inspects the same index and branch, never recreates/reset them.
        std::fs::write(work.join("file"), "staged recovery work").unwrap();
        git_text(&work, &["add", "file"]).unwrap();
        let index = std::fs::read(admin.join("index")).unwrap();
        assert!(remove_one(&work).is_err());
        assert_eq!(std::fs::read(admin.join("index")).unwrap(), index);
        assert_eq!(
            git_text(&work, &["show", ":file"]).unwrap(),
            "staged recovery work"
        );
        assert_eq!(git_text(&work, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            std::fs::read_to_string(&receipt).unwrap(),
            "validation pending; no completed task graph"
        );
        // The fixture owner finishes its work, records completion, and explicitly
        // releases only its own lock. Ordinary safe removal still retains commits.
        git_text(&work, &["commit", "-m", "completed fixture work"]).unwrap();
        let completed = git_text(&work, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(&receipt, "completed fixture work").unwrap();
        git_text(&main, &["worktree", "unlock", work.to_str().unwrap()]).unwrap();
        remove_one(&work).unwrap();
        assert!(!work.exists() && !admin.exists());
        assert_eq!(
            git_text(&main, &["rev-parse", "refs/heads/owned"]).unwrap(),
            completed
        );
        assert_eq!(
            std::fs::read_to_string(&receipt).unwrap(),
            "completed fixture work"
        );
        std::fs::remove_dir_all(root).unwrap();
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
                    discard_ignored: false,
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
                .to_string()
                .contains("Primary")
        );
        assert!(
            check(&w, &[], now())
                .unwrap_err()
                .to_string()
                .contains("Recently")
        );
        assert!(
            check(&w, &[work.join("file")], future)
                .unwrap_err()
                .to_string()
                .contains("Open")
        );
        let mut locked = w.clone();
        locked.locked = true;
        assert!(
            check(&locked, &[], future)
                .unwrap_err()
                .to_string()
                .contains("Locked")
        );
        git_text(&work, &["update-index", "--assume-unchanged", "file"]).unwrap();
        assert!(
            check(&w, &[], future)
                .unwrap_err()
                .to_string()
                .contains("index flags")
        );
        git_text(&work, &["update-index", "--no-assume-unchanged", "file"]).unwrap();
        git_text(&work, &["update-index", "--skip-worktree", "file"]).unwrap();
        assert!(
            check(&w, &[], future)
                .unwrap_err()
                .to_string()
                .contains("index flags")
        );
        git_text(&work, &["update-index", "--no-skip-worktree", "file"]).unwrap();
        std::fs::write(work.join("private.env"), "important").unwrap();
        assert!(
            check(&w, &[], future)
                .unwrap_err()
                .to_string()
                .contains("untracked")
        );
        std::fs::remove_file(work.join("private.env")).unwrap();
        std::fs::write(main.join(".git/info/exclude"), "secret.env\n").unwrap();
        std::fs::write(work.join("secret.env"), "must survive").unwrap();
        assert!(
            check(&w, &[], future)
                .unwrap_err()
                .to_string()
                .contains("ignored")
        );
        std::fs::remove_file(work.join("secret.env")).unwrap();
        git_text(&main, &["worktree", "lock", work.to_str().unwrap()]).unwrap();
        assert!(
            check(&list(&main).unwrap()[1], &[], future)
                .unwrap_err()
                .to_string()
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
            discard_ignored: false,
        };
        assert!(eligible(&unmerged, &main, &[], &[], &table, manual, now()).is_ok());
        git_text(&work, &["checkout", "--detach"]).unwrap();
        assert!(
            check(&list(&main).unwrap()[1], &[], future)
                .unwrap_err()
                .to_string()
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
            .to_string()
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

/// Recheck Git state and inspect databases before any deletion. Git itself enforces
/// ownership locks and refuses newly dirty work at the final removal boundary.
fn remove_checkout(path: &Path) -> io::Result<()> {
    remove_checkout_with_policy(path, false)
}

fn remove_checkout_with_policy(path: &Path, discard_ignored: bool) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if path.canonicalize()? != path {
        return Err(io::Error::other("Noncanonical checkout preserved"));
    }
    let trees = list(path)?;
    let main = trees
        .first()
        .ok_or_else(|| io::Error::other("No registered worktrees"))?;
    let selected = trees
        .iter()
        .find(|w| w.path == path)
        .ok_or_else(|| io::Error::other("Worktree registration changed; preserved"))?;
    eligible(
        selected,
        &main.path,
        &[],
        &[],
        &Table::new(),
        Policy {
            min_age: 0,
            manual: true,
            discard_ignored,
        },
        now(),
    )?;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let device = std::fs::symlink_metadata(path)?.dev();
    let mut pending = vec![path.to_path_buf()];
    while let Some(entry) = pending.pop() {
        if std::time::Instant::now() >= deadline || pending.len() > 100000 {
            return Err(io::Error::other(
                "Database inspection limit reached; preserved",
            ));
        }
        let metadata = std::fs::symlink_metadata(&entry)?;
        if metadata.dev() != device || super::sweep::filesystem_protected(&metadata) {
            return Err(super::preserved(
                "Filesystem boundary or protection preserved",
            ));
        }
        if super::databases::protected(&entry)? {
            return Err(super::preserved("SQLite database or sidecar preserved"));
        }
        if metadata.is_dir() {
            for child in std::fs::read_dir(&entry)? {
                let child = child?;
                if child.file_name() != ".git" {
                    pending.push(child.path());
                }
                if pending.len() > 100000 || std::time::Instant::now() >= deadline {
                    return Err(io::Error::other(
                        "Database inspection limit reached; preserved",
                    ));
                }
            }
        }
    }
    git_text(
        &main.path,
        &[
            "worktree",
            "remove",
            "--",
            path.to_str()
                .ok_or_else(|| io::Error::other("Non-UTF-8 checkout preserved"))?,
        ],
    )?;
    Ok(())
}

fn expired(w: &Worktree, at: u64) -> io::Result<bool> {
    let cutoff = at.saturating_sub(86400);
    let admin = PathBuf::from(git_text(&w.path, &["rev-parse", "--absolute-git-dir"])?);
    for p in [
        w.path.join(".git"),
        admin.join("HEAD"),
        admin.join("index"),
        admin.join("logs/HEAD"),
    ] {
        if p.exists() && modified(&p)? > cutoff {
            return Ok(false);
        }
    }
    super::visit_nul_output(
        &mut git(
            &w.path,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ],
        ),
        Duration::from_secs(30),
        |name| {
            let relative = Path::new(std::ffi::OsStr::from_bytes(name));
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(io::Error::other("Invalid checkout path"));
            }
            let p = w.path.join(relative);
            match std::fs::symlink_metadata(p) {
                Ok(m) => {
                    use std::os::unix::fs::MetadataExt;
                    if m.mtime().max(m.ctime()).max(0) as u64 > cutoff {
                        return Ok(false);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            Ok(true)
        },
    )
}

/// The aggressive age policy does not waive ownership or recovery protection.
fn aggressive_eligible(
    w: &Worktree,
    main: &Path,
    allowed: &[PathBuf],
    active: &[PathBuf],
    table: &Table,
    at: u64,
) -> io::Result<String> {
    let head = eligible(
        w,
        main,
        allowed,
        active,
        table,
        Policy {
            min_age: 86400,
            manual: false,
            discard_ignored: true,
        },
        at,
    )?;
    if !expired(w, at)? {
        return Err(super::preserved(
            "Source or Git activity within 24 hours; preserved",
        ));
    }
    Ok(head)
}

fn aggressive_clean(
    config: &Config,
    table: &Table,
    observations: &mut BTreeMap<String, Observation>,
    apply: bool,
) -> io::Result<(Vec<Item>, usize)> {
    if table.is_empty() {
        return Err(io::Error::other(
            "Process inventory unavailable; cannot protect running Codex worktrees",
        ));
    }
    let (fresh_table, active_paths) = activity()?;
    let mut allowed = roots(config);
    for p in ["/private/tmp", "/tmp", "/Users/Shared"] {
        if let Ok(p) = PathBuf::from(p).canonicalize()
            && !allowed.contains(&p)
        {
            allowed.push(p);
        }
    }
    // Codex nests repositories one level below its randomly named worktree slot.
    let mut search = allowed.clone();
    for root in &allowed {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir())
                    && !entry.path().join(".git").exists()
                {
                    search.push(entry.path());
                }
            }
        }
    }
    let mut candidates = Vec::new();
    let mut items = Vec::new();
    for repo in repositories(&search) {
        match list(&repo) {
            Ok(trees) => {
                if let Some(main) = trees.first().map(|w| w.path.clone()) {
                    candidates.extend(trees.into_iter().skip(1).map(|w| (w, main.clone())));
                }
            }
            Err(e) => items.push(Item {
                name: repo.display().to_string(),
                detail: format!("Inspection failed: {e}"),
                eligible: false,
                error: Some(e.to_string()),
                worktree: None,
            }),
        }
    }
    candidates.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    candidates.dedup_by(|a, b| a.0.path == b.0.path);
    let cursor = observations
        .get("aggressive-cursor")
        .and_then(|o| o.activity_fingerprint.clone())
        .unwrap_or_default();
    let split =
        candidates.partition_point(|(w, _)| w.path.to_string_lossy().as_ref() <= cursor.as_str());
    candidates.rotate_left(split);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut removed = 0;
    for (w, main) in candidates {
        if std::time::Instant::now() >= deadline {
            break;
        }
        let name = w.path.display().to_string();
        observations.insert(
            "aggressive-cursor".into(),
            Observation {
                first_seen: now(),
                last_seen: now(),
                cpu_seconds: 0.0,
                activity_fingerprint: Some(name.clone()),
            },
        );
        if !allowed.iter().any(|r| under(&w.path, r)) || w.bare {
            continue;
        }
        if let Err(detail) =
            aggressive_eligible(&w, &main, &allowed, &active_paths, &fresh_table, now())
        {
            items.push(ineligible_item(name, detail, details(&w, "", &None, now())));
            continue;
        }
        let result = (|| -> io::Result<bool> {
            if apply {
                // Recheck all activity, registration and Git state, then use
                // ordinary Git removal. Missing checkout indexes stay in place.
                remove_one_with_policy(&w.path, true)?;
                removed += 1;
            }
            Ok(true)
        })();
        let error = result
            .as_ref()
            .err()
            .filter(|e| !super::is_preserved(e))
            .map(ToString::to_string);
        let (eligible, detail) = match result {
            Ok(true) => (
                true,
                if apply {
                    "Removed clean expired checkout; branch retained"
                } else {
                    "Clean and unused for 24 hours; ownership and recovery checks passed"
                }
                .into(),
            ),
            Ok(false) => (false, "Source or Git activity within 24 hours".into()),
            Err(e) if super::is_preserved(&e) => (false, e.to_string()),
            Err(e) => (false, format!("Cleanup failed: {e}")),
        };
        items.push(Item {
            name,
            detail,
            eligible,
            worktree: Some(details(&w, "", &None, now())),
            error,
        });
    }
    Ok((items, removed))
}

#[cfg(test)]
mod aggressive_tests {
    use super::*;
    #[test]
    fn aggressive_cleanup_accepts_ignored_files_in_an_unused_checkout() {
        let root =
            std::env::temp_dir().join(format!("harvester-ignored-worktree-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let main = root.join("main");
        let work = root.join("work");
        std::fs::create_dir(&main).unwrap();
        git_text(&main, &["init", "-b", "main"]).unwrap();
        git_text(&main, &["config", "user.email", "test@example.invalid"]).unwrap();
        git_text(&main, &["config", "user.name", "Test"]).unwrap();
        std::fs::write(main.join("file"), "committed").unwrap();
        std::fs::write(main.join(".gitignore"), ".claude/\nnode_modules/\n").unwrap();
        git_text(&main, &["add", "."]).unwrap();
        git_text(&main, &["commit", "-m", "fixture"]).unwrap();
        git_text(
            &main,
            &["worktree", "add", "-b", "completed", work.to_str().unwrap()],
        )
        .unwrap();
        std::fs::create_dir(work.join(".claude")).unwrap();
        std::fs::write(work.join(".claude/settings.local.json"), "{}").unwrap();
        let checkout = list(&main).unwrap().remove(1);
        assert!(
            aggressive_eligible(
                &checkout,
                &main,
                std::slice::from_ref(&root),
                &[],
                &Table::new(),
                now() + 172800,
            )
            .is_ok(),
            "ignored settings alone must not retain an otherwise eligible checkout"
        );
        assert!(
            remove_checkout(&work).is_err(),
            "ordinary removal stays conservative"
        );
        assert!(
            aggressive_eligible(
                &checkout,
                &main,
                std::slice::from_ref(&root),
                std::slice::from_ref(&work),
                &Table::new(),
                now() + 172800,
            )
            .unwrap_err()
            .to_string()
            .contains("Open")
        );
        std::fs::create_dir(work.join("node_modules")).unwrap();
        let database = work.join("node_modules/History");
        std::fs::write(&database, b"SQLite format 3\0fixture").unwrap();
        assert!(
            remove_checkout_with_policy(&work, true)
                .unwrap_err()
                .to_string()
                .contains("SQLite")
        );
        assert!(database.exists() && work.join(".claude/settings.local.json").exists());
        std::fs::remove_file(database).unwrap();
        std::fs::write(work.join("receipt"), "untracked recovery evidence").unwrap();
        assert!(remove_checkout_with_policy(&work, true).is_err());
        assert!(work.join("receipt").exists());
        std::fs::remove_file(work.join("receipt")).unwrap();
        std::fs::write(work.join("file"), "new source edits").unwrap();
        assert!(remove_checkout_with_policy(&work, true).is_err());
        assert_eq!(
            std::fs::read_to_string(work.join("file")).unwrap(),
            "new source edits"
        );
        std::fs::write(work.join("file"), "committed").unwrap();
        #[cfg(target_os = "macos")]
        {
            struct RestoreFlags(PathBuf);
            impl Drop for RestoreFlags {
                fn drop(&mut self) {
                    let path = std::ffi::CString::new(self.0.as_os_str().as_bytes()).unwrap();
                    unsafe { libc::chflags(path.as_ptr(), 0) };
                }
            }
            let protected = work.join("node_modules/protected.log");
            std::fs::write(&protected, "preserved artifact").unwrap();
            let name = std::ffi::CString::new(protected.as_os_str().as_bytes()).unwrap();
            assert_eq!(
                unsafe { libc::chflags(name.as_ptr(), libc::UF_IMMUTABLE) },
                0
            );
            let flags = RestoreFlags(protected.clone());
            assert!(
                remove_checkout_with_policy(&work, true)
                    .unwrap_err()
                    .to_string()
                    .contains("Filesystem")
            );
            assert!(work.join("file").exists());
            drop(flags);
            std::fs::remove_file(protected).unwrap();
        }
        git_text(&main, &["worktree", "lock", work.to_str().unwrap()]).unwrap();
        assert!(
            remove_checkout_with_policy(&work, true)
                .unwrap_err()
                .to_string()
                .contains("Locked")
        );
        git_text(&main, &["worktree", "unlock", work.to_str().unwrap()]).unwrap();
        remove_checkout_with_policy(&work, true).unwrap();
        assert!(!work.exists());
        assert!(git_text(&main, &["rev-parse", "completed"]).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn empty_git_marker_is_not_a_repository() {
        let root = std::env::temp_dir().join(format!("harvester-empty-git-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert!(repositories(std::slice::from_ref(&root)).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn large_git_index_streams_and_missing_registration_preserves_recovery() {
        use std::io::Write;
        let root =
            std::env::temp_dir().join(format!("harvester-large-index-{}", std::process::id()));
        std::fs::create_dir_all(root.join("main")).unwrap();
        let root = root.canonicalize().unwrap();
        let main = root.join("main");
        git_text(&main, &["init", "-b", "main"]).unwrap();
        git_text(&main, &["config", "user.email", "test@example.invalid"]).unwrap();
        git_text(&main, &["config", "user.name", "Test"]).unwrap();
        std::fs::write(main.join("seed"), "fixture").unwrap();
        git_text(&main, &["add", "seed"]).unwrap();
        git_text(&main, &["commit", "-m", "fixture"]).unwrap();
        let work = root.join("work");
        git_text(
            &main,
            &["worktree", "add", "--detach", work.to_str().unwrap()],
        )
        .unwrap();
        let w = list(&main).unwrap().remove(1);
        let hash = git_text(&work, &["rev-parse", "HEAD:seed"]).unwrap();
        let input = root.join("index-input");
        let mut f = std::io::BufWriter::new(std::fs::File::create(&input).unwrap());
        let prefix = format!("{}/", "segment".repeat(5)).repeat(9);
        for i in 0..30000 {
            writeln!(f, "100644 {hash}\t{prefix}{i:08}").unwrap();
        }
        f.flush().unwrap();
        let result = git(&work, &["update-index", "--index-info"])
            .stdin(std::fs::File::open(input).unwrap())
            .output()
            .unwrap();
        assert!(result.status.success());
        let refusal = aggressive_eligible(
            &w,
            &main,
            std::slice::from_ref(&root),
            &[],
            &Table::new(),
            now() + 172800,
        )
        .unwrap_err();
        assert!(
            refusal
                .to_string()
                .contains("Modified, untracked, or ignored"),
            "{refusal}"
        );
        assert!(expired(&w, now() + 172800).unwrap());
        std::fs::write(work.join("recent"), "keep").unwrap();
        std::fs::File::options()
            .write(true)
            .open(work.join("recent"))
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(now() + 172800))
            .unwrap();
        assert!(!expired(&w, now() + 172800).unwrap());
        let admin = PathBuf::from(git_text(&work, &["rev-parse", "--absolute-git-dir"]).unwrap());
        let index = std::fs::read(admin.join("index")).unwrap();
        // Move only this fixture's checkout aside to simulate loss; keep its data.
        std::fs::rename(&work, root.join("saved-work")).unwrap();
        let refusal = aggressive_eligible(
            &w,
            &main,
            std::slice::from_ref(&root),
            &[],
            &Table::new(),
            now() + 172800,
        )
        .unwrap_err();
        assert!(
            refusal
                .to_string()
                .contains("Missing checkout; metadata preserved")
        );
        assert_eq!(list(&main).unwrap().len(), 2);
        assert_eq!(std::fs::read(admin.join("index")).unwrap(), index);
        assert_eq!(git_text(&main, &["rev-parse", "HEAD"]).unwrap(), w.head);
        assert!(main.join("seed").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn aggressive_cleanup_preserves_ownership_edits_receipts_and_databases() {
        let root =
            std::env::temp_dir().join(format!("harvester-old-worktree-{}", std::process::id()));
        std::fs::create_dir_all(root.join("main")).unwrap();
        let root = root.canonicalize().unwrap();
        let main = root.join("main");
        let work = root.join("work");
        git_text(&main, &["init", "-b", "main"]).unwrap();
        git_text(&main, &["config", "user.email", "test@example.invalid"]).unwrap();
        git_text(&main, &["config", "user.name", "Test"]).unwrap();
        std::fs::write(main.join("file"), "committed").unwrap();
        std::fs::write(main.join(".gitignore"), "node_modules\n").unwrap();
        git_text(&main, &["add", "."]).unwrap();
        git_text(&main, &["commit", "-m", "fixture"]).unwrap();
        git_text(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "owned",
                "--lock",
                work.to_str().unwrap(),
            ],
        )
        .unwrap();
        std::fs::write(work.join("file"), "staged recovery work").unwrap();
        std::fs::create_dir(work.join("node_modules")).unwrap();
        std::fs::write(work.join("node_modules/artifact"), "large ignored build").unwrap();
        assert_eq!(
            common_directory(&main).unwrap(),
            common_directory(&work).unwrap()
        );
        assert_eq!(repositories(std::slice::from_ref(&root)).len(), 1);
        git_text(&work, &["add", "file"]).unwrap();
        let receipt = root.join("receipt");
        std::fs::write(&receipt, "validation pending").unwrap();
        let w = list(&main).unwrap().remove(1);
        for active in [vec![], vec![work.join("file")], vec![]] {
            assert!(
                aggressive_eligible(
                    &w,
                    &main,
                    std::slice::from_ref(&root),
                    &active,
                    &Table::new(),
                    now() + 172800
                )
                .unwrap_err()
                .to_string()
                .contains("Locked")
            );
        }
        assert!(!expired(&w, now()).unwrap());
        assert!(expired(&w, now() + 172800).unwrap());
        let admin = PathBuf::from(git_text(&work, &["rev-parse", "--absolute-git-dir"]).unwrap());
        let index = std::fs::read(admin.join("index")).unwrap();
        let refusal =
            remove_checkout(&work).expect_err("expired ownership locks must survive cleanup");
        assert!(refusal.to_string().contains("Locked"));
        assert_eq!(std::fs::read(admin.join("index")).unwrap(), index);
        assert_eq!(
            std::fs::read_to_string(work.join("file")).unwrap(),
            "staged recovery work"
        );
        git_text(&main, &["worktree", "unlock", work.to_str().unwrap()]).unwrap();
        assert!(
            remove_checkout(&work).is_err(),
            "uncommitted edits must survive cleanup"
        );
        let unlocked = list(&main).unwrap().remove(1);
        assert!(
            aggressive_eligible(
                &unlocked,
                &main,
                std::slice::from_ref(&root),
                &[],
                &Table::new(),
                now() + 172800
            )
            .unwrap_err()
            .to_string()
            .contains("Modified")
        );
        assert_eq!(std::fs::read(admin.join("index")).unwrap(), index);
        assert_eq!(
            std::fs::read_to_string(&receipt).unwrap(),
            "validation pending"
        );
        let db = rusqlite::Connection::open(work.join("node_modules/History")).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE valuable(value); INSERT INTO valuable VALUES (42);").unwrap();
        assert!(remove_checkout(&work).is_err());
        assert!(work.join("node_modules/artifact").exists());
        assert!(work.join(".git").exists() && work.join("node_modules/History-wal").exists());
        assert_eq!(
            db.query_row("SELECT value FROM valuable", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            42
        );
        drop(db);
        // Simulate the owner explicitly relocating its database before retrying.
        std::fs::rename(work.join("node_modules/History"), root.join("saved.sqlite")).unwrap();
        std::fs::remove_file(work.join("node_modules/artifact")).unwrap();
        std::fs::remove_dir(work.join("node_modules")).unwrap();
        git_text(&work, &["commit", "-m", "completed fixture"]).unwrap();
        let completed = list(&main).unwrap().remove(1);
        assert!(
            aggressive_eligible(
                &completed,
                &main,
                std::slice::from_ref(&root),
                &[work.join("file")],
                &Table::new(),
                now() + 172800
            )
            .unwrap_err()
            .to_string()
            .contains("Open")
        );
        assert!(
            aggressive_eligible(
                &completed,
                &main,
                std::slice::from_ref(&root),
                &[],
                &Table::new(),
                now() + 172800
            )
            .is_ok()
        );
        // Even a clean tracked SQLite file prevents partial checkout deletion.
        std::fs::write(work.join("History"), b"SQLite format 3\0fixture").unwrap();
        git_text(&work, &["add", "History"]).unwrap();
        git_text(&work, &["commit", "-m", "database fixture"]).unwrap();
        assert!(
            remove_checkout(&work)
                .unwrap_err()
                .to_string()
                .contains("SQLite")
        );
        assert!(work.join("file").exists());
        git_text(&work, &["rm", "History"]).unwrap();
        git_text(&work, &["commit", "-m", "owner completed"]).unwrap();
        let head = git_text(&work, &["rev-parse", "HEAD"]).unwrap();
        remove_checkout(&work).unwrap();
        assert_eq!(git_text(&main, &["rev-parse", "owned"]).unwrap(), head);
        assert_eq!(
            std::fs::read_to_string(receipt).unwrap(),
            "validation pending"
        );
        assert!(!work.exists());
        assert_eq!(list(&main).unwrap().len(), 1);
        assert!(main.join("file").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
