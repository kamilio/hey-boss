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
    #[serde(skip)]
    volume: Option<(u64, bool)>,
    #[serde(skip)]
    protected_directory: bool,
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
            volume: None,
            protected_directory: false,
            offset: 0,
            identity: None,
            batch: VecDeque::new(),
        }
    }
    fn volume(&mut self) -> io::Result<(u64, bool)> {
        if let Some(volume) = self.volume {
            return Ok(volume);
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&self.path)?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // The descriptor pins the directory while inspecting its volume.
        if unsafe { libc::fstatvfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let stat = unsafe { stat.assume_init() };
        let metadata = file.metadata()?;
        let volume = (metadata.dev(), stat.f_flag & libc::ST_RDONLY != 0);
        self.protected_directory =
            filesystem_protected(&metadata) || repository_metadata(&self.path)?;
        self.volume = Some(volume);
        Ok(volume)
    }
    fn next(&mut self) -> io::Result<Option<PathBuf>> {
        // Saved batches must obey the same filesystem protections as fresh reads.
        if self.volume()?.1 || self.protected_directory {
            return Ok(None);
        }
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
pub(super) fn filesystem_protected(metadata: &fs::Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        // Immutable and append-only flags prohibit unlinking even owned files.
        // Preserve these explicit protections; ordinary mode 0444 is different.
        std::os::macos::fs::MetadataExt::st_flags(metadata)
            & (libc::UF_IMMUTABLE | libc::SF_IMMUTABLE | libc::UF_APPEND | libc::SF_APPEND)
            != 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = metadata;
        false
    }
}
// Deserialized frames recheck the kernel once per slice. Validate ancestors too:
// a saved cursor may already be several levels below a newly mounted directory.
fn traversal_device(frames: &mut Vec<Frame>) -> io::Result<Option<u64>> {
    let mut device = None;
    for index in 0..frames.len() {
        match frames[index].volume() {
            Ok((current, readonly)) => {
                if readonly
                    || frames[index].protected_directory
                    || device.is_some_and(|root| root != current)
                {
                    frames.truncate(index);
                    return Ok(None);
                }
                device = Some(current);
            }
            Err(error) => {
                let error = io::Error::new(
                    error.kind(),
                    format!("{}: {error}", frames[index].path.display()),
                );
                frames.truncate(index);
                return Err(error);
            }
        }
    }
    Ok(device)
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

fn downloaded_source(path: &std::path::Path) -> bool {
    // Package managers assume extracted sources are complete. Nested build/target
    // folders can be source, including read-only Go modules under any GOPATH.
    path.ancestors().any(|dir| {
        matches!(
            (
                dir.file_name().and_then(|name| name.to_str()),
                dir.parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str()),
            ),
            (Some("src"), Some("registry")) | (Some("mod"), Some("pkg"))
        )
    })
}

fn project_cache(path: &std::path::Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some(
            "node_modules"
                | "out"
                | "dist"
                | "build"
                | "target"
                | ".cache"
                | ".turbo"
                | ".next"
                | ".vite"
        )
    )
}

fn discover_projects(p: &mut Progress) {
    discover_projects_until(p, Instant::now() + Duration::from_secs(3));
}

