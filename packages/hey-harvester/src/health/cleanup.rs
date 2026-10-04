//! A candidate list is never authorization: release and ownership are read again at removal.
use super::{preserved, worktrees};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize, Deserialize)]
struct Release {
    version: u32,
    path: PathBuf,
    owner: String,
    at: u64,
    fingerprint: String,
}

fn git_admin(root: &Path) -> io::Result<PathBuf> {
    // Inspect this marker explicitly: discovery can skip a corrupt .git directory
    // and silently return a different enclosing repository.
    worktrees::git_text(root, &["--git-dir=.git", "rev-parse", "--absolute-git-dir"])
        .map(PathBuf::from)
}

fn checkout(path: &Path) -> io::Result<(PathBuf, PathBuf)> {
    if !path.is_absolute() || path.canonicalize()? != path || !fs::symlink_metadata(path)?.is_dir()
    {
        return Err(preserved("Noncanonical cleanup target; preserved"));
    }
    let mut root = None;
    for parent in path.ancestors() {
        match fs::symlink_metadata(parent.join(".git")) {
            Ok(_) => {
                root = Some(parent);
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let root = root
        .ok_or_else(|| preserved("Unknown checkout ownership; preserved"))?
        .to_path_buf();
    if path != root && path.file_name().is_none_or(|n| n != "node_modules") {
        return Err(preserved(
            "Only a checkout or its node_modules may be released; preserved",
        ));
    }
    let admin = git_admin(&root)?;
    if admin.canonicalize()? != admin {
        return Err(preserved("Symlinked Git administration; preserved"));
    }
    Ok((root, admin))
}

fn release_path(path: &Path, admin: &Path) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    admin.join(format!(
        "cleanup-release-{:x}.json",
        Sha256::digest(path.as_os_str().as_bytes())
    ))
}

fn fingerprint(path: &Path, root: &Path, admin: &Path) -> io::Result<String> {
    let mut hash = Sha256::new();
    hash.update(worktrees::git_text(root, &["rev-parse", "HEAD"])?);
    for p in [
        path.to_path_buf(),
        root.to_path_buf(),
        admin.join("HEAD"),
        admin.join("index"),
    ] {
        let m = fs::symlink_metadata(p)?;
        hash.update(format!(
            "{}:{}:{}:{}:{}:{}",
            m.dev(),
            m.ino(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec()
        ));
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify(path: &Path, active: &[PathBuf], released: bool) -> io::Result<()> {
    let (root, admin) = checkout(path)?;
    // Match each declared session's path, never collapse sessions by application PID.
    for owned in active {
        let owned = owned.canonicalize().unwrap_or_else(|_| owned.clone());
        if owned.starts_with(&root) || owned.starts_with(&admin) {
            return Err(preserved(format!(
                "Active, queued or retained owner at {}; preserved",
                owned.display()
            )));
        }
    }
    // Parent checkout locks also protect nested dependencies/checkouts.
    for parent in root.ancestors() {
        let marker = parent.join(".git");
        let metadata = match fs::symlink_metadata(&marker) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        // Only an ordinary, provably empty ancestor directory is inert. Target
        // markers and symlinks still require valid Git metadata; inspection errors
        // and partial metadata fail closed.
        if parent != root
            && metadata.is_dir()
            && fs::read_dir(&marker)?.next().transpose()?.is_none()
        {
            continue;
        }
        let git = git_admin(parent)?;
        match fs::read_to_string(git.join("locked")) {
            Ok(reason) => {
                return Err(preserved(format!(
                    "Locked worktree; preserved — {}",
                    reason.trim()
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    if released {
        let mut file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(release_path(path, &admin))
            .map_err(|e| {
                preserved(format!(
                    "No readable explicit cleanup release; preserved ({e})"
                ))
            })?;
        let mut bytes = Vec::new();
        if !file.metadata()?.is_file() {
            return Err(preserved("Non-file cleanup release; preserved"));
        }
        Read::by_ref(&mut file).take(8193).read_to_end(&mut bytes)?;
        if bytes.len() > 8192 {
            return Err(preserved("Cleanup release exceeds limit; preserved"));
        }
        let release: Release = serde_json::from_slice(&bytes)
            .map_err(|e| preserved(format!("Invalid cleanup release; preserved ({e})")))?;
        if release.version != 1
            || release.path != path
            || release.owner.trim().is_empty()
            || release.fingerprint != fingerprint(path, &root, &admin)?
        {
            return Err(preserved(
                "Checkout changed since explicit cleanup release; preserved",
            ));
        }
    }
    Ok(())
}

/// A release is a recorded owner attestation, not a force option or a lease expiry.
fn release(path: &Path, owner: &str, active: &[PathBuf]) -> io::Result<()> {
    if owner.trim().is_empty() || owner.len() > 256 || owner.chars().any(char::is_control) {
        return Err(io::Error::other(
            "Supply the releasing owner's session identity",
        ));
    }
    verify(path, active, false)?;
    let (root, admin) = checkout(path)?;
    let record = Release {
        version: 1,
        path: path.into(),
        owner: owner.into(),
        at: super::now(),
        fingerprint: fingerprint(path, &root, &admin)?,
    };
    let bytes = serde_json::to_vec(&record)?;
    // Atomic create prevents replacing a prior owner's release or following a symlink.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(release_path(path, &admin))?;
    file.write_all(&bytes)?;
    file.sync_all()
}

pub(super) fn check(path: &Path) -> io::Result<()> {
    // Cheap release/lock gate first; protected legacy candidates need no global process scan.
    verify(path, &[], true)?;
    declared_owner_guard(path)?;
    let active = current_paths(path)?;
    verify(path, &active, true)
}

// Cleanup needs ownership, open files and command paths, not full conversation
// histories. Keep the destructive preflight bounded by the existing inspectors.
fn current_paths(path: &Path) -> io::Result<Vec<PathBuf>> {
    let (root, _) = checkout(path)?;
    let table = super::processes::inventory()?;
    let mut paths = worktrees::open_paths()?;
    paths.extend(super::workload_ownership::declared_roots()?);
    let name = root.to_string_lossy();
    if table
        .values()
        .filter(|p| p.pid != std::process::id())
        .any(|p| {
            p.arguments.contains(&format!("{name}/"))
                || p.arguments.split_whitespace().any(|s| s == name)
        })
    {
        return Err(preserved(
            "Checkout referenced by a running process; preserved",
        ));
    }
    Ok(paths)
}

fn declared_owner_guard(path: &Path) -> io::Result<()> {
    let (root, _) = checkout(path)?;
    for owned in super::workload_ownership::declared_roots()? {
        if root.starts_with(&owned) || owned.starts_with(&root) {
            return Err(preserved(format!(
                "Declared active, queued or retained owner at {}; preserved",
                owned.display()
            )));
        }
    }
    Ok(())
}

pub(super) fn release_status(path: &Path) -> io::Result<()> {
    verify(path, &[], true)
}

#[cfg(test)]
pub(super) fn release_fixture(path: &Path) {
    release(path, "fixture-owner", &[]).unwrap();
}

pub(super) fn consume(path: &Path) -> io::Result<()> {
    check(path)?;
    let (_, admin) = checkout(path)?;
    fs::remove_file(release_path(path, &admin))
}

fn remove_dependencies(path: &Path) -> io::Result<()> {
    if path.file_name().is_none_or(|n| n != "node_modules") {
        return Err(preserved(
            "Select the exact node_modules directory; preserved",
        ));
    }
    check(path)?;
    // Refuse the entire candidate before changing anything if it contains local
    // databases, mounted content, filesystem protections, or repository metadata.
    inspect_tree(path)?;
    consume(path)?; // fresh ownership, identity and release at the destructive boundary
    fs::remove_dir_all(path)
}

fn inspect_tree(path: &Path) -> io::Result<()> {
    let device = fs::symlink_metadata(path)?.dev();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut pending = vec![path.to_path_buf()];
    while let Some(p) = pending.pop() {
        if pending.len() > 100000 || std::time::Instant::now() >= deadline {
            return Err(preserved("Dependency inspection limit; preserved"));
        }
        let m = fs::symlink_metadata(&p)?;
        if m.dev() != device
            || super::sweep::filesystem_protected(&m)
            || super::databases::protected(&p)?
            || p.file_name().is_some_and(|n| n == ".git")
        {
            return Err(preserved(format!(
                "Protected dependency content at {}; preserved",
                p.display()
            )));
        }
        if m.is_dir() {
            for child in fs::read_dir(p)? {
                pending.push(child?.path());
                if pending.len() > 100000 {
                    return Err(preserved("Dependency inspection limit; preserved"));
                }
            }
        } else if !m.is_file() && !m.file_type().is_symlink() {
            return Err(preserved("Special dependency file; preserved"));
        }
    }
    Ok(())
}

#[derive(Serialize)]
pub struct Receipt {
    pub version: u32,
    pub at: u64,
    pub path: PathBuf,
    pub action: String,
    pub decision: String,
    pub reason: String,
}

fn record_receipt(snapshot: &mut super::Snapshot, receipt: &Receipt) -> io::Result<()> {
    let mut discard = snapshot
        .activity
        .iter()
        .filter(|e| e.category == "cleanup-receipt")
        .count()
        .saturating_sub(99);
    snapshot.activity.retain(|e| {
        if e.category == "cleanup-receipt" && discard > 0 {
            discard -= 1;
            false
        } else {
            true
        }
    });
    snapshot.record("cleanup-receipt", "");
    // General activity text is truncated for display. Receipts must remain valid
    // JSON with the exact target, including long paths; bound by receipt count.
    snapshot.activity.last_mut().unwrap().message = serde_json::to_string(receipt)?;
    Ok(())
}

pub fn run(
    store: &super::Store,
    path: &Path,
    action: &str,
    owner: Option<&str>,
) -> io::Result<Receipt> {
    let _lock = store.lock()?;
    let mut state = store.state()?;
    state
        .snapshot
        .record("cleanup-request", format!("{action}: {}", path.display()));
    store.save("state.json", &state)?; // no destructive operation without its request receipt
    let result = match action {
        "check" => check(path),
        "release" => declared_owner_guard(path)
            .and_then(|()| current_paths(path))
            .and_then(|active| release(path, owner.unwrap_or(""), &active)),
        "retain" => checkout(path).and_then(|(_, admin)| {
            match fs::remove_file(release_path(path, &admin)) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            }
        }),
        "remove-dependencies" => remove_dependencies(path),
        _ => Err(io::Error::other("Unknown cleanup action")),
    };
    let receipt = Receipt {
        version: 1,
        at: super::now(),
        path: path.into(),
        action: action.into(),
        decision: if result.is_ok() {
            "accepted"
        } else {
            "rejected"
        }
        .into(),
        reason: match result {
            Ok(()) => match action {
                "check" => {
                    "Explicit release and current ownership checks passed; removal will recheck"
                        .into()
                }
                "release" => format!(
                    "Explicit cleanup release recorded by {}",
                    owner.unwrap_or("")
                ),
                "retain" => "Cleanup release withdrawn; target retained".into(),
                _ => "Released dependencies removed".into(),
            },
            Err(e) => e.to_string(),
        },
    };
    record_receipt(&mut state.snapshot, &receipt)?;
    store.save("state.json", &state)?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "cleanup-guard-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            let path = path.canonicalize().unwrap();
            for args in [
                vec!["init"],
                vec![
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "fixture",
                ],
            ] {
                worktrees::git_text(&path, &args).unwrap();
            }
            // Git does not create an index for an empty commit.
            worktrees::git_text(&path, &["read-tree", "HEAD"]).unwrap();
            fs::create_dir_all(path.join("node_modules/.bin")).unwrap();
            fs::write(path.join("node_modules/.bin/tsc"), "fixture").unwrap();
            Self(path)
        }

        fn worktree_under(&self, ancestor: &Path) -> PathBuf {
            fs::create_dir_all(ancestor.join(".git")).unwrap();
            let root = ancestor.join("checkout");
            worktrees::git_text(
                &self.0,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    root.to_str().unwrap(),
                    "HEAD",
                ],
            )
            .unwrap();
            fs::create_dir(root.join("node_modules")).unwrap();
            root
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn long_rejection_receipts_remain_parseable_and_bounded() {
        let mut snapshot = super::super::Snapshot::default();
        let receipt = Receipt {
            version: 1,
            at: 1,
            path: PathBuf::from(format!("/{}", "long/".repeat(500))),
            action: "check".into(),
            decision: "rejected".into(),
            reason: "Unknown ownership; preserved".into(),
        };
        for _ in 0..150 {
            record_receipt(&mut snapshot, &receipt).unwrap();
        }
        assert_eq!(snapshot.activity.len(), 100);
        for event in snapshot.activity {
            let json: serde_json::Value = serde_json::from_str(&event.message).unwrap();
            assert_eq!(json["path"], receipt.path.to_str().unwrap());
            assert_eq!(json["decision"], "rejected");
        }
    }

    #[test]
    fn unknown_ownership_is_not_permission_to_remove_old_dependencies() {
        let f = Fixture::new();
        let path = f.0.join("node_modules");
        let file = fs::File::options()
            .write(true)
            .open(path.join(".bin/tsc"))
            .unwrap();
        file.set_modified(std::time::UNIX_EPOCH).unwrap();
        assert!(
            verify(&path, &[], true)
                .unwrap_err()
                .to_string()
                .contains("release")
        );
        assert!(path.join(".bin/tsc").exists());
    }

    #[test]
    fn empty_ancestor_marker_reaches_release_gate_for_registered_worktree() {
        let f = Fixture::new();
        let ancestor = Fixture::new();
        fs::remove_dir_all(ancestor.0.join(".git")).unwrap();
        let root = f.worktree_under(&ancestor.0);
        for path in [&root, &root.join("node_modules")] {
            assert!(
                verify(path, &[], true)
                    .unwrap_err()
                    .to_string()
                    .contains("No readable explicit cleanup release")
            );
            release(path, "finished-session", &[]).unwrap();
            verify(path, &[], true).unwrap();
            assert!(
                verify(path, std::slice::from_ref(&root), true)
                    .unwrap_err()
                    .to_string()
                    .contains("retained owner")
            );
        }
        assert!(
            fs::read_dir(ancestor.0.join(".git"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn empty_ancestor_does_not_hide_real_parent_lock_or_later_metadata_changes() {
        let f = Fixture::new();
        let parent = f.worktree_under(&f.0.join("parent"));
        let ancestor = parent.join("ancestor");
        let root = f.worktree_under(&ancestor);
        let path = root.join("node_modules");
        release(&path, "finished-session", &[]).unwrap();
        verify(&path, &[], true).unwrap();
        worktrees::git_text(
            &f.0,
            &[
                "worktree",
                "lock",
                parent.to_str().unwrap(),
                "--reason",
                "parent validation",
            ],
        )
        .unwrap();
        assert!(
            verify(&path, &[], true)
                .unwrap_err()
                .to_string()
                .contains("parent validation")
        );
        worktrees::git_text(&f.0, &["worktree", "unlock", parent.to_str().unwrap()]).unwrap();
        fs::write(ancestor.join(".git/HEAD"), "corrupt").unwrap();
        assert!(verify(&path, &[], true).is_err());
        let (_, admin) = checkout(&path).unwrap();
        assert!(release_path(&path, &admin).exists());
        assert!(path.is_dir());
    }

    #[test]
    fn uncertain_ancestor_markers_are_not_treated_as_empty_directories() {
        for kind in ["nonempty", "file", "symlink", "dangling"] {
            let f = Fixture::new();
            let root = f.worktree_under(&f.0.join("ancestor"));
            let marker = f.0.join("ancestor/.git");
            fs::remove_dir(&marker).unwrap();
            match kind {
                "nonempty" => {
                    fs::create_dir(&marker).unwrap();
                    fs::write(marker.join("HEAD"), "corrupt").unwrap();
                }
                "file" => fs::write(&marker, "corrupt").unwrap(),
                _ => {
                    let destination = f.0.join("empty");
                    if kind == "symlink" {
                        fs::create_dir(&destination).unwrap();
                    }
                    std::os::unix::fs::symlink(destination, &marker).unwrap();
                }
            }
            assert!(verify(&root, &[], false).is_err(), "{kind}");
        }
    }

    #[test]
    fn empty_ancestor_does_not_allow_corrupt_target_or_administration() {
        for kind in ["empty", "gitfile", "admin", "dangling"] {
            let f = Fixture::new();
            let root = f.worktree_under(&f.0.join("ancestor"));
            let (_, admin) = checkout(&root).unwrap();
            match kind {
                "empty" => {
                    fs::remove_file(root.join(".git")).unwrap();
                    fs::create_dir(root.join(".git")).unwrap();
                }
                "gitfile" => fs::write(root.join(".git"), "corrupt").unwrap(),
                "dangling" => {
                    fs::remove_file(root.join(".git")).unwrap();
                    std::os::unix::fs::symlink(root.join("missing"), root.join(".git")).unwrap();
                }
                _ => fs::remove_file(admin.join("HEAD")).unwrap(),
            }
            assert!(verify(&root, &[], false).is_err(), "{kind}");
            assert!(
                verify(&root.join("node_modules"), &[], false).is_err(),
                "{kind}"
            );
        }
    }

    #[test]
    fn ownership_changes_after_candidate_load_reject_removal_and_preserve_release() {
        let f = Fixture::new();
        let path = f.0.join("node_modules");
        release(&path, "session-one", &[]).unwrap();
        verify(&path, &[], true).unwrap(); // candidate list was loaded
        for owned in [f.0.clone(), f.0.join("validation")] {
            assert!(
                verify(&path, &[owned], true)
                    .unwrap_err()
                    .to_string()
                    .contains("retained owner")
            );
            assert!(path.join(".bin/tsc").exists());
        }
        fs::write(
            f.0.join(".git/locked"),
            "session-two: queued validation; keep",
        )
        .unwrap();
        assert!(
            verify(&path, &[], true)
                .unwrap_err()
                .to_string()
                .contains("session-two")
        );
        assert!(path.join(".bin/tsc").exists());
    }

    #[test]
    fn explicitly_released_fixture_is_cleanable_but_replacement_is_not() {
        let f = Fixture::new();
        let path = f.0.join("node_modules");
        release(&path, "finished-session", &[]).unwrap();
        verify(&path, &[], true).unwrap();
        inspect_tree(&path).unwrap();
        let (_, admin) = checkout(&path).unwrap();
        fs::remove_file(release_path(&path, &admin)).unwrap();
        fs::remove_dir_all(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(verify(&path, &[], true).is_err());
        release(&path, "finished-session", &[]).unwrap();
        fs::rename(&path, f.0.join("old-dependencies")).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(
            verify(&path, &[], true)
                .unwrap_err()
                .to_string()
                .contains("changed")
        );
    }

    #[test]
    fn malformed_releases_databases_and_symlinks_fail_closed() {
        let f = Fixture::new();
        let path = f.0.join("node_modules");
        release(&path, "finished-session", &[]).unwrap();
        fs::write(path.join("saved.sqlite"), "local data").unwrap();
        assert!(inspect_tree(&path).is_err());
        assert!(path.join("saved.sqlite").exists());
        let (_, admin) = checkout(&path).unwrap();
        fs::write(release_path(&path, &admin), "broken").unwrap();
        assert!(verify(&path, &[], true).is_err());
        std::os::unix::fs::symlink(&path, f.0.join("alias")).unwrap();
        assert!(verify(&f.0.join("alias"), &[], true).is_err());
    }
}
