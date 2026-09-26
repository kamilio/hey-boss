//! Detached jobs can retain checkout ownership after losing their Codex ancestor.
use super::processes::{Table, identity};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(super) struct Protection {
    pub reason: String,
    pub uncertain: bool,
}

fn checkouts(path: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let mut result = Vec::new();
    for root in path.ancestors() {
        let marker = root.join(".git");
        let admin = match fs::symlink_metadata(&marker) {
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e),
            Ok(m) if m.is_dir() => marker,
            Ok(m) if m.is_file() => {
                let pointer = fs::read_to_string(marker)?;
                let target = pointer
                    .trim()
                    .strip_prefix("gitdir: ")
                    .ok_or_else(|| io::Error::other("Invalid checkout ownership pointer"))?;
                let admin = root.join(target);
                if !fs::metadata(&admin)?.is_dir() {
                    return Err(io::Error::other("Missing checkout ownership directory"));
                }
                admin
            }
            Ok(_) => return Err(io::Error::other("Symlinked checkout ownership pointer")),
        };
        result.push((root.to_owned(), admin));
    }
    Ok(result)
}

fn owned_path(path: &Path, active: &BTreeSet<PathBuf>) -> io::Result<bool> {
    if !path.is_absolute() {
        return Ok(false);
    }
    let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    if active.iter().any(|root| path.starts_with(root)) {
        return Ok(true);
    }
    for (_, admin) in checkouts(&path)? {
        match fs::symlink_metadata(admin.join("locked")) {
            Ok(_) => return Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

type Paths = BTreeMap<u32, Result<Vec<PathBuf>, String>>;

#[cfg(target_os = "macos")]
fn paths(ids: &BTreeSet<u32>) -> Paths {
    use std::os::unix::ffi::OsStringExt;
    use std::process::Command;
    use std::time::Duration;
    let list = ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let read = || -> io::Result<BTreeMap<u32, Vec<PathBuf>>> {
        let output = super::output_with_limit(
            Command::new("/usr/sbin/lsof").args([
                "-nP",
                "-a",
                "-u",
                &unsafe { libc::geteuid() }.to_string(),
                "-p",
                &list,
                "-F0pfn",
            ]),
            Duration::from_secs(30),
            16 * 1024 * 1024,
        )?;
        if !output.status.success() || !output.stderr.is_empty() {
            return Err(io::Error::other("Cannot inspect workload files"));
        }
        let mut result = BTreeMap::<u32, Vec<PathBuf>>::new();
        let mut cwd = BTreeSet::new();
        let mut pid = 0;
        let mut descriptor = Vec::new();
        for field in output.stdout.split(|b| *b == 0) {
            let field = field.strip_prefix(b"\n").unwrap_or(field);
            match field.split_first() {
                Some((b'p', value)) => {
                    pid = std::str::from_utf8(value)
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| io::Error::other("Invalid workload process field"))?;
                    descriptor.clear();
                }
                Some((b'f', value)) => descriptor = value.to_vec(),
                Some((b'n', value)) if value.starts_with(b"/") && ids.contains(&pid) => {
                    if descriptor == b"cwd" {
                        cwd.insert(pid);
                    }
                    result
                        .entry(pid)
                        .or_default()
                        .push(PathBuf::from(std::ffi::OsString::from_vec(value.to_vec())));
                }
                _ => {}
            }
        }
        result.retain(|pid, _| cwd.contains(pid));
        Ok(result)
    };
    match read() {
        Ok(mut result) => ids
            .iter()
            .map(|pid| {
                (
                    *pid,
                    result
                        .remove(pid)
                        .ok_or_else(|| "Cannot inspect workload cwd".into()),
                )
            })
            .collect(),
        Err(error) => ids
            .iter()
            .map(|pid| (*pid, Err(error.to_string())))
            .collect(),
    }
}

#[cfg(target_os = "linux")]
fn paths(ids: &BTreeSet<u32>) -> Paths {
    ids.iter()
        .map(|pid| {
            let read = || -> io::Result<Vec<PathBuf>> {
                let directory = PathBuf::from(format!("/proc/{pid}"));
                if super::linux::authentication_service(&directory) {
                    return Ok(Vec::new());
                }
                let mut result = vec![fs::read_link(directory.join("cwd"))?];
                result.extend(
                    super::linux::descriptors(*pid)?
                        .into_iter()
                        .map(|(_, path)| path),
                );
                Ok(result)
            };
            (*pid, read().map_err(|e| e.to_string()))
        })
        .collect()
}

// Protect both descendants and their controllers, without expanding unrelated
// siblings merely because their common shell is an ancestor of owned work.
fn extend_family(table: &Table, protected: &mut BTreeMap<u32, Protection>) {
    loop {
        let additions: Vec<_> = table
            .values()
            .filter_map(|p| {
                (!protected.contains_key(&p.pid))
                    .then(|| protected.get(&p.parent).cloned().map(|why| (p.pid, why)))
                    .flatten()
            })
            .collect();
        if additions.is_empty() {
            break;
        }
        protected.extend(additions);
    }
    for pid in protected.keys().copied().collect::<Vec<_>>() {
        let why = protected[&pid].clone();
        let mut parent = table.get(&pid).map_or(0, |p| p.parent);
        let mut visited = BTreeSet::new();
        while parent > 1 && visited.insert(parent) {
            protected.entry(parent).or_insert_with(|| why.clone());
            parent = table.get(&parent).map_or(0, |p| p.parent);
        }
    }
}

pub(super) fn protected(table: &Table, candidates: &BTreeSet<u32>) -> BTreeMap<u32, Protection> {
    if candidates.is_empty() {
        return BTreeMap::new();
    }
    let agents = crate::agents::scan();
    let active = || -> io::Result<BTreeSet<PathBuf>> {
        if !agents.warnings.is_empty() {
            return Err(io::Error::other("Agent ownership inventory is incomplete"));
        }
        let mut roots = BTreeSet::new();
        for agent in &agents.agents {
            if let Some(git) = &agent.git {
                roots.insert(PathBuf::from(&git.worktree));
            }
            if let Some(cwd) = &agent.cwd
                && let Some((root, _)) = checkouts(Path::new(cwd))?.first()
            {
                roots.insert(root.clone());
            }
        }
        Ok(roots
            .into_iter()
            .map(|p| p.canonicalize().unwrap_or(p))
            .collect())
    };
    let active = match active() {
        Ok(paths) => paths,
        Err(e) => {
            return candidates
                .iter()
                .map(|pid| {
                    (
                        *pid,
                        Protection {
                            reason: e.to_string(),
                            uncertain: true,
                        },
                    )
                })
                .collect();
        }
    };
    let mut subjects = candidates.clone();
    for pid in candidates {
        let mut parent = table.get(pid).map_or(0, |p| p.parent);
        while parent > 1
            && table
                .get(&parent)
                .is_some_and(|p| p.uid == unsafe { libc::geteuid() })
            && subjects.insert(parent)
        {
            parent = table[&parent].parent;
        }
    }
    let mut protected = BTreeMap::new();
    let mut known = BTreeMap::<PathBuf, Result<bool, String>>::new();
    for (pid, paths) in paths(&subjects) {
        if identity(pid).is_none() {
            continue;
        }
        let check = || -> Result<bool, String> {
            let mut paths = paths?;
            if let Some(p) = table.get(&pid) {
                paths.push(PathBuf::from(&p.executable));
            }
            for path in paths {
                let owned = known
                    .entry(path.clone())
                    .or_insert_with(|| owned_path(&path, &active).map_err(|e| e.to_string()));
                if *owned.as_ref().map_err(Clone::clone)? {
                    return Ok(true);
                }
            }
            Ok(false)
        };
        match check() {
            Ok(false) => {}
            Ok(true) => {
                protected.insert(
                    pid,
                    Protection {
                        reason: "Active or locked worktree; workload and controller preserved"
                            .into(),
                        uncertain: false,
                    },
                );
            }
            Err(error) => {
                protected.insert(
                    pid,
                    Protection {
                        reason: format!("Cannot verify workload ownership; preserved: {error}"),
                        uncertain: true,
                    },
                );
            }
        }
    }
    extend_family(table, &mut protected);
    protected.retain(|pid, _| candidates.contains(pid));
    protected
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_covers_open_files_nested_checkouts_and_new_locks() {
        let root = std::env::temp_dir().join(format!("hb-workload-paths-{}", std::process::id()));
        let work = root.join("owned work");
        let nested = work.join("nested");
        fs::create_dir_all(nested.join(".git")).unwrap();
        fs::create_dir_all(root.join("admin")).unwrap();
        fs::write(work.join(".git"), "gitdir: ../admin\n").unwrap();
        let file = nested.join("open\nfile.log");
        fs::write(&file, "fixture").unwrap();
        let active = BTreeSet::new();
        assert!(!owned_path(&file, &active).unwrap());
        fs::write(root.join("admin/locked"), "queued owner").unwrap();
        assert!(owned_path(&file, &active).unwrap());
        fs::remove_file(root.join("admin/locked")).unwrap();
        assert!(!owned_path(&file, &active).unwrap());
        let active = BTreeSet::from([work.canonicalize().unwrap()]);
        assert!(owned_path(&file, &active).unwrap());
        assert!(!owned_path(&root.join("owned work-other/file"), &active).unwrap());
        fs::write(work.join(".git"), "unreadable ownership format").unwrap();
        assert!(owned_path(&file, &BTreeSet::new()).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protects_descendants_and_controllers_without_protecting_unrelated_siblings() {
        let process = |pid, parent| super::super::processes::Process {
            pid,
            parent,
            uid: 1,
            age_seconds: 4000,
            cpu_seconds: 0.0,
            executable: "node".into(),
            identity: "fixture".into(),
            arguments: String::new(),
        };
        let table = Table::from([
            (10, process(10, 1)),
            (11, process(11, 10)),
            (12, process(12, 11)),
            (13, process(13, 10)),
        ]);
        for uncertain in [false, true] {
            let mut protected = BTreeMap::from([(
                11,
                Protection {
                    reason: "owner or inspection failure".into(),
                    uncertain,
                },
            )]);
            extend_family(&table, &mut protected);
            assert_eq!(
                protected.keys().copied().collect::<Vec<_>>(),
                vec![10, 11, 12]
            );
            assert_eq!(protected[&10].uncertain, uncertain);
        }
    }
}
