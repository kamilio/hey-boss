//! Resumable, bounded file expiration. New files never shelter old siblings.
use super::{Config, Item, databases, now};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;
use std::{
    collections::{BTreeSet, VecDeque},
    fs, io,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Default, Serialize, Deserialize)]
pub struct Progress {
    roots: VecDeque<PathBuf>,
    stack: Vec<Frame>,
    discovered_at: u64,
    #[serde(default)]
    project_roots: VecDeque<PathBuf>,
    #[serde(default)]
    projects: Vec<Frame>,
}
#[derive(Serialize, Deserialize)]
struct Frame {
    path: PathBuf,
    after: Option<PathBuf>,
    #[serde(skip)]
    batch: VecDeque<PathBuf>,
}
impl Frame {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            after: None,
            batch: VecDeque::new(),
        }
    }
    fn next(&mut self) -> io::Result<Option<PathBuf>> {
        if self.batch.is_empty() {
            // Bound persisted memory even for flat directories with millions of files.
            let mut first = BTreeSet::new();
            for entry in fs::read_dir(&self.path)? {
                let p = entry?.path();
                if self.after.as_ref().is_some_and(|last| p <= *last) {
                    continue;
                }
                first.insert(p);
                if first.len() > 1024 {
                    first.pop_last();
                }
            }
            self.batch.extend(first);
        }
        let p = self.batch.pop_front();
        if let Some(p) = &p {
            self.after = Some(p.clone());
        }
        Ok(p)
    }
}
fn add(roots: &mut BTreeSet<PathBuf>, path: PathBuf) {
    if let Ok(p) = path.canonicalize() {
        roots.insert(p);
    }
}
fn discover() -> VecDeque<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut roots = BTreeSet::new();
    for name in [
        ".npm",
        ".cache",
        ".bun/install/cache",
        ".yarn/cache",
        "Library/Caches",
        ".codex/log",
    ] {
        add(&mut roots, home.join(name));
    }
    add(&mut roots, std::env::temp_dir());
    add(&mut roots, PathBuf::from("/tmp"));
    #[cfg(target_os = "macos")]
    if let Ok(temp) =
        super::text(std::process::Command::new("/usr/bin/getconf").arg("DARWIN_USER_TEMP_DIR"))
    {
        let p = PathBuf::from(temp.trim());
        add(&mut roots, p.clone());
        if let Some(parent) = p.parent() {
            add(
                &mut roots,
                parent.join("X/com.google.Chrome.code_sign_clone"),
            );
        }
    }
    for browser in ["Chrome", "Chrome Beta"] {
        let profiles = home
            .join("Library/Application Support/Google")
            .join(browser);
        let mut folders = vec![profiles.clone()];
        if let Ok(entries) = fs::read_dir(profiles) {
            folders.extend(
                entries
                    .flatten()
                    .filter(|e| {
                        e.file_name() == "Default"
                            || e.file_name().to_string_lossy().starts_with("Profile ")
                    })
                    .map(|e| e.path()),
            );
        }
        for folder in folders {
            for cache in [
                "Cache",
                "Code Cache",
                "GPUCache",
                "DawnCache",
                "ShaderCache",
                "GrShaderCache",
                "DawnGraphiteCache",
                "DawnWebGPUCache",
                "Media Cache",
            ] {
                add(&mut roots, folder.join(cache));
            }
        }
    }
    // Avoid duplicate sweeps of nested roots.
    let all = roots.clone();
    roots
        .into_iter()
        .filter(|p| !all.iter().any(|r| r != p && p.starts_with(r)))
        .collect()
}

fn discover_projects(p: &mut Progress) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut visited = 0;
    while Instant::now() < deadline && visited < 10000 && p.roots.len() < 4096 {
        if p.projects.is_empty() {
            let Some(root) = p.project_roots.pop_front() else {
                break;
            };
            p.projects.push(Frame::new(root));
        }
        let frame = p.projects.last_mut().unwrap();
        if frame.path.canonicalize().ok().as_ref() != Some(&frame.path) {
            p.projects.pop();
            continue;
        }
        visited += 1;
        match frame.next() {
            Ok(Some(path)) => {
                if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                    continue;
                }
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if matches!(
                    name.as_ref(),
                    "node_modules"
                        | "out"
                        | "dist"
                        | "build"
                        | "target"
                        | ".turbo"
                        | ".next"
                        | ".vite"
                ) {
                    p.roots.push_back(path);
                } else if !matches!(name.as_ref(), ".git" | ".wrangler") && p.projects.len() < 7 {
                    p.projects.push(Frame::new(path));
                }
            }
            _ => {
                p.projects.pop();
            }
        }
    }
}
fn old_enough(path: &std::path::Path, m: &fs::Metadata, at: u64, clone_file: bool) -> bool {
    let newest = if clone_file {
        // Hard-link churn changes ctime across every signing copy. Age the copy's
        // container and file content instead, so old clones actually expire.
        let container = path.ancestors().find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("code_sign_clone."))
        });
        let Some(created) = container
            .and_then(|p| fs::symlink_metadata(p).ok())
            .map(|m| m.mtime().max(m.ctime()))
        else {
            return false;
        };
        m.mtime().max(created)
    } else {
        m.mtime().max(m.ctime())
    };
    at.saturating_sub(newest.max(0) as u64) >= 86400
}

