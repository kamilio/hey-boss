//! Resumable, bounded file expiration. New files never shelter old siblings.
use super::{Config, Item, databases, now};
use serde::{Deserialize, Serialize};
use std::os::{
    fd::AsRawFd,
    unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
};
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
    #[serde(default)]
    deferred: VecDeque<Vec<Frame>>,
    #[serde(default)]
    pub stats: Statistics,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Statistics {
    pub pass_started_at: u64,
    pub last_completed_at: Option<u64>,
    pub last_pass_seconds: Option<u64>,
    pub visited_this_cycle: u64,
    pub removed_this_cycle: u64,
    /// Logical file lengths, not net free-space gain (hard links may share blocks).
    pub unlinked_bytes_this_cycle: u64,
    pub visited_this_pass: u64,
    pub removed_this_pass: u64,
    pub roots_pending: usize,
    pub discovery_pending: bool,
    pub slice_millis: u64,
}
impl Statistics {
    pub fn last_completion(&self) -> String {
        self.last_completed_at
            .map(|at| format!("{}s ago", now().saturating_sub(at)))
            .unwrap_or_else(|| "not yet observed".into())
    }
}
#[derive(Serialize, Deserialize)]
struct Frame {
    path: PathBuf,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    identity: Option<(u64, u64)>,
    #[serde(default)]
    batch: VecDeque<Vec<u8>>,
}
impl Frame {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            identity: None,
            batch: VecDeque::new(),
        }
    }
    fn next(&mut self) -> io::Result<Option<PathBuf>> {
        if self.batch.is_empty() {
            self.refill()?;
        }
        Ok(self
            .batch
            .pop_front()
            .map(|name| self.path.join(std::ffi::OsStr::from_bytes(&name))))
    }
    fn refill(&mut self) -> io::Result<()> {
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&self.path)?;
        let meta = file.metadata()?;
        let identity = (meta.dev(), meta.ino());
        if self.identity != Some(identity) {
            self.offset = 0;
            self.identity = Some(identity);
        }
        let fd = file.as_raw_fd();
        if unsafe { libc::lseek(fd, self.offset, libc::SEEK_SET) } == -1 {
            return Err(io::Error::last_os_error());
        }
        // Persist the kernel chunk offset AND unconsumed names. telldir cookies
        // belong to a DIR stream; macOS d_seekoff is often zero. Neither can
        // safely resume a reopened stream. One bounded read avoids rescanning
        // a huge flat directory for each batch or scheduling interval.
        let mut bytes = [0u8; 8192];
        #[cfg(target_os = "macos")]
        let count = {
            unsafe extern "C" {
                fn __getdirentries64(
                    fd: libc::c_int,
                    buf: *mut libc::c_void,
                    size: libc::size_t,
                    base: *mut libc::off_t,
                ) -> libc::ssize_t;
            }
            let mut base = 0;
            unsafe { __getdirentries64(fd, bytes.as_mut_ptr().cast(), bytes.len(), &mut base) }
        };
        #[cfg(target_os = "linux")]
        let count =
            unsafe { libc::syscall(libc::SYS_getdents64, fd, bytes.as_mut_ptr(), bytes.len()) }
                as isize;
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        let offset = unsafe { libc::lseek(fd, 0, libc::SEEK_CUR) };
        if offset == -1 {
            return Err(io::Error::last_os_error());
        }
        if count > 0 && offset == self.offset {
            return Err(io::Error::other("Directory cursor did not advance"));
        }
        #[cfg(target_os = "macos")]
        let name_start = 21;
        #[cfg(target_os = "linux")]
        let name_start = 19;
        let mut pos = 0;
        let mut batch = VecDeque::new();
        while pos < count as usize {
            let remaining = &bytes[pos..count as usize];
            if remaining.len() <= name_start {
                return Err(io::Error::other("Truncated directory entry"));
            }
            let length = u16::from_ne_bytes([remaining[16], remaining[17]]) as usize;
            if length <= name_start || length > remaining.len() {
                return Err(io::Error::other("Invalid directory entry length"));
            }
            let name = &remaining[name_start..length];
            let end = name
                .iter()
                .position(|b| *b == 0)
                .ok_or_else(|| io::Error::other("Unterminated directory entry"))?;
            let name = &name[..end];
            if !name.is_empty() && name != b"." && name != b".." {
                if name.contains(&b'/') {
                    return Err(io::Error::other("Invalid directory entry name"));
                }
                batch.push_back(name.to_vec());
            }
            pos += length;
        }
        self.offset = offset;
        self.batch = batch;
        Ok(())
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

