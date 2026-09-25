//! Fleet rollouts use immutable snapshots and enforce ordering at each destination.
use crate::upgrade_provenance::{Receipt, Source, authorize};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

const INPUTS: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "src",
    "skills/hey-boss",
    "packages/hey-gh",
    "packages/hey-harvester",
    "tools/upgrade_hey_boss.py",
    "tools/drain_github_issues.py",
    "hey_boss_daemon.swift",
    "package_hey_boss.swift",
    "setup_hey_boss.swift",
    "assets",
];
const PAYLOAD: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "src",
    "skills/hey-boss",
    "packages/hey-gh",
    "packages/hey-harvester",
    "tools/upgrade_hey_boss.py",
    "tools/drain_github_issues.py",
    "hey_boss_daemon.swift",
    "package_hey_boss.swift",
    "setup_hey_boss.swift",
    "assets",
    "tests",
    "README.md",
    "LICENSE",
];
const UPSTREAM: &str = "https://github.com/kamilio/hey-boss.git";
const MANIFEST: &str = ".hey-boss-source.json";

#[derive(Args)]
pub struct Options {
    /// Explicit development checkout; remember its location, not uncommitted changes.
    #[arg(long)]
    source: Option<PathBuf>,
    /// Report mismatches and installation provenance without installing.
    #[arg(long)]
    check: bool,
    /// Reinstall a matching build (never bypass source ordering).
    #[arg(long)]
    force: bool,
    #[arg(long, conflicts_with = "host")]
    local_only: bool,
    #[arg(long)]
    host: Vec<String>,
    #[arg(long)]
    json: bool,
    #[arg(long, hide = true)]
    inspect_installation: bool,
    #[arg(long, hide = true)]
    apply_snapshot: Option<PathBuf>,
    #[arg(long, hide = true)]
    binary: Option<PathBuf>,
    #[arg(long, hide = true)]
    observed_generation: Option<u64>,
}

fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn home() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| error("HOME is unset"))
}
fn state() -> io::Result<PathBuf> {
    Ok(home()?.join(".local/share/hey-boss"))
}
fn output(command: &mut Command) -> io::Result<Vec<u8>> {
    let result = command.output()?;
    if !result.status.success() {
        return Err(error(format!(
            "Command {:?} failed: {}",
            command.get_program(),
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    Ok(result.stdout)
}
fn git(root: &Path, args: &[&str]) -> io::Result<String> {
    Ok(
        String::from_utf8_lossy(&output(Command::new("git").arg("-C").arg(root).args(args))?)
            .trim()
            .into(),
    )
}
fn json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)
}
fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    atomic_write(path, &bytes, 0o600)
}
fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| error("Missing parent"))?;
    fs::create_dir_all(parent)?;
    let temp = Temp::new_in(parent)?;
    let file = temp.0.join("replacement");
    let mut handle = fs::File::create(&file)?;
    handle.set_permissions(fs::Permissions::from_mode(mode))?;
    handle.write_all(bytes)?;
    handle.sync_all()?;
    fs::rename(file, path)?;
    fs::File::open(parent)?.sync_all()
}
fn atomic_copy(source: &Path, target: &Path, mode: u32) -> io::Result<()> {
    atomic_write(target, &fs::read(source)?, mode)
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> io::Result<Self> {
        Self::new_in(&std::env::temp_dir())
    }
    fn new_in(parent: &Path) -> io::Result<Self> {
        for n in 0..1000 {
            let path = parent.join(format!(
                "hey-boss-106-{}-{}-{n}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
                    return Ok(Self(path));
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(error("Cannot create staging directory"))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) struct InstallLock(fs::File);
impl InstallLock {
    pub(crate) fn acquire(state: &Path) -> io::Result<Self> {
        fs::create_dir_all(state)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(state.join("upgrade.lock"))?;
        // Queue rollouts; re-read provenance only after the preceding installer finishes.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(file))
    }
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn collect(root: &Path, path: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(error(format!(
            "Source symlink is unsupported: {}",
            path.display()
        )));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            collect(root, &entry?.path(), files)?;
        }
    } else if metadata.is_file() {
        files.push(
            path.strip_prefix(root)
                .map_err(io::Error::other)?
                .to_owned(),
        );
    } else {
        return Err(error("Unsupported source entry"));
    }
    Ok(())
}
fn build_id(root: &Path) -> io::Result<String> {
    let mut files = Vec::new();
    for name in INPUTS {
        collect(root, &root.join(name), &mut files)?;
    }
    // build.rs sorts complete relative names, rather than Path components.
    files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    let mut value = 0xcbf29ce484222325u64;
    for file in files {
        for byte in file
            .to_string_lossy()
            .bytes()
            .chain([0])
            .chain(fs::read(root.join(&file))?)
            .chain([0])
        {
            value = (value ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("{value:016x}"))
}
fn copy_tree(source: &Path, target: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, target)?;
    } else {
        return Err(error("Unsupported source entry (including symlinks)"));
    }
    Ok(())
}
fn repository(root: &Path) -> Option<String> {
    git(root, &["remote", "get-url", "origin"]).ok().map(|url| {
        url.trim_end_matches(".git")
            .trim_start_matches("git@")
            .trim_start_matches("https://")
            .trim_start_matches("ssh://git@")
            .replacen("github.com:", "github.com/", 1)
    })
}
fn snapshot(options: &Options, destination: &Path) -> io::Result<(Source, PathBuf)> {
    let state = state()?;
    snapshot_at(options, destination, &state)
}

fn snapshot_at(
    options: &Options,
    destination: &Path,
    state: &Path,
) -> io::Result<(Source, PathBuf)> {
    fs::create_dir_all(state)?;
    let _source_lock = InstallLock::acquire(&state.join("source-cache"))?;
    let saved = fs::read_to_string(state.join("upgrade-source")).ok();
    let root = if let Some(source) = &options.source {
        source.canonicalize()?
    } else if let Some(saved) = saved {
        PathBuf::from(saved.trim()).canonicalize()?
    } else {
        let cache = state.join("upgrade-checkout");
        if !cache.exists() {
            output(Command::new("git").args(["clone", UPSTREAM]).arg(&cache))?;
        }
        cache
    };
    let explicit = options.source.is_some();
    let commit = if explicit {
        git(&root, &["rev-parse", "HEAD"]).ok()
    } else {
        // A remembered checkout is a location only. Never implicitly ship dirty work.
        if repository(&root).is_some() {
            if git(&root, &["rev-parse", "--is-shallow-repository"])? == "true" {
                git(&root, &["fetch", "--unshallow", "origin", "main"])?;
            } else {
                git(&root, &["fetch", "origin", "main"])?;
            }
            Some(git(&root, &["rev-parse", "refs/remotes/origin/main"])?)
        } else {
            Some(git(&root, &["rev-parse", "refs/heads/main"])?)
        }
    };
    let clean_main = explicit
        && commit.is_some()
        && git(&root, &["rev-parse", "refs/heads/main"]).ok() == commit
        && git(
            &root,
            &[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--",
                "Cargo.toml",
                "Cargo.lock",
                "build.rs",
                "src",
                "skills/hey-boss",
                "packages/hey-gh",
                "packages/hey-harvester",
                "tools/upgrade_hey_boss.py",
                "tools/drain_github_issues.py",
                "hey_boss_daemon.swift",
                "package_hey_boss.swift",
                "setup_hey_boss.swift",
                "assets",
            ],
        )
        .is_ok_and(|s| s.is_empty());
    let kind = if !explicit || clean_main {
        "main"
    } else {
        "development"
    };
    if kind == "main" {
        let archive = output(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["archive", "--format=tar"])
                .arg(commit.as_ref().unwrap())
                .args(PAYLOAD),
        )?;
        let mut child = Command::new("tar")
            .args(["-xf", "-", "-C"])
            .arg(destination)
            .stdin(Stdio::piped())
            .spawn()?;
        child.stdin.take().unwrap().write_all(&archive)?;
        if !child.wait()?.success() {
            return Err(error("Cannot extract committed snapshot"));
        }
    } else {
        let before = build_id(&root)?;
        for name in PAYLOAD {
            copy_tree(&root.join(name), &destination.join(name))?;
        }
        if before != build_id(destination)?
            || before != build_id(&root)?
            || git(&root, &["rev-parse", "HEAD"]).ok() != commit
        {
            return Err(error(
                "Development source changed while staging; retry from a stable checkout",
            ));
        }
    }
    let ancestors = commit
        .as_ref()
        .map(|c| git(&root, &["rev-list", c]))
        .transpose()?
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let source = Source {
        kind: kind.into(),
        repository: repository(&root),
        commit,
        ancestors,
        build: build_id(destination)?,
    };
    write_json(&destination.join(MANIFEST), &source)?;
    Ok((source, root))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Installation {
    build: Option<String>,
    receipt: Option<Receipt>,
}
fn installed_id(binary: &Path) -> Option<String> {
    let bytes = output(Command::new(binary).arg("--version")).ok()?;
    let version = String::from_utf8_lossy(&bytes);
    let build = version.split("(build ").nth(1)?.split(')').next()?;
    (build.len() == 16 && build.bytes().all(|b| b.is_ascii_hexdigit())).then(|| build.into())
}
fn inspect(binary: &Path, state: &Path) -> io::Result<Installation> {
    let _lock = InstallLock::acquire(state)?;
    inspect_unlocked(binary, state)
}
fn inspect_unlocked(binary: &Path, state: &Path) -> io::Result<Installation> {
    let path = state.join("upgrade-receipt.json");
    let receipt = if path.exists() {
        Some(json(&path)?)
    } else {
        None
    };
    Ok(Installation {
        build: installed_id(binary),
        receipt,
    })
}
fn ssh(host: &str) -> io::Result<Command> {
    if host.is_empty()
        || host.starts_with('-')
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._:[]-".contains(&b))
    {
        return Err(error("Invalid SSH host"));
    }
    let mut command = Command::new("ssh");
    command
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            host,
        ])
        .env("SFT_NO_BROWSER", "1");
    Ok(command)
}
fn target_inspect(host: &str, binary: &Path) -> io::Result<Installation> {
    if host == "local" {
        return inspect(binary, &state()?);
    }
    let mut command = ssh(host)?;
    command.arg("\"$HOME/.local/bin/hey-boss\" upgrade --inspect-installation --json");
    match output(&mut command)
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(io::Error::other))
    {
        Ok(installation) => Ok(installation),
        Err(_) => {
            // Bootstrap older CLIs, but do not hide SSH failure or a corrupt receipt.
            let bytes = output(ssh(host)?.arg("if test -f \"$HOME/.local/share/hey-boss/upgrade-receipt.json\"; then exit 1; fi; \"$HOME/.local/bin/hey-boss\" --version"))?;
            let version = String::from_utf8_lossy(&bytes);
            Ok(Installation {
                build: version
                    .split("(build ")
                    .nth(1)
                    .and_then(|s| s.split(')').next())
                    .map(str::to_owned),
                receipt: None,
            })
        }
    }
}
fn remote_apply(
    snapshot: &Path,
    host: &str,
    observed: Option<u64>,
    force: bool,
) -> io::Result<Installation> {
    let archive = output(
        Command::new("tar")
            .env("COPYFILE_DISABLE", "1")
            .arg("--no-xattrs")
            .arg("-czf")
            .arg("-")
            .arg("-C")
            .arg(snapshot)
            .args(PAYLOAD)
            .arg(MANIFEST),
    )?;
    // Compile the staged implementation on the target, so old installed CLIs also
    // enter the new guard on their very first upgrade. No downloaded executable.
    let script = format!(
        "set -eu; export PATH=\"$HOME/.cargo/bin:/opt/homebrew/bin:$PATH\"; stage=$(mktemp -d); trap 'rm -rf \"$stage\"' EXIT; tar -xzf - -C \"$stage\"; export CARGO_TARGET_DIR=\"$HOME/.cache/hey-boss/bootstrap\"; cargo build --quiet --locked --release --manifest-path \"$stage/Cargo.toml\"; cp \"$CARGO_TARGET_DIR/release/hey-boss\" \"$stage/guard\"; \"$stage/guard\" upgrade --apply-snapshot \"$stage\" --binary \"$HOME/.local/bin/hey-boss\" --json{}{}",
        observed
            .map(|n| format!(" --observed-generation {n}"))
            .unwrap_or_default(),
        if force { " --force" } else { "" }
    );
    let mut child = ssh(host)?
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&archive));
    let result = child.wait_with_output()?;
    let sent = writer
        .join()
        .map_err(|_| error("Archive writer panicked"))?;
    if !result.status.success() {
        return Err(error(String::from_utf8_lossy(&result.stderr).into_owned()));
    }
    sent?;
    serde_json::from_slice(&result.stdout).map_err(io::Error::other)
}

