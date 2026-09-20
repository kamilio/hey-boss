//! Only known disposable subtrees; inspect twice and recheck activity before deletion.
use super::{Item, Observation};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_CANDIDATES: usize = 256;
const CACHE_NAMES: &[&str] = &[
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "ShaderCache",
    "GrShaderCache",
    "GraphiteDawnCache",
    "download_cache",
    "Media Cache",
];

#[derive(Clone)]
struct Candidate {
    path: PathBuf,
    min_age: u64,
}

fn add(candidates: &mut Vec<Candidate>, path: PathBuf, min_age: u64) {
    if candidates.len() < 2048 && path.exists() {
        candidates.push(Candidate { path, min_age });
    }
}

fn discover(home: &Path, temp: Option<&Path>) -> io::Result<Vec<Candidate>> {
    let mut candidates = Vec::new();
    for path in [
        home.join(".npm/_cacache"),
        home.join("Library/Caches/pip"),
        home.join("Library/Caches/uv"),
        home.join(".cache/pip"),
        home.join(".cache/uv"),
    ] {
        add(&mut candidates, path, 86400);
    }
    for browser in ["Chrome", "Chrome Beta"] {
        add(
            &mut candidates,
            home.join("Library/Caches/Google").join(browser),
            86400,
        );
        let root = home
            .join("Library/Application Support/Google")
            .join(browser);
        for name in CACHE_NAMES {
            add(&mut candidates, root.join(name), 86400);
        }
        if root.is_dir() {
            for entry in fs::read_dir(root)?.take(256) {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == "Default"
                    || name
                        .strip_prefix("Profile ")
                        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                {
                    for cache in CACHE_NAMES {
                        add(&mut candidates, entry.path().join(cache), 86400);
                    }
                }
            }
        }
    }
    if let Some(temp) = temp {
        let root = temp
            .parent()
            .unwrap_or(temp)
            .join("X/com.google.Chrome.code_sign_clone");
        if root.is_dir() {
            for entry in fs::read_dir(root)?.take(1024) {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("code_sign_clone.")
                {
                    // Recognize Chrome's signing copies, never arbitrary app or temp directories.
                    let children: Vec<_> = fs::read_dir(entry.path())?
                        .take(2)
                        .collect::<Result<_, _>>()?;
                    if children.len() == 1 && children[0].file_name() == "Google Chrome.app.bundle"
                    {
                        add(&mut candidates, entry.path(), 3600);
                    }
                }
            }
        }
        for entry in fs::read_dir(temp)?.take(20000) {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if ["hb-health-", "hey-boss-cache-test-"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
            {
                add(&mut candidates, entry.path(), 86400);
            }
        }
    }
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    candidates.dedup_by(|a, b| a.path == b.path);
    // New active copies must not indefinitely hide older entries behind the cap.
    candidates.sort_by_cached_key(|c| {
        fs::symlink_metadata(&c.path)
            .map(|m| m.mtime())
            .unwrap_or(i64::MAX)
    });
    candidates.truncate(MAX_CANDIDATES);
    Ok(candidates)
}

fn fingerprint(candidate: &Candidate, active: &[PathBuf], at: u64) -> io::Result<String> {
    let root = &candidate.path;
    if !root.is_absolute() || root.canonicalize()? != *root {
        return Err(io::Error::other(
            "Symlinked or noncanonical cache; preserved",
        ));
    }
    if active.iter().any(|p| p.starts_with(root)) {
        return Err(io::Error::other("Open in a process; preserved"));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut pending = vec![root.clone()];
    let mut hash = Sha256::new();
    let mut count = 0;
    while let Some(path) = pending.pop() {
        count += 1;
        if count > 100000 || Instant::now() >= deadline {
            return Err(io::Error::other(
                "Cache inspection limit reached; preserved",
            ));
        }
        let m = fs::symlink_metadata(&path)?;
        if m.uid() != unsafe { libc::geteuid() }
            || (!m.is_dir() && !m.is_file() && !m.file_type().is_symlink())
        {
            return Err(io::Error::other("Unowned or special cache file; preserved"));
        }
        let newest = m.mtime().max(m.ctime()).max(0) as u64;
        if at.saturating_sub(newest) < candidate.min_age {
            return Err(io::Error::other("Recently used or changed; preserved"));
        }
        use std::os::unix::ffi::OsStrExt;
        hash.update(path.as_os_str().as_bytes());
        for value in [
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime() as u64,
            m.mtime_nsec() as u64,
            m.ctime() as u64,
            m.ctime_nsec() as u64,
            m.mode() as u64,
        ] {
            hash.update(value.to_le_bytes());
        }
        if m.is_dir() {
            let mut children = Vec::new();
            for entry in fs::read_dir(&path)? {
                if children.len() + pending.len() + count >= 100000 || Instant::now() >= deadline {
                    return Err(io::Error::other(
                        "Cache inspection limit reached; preserved",
                    ));
                }
                children.push(entry?.path());
            }
            children.sort();
            pending.extend(children);
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn run(
    candidates: &[Candidate],
    observations: &mut BTreeMap<String, Observation>,
    active: &[PathBuf],
    at: u64,
    config: &super::Config,
    apply: bool,
    mut refresh: impl FnMut() -> io::Result<Vec<PathBuf>>,
) -> io::Result<(Vec<Item>, usize)> {
    let mut retained = BTreeSet::new();
    let mut items = Vec::new();
    let mut removed = 0;
    let started = Instant::now();
    for candidate in candidates.iter().take(MAX_CANDIDATES) {
        let name = candidate.path.display().to_string();
        let mut eligible = false;
        let check = if started.elapsed() > Duration::from_secs(60) {
            Err(io::Error::other(
                "Cache cycle inspection limit reached; preserved",
            ))
        } else {
            fingerprint(candidate, active, at)
        };
        let detail = match check {
            Err(e) => e.to_string(),
            Ok(identity) => {
                let first = observations
                    .get(&name)
                    .filter(|o| {
                        o.activity_fingerprint.as_deref() == Some(&identity)
                            && at >= o.last_seen
                            && at - o.last_seen <= config.interval_seconds.saturating_mul(3)
                    })
                    .map_or(at, |o| o.first_seen);
                observations.insert(
                    name.clone(),
                    Observation {
                        first_seen: first,
                        last_seen: at,
                        cpu_seconds: 0.0,
                        activity_fingerprint: Some(identity.clone()),
                    },
                );
                retained.insert(name.clone());
                eligible = at.saturating_sub(first) >= config.observation_seconds;
                if eligible && apply {
                    let fresh = refresh().and_then(|paths| fingerprint(candidate, &paths, at));
                    if !fresh.is_ok_and(|value| value == identity) {
                        observations.remove(&name);
                        eligible = false;
                        "Activity or identity changed before deletion; preserved".into()
                    } else {
                        let result = fs::symlink_metadata(&candidate.path).and_then(|m| {
                            if m.is_dir() {
                                fs::remove_dir_all(&candidate.path)
                            } else {
                                fs::remove_file(&candidate.path)
                            }
                        });
                        match result {
                            Ok(()) => {
                                removed += 1;
                                observations.remove(&name);
                                "Removed inactive disposable cache".into()
                            }
                            Err(e) => format!("Cache removal failed: {e}"),
                        }
                    }
                } else if eligible {
                    "Inactive disposable cache; eligible".into()
                } else {
                    "Inactive cache; observing before cleanup".into()
                }
            }
        };
        items.push(Item {
            name,
            detail,
            eligible,
            worktree: None,
        });
    }
    observations.retain(|key, _| retained.contains(key));
    Ok((items, removed))
}

pub(super) fn clean(
    config: &super::Config,
    observations: &mut BTreeMap<String, Observation>,
    apply: bool,
) -> io::Result<(Vec<Item>, usize)> {
    let home = PathBuf::from(
        std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME unavailable"))?,
    );
    #[cfg(target_os = "macos")]
    let temp =
        super::text(std::process::Command::new("/usr/bin/getconf").arg("DARWIN_USER_TEMP_DIR"))
            .ok()
            .and_then(|p| PathBuf::from(p.trim()).canonicalize().ok());
    #[cfg(not(target_os = "macos"))]
    let temp: Option<PathBuf> = None;
    let candidates = discover(&home, temp.as_deref())?;
    if candidates.is_empty() {
        observations.clear();
        return Ok((vec![], 0));
    }
    let paths = || {
        super::worktrees::open_paths().map(|paths| {
            paths
                .into_iter()
                .map(|p| p.canonicalize().unwrap_or(p))
                .collect::<Vec<_>>()
        })
    };
    let active = paths()?;
    let mut refreshed: Option<(Instant, Vec<PathBuf>)> = None;
    run(
        &candidates,
        observations,
        &active,
        super::now(),
        config,
        apply,
        || {
            if refreshed
                .as_ref()
                .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(5))
            {
                refreshed = Some((Instant::now(), paths()?));
                // Timestamp after inspection so slow lsof does not trigger repeated scans.
                refreshed.as_mut().unwrap().0 = Instant::now();
            }
            Ok(refreshed.as_ref().unwrap().1.clone())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);

    fn config() -> super::super::Config {
        super::super::Config {
            observation_seconds: 60,
            ..super::super::Config::default()
        }
    }

    #[test]
    fn discovery_is_limited_to_disposable_names_and_chrome_signing_copies() {
        let root = fixture();
        let profile = root.join("Library/Application Support/Google/Chrome/Default");
        fs::create_dir_all(profile.join("Code Cache")).unwrap();
        fs::create_dir_all(profile.join("Service Worker")).unwrap();
        fs::write(profile.join("Cookies"), "keep").unwrap();
        let temp = root.join("T");
        fs::create_dir_all(temp.join("hb-health-old-fixture")).unwrap();
        fs::create_dir_all(temp.join("unrelated-project")).unwrap();
        let clones = root.join("X/com.google.Chrome.code_sign_clone");
        fs::create_dir_all(clones.join("code_sign_clone.valid/Google Chrome.app.bundle")).unwrap();
        fs::create_dir_all(clones.join("code_sign_clone.other/Important.app")).unwrap();
        let candidates = discover(&root, Some(&temp)).unwrap();
        assert_eq!(candidates.len(), 3);
        assert!(
            candidates
                .iter()
                .any(|c| c.path == profile.join("Code Cache"))
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.path == clones.join("code_sign_clone.valid") && c.min_age == 3600)
        );
        assert!(!candidates.iter().any(|c| c.path == profile
            || c.path.ends_with("Service Worker")
            || c.path.ends_with("unrelated-project")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn internal_symlinks_never_delete_targets_and_failed_activity_check_preserves_cache() {
        let root = fixture();
        symlink(root.join("History"), root.join("Cache/link")).unwrap();
        let candidate = Candidate {
            path: root.join("Cache"),
            min_age: 0,
        };
        let at = super::super::now() + 100;
        let mut observations = BTreeMap::new();
        run(
            std::slice::from_ref(&candidate),
            &mut observations,
            &[],
            at,
            &config(),
            false,
            || Ok(vec![]),
        )
        .unwrap();
        assert_eq!(
            run(
                std::slice::from_ref(&candidate),
                &mut observations,
                &[],
                at + 60,
                &config(),
                true,
                || Err(io::Error::other("Incomplete inventory"))
            )
            .unwrap()
            .1,
            0
        );
        assert!(root.join("Cache").exists());
        run(
            std::slice::from_ref(&candidate),
            &mut observations,
            &[],
            at + 61,
            &config(),
            false,
            || Ok(vec![]),
        )
        .unwrap();
        assert_eq!(
            run(
                &[candidate],
                &mut observations,
                &[],
                at + 121,
                &config(),
                true,
                || Ok(vec![])
            )
            .unwrap()
            .1,
            1
        );
        assert_eq!(fs::read_to_string(root.join("History")).unwrap(), "keep");
        fs::remove_dir_all(root).unwrap();
    }

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "hey-boss-cache-test-{}-{}-{}",
            std::process::id(),
            super::super::now(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("Cache")).unwrap();
        fs::write(root.join("Cache/data"), "disposable").unwrap();
        fs::write(root.join("History"), "keep").unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn cleanup_requires_quiet_observation_and_preserves_profile_data() {
        let root = fixture();
        let candidate = Candidate {
            path: root.join("Cache"),
            min_age: 0,
        };
        let at = super::super::now() + 100;
        let mut observations = BTreeMap::new();
        assert_eq!(
            run(
                std::slice::from_ref(&candidate),
                &mut observations,
                &[],
                at,
                &config(),
                false,
                || Ok(vec![])
            )
            .unwrap()
            .1,
            0
        );
        assert!(root.join("Cache").exists());
        assert_eq!(
            run(
                std::slice::from_ref(&candidate),
                &mut observations,
                &[],
                at + 60,
                &config(),
                false,
                || Ok(vec![])
            )
            .unwrap()
            .1,
            0
        );
        assert_eq!(
            run(
                &[candidate],
                &mut observations,
                &[],
                at + 61,
                &config(),
                true,
                || Ok(vec![])
            )
            .unwrap()
            .1,
            1
        );
        assert!(root.join("History").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recent_open_changed_and_symlinked_candidates_are_preserved() {
        let root = fixture();
        let candidate = Candidate {
            path: root.join("Cache"),
            min_age: 86400,
        };
        let mut observations = BTreeMap::new();
        let at = super::super::now();
        assert!(
            !run(
                std::slice::from_ref(&candidate),
                &mut observations,
                &[],
                at,
                &config(),
                true,
                || Ok(vec![])
            )
            .unwrap()
            .0[0]
                .eligible
        );
        let candidate = Candidate {
            min_age: 0,
            ..candidate
        };
        run(
            std::slice::from_ref(&candidate),
            &mut observations,
            &[],
            at + 100,
            &config(),
            false,
            || Ok(vec![]),
        )
        .unwrap();
        let open = vec![root.join("Cache/data")];
        assert_eq!(
            run(
                std::slice::from_ref(&candidate),
                &mut observations,
                &[],
                at + 160,
                &config(),
                true,
                || Ok(open.clone())
            )
            .unwrap()
            .1,
            0
        );
        run(
            std::slice::from_ref(&candidate),
            &mut observations,
            &[],
            at + 200,
            &config(),
            false,
            || Ok(vec![]),
        )
        .unwrap();
        fs::write(root.join("Cache/new"), "changed").unwrap();
        assert_eq!(
            run(
                std::slice::from_ref(&candidate),
                &mut observations,
                &[],
                at + 260,
                &config(),
                true,
                || Ok(vec![])
            )
            .unwrap()
            .1,
            0
        );
        fs::rename(root.join("Cache"), root.join("saved")).unwrap();
        symlink(root.join("saved"), root.join("Cache")).unwrap();
        assert_eq!(
            run(
                &[candidate],
                &mut observations,
                &[],
                at + 400,
                &config(),
                true,
                || Ok(vec![])
            )
            .unwrap()
            .1,
            0
        );
        assert!(root.join("saved/data").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