fn advance(
    progress: &mut Progress,
    at: u64,
    apply: bool,
    deadline: Instant,
    maximum: usize,
) -> (usize, usize, Vec<String>) {
    let mut removed = 0;
    let mut protected = 0;
    let mut errors = Vec::new();
    let mut visited = 0;
    while visited < maximum && Instant::now() < deadline {
        if progress.stack.is_empty() {
            let Some(path) = progress.roots.pop_front() else {
                break;
            };
            progress.stack.push(Frame::new(path));
        }
        let frame = progress.stack.last_mut().unwrap();
        // Never follow a changed ancestor or walk into a repository from /tmp.
        if frame.path.canonicalize().ok().as_ref() != Some(&frame.path)
            || frame.path.join(".git").exists()
            || databases::protected(&frame.path).unwrap_or(true)
        {
            progress.stack.pop();
            continue;
        }
        let next = frame.next();
        visited += 1;
        match next {
            Ok(Some(path)) => {
                let result = (|| -> io::Result<()> {
                    let m = fs::symlink_metadata(&path)?;
                    let clone_file = m.is_file()
                        && path.components().any(|c| {
                            c.as_os_str()
                                .to_string_lossy()
                                .starts_with("code_sign_clone.")
                        });
                    if m.uid() != unsafe { libc::geteuid() } && !clone_file {
                        return Ok(());
                    }
                    if path.file_name().is_some_and(|n| n == ".git") || databases::protected(&path)?
                    {
                        protected += 1;
                        return Ok(());
                    }
                    if m.is_dir() {
                        if progress.stack.len() < 128 {
                            progress.stack.push(Frame::new(path));
                        }
                    } else if (m.is_file() || m.file_type().is_symlink())
                        && old_enough(&path, &m, at, clone_file)
                        && apply
                    {
                        fs::remove_file(path)?;
                        removed += 1;
                    }
                    Ok(())
                })();
                if let Err(e) = result
                    && e.kind() != io::ErrorKind::NotFound
                    && errors.len() < 8
                {
                    errors.push(e.to_string());
                }
            }
            Ok(None) => {
                let frame = progress.stack.pop().unwrap();
                if apply
                    && !progress.stack.is_empty()
                    && fs::symlink_metadata(&frame.path)
                        .is_ok_and(|m| old_enough(&frame.path, &m, at, false))
                {
                    let _ = fs::remove_dir(frame.path);
                }
            }
            Err(e) => {
                progress.stack.pop();
                if errors.len() < 8 {
                    errors.push(e.to_string());
                }
            }
        }
    }
    (removed, protected, errors)
}

pub(super) fn clean(
    config: &Config,
    progress: &mut Progress,
    apply: bool,
) -> io::Result<(Vec<Item>, usize)> {
    if !apply {
        return Ok((vec![Item { name: "24-hour cache expiration".into(), detail: "Enabled roots are inspected incrementally during cleanup; SQLite and sidecars are always retained".into(), eligible: true, worktree: None }], 0));
    }
    if progress.roots.is_empty()
        && progress.stack.is_empty()
        && progress.projects.is_empty()
        && progress.project_roots.is_empty()
    {
        progress.roots = discover();
        progress.project_roots = config
            .workspace_roots
            .iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        progress.discovered_at = now();
    }
    discover_projects(progress);
    let (removed, protected, errors) = advance(
        progress,
        now(),
        apply,
        Instant::now() + Duration::from_secs(25),
        100_000,
    );
    let detail = format!(
        "Deleted {removed} expired files; preserved {protected} database paths; {} roots pending; {}. {}",
        progress.roots.len(),
        if progress.stack.is_empty() {
            "between roots"
        } else {
            "cursor saved for next cycle"
        },
        errors.join("; ")
    );
    Ok((
        vec![Item {
            name: "24-hour cache expiration".into(),
            detail,
            eligible: true,
            worktree: None,
        }],
        removed,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expires_old_siblings_resumes_and_preserves_live_sqlite_and_symlink_targets() {
        let root = std::env::temp_dir().join(format!("harvester-sweep-{}", std::process::id()));
        fs::create_dir_all(root.join("cache/nested")).unwrap();
        let root = root.canonicalize().unwrap();
        let cache = root.join("cache");
        let db = rusqlite::Connection::open(cache.join("nested/History")).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE valuable(value); INSERT INTO valuable VALUES (42);").unwrap();
        let old = cache.join("a-old");
        fs::write(&old, b"old").unwrap();
        fs::write(cache.join("z-new"), b"new").unwrap();
        fs::write(root.join("outside"), b"keep").unwrap();
        std::os::unix::fs::symlink(root.join("outside"), cache.join("link")).unwrap();
        // Future evaluation ages existing files; fresh sibling is stamped later.
        let at = now() + 90000;
        fs::File::options()
            .write(true)
            .open(cache.join("z-new"))
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(at))
            .unwrap();
        let mut p = Progress {
            roots: VecDeque::from([cache.clone()]),
            ..Default::default()
        };
        let mut removed = 0;
        for _ in 0..20 {
            removed += advance(&mut p, at, true, Instant::now() + Duration::from_secs(5), 2).0;
            p = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
            if p.stack.is_empty() && p.roots.is_empty() {
                break;
            }
        }
        assert_eq!(removed, 2);
        assert!(!old.exists());
        assert!(cache.join("z-new").exists());
        assert!(root.join("outside").exists());
        assert!(cache.join("nested/History-wal").exists());
        assert_eq!(
            db.query_row("SELECT value FROM valuable", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            42
        );
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }
    use std::time::UNIX_EPOCH;
}