fn build(snapshot: &Path, target: &Path) -> io::Result<PathBuf> {
    // Git archives retain commit mtimes, which can predate a cached build of a
    // different snapshot. Refresh the script so Cargo recomputes this build ID.
    fs::File::open(snapshot.join("build.rs"))?.set_modified(SystemTime::now())?;
    let path = format!(
        "{}:/opt/homebrew/bin:{}",
        home()?.join(".cargo/bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    output(
        Command::new("cargo")
            .args([
                "build",
                "--quiet",
                "--locked",
                "--release",
                "--manifest-path",
            ])
            .arg(snapshot.join("Cargo.toml"))
            .env("PATH", path)
            .env("CARGO_TARGET_DIR", target),
    )?;
    Ok(target.join("release/hey-boss"))
}

// Installer implementation lives separately from snapshot and reporting policy.
#[path = "upgrade_install.rs"]
mod install;

fn ordered_install<T>(
    state: &Path,
    source: &Source,
    observed: Option<u64>,
    operation: impl FnOnce(Option<Receipt>) -> io::Result<T>,
) -> io::Result<T> {
    let _lock = InstallLock::acquire(state)?;
    let path = state.join("upgrade-receipt.json");
    let previous: Option<Receipt> = if path.exists() {
        Some(json(&path)?)
    } else {
        None
    };
    authorize(source, previous.as_ref(), observed)?;
    operation(previous)
}

fn apply(
    snapshot: &Path,
    binary: &Path,
    observed: Option<u64>,
    force: bool,
) -> io::Result<Installation> {
    let source: Source = json(&snapshot.join(MANIFEST))?;
    if build_id(snapshot)? != source.build {
        return Err(error("Received source does not match its provenance"));
    }
    let state = state()?;
    ordered_install(&state, &source, observed, |previous_receipt| {
        let previous = inspect_unlocked(binary, &state)?;
        if previous.build.as_deref() == Some(&source.build)
            && !force
            && previous
                .receipt
                .as_ref()
                .is_some_and(|r| r.source.same_release(&source))
        {
            return Ok(previous);
        }
        let mut installed_source = source.clone();
        installed_source.ancestors.clear();
        let receipt = Receipt {
            generation: previous_receipt.as_ref().map_or(1, |r| r.generation + 1),
            source: installed_source,
            installed_at: now(),
        };
        if previous.build.as_deref() == Some(&source.build) && !force {
            // Identical inputs at a newer commit need only advance provenance,
            // without compiling Swift or restarting healthy services.
            write_json(&state.join("upgrade-receipt.json"), &receipt)?;
            return inspect_unlocked(binary, &state);
        }
        let target = home()?.join(".cache/hey-boss/build");
        let built = build(snapshot, &target)?;
        if installed_id(&built).as_deref() != Some(&source.build) {
            return Err(error("Built CLI does not match source snapshot"));
        }
        install::publish(snapshot, binary, &built, &state, &receipt)?;
        inspect_unlocked(binary, &state)
    })
}

#[derive(Serialize)]
struct Report {
    host: String,
    build: String,
    installed_build: Option<String>,
    status: String,
    before: Option<Installation>,
    verified: Option<Installation>,
    final_installation: Option<Installation>,
    error: Option<String>,
}
fn rollout(options: &Options, binary: &Path) -> io::Result<i32> {
    let temp = Temp::new()?;
    let (mut source, root) = snapshot(options, &temp.0)?;
    let mut hosts = vec!["local".to_owned()];
    if !options.local_only {
        let registered = fs::read_to_string(state()?.join("companion-hosts")).unwrap_or_default();
        let targets = if options.host.is_empty() {
            registered.lines().map(str::to_owned).collect()
        } else {
            options.host.clone()
        };
        for host in targets {
            if !host.is_empty() && !hosts.contains(&host) {
                hosts.push(host);
            }
        }
    }
    // Observe all destinations before any builds: a queued dev snapshot must not
    // become authorized by observing an intervening install after a long compile.
    let observations: Vec<_> = hosts.iter().map(|h| target_inspect(h, binary)).collect();
    let mut reports = Vec::new();
    for (host, before) in hosts.into_iter().zip(observations) {
        let mut entry = Report {
            host: host.clone(),
            build: source.build.clone(),
            installed_build: None,
            status: "failed".into(),
            before: None,
            verified: None,
            final_installation: None,
            error: None,
        };
        let result = (|| -> io::Result<()> {
            let before = before?;
            let observed = before.receipt.as_ref().map(|r| r.generation);
            entry.installed_build = before.build.clone();
            entry.before = Some(before.clone());
            if options.check {
                entry.status = if before.build.as_deref() == Some(&source.build) {
                    "current"
                } else {
                    "outdated"
                }
                .into();
                entry.verified = Some(before);
            } else {
                if !options.json {
                    println!(
                        "{host}: checking and installing {} ({} {})",
                        source.build,
                        source.kind,
                        source.commit.as_deref().unwrap_or("unknown commit")
                    );
                }
                let installed = if !options.force
                    && before.build.as_deref() == Some(&source.build)
                    && before
                        .receipt
                        .as_ref()
                        .is_some_and(|r| r.source.same_release(&source))
                {
                    before.clone()
                } else if host == "local" {
                    apply(&temp.0, binary, observed, options.force)?
                } else {
                    remote_apply(&temp.0, &host, observed, options.force)?
                };
                if installed.build.as_deref() != Some(&source.build)
                    || !installed
                        .receipt
                        .as_ref()
                        .is_some_and(|r| r.source.same_release(&source))
                {
                    return Err(error("Installed source/build verification failed"));
                }
                entry.status = if installed == before {
                    "current"
                } else {
                    "updated"
                }
                .into();
                entry.verified = Some(installed);
            }
            Ok(())
        })();
        if let Err(e) = result {
            entry.error = Some(e.to_string());
        }
        if !options.json {
            println!(
                "{host}: {}{}",
                entry.status,
                entry
                    .error
                    .as_ref()
                    .map(|e| format!(" — {e}"))
                    .unwrap_or_default()
            );
        }
        reports.push(entry);
    }
    // A successful per-host install is historical evidence, not a promise that
    // another rollout did not run during the remaining fleet build.
    if !options.check {
        for entry in &mut reports {
            match target_inspect(&entry.host, binary) {
                Ok(final_installation) => {
                    if entry
                        .verified
                        .as_ref()
                        .is_some_and(|verified| verified != &final_installation)
                    {
                        entry.status = "superseded".into();
                        entry.error = Some(
                            "Another upgrade changed this installation after verification".into(),
                        );
                    }
                    entry.final_installation = Some(final_installation);
                }
                Err(e) => {
                    entry.status = "failed".into();
                    entry.error = Some(format!("Final verification: {e}"));
                }
            }
        }
    }
    if options.source.is_some() && !options.check && reports[0].error.is_none() {
        atomic_write(
            &state()?.join("upgrade-source"),
            format!("{}\n", root.display()).as_bytes(),
            0o600,
        )?;
    }
    source.ancestors.clear();
    if options.json {
        println!(
            "{}",
            serde_json::to_string(
                &serde_json::json!({"build": source.build, "source": source, "machines": reports})
            )
            .map_err(io::Error::other)?
        );
    } else {
        for entry in &reports {
            if entry.status == "superseded" {
                println!(
                    "{}: superseded — {}",
                    entry.host,
                    entry.error.as_deref().unwrap_or_default()
                );
            }
        }
    }
    Ok(if reports.iter().any(|r| r.error.is_some()) {
        1
    } else if options.check && reports.iter().any(|r| r.status == "outdated") {
        2
    } else {
        0
    })
}

pub fn run(options: &Options) -> io::Result<()> {
    let binary = options
        .binary
        .clone()
        .unwrap_or(std::env::current_exe()?)
        .canonicalize()?;
    if options.inspect_installation {
        println!(
            "{}",
            serde_json::to_string(&inspect(&binary, &state()?)?).map_err(io::Error::other)?
        );
        return Ok(());
    }
    if let Some(snapshot) = &options.apply_snapshot {
        let installation = apply(
            snapshot,
            &binary,
            options.observed_generation,
            options.force,
        )?;
        println!(
            "{}",
            serde_json::to_string(&installation).map_err(io::Error::other)?
        );
        return Ok(());
    }
    std::process::exit(rollout(options, &binary)?);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archived_snapshots_refresh_the_cached_build_stamp() {
        let temp = Temp::new().unwrap();
        let target = temp.0.join("target");
        let old = SystemTime::now() - std::time::Duration::from_secs(3600);
        for (name, stamp) in [
            ("first", "1111111111111111"),
            ("second", "2222222222222222"),
        ] {
            let root = temp.0.join(name);
            fs::create_dir_all(root.join("src")).unwrap();
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname='hey-boss'\nversion='0.1.0'\nedition='2024'\nbuild='build.rs'\n",
            )
            .unwrap();
            fs::write(
                root.join("Cargo.lock"),
                "version = 4\n[[package]]\nname = 'hey-boss'\nversion = '0.1.0'\n",
            )
            .unwrap();
            fs::write(root.join("build.rs"), "fn main(){println!(\"cargo:rerun-if-changed=src\");let stamp=std::fs::read_to_string(\"src/stamp\").unwrap();println!(\"cargo:rustc-env=STAMP={stamp}\");}").unwrap();
            fs::write(
                root.join("src/main.rs"),
                "fn main(){println!(\"hey-boss 0.1.0 (build {})\",env!(\"STAMP\"));}",
            )
            .unwrap();
            fs::write(root.join("src/stamp"), stamp).unwrap();
            for path in [
                "Cargo.toml",
                "Cargo.lock",
                "build.rs",
                "src/main.rs",
                "src/stamp",
                "src",
            ] {
                fs::File::open(root.join(path))
                    .unwrap()
                    .set_modified(old)
                    .unwrap();
            }
            let binary = build(&root, &target).unwrap();
            assert_eq!(
                installed_id(&binary).as_deref(),
                Some(stamp),
                "The new archived snapshot must not reuse the earlier stamp"
            );
        }
    }
    #[test]
    fn source_identity_matches_compiler_identity() {
        assert_eq!(
            build_id(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap(),
            env!("HEY_BOSS_BUILD_ID")
        );
    }
    use std::sync::mpsc;

    fn source(commit: &str, ancestors: &[&str], kind: &str) -> Source {
        Source {
            kind: kind.into(),
            repository: Some("repo".into()),
            commit: Some(commit.into()),
            ancestors: ancestors.iter().map(|s| (*s).into()).collect(),
            build: commit.into(),
        }
    }
    fn save(state: &Path, source: Source, previous: Option<Receipt>) -> io::Result<()> {
        write_json(
            &state.join("upgrade-receipt.json"),
            &Receipt {
                generation: previous.map_or(1, |r| r.generation + 1),
                source,
                installed_at: now(),
            },
        )
    }
    #[test]
    fn old_staged_rollout_queued_after_newer_install_never_publishes() {
        let temp = Temp::new().unwrap();
        let state = temp.0.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let newer_state = state.clone();
        let newer = std::thread::spawn(move || {
            let new = source("new", &["new", "old"], "main");
            ordered_install(&newer_state, &new, None, |previous| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                save(&newer_state, new.clone(), previous)
            })
        });
        entered_rx.recv().unwrap();
        let older_state = state.clone();
        let (queued_tx, queued_rx) = mpsc::channel();
        let older = std::thread::spawn(move || {
            let old = source("old", &["old"], "main");
            queued_tx.send(()).unwrap();
            ordered_install(&older_state, &old, None, |_| {
                fs::write(older_state.join("stale-published"), "bad")
            })
        });
        queued_rx.recv().unwrap();
        release_tx.send(()).unwrap();
        newer.join().unwrap().unwrap();
        assert!(
            older
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("stale")
        );
        assert!(!state.join("stale-published").exists());
        let installed: Receipt = json(&state.join("upgrade-receipt.json")).unwrap();
        assert_eq!(installed.source.commit.as_deref(), Some("new"));
    }
    #[test]
    fn queued_development_is_rejected_after_new_main_and_failed_install_preserves_receipt() {
        let temp = Temp::new().unwrap();
        let initial = source("old", &["old"], "main");
        save(&temp.0, initial, None).unwrap();
        let newer = source("new", &["new", "old"], "main");
        ordered_install(&temp.0, &newer, Some(1), |previous| {
            save(&temp.0, newer.clone(), previous)
        })
        .unwrap();
        let dev = source("new", &["new", "old"], "development");
        assert!(
            ordered_install(&temp.0, &dev, Some(1), |_| -> io::Result<()> {
                panic!("stale dev published")
            })
            .is_err()
        );
        let before = fs::read(temp.0.join("upgrade-receipt.json")).unwrap();
        assert!(
            ordered_install(&temp.0, &dev, Some(2), |_| Err::<(), _>(error(
                "migration failed"
            )))
            .is_err()
        );
        assert_eq!(
            before,
            fs::read(temp.0.join("upgrade-receipt.json")).unwrap()
        );
    }
    fn options(source: Option<PathBuf>) -> Options {
        Options {
            source,
            check: true,
            force: false,
            local_only: true,
            host: Vec::new(),
            json: true,
            inspect_installation: false,
            apply_snapshot: None,
            binary: None,
            observed_generation: None,
        }
    }
    fn fixture(root: &Path) {
        for name in PAYLOAD {
            let path = root.join(name);
            if [
                "src",
                "skills/hey-boss",
                "packages/hey-gh",
                "packages/hey-harvester",
                "assets",
                "tests",
            ]
            .contains(name)
            {
                fs::create_dir_all(&path).unwrap();
                fs::write(path.join("fixture"), name).unwrap();
            } else {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, name).unwrap();
            }
        }
        git(root, &["init", "-b", "main"]).unwrap();
        git(root, &["add", "."]).unwrap();
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
    }
    #[test]
    fn remembered_dirty_checkout_ships_committed_main_only_and_explicit_source_records_development()
    {
        let repo = Temp::new().unwrap();
        fixture(&repo.0);
        let state = Temp::new().unwrap();
        fs::write(
            state.0.join("upgrade-source"),
            repo.0.to_string_lossy().as_bytes(),
        )
        .unwrap();
        let clean = Temp::new().unwrap();
        let (main, _) = snapshot_at(&options(None), &clean.0, &state.0).unwrap();
        fs::write(repo.0.join("src/fixture"), "uncommitted work").unwrap();
        let normal = Temp::new().unwrap();
        let (again, _) = snapshot_at(&options(None), &normal.0, &state.0).unwrap();
        assert_eq!(main, again);
        assert_eq!(fs::read(normal.0.join("src/fixture")).unwrap(), b"src");
        let explicit = Temp::new().unwrap();
        let (dev, _) = snapshot_at(&options(Some(repo.0.clone())), &explicit.0, &state.0).unwrap();
        assert_eq!(dev.kind, "development");
        assert_eq!(main.commit, dev.commit);
        assert_ne!(main.build, dev.build);
        assert_eq!(
            fs::read(explicit.0.join("src/fixture")).unwrap(),
            b"uncommitted work"
        );
    }
    #[test]
    fn clean_main_archive_identity_matches_checkout_and_detects_native_asset_changes() {
        let repo = Temp::new().unwrap();
        fixture(&repo.0);
        let state = Temp::new().unwrap();
        let archive = Temp::new().unwrap();
        let (source, _) =
            snapshot_at(&options(Some(repo.0.clone())), &archive.0, &state.0).unwrap();
        assert_eq!(source.kind, "main");
        assert_eq!(source.build, build_id(&repo.0).unwrap());
        fs::write(repo.0.join("assets/fixture"), "changed").unwrap();
        assert_ne!(source.build, build_id(&repo.0).unwrap());
    }

    #[test]
    fn snapshot_fingerprint_matches_real_build_script_with_shared_path_stems() {
        let repo = Temp::new().unwrap();
        fixture(&repo.0);
        fs::write(repo.0.join("src/component.rs"), "module").unwrap();
        fs::create_dir(repo.0.join("src/component")).unwrap();
        fs::write(repo.0.join("src/component/nested.rs"), "nested module").unwrap();
        let helper = Temp::new().unwrap();
        let script = helper.0.join("build-script.rs");
        let executable = helper.0.join("build-script");
        fs::write(&script, include_str!("../build.rs")).unwrap();
        output(
            Command::new("rustc")
                .arg(&script)
                .arg("-o")
                .arg(&executable),
        )
        .unwrap();
        let bytes = output(Command::new(&executable).env("CARGO_MANIFEST_DIR", &repo.0)).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let compiler_id = text
            .lines()
            .find_map(|line| line.strip_prefix("cargo:rustc-env=HEY_BOSS_BUILD_ID="))
            .unwrap();
        assert_eq!(build_id(&repo.0).unwrap(), compiler_id);
    }
    #[test]
    fn invalid_hosts_and_corrupt_receipts_fail_before_publication() {
        for host in ["", "-oProxyCommand=bad", "devbox;touch bad", "devbox bad"] {
            assert!(ssh(host).is_err());
        }
        assert!(ssh("user@devbox").is_ok());
        let state = Temp::new().unwrap();
        fs::write(state.0.join("upgrade-receipt.json"), "invalid").unwrap();
        assert!(
            ordered_install(
                &state.0,
                &source("main", &["main"], "main"),
                None,
                |_| -> io::Result<()> { panic!("published despite corrupt receipt") }
            )
            .is_err()
        );
    }
}