fn discover_projects_until(p: &mut Progress, deadline: Instant) {
    let mut checked_root = PathBuf::new();
    // Large source forests must use the time slice, not spend days waiting
    // between short 10,000-entry batches. The queue still bounds saved state.
    while Instant::now() < deadline && p.roots.len() < 4096 {
        if p.projects.is_empty() {
            let Some(root) = p.project_roots.pop_front() else {
                break;
            };
            p.projects.push(Frame::new(root));
        }
        let root = &p.projects[0].path;
        if *root != checked_root {
            // A saved cursor can start inside bare Git refs named build or out.
            match checkout_admin(root) {
                Ok(Some(admin)) if root.starts_with(&admin) => {
                    p.projects.clear();
                    continue;
                }
                Err(_) => {
                    p.projects.clear();
                    continue;
                }
                _ => checked_root = root.clone(),
            }
        }
        // Check before promoting legacy cache cursors into cleanup roots.
        let Ok(Some(device)) = traversal_device(&mut p.projects) else {
            continue;
        };
        if let Some(index) = p
            .projects
            .iter()
            .position(|frame| downloaded_source(&frame.path))
        {
            p.projects.truncate(index);
            continue;
        }
        // Older versions descended into project .cache directories without
        // cleaning them. Promote saved cursors too, including nested frames.
        if let Some(index) = p
            .projects
            .iter()
            .position(|frame| project_cache(&frame.path))
        {
            p.roots.push_back(p.projects[index].path.clone());
            p.projects.truncate(index);
            continue;
        }
        let frame = p.projects.last_mut().unwrap();
        if frame.path.canonicalize().ok().as_ref() != Some(&frame.path) {
            p.projects.pop();
            continue;
        }
        match frame.next() {
            Ok(Some(path)) => {
                if !fs::symlink_metadata(&path)
                    .is_ok_and(|m| m.is_dir() && m.dev() == device && !filesystem_protected(&m))
                {
                    continue;
                }
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if project_cache(&path) {
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

fn repository_metadata(path: &std::path::Path) -> io::Result<bool> {
    // Bare repositories have no enclosing .git marker. Use their layout, not a
    // directory suffix; cache expiration must never erase worktree registries.
    for (name, directory) in [("HEAD", false), ("objects", true), ("refs", true)] {
        match fs::symlink_metadata(path.join(name)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
            Ok(m) if m.is_dir() == directory || m.file_type().is_symlink() => {}
            Ok(_) => return Ok(false),
        }
    }
    Ok(true)
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

// Project cache roots and saved cursors start below .git. Resolve ownership once
// per root visit, then cheaply recheck the lock before each entry is unlinked.
fn checkout_admin(path: &std::path::Path) -> io::Result<Option<PathBuf>> {
    for parent in path.ancestors() {
        if repository_metadata(parent)? {
            return Ok(Some(parent.to_path_buf()));
        }
        let marker = parent.join(".git");
        match fs::symlink_metadata(&marker) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
            Ok(m) if m.is_dir() => return Ok(Some(marker)),
            Ok(m) if m.is_file() => {
                let value = fs::read_to_string(marker)?;
                let target = value
                    .trim()
                    .strip_prefix("gitdir: ")
                    .ok_or_else(|| io::Error::other("Unknown checkout ownership; preserved"))?;
                let admin = parent.join(target);
                if !fs::metadata(&admin)?.is_dir() {
                    return Err(io::Error::other("Missing checkout ownership; preserved"));
                }
                return Ok(Some(admin));
            }
            Ok(_) => return Err(io::Error::other("Symlinked checkout ownership; preserved")),
        }
    }
    Ok(None)
}

fn advance(
    progress: &mut Progress,
    at: u64,
    apply: bool,
    deadline: Instant,
    maximum: usize,
) -> (usize, usize, Vec<String>) {
    match super::workload_ownership::declared_roots() {
        Ok(owned) => advance_with_owners(progress, at, apply, deadline, maximum, &owned),
        Err(error) => (
            0,
            0,
            vec![format!(
                "Cannot verify declared issue ownership; caches preserved: {error}"
            )],
        ),
    }
}

fn advance_with_owners(
    progress: &mut Progress,
    at: u64,
    apply: bool,
    deadline: Instant,
    maximum: usize,
    owned: &BTreeSet<PathBuf>,
) -> (usize, usize, Vec<String>) {
    let mut removed = 0;
    let mut protected = 0;
    let mut errors = Vec::new();
    let mut visited = 0;
    let mut root_visits = 0;
    let mut owner_root = PathBuf::new();
    let mut owner_admin = None;
    let mut owner_declared = false;
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
        let root = &progress.stack[0].path;
        let ownership = (|| -> io::Result<bool> {
            if *root != owner_root {
                owner_declared = owned.iter().any(|path| root.starts_with(path));
                owner_admin = if owner_declared {
                    None
                } else {
                    checkout_admin(root)?
                };
                owner_root = root.clone();
            }
            if owner_declared {
                return Ok(true);
            }
            if let Some(admin) = &owner_admin {
                if root.starts_with(admin) {
                    return Ok(true);
                }
                match fs::symlink_metadata(admin.join("locked")) {
                    Ok(_) => return Ok(true),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(false)
        })();
        match ownership {
            Ok(false) => {}
            other => {
                if let Err(e) = other {
                    if errors.len() < 8 {
                        errors.push(format!("{}: {e}", root.display()));
                    }
                } else {
                    protected += 1;
                }
                progress.stack.clear();
                continue;
            }
        }
        let device = match traversal_device(&mut progress.stack) {
            Ok(Some(device)) => device,
            Ok(None) => continue,
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound && errors.len() < 8 {
                    errors.push(error.to_string());
                }
                continue;
            }
        };
        let frame = progress.stack.last_mut().unwrap();
        // Never follow a changed ancestor or walk into a repository from /tmp.
        let permitted = (|| -> io::Result<bool> {
            Ok(!downloaded_source(&frame.path)
                && frame.path.canonicalize()? == frame.path
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
                    if m.dev() != device {
                        return Ok(());
                    }
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
                    if filesystem_protected(&m)
                        || path.file_name().is_some_and(|n| n == ".git")
                        || downloaded_source(&path)
                        || databases::protected(&path)?
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
        "Deleted {removed} expired files; inspected {} entries in {} ms; preserved {protected} database paths, owned worktrees or flagged files; {} roots pending; {}; pass age {}s. {}",
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

    fn bare_git_expiration(resumed: bool, discovery: bool) {
        let root = std::env::temp_dir().join(format!(
            "harvester-bare-{resumed}-{discovery}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(&root)
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "seed"]);
        git(&["-C", "seed", "commit", "--allow-empty", "-m", "fixture"]);
        git(&["clone", "--bare", "seed", "repository.git"]);
        let work = root.join("held");
        git(&[
            "-C",
            "repository.git",
            "worktree",
            "add",
            "--detach",
            work.to_str().unwrap(),
            "HEAD",
        ]);
        git(&[
            "-C",
            "repository.git",
            "worktree",
            "lock",
            work.to_str().unwrap(),
        ]);
        let registry = root.join("repository.git/worktrees/held");
        assert!(registry.join("locked").exists());
        fs::write(root.join("expired"), "disposable").unwrap();
        let mut p = Progress {
            roots: VecDeque::from([root.clone()]),
            ..Progress::default()
        };
        if discovery {
            git(&["-C", "repository.git", "branch", "build/held", "HEAD"]);
            p.roots.clear();
            p.projects
                .push(Frame::new(root.join("repository.git/refs/heads/build")));
            discover_projects_until(&mut p, Instant::now() + Duration::from_secs(5));
            fs::remove_dir_all(root).unwrap();
            assert!(
                p.roots.is_empty(),
                "Git branch refs must not become disposable build caches"
            );
            return;
        }
        if resumed {
            let mut saved = Frame::new(registry.clone());
            saved.refill().unwrap();
            p.stack.push(saved);
            p = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
        }
        let (_, _, errors) = advance_with_owners(
            &mut p,
            now() + 172800,
            true,
            Instant::now() + Duration::from_secs(5),
            1000,
            &BTreeSet::new(),
        );
        let retained = registry.join("locked").exists()
            && registry.join("gitdir").exists()
            && root.join("repository.git/HEAD").exists();
        let expired = !root.join("expired").exists();
        fs::remove_dir_all(root).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert!(retained, "Git metadata was expired (resumed={resumed})");
        assert!(expired, "ordinary temporary files should still expire");
    }

    #[test]
    fn temp_expiration_preserves_bare_git_metadata() {
        bare_git_expiration(false, false);
    }

    #[test]
    fn temp_expiration_preserves_saved_worktree_registry() {
        bare_git_expiration(true, false);
    }

    #[test]
    fn bare_git_refs_are_not_promoted_from_saved_cache_discovery() {
        bare_git_expiration(false, true);
    }

    #[cfg(target_os = "macos")]
    struct FileFlags(PathBuf);
    #[cfg(target_os = "macos")]
    impl FileFlags {
        fn set(path: PathBuf, flags: u32) -> Self {
            let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::chflags(name.as_ptr(), flags) }, 0);
            Self(path)
        }
    }
    #[cfg(target_os = "macos")]
    impl Drop for FileFlags {
        fn drop(&mut self) {
            let name = std::ffi::CString::new(self.0.as_os_str().as_bytes()).unwrap();
            unsafe { libc::chflags(name.as_ptr(), 0) };
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn filesystem_flags_preserve_files_and_saved_directory_batches() {
        use std::os::unix::fs::PermissionsExt;
        for flags in [libc::UF_IMMUTABLE, libc::UF_APPEND] {
            let root = std::env::temp_dir()
                .join(format!("harvester-flags-{flags}-{}", std::process::id()));
            fs::create_dir_all(root.join("retained")).unwrap();
            let root = root.canonicalize().unwrap();
            fs::write(root.join("receipt"), "retained file").unwrap();
            fs::write(root.join("retained/receipt"), "retained directory").unwrap();
            fs::write(root.join("expired"), "ordinary read-only file").unwrap();
            fs::set_permissions(root.join("expired"), fs::Permissions::from_mode(0o444)).unwrap();
            let mut saved = Frame::new(root.join("retained"));
            saved.refill().unwrap();
            let guards = [
                FileFlags::set(root.join("receipt"), flags),
                FileFlags::set(root.join("retained"), flags),
            ];
            let mut p = Progress {
                stack: vec![saved],
                roots: VecDeque::from([root.clone()]),
                ..Progress::default()
            };
            let (removed, _, errors) = advance_with_owners(
                &mut p,
                now() + 172800,
                true,
                Instant::now() + Duration::from_secs(5),
                100,
                &BTreeSet::new(),
            );
            let retained = root.join("receipt").exists() && root.join("retained/receipt").exists();
            let expired = !root.join("expired").exists();
            drop(guards);
            fs::remove_dir_all(root).unwrap();
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(removed, 1);
            assert!(retained && expired);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn filesystem_flags_stop_saved_cache_discovery() {
        let root =
            std::env::temp_dir().join(format!("harvester-flags-discovery-{}", std::process::id()));
        fs::create_dir_all(root.join("out")).unwrap();
        let root = root.canonicalize().unwrap();
        let guard = FileFlags::set(root.join("out"), libc::UF_IMMUTABLE);
        let mut p = Progress {
            projects: vec![Frame::new(root.join("out"))],
            ..Progress::default()
        };
        discover_projects_until(&mut p, Instant::now() + Duration::from_secs(5));
        drop(guard);
        fs::remove_dir_all(root).unwrap();
        assert!(
            p.roots.is_empty(),
            "flagged saved caches must not be promoted"
        );
    }

    #[test]
    fn mount_boundaries_revalidate_saved_volume_and_reject_symlinks() {
        let root =
            std::env::temp_dir().join(format!("harvester-volume-stat-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join("keep"), "saved batch").unwrap();
        let mut frame = Frame::new(root.clone());
        frame.refill().unwrap();
        let device = fs::metadata(&root).unwrap().dev();
        assert_eq!(frame.volume().unwrap(), (device, false));
        frame.volume = Some((device + 1, true));
        assert!(frame.next().unwrap().is_none());
        assert!(!frame.batch.is_empty());
        let saved = serde_json::to_string(&frame).unwrap();
        let mut restored: Frame = serde_json::from_str(&saved).unwrap();
        assert!(restored.volume.is_none());
        assert_eq!(restored.volume().unwrap(), (device, false));
        assert_eq!(restored.next().unwrap(), Some(root.join("keep")));
        std::os::unix::fs::symlink(&root, root.join("link")).unwrap();
        assert!(Frame::new(root.join("link")).volume().is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mount_boundaries_preserve_readonly_roots_and_resumed_mounts() {
        for readonly in [true, false] {
            let root = std::env::temp_dir()
                .join(format!("harvester-mount-{readonly}-{}", std::process::id()));
            fs::create_dir_all(root.join("mounted")).unwrap();
            let root = root.canonicalize().unwrap();
            fs::write(root.join("mounted/keep"), "mounted data").unwrap();
            fs::write(root.join("expired"), "disposable").unwrap();
            let device = fs::metadata(&root).unwrap().dev();
            let mut parent = Frame::new(root.clone());
            parent.refill().unwrap();
            parent.batch.retain(|name| name != b"mounted");
            parent.volume = Some((device, false));
            let mut mounted = Frame::new(root.join("mounted"));
            // Simulate the kernel's volume metadata, without mounting a real disk.
            mounted.volume = Some((if readonly { device } else { device + 1 }, readonly));
            let mut p = Progress::default();
            if readonly {
                p.stack.push(mounted);
                p.deferred.push_back(vec![parent]);
            } else {
                p.stack = vec![parent, mounted];
            }
            let (removed, _, errors) = advance_with_owners(
                &mut p,
                now() + 172800,
                true,
                Instant::now() + Duration::from_secs(5),
                100,
                &BTreeSet::new(),
            );
            assert!(errors.is_empty());
            assert_eq!(removed, 1, "only the unmounted sibling may expire");
            assert_eq!(
                fs::read_to_string(root.join("mounted/keep")).unwrap(),
                "mounted data"
            );
            assert!(!root.join("expired").exists());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn mount_boundaries_stop_saved_discovery_before_cache_promotion() {
        let root =
            std::env::temp_dir().join(format!("harvester-mount-discovery-{}", std::process::id()));
        fs::create_dir_all(root.join("mounted/out")).unwrap();
        fs::create_dir_all(root.join("normal/out")).unwrap();
        let root = root.canonicalize().unwrap();
        let device = fs::metadata(&root).unwrap().dev();
        for readonly in [false, true] {
            let mut parent = Frame::new(root.clone());
            parent.refill().unwrap();
            parent.batch.retain(|name| name != b"mounted");
            parent.volume = Some((device, false));
            let mut mounted = Frame::new(root.join("mounted/out"));
            mounted.volume = Some((if readonly { device } else { device + 1 }, readonly));
            let mut p = Progress {
                projects: vec![parent, mounted],
                ..Progress::default()
            };
            discover_projects_until(&mut p, Instant::now() + Duration::from_secs(5));
            assert_eq!(p.roots, VecDeque::from([root.join("normal/out")]));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resumed_cache_sweep_preserves_declared_issue_work_and_expires_unowned_output() {
        let root =
            std::env::temp_dir().join(format!("harvester-declared-cache-{}", std::process::id()));
        let work = root.join("work");
        let cache = work.join("out");
        let other = root.join("unowned-out");
        fs::create_dir_all(work.join(".git")).unwrap();
        fs::create_dir_all(&cache).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(cache.join("receipt"), "active validation").unwrap();
        fs::write(other.join("expired"), "disposable").unwrap();
        let mut p = Progress::default();
        p.stack.push(Frame::new(cache.canonicalize().unwrap()));
        p.roots.push_back(other.canonicalize().unwrap());
        let owned = BTreeSet::from([work.canonicalize().unwrap()]);
        let (removed, preserved, errors) = advance_with_owners(
            &mut p,
            now() + 172800,
            true,
            Instant::now() + Duration::from_secs(5),
            100,
            &owned,
        );
        assert!(errors.is_empty());
        assert_eq!(removed, 1);
        assert_eq!(preserved, 1);
        assert_eq!(
            fs::read_to_string(cache.join("receipt")).unwrap(),
            "active validation"
        );
        assert!(!other.join("expired").exists());
        p.stack.push(Frame::new(cache.canonicalize().unwrap()));
        assert_eq!(
            advance_with_owners(
                &mut p,
                now() + 172800,
                true,
                Instant::now() + Duration::from_secs(5),
                100,
                &BTreeSet::new()
            )
            .0,
            1
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn resumed_cache_sweep_preserves_locked_worktree_receipts() {
        let root =
            std::env::temp_dir().join(format!("harvester-owned-cache-{}", std::process::id()));
        let cache = root.join("work/out");
        fs::create_dir_all(&cache).unwrap();
        fs::create_dir_all(root.join("admin")).unwrap();
        fs::write(
            root.join("work/.git"),
            format!("gitdir: {}\n", root.join("admin").display()),
        )
        .unwrap();
        fs::write(root.join("admin/locked"), "issue 147; queued owner").unwrap();
        fs::write(cache.join("receipt"), "validation pending").unwrap();
        let mut p = Progress::default();
        p.stack.push(Frame::new(cache.canonicalize().unwrap()));
        let at = now() + 172800;
        let (removed, _, errors) = advance(
            &mut p,
            at,
            true,
            Instant::now() + Duration::from_secs(5),
            100,
        );
        assert!(errors.is_empty());
        assert_eq!(
            removed, 0,
            "a saved cache cursor must honor its checkout owner"
        );
        assert_eq!(
            fs::read_to_string(cache.join("receipt")).unwrap(),
            "validation pending"
        );
        // Completion is explicit; a later sweep may reclaim this fixture's output.
        fs::remove_file(root.join("admin/locked")).unwrap();
        p.stack.push(Frame::new(cache.canonicalize().unwrap()));
        assert_eq!(
            advance(
                &mut p,
                at,
                true,
                Instant::now() + Duration::from_secs(5),
                100
            )
            .0,
            1
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn discovery_uses_available_time_and_resumes_large_source_trees() {
        let root =
            std::env::temp_dir().join(format!("harvester-large-discovery-{}", std::process::id()));
        fs::create_dir_all(root.join("project/src")).unwrap();
        fs::create_dir_all(root.join("project/.cache")).unwrap();
        let root = root.canonicalize().unwrap();
        for index in 0..12_000 {
            fs::write(root.join(format!("project/src/{index}.ts")), b"source").unwrap();
        }
        let mut p = Progress {
            project_roots: VecDeque::from([root.clone()]),
            ..Default::default()
        };
        discover_projects_until(&mut p, Instant::now());
        assert_eq!(p.project_roots, VecDeque::from([root.clone()]));
        p = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
        discover_projects_until(&mut p, Instant::now() + Duration::from_secs(30));
        let complete = p.projects.is_empty() && p.project_roots.is_empty();
        let scheduled = p.roots.contains(&root.join("project/.cache"));
        let sources = fs::read_dir(root.join("project/src")).unwrap().count();
        fs::remove_dir_all(root).unwrap();
        assert!(
            complete,
            "Unused discovery time must reach caches beyond large source trees"
        );
        assert!(scheduled);
        assert_eq!(sources, 12_000);
    }

    #[test]
    fn project_local_cache_expires_old_files_but_keeps_sqlite_fresh_files_and_checkouts() {
        let root =
            std::env::temp_dir().join(format!("harvester-project-cache-{}", std::process::id()));
        fs::create_dir_all(root.join("project/.cache/checkout/.git")).unwrap();
        let root = root.canonicalize().unwrap();
        let cache = root.join("project/.cache");
        fs::write(cache.join("expired"), b"old cache").unwrap();
        fs::write(cache.join("fresh"), b"fresh cache").unwrap();
        fs::write(cache.join("checkout/.git/HEAD"), b"ref: refs/heads/main").unwrap();
        fs::write(cache.join("checkout/source"), b"preserved checkout").unwrap();
        let db = rusqlite::Connection::open(cache.join("History")).unwrap();
        db.execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE value(n); INSERT INTO value VALUES (42);",
        )
        .unwrap();
        let at = now() + 90000;
        fs::File::options()
            .write(true)
            .open(cache.join("fresh"))
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(at))
            .unwrap();
        let mut p = Progress {
            project_roots: VecDeque::from([root.clone()]),
            ..Default::default()
        };
        discover_projects(&mut p);
        assert!(
            p.roots.contains(&cache),
            "Project-local caches must be scheduled for expiration"
        );
        advance(
            &mut p,
            at,
            true,
            Instant::now() + Duration::from_secs(5),
            100,
        );
        assert!(!cache.join("expired").exists());
        assert!(cache.join("fresh").exists());
        assert!(cache.join("checkout/source").exists());
        assert!(cache.join("History-wal").exists());
        assert_eq!(
            db.query_row("SELECT n FROM value", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            42
        );
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn downloaded_sources_survive_discovery_and_saved_sweeps() {
        for (index, (base, first, second)) in [
            (
                "registry/src/index/crate-1.0.0",
                "src/target/apple.rs",
                "build/build.rs",
            ),
            (
                "custom-gopath/pkg/mod/example.com/pkg@v1.0.0",
                "target/source.go",
                "build/appveyor/check.bat",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let root = std::env::temp_dir().join(format!(
                "harvester-downloaded-sources-{}-{index}",
                std::process::id()
            ));
            let sources = [
                PathBuf::from(base).join(first),
                PathBuf::from(base).join(second),
            ];
            for path in sources.iter().cloned().chain([
                PathBuf::from("project/target/output"),
                PathBuf::from("project/.cache/go-build/output"),
            ]) {
                let file = root.join(path);
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(file, b"fixture").unwrap();
            }
            let root = root.canonicalize().unwrap();
            let at = now() + 90000;
            let mut p = Progress {
                project_roots: VecDeque::from([root.clone()]),
                ..Default::default()
            };
            discover_projects(&mut p);
            advance(
                &mut p,
                at,
                true,
                Instant::now() + Duration::from_secs(5),
                100,
            );
            assert!(!root.join("project/target/output").exists());
            assert!(!root.join("project/.cache/go-build/output").exists());
            for path in &sources {
                assert_eq!(
                    fs::read(root.join(path)).unwrap_or_default(),
                    b"fixture",
                    "Downloaded source was removed: {}",
                    path.display()
                );
            }
            let target = root.join(&sources[0]).parent().unwrap().to_owned();
            let build = root.join(&sources[1]).parent().unwrap().to_owned();
            let mut saved = Frame::new(target.clone());
            saved.refill().unwrap();
            let mut p = Progress {
                roots: VecDeque::from([target]),
                stack: vec![saved],
                deferred: VecDeque::from([vec![Frame::new(build.clone())]]),
                projects: vec![Frame::new(build)],
                ..Default::default()
            };
            p = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
            discover_projects(&mut p);
            advance(
                &mut p,
                at,
                true,
                Instant::now() + Duration::from_secs(5),
                100,
            );
            for path in &sources {
                assert_eq!(
                    fs::read(root.join(path)).unwrap_or_default(),
                    b"fixture",
                    "Saved cursor removed source: {}",
                    path.display()
                );
            }
            assert!(
                p.roots.is_empty()
                    && p.stack.is_empty()
                    && p.deferred.is_empty()
                    && p.projects.is_empty()
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn saved_discovery_inside_project_cache_promotes_the_whole_cache() {
        let root =
            std::env::temp_dir().join(format!("harvester-cache-cursor-{}", std::process::id()));
        fs::create_dir_all(root.join(".cache/nested")).unwrap();
        let root = root.canonicalize().unwrap();
        let cache = root.join(".cache");
        // A prior binary has already walked past this entry; migration must restart cleanup at the cache root.
        fs::write(cache.join("expired-before-cursor"), b"old cache").unwrap();
        let mut p = Progress {
            projects: vec![
                Frame::new(root.clone()),
                Frame::new(cache.clone()),
                Frame::new(cache.join("nested")),
            ],
            ..Default::default()
        };
        let _ = p.projects[0].next().unwrap();
        let _ = p.projects[1].next().unwrap();
        p = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
        discover_projects(&mut p);
        assert_eq!(p.roots, VecDeque::from([cache.clone()]));
        assert!(p.projects.is_empty());
        advance(
            &mut p,
            now() + 90000,
            true,
            Instant::now() + Duration::from_secs(5),
            100,
        );
        assert!(!cache.join("expired-before-cursor").exists());
        fs::remove_dir_all(root).unwrap();
    }

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