fn checkout_marker(path: &std::path::Path) -> io::Result<bool> {
    let marker = path.join(".git");
    match fs::symlink_metadata(&marker) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
        Ok(m) if m.is_dir() => {
            // Some tools leave an empty .git in /tmp. It must not disable the
            // whole temp sweep. Nonempty or unreadable markers stay protected.
            Ok(fs::read_dir(marker)?.next().transpose()?.is_some())
        }
        Ok(_) => Ok(true),
    }
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
    let mut root_visits = 0;
    while visited < maximum && Instant::now() < deadline {
        if root_visits >= 256 && !progress.stack.is_empty() {
            progress
                .deferred
                .push_back(std::mem::take(&mut progress.stack));
        }
        if progress.stack.is_empty() {
            root_visits = 0;
            // Give later roots a turn without retaining thousands of open
            // traversal stacks. Estimate their serialized size conservatively.
            let deferred_bytes: usize = progress
                .deferred
                .iter()
                .flatten()
                .map(|frame| {
                    frame.path.as_os_str().as_bytes().len() * 6
                        + 128
                        + frame
                            .batch
                            .iter()
                            .map(|name| name.len() * 4 + 4)
                            .sum::<usize>()
                })
                .sum();
            if !progress.roots.is_empty()
                && progress.deferred.len() < 8
                && deferred_bytes < 512 * 1024
            {
                progress
                    .stack
                    .push(Frame::new(progress.roots.pop_front().unwrap()));
            } else if let Some(stack) = progress.deferred.pop_front() {
                progress.stack = stack;
            } else {
                break;
            }
        }
        let frame = progress.stack.last_mut().unwrap();
        // Never follow a changed ancestor or walk into a repository from /tmp.
        let permitted = (|| -> io::Result<bool> {
            Ok(frame.path.canonicalize()? == frame.path
                && !checkout_marker(&frame.path)?
                && !databases::protected(&frame.path)?)
        })();
        if let Err(e) = &permitted
            && e.kind() != io::ErrorKind::NotFound
            && errors.len() < 8
        {
            errors.push(format!("{}: {e}", frame.path.display()));
        }
        if !permitted.unwrap_or(false) {
            progress.stack.pop();
            continue;
        }
        let next = frame.next();
        visited += 1;
        root_visits += 1;
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
                    // Fresh files cannot be deleted. Avoid opening every fresh
                    // build artifact just to inspect its database header.
                    if !m.is_dir() && !old_enough(&path, &m, at, clone_file) {
                        return Ok(());
                    }
                    if path.file_name().is_some_and(|n| n == ".git") || databases::protected(&path)?
                    {
                        protected += 1;
                        return Ok(());
                    }
                    if m.is_dir() {
                        if progress.stack.len() < 128 {
                            progress.stack.push(Frame::new(path.clone()));
                        }
                    } else if (m.is_file() || m.file_type().is_symlink())
                        && old_enough(&path, &m, at, clone_file)
                        && apply
                    {
                        fs::remove_file(&path)?;
                        removed += 1;
                        progress.stats.unlinked_bytes_this_cycle += m.len();
                    }
                    Ok(())
                })();
                if let Err(e) = result
                    && e.kind() != io::ErrorKind::NotFound
                    && errors.len() < 8
                {
                    errors.push(format!("{}: {e}", path.display()));
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
                let failed = progress.stack.pop().unwrap();
                if e.kind() != io::ErrorKind::NotFound && errors.len() < 8 {
                    errors.push(format!("{}: {e}", failed.path.display()));
                }
            }
        }
    }
    progress.stats.visited_this_cycle += visited as u64;
    progress.stats.visited_this_pass += visited as u64;
    progress.stats.removed_this_cycle += removed as u64;
    progress.stats.removed_this_pass += removed as u64;
    (removed, protected, errors)
}

