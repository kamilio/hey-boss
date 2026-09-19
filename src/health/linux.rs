//! Linux activity inspection via procfs, independent of unrelated mount warnings.
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) fn descriptors(pid: u32) -> io::Result<Vec<(u32, PathBuf)>> {
    let mut result = Vec::new();
    for entry in fs::read_dir(format!("/proc/{pid}/fd"))? {
        let entry = entry?;
        let Some(fd) = entry.file_name().to_str().and_then(|s| s.parse().ok()) else {
            continue;
        };
        match fs::read_link(entry.path()) {
            Ok(path) => result.push((fd, path)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {} // Descriptor closed during inspection.
            Err(e) => return Err(e),
        }
    }
    Ok(result)
}

// Linux deliberately hides descriptors of credential daemons even from their owner.
// These services do not run checkout workloads; inspect their child jobs separately.
// Unknown non-dumpable processes still stop cleanup.
pub(super) fn authentication_service(proc: &Path) -> bool {
    let Ok(comm) = fs::read_to_string(proc.join("comm")) else {
        return false;
    };
    let Ok(args) = fs::read(proc.join("cmdline")) else {
        return false;
    };
    let args: Vec<_> = args
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    if comm.trim() == "(sd-pam)" {
        let parent = fs::read_to_string(proc.join("status")).ok().and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PPid:").map(|v| v.trim().to_owned()))
        });
        return parent.is_some_and(|pid| {
            let parent = PathBuf::from(format!("/proc/{pid}"));
            fs::read_link(parent.join("exe")).is_ok_and(|p| {
                p == Path::new("/usr/lib/systemd/systemd") || p == Path::new("/lib/systemd/systemd")
            }) && fs::read_to_string(proc.join("cgroup"))
                .ok()
                .zip(fs::read_to_string(parent.join("cgroup")).ok())
                .is_some_and(|(a, b)| a == b && a.contains("/init.scope"))
        });
    }
    if comm.trim() == "sshd" && args.len() == 1 {
        // SSH transport parents use this title. SFTP and unknown SSH jobs are not exempt.
        return args[0]
            .strip_prefix("sshd: ")
            .and_then(|s| s.trim_end().split_once('@'))
            .is_some_and(|(user, channel)| {
                !user.is_empty()
                    && (channel == "notty"
                        || channel.strip_prefix("pts/").is_some_and(|s| {
                            !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
                        }))
            });
    }
    if !matches!(comm.trim(), "ssh-agent" | "gpg-agent") {
        return false;
    }
    let Some(argument) = args.first() else {
        return false;
    };
    let executable = if argument == comm.trim() {
        PathBuf::from("/usr/bin").join(argument)
    } else {
        PathBuf::from(argument)
    };
    if !matches!(executable.parent(), Some(p) if p == Path::new("/usr/bin") || p == Path::new("/bin"))
        || executable.file_name().and_then(|s| s.to_str()) != Some(comm.trim())
    {
        return false;
    }
    // An explicit key directory could belong to a checkout; keep that case protected.
    if args
        .iter()
        .any(|s| s == "--homedir" || s.starts_with("--homedir="))
    {
        return false;
    }
    fs::metadata(&executable).is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0)
}

pub fn open_paths() -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if metadata.uid() != unsafe { libc::geteuid() } {
            continue;
        }
        let result = (|| {
            for name in ["cwd", "exe"] {
                match fs::read_link(entry.path().join(name)) {
                    Ok(path) => paths.push(path),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {} // Zombies have no cwd/exe.
                    Err(e) => return Err(e),
                }
            }
            for (_, path) in descriptors(pid)? {
                if path.is_absolute() {
                    paths.push(path);
                }
            }
            let maps = fs::read_to_string(entry.path().join("maps"))?;
            for line in maps.lines() {
                let mut rest = line;
                for _ in 0..5 {
                    rest = rest.trim_start();
                    rest = rest.find(char::is_whitespace).map_or("", |at| &rest[at..]);
                }
                let path = rest.trim_start();
                if path.starts_with('/') {
                    paths.push(PathBuf::from(path.replace("\\012", "\n")));
                }
            }
            Ok::<_, io::Error>(())
        })();
        if let Err(error) = result {
            let exited = fs::read_to_string(entry.path().join("stat")).is_ok_and(|s| {
                s.rsplit_once(')')
                    .and_then(|(_, fields)| fields.split_whitespace().next())
                    .is_some_and(|state| matches!(state, "Z" | "X"))
            });
            if !entry.path().exists()
                || exited
                || (error.kind() == io::ErrorKind::PermissionDenied
                    && authentication_service(&entry.path()))
            {
                continue;
            } // Exited process or known credential daemon; child jobs are still inspected.
            return Err(io::Error::other(format!(
                "Cannot inspect PID {pid}; worktrees preserved: {error}"
            )));
        }
    }
    Ok(paths)
}

fn idle_tcp(contents: &str) -> BTreeSet<String> {
    contents
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (fields.len() >= 10 && matches!(fields[3], "0A" | "07")).then(|| fields[9].to_owned())
        })
        .collect()
}

pub fn disconnected(ids: &BTreeSet<u32>) -> io::Result<bool> {
    for pid in ids {
        let fds = descriptors(*pid)?;
        let mut idle = BTreeSet::new();
        for table in ["tcp", "tcp6"] {
            let path = format!("/proc/{pid}/net/{table}");
            match fs::read_to_string(path) {
                Ok(contents) => idle.extend(idle_tcp(&contents)),
                Err(e) if e.kind() == io::ErrorKind::NotFound && table == "tcp6" => {}
                Err(e) => return Err(e),
            }
        }
        for (fd, path) in fds {
            let value = path.to_string_lossy();
            if value.starts_with("pipe:[") {
                return Ok(false);
            }
            if let Some(inode) = value
                .strip_prefix("socket:[")
                .and_then(|s| s.strip_suffix(']'))
            {
                // Unknown/Unix/UDP sockets are preserved, including controller IPC.
                if fd <= 3 || !idle.contains(inode) {
                    return Ok(false);
                }
            }
            if fd <= 2 && value.starts_with("/dev/") && path != Path::new("/dev/null") {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_processes_and_sftp_are_not_credential_exemptions() {
        let root = std::env::temp_dir().join(format!("hb-proc-auth-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("comm"), "sshd\n").unwrap();
        for title in [
            "sshd: user@internal-sftp",
            "sshd: user@unknown",
            "custom-ssh-job",
        ] {
            fs::write(root.join("cmdline"), title).unwrap();
            assert!(!authentication_service(&root));
        }
        fs::write(root.join("cmdline"), "sshd: user@notty     ").unwrap();
        assert!(authentication_service(&root));
        fs::write(root.join("comm"), "ssh-agent\n").unwrap();
        fs::write(root.join("cmdline"), "/tmp/ssh-agent\0").unwrap();
        assert!(!authentication_service(&root));
        fs::write(root.join("comm"), "codex\n").unwrap();
        fs::write(root.join("cmdline"), "/usr/bin/codex\0").unwrap();
        assert!(!authentication_service(&root));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn only_listening_or_closed_tcp_is_idle() {
        let input = "header\n0: addr remote 0A q timer retry uid timeout 123\n1: addr remote 01 q timer retry uid timeout 456\n2: addr remote 07 q timer retry uid timeout 789\n";
        assert_eq!(
            idle_tcp(input),
            BTreeSet::from(["123".into(), "789".into()])
        );
    }
}