pub(super) fn clean(
    config: &Config,
    progress: &mut Progress,
    apply: bool,
) -> io::Result<(Vec<Item>, usize)> {
    if !apply {
        return Ok((vec![Item { name: "24-hour cache expiration".into(), detail: "Enabled roots are inspected incrementally during cleanup; SQLite and sidecars are always retained".into(), eligible: true, worktree: None, error: None }], 0));
    }
    if progress.roots.is_empty()
        && progress.stack.is_empty()
        && progress.projects.is_empty()
        && progress.project_roots.is_empty()
        && progress.deferred.is_empty()
    {
        progress.roots = discover();
        progress.project_roots = config
            .workspace_roots
            .iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        progress.discovered_at = now();
        progress.stats.pass_started_at = now();
        progress.stats.visited_this_pass = 0;
        progress.stats.removed_this_pass = 0;
    }
    if progress.stats.pass_started_at == 0 {
        progress.stats.pass_started_at = progress.discovered_at;
    }
    progress.stats.visited_this_cycle = 0;
    progress.stats.removed_this_cycle = 0;
    progress.stats.unlinked_bytes_this_cycle = 0;
    let started = Instant::now();
    discover_projects(progress);
    let (removed, protected, errors) = advance(
        progress,
        now(),
        apply,
        Instant::now() + Duration::from_secs(25),
        100_000,
    );
    progress.stats.slice_millis = started.elapsed().as_millis() as u64;
    progress.stats.roots_pending =
        progress.roots.len() + progress.deferred.len() + usize::from(!progress.stack.is_empty());
    progress.stats.discovery_pending =
        !progress.projects.is_empty() || !progress.project_roots.is_empty();
    if progress.stats.roots_pending == 0 && !progress.stats.discovery_pending {
        progress.stats.last_completed_at = Some(now());
        progress.stats.last_pass_seconds =
            Some(now().saturating_sub(progress.stats.pass_started_at));
    }
    let detail = format!(
        "Deleted {removed} expired files; inspected {} entries in {} ms; preserved {protected} database paths; {} roots pending; {}; pass age {}s. {}",
        progress.stats.visited_this_cycle,
        progress.stats.slice_millis,
        progress.stats.roots_pending,
        if progress.stack.is_empty() {
            "between roots"
        } else {
            "cursor saved for next cycle"
        },
        now().saturating_sub(progress.stats.pass_started_at),
        errors.join("; ")
    );
    Ok((
        vec![Item {
            name: "24-hour cache expiration".into(),
            detail,
            eligible: true,
            worktree: None,
            error: (!errors.is_empty()).then(|| errors.join("; ")),
        }],
        removed,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_git_marker_does_not_disable_temp_cleanup_but_real_checkouts_stay_protected() {
        let root = std::env::temp_dir().join(format!("harvester-marker-{}", std::process::id()));
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("repo/.git")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join("expired"), b"disposable").unwrap();
        fs::write(root.join("repo/.git/HEAD"), b"ref: refs/heads/main").unwrap();
        fs::write(root.join("repo/source"), b"keep").unwrap();
        let mut p = Progress {
            roots: VecDeque::from([root.clone()]),
            ..Default::default()
        };
        advance(
            &mut p,
            now() + 90000,
            true,
            Instant::now() + Duration::from_secs(5),
            100,
        );
        assert!(!root.join("expired").exists());
        assert!(root.join("repo/source").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn large_root_does_not_starve_later_cleanup_and_rotated_cursors_resume() {
        let root = std::env::temp_dir().join(format!("harvester-fair-{}", std::process::id()));
        fs::create_dir_all(root.join("large")).unwrap();
        fs::create_dir_all(root.join("small")).unwrap();
        let root = root.canonicalize().unwrap();
        let at = now() + 90000;
        for n in 0..1000 {
            let path = root.join(format!("large/fresh-{n}"));
            fs::write(&path, b"fresh").unwrap();
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_secs(at))
                .unwrap();
        }
        let old = root.join("small/expired");
        fs::write(&old, b"old").unwrap();
        let mut p = Progress {
            roots: VecDeque::from([root.join("large"), root.join("small")]),
            ..Default::default()
        };
        advance(
            &mut p,
            at,
            true,
            Instant::now() + Duration::from_secs(10),
            512,
        );
        assert!(
            !old.exists(),
            "A large retained root must not block later disposable files"
        );
        assert_eq!(fs::read_dir(root.join("large")).unwrap().count(), 1000);
        assert!(!p.deferred.is_empty() || !p.stack.is_empty());
        p = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
        for _ in 0..10 {
            advance(
                &mut p,
                at,
                true,
                Instant::now() + Duration::from_secs(10),
                512,
            );
            p = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
        }
        assert!(p.deferred.is_empty() && p.stack.is_empty() && p.roots.is_empty());
        assert_eq!(fs::read_dir(root.join("large")).unwrap().count(), 1000);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn flat_directory_resumes_without_retained_entries_starving_later_files() {
        let root = std::env::temp_dir().join(format!("harvester-flat-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let expected: BTreeSet<_> = (0..4096)
            .map(|n| {
                let path = root.join(format!("entry-{n:05}"));
                fs::write(&path, b"retained").unwrap();
                path
            })
            .collect();
        let mut frame = Frame::new(root.clone());
        let mut seen = BTreeSet::new();
        while let Some(path) = frame.next().unwrap() {
            assert!(seen.insert(path));
            // A step must only read a bounded kernel chunk, never sort the directory.
            assert!(frame.batch.len() < 512);
            if seen.len() % 113 == 0 {
                frame = serde_json::from_slice(&serde_json::to_vec(&frame).unwrap()).unwrap();
            }
        }
        assert_eq!(seen, expected);
        fs::remove_dir_all(root).unwrap();
    }
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
