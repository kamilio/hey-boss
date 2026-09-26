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

fn status_field<'a>(status: &'a str, name: &str) -> Option<&'a str> {
    status
        .lines()
        .find_map(|line| line.strip_prefix(name))
        .map(str::trim)
}

fn process_uid_is(status: &str, uid: u32) -> bool {
    status_field(status, "Uid:").is_some_and(|value| {
        let mut ids = value.split_whitespace();
        (0..4).all(|_| ids.next().and_then(|id| id.parse::<u32>().ok()) == Some(uid))
            && ids.next().is_none()
    })
}

fn systemd_user_manager(args: &[String], status: &str, cgroup: &str, uid: u32) -> bool {
    if !matches!(args, [binary, flag]
        if matches!(binary.as_str(), "/usr/lib/systemd/systemd" | "/lib/systemd/systemd")
            && flag == "--user")
    {
        return false;
    }
    let manager_scope = format!("0::/user.slice/user-{uid}.slice/user@{uid}.service/init.scope");
    status_field(status, "Name:") == Some("systemd")
        && status_field(status, "PPid:") == Some("1")
        && process_uid_is(status, uid)
        && cgroup.lines().any(|line| line == manager_scope)
}

fn process_arguments(proc: &Path) -> Option<Vec<String>> {
    fs::read(proc.join("cmdline")).ok().map(|args| {
        args.split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    })
}

fn verified_user_manager(proc: &Path, args: &[String]) -> bool {
    let Ok(status) = fs::read_to_string(proc.join("status")) else {
        return false;
    };
    let Ok(cgroup) = fs::read_to_string(proc.join("cgroup")) else {
        return false;
    };
    if !systemd_user_manager(args, &status, &cgroup, unsafe { libc::geteuid() }) {
        return false;
    }
    fs::metadata(&args[0]).is_ok_and(|m| {
        m.is_file() && m.uid() == 0 && m.mode() & 0o022 == 0 && m.mode() & 0o111 != 0
    })
}

fn ssh_transport_without_channel(proc: &Path, user: &str) -> bool {
    if user.is_empty() || user.chars().any(|c| c.is_whitespace() || c == '@') {
        return false;
    }
    let Ok(status) = fs::read_to_string(proc.join("status")) else {
        return false;
    };
    let uid = unsafe { libc::geteuid() };
    if status_field(&status, "Name:") != Some("sshd") || !process_uid_is(&status, uid) {
        return false;
    }
    let Some(parent) = status_field(&status, "PPid:")
        .and_then(|pid| pid.parse::<u32>().ok())
        .filter(|pid| *pid > 1)
        .and_then(|pid| proc.parent().map(|root| root.join(pid.to_string())))
    else {
        return false;
    };
    let Ok(parent_status) = fs::read_to_string(parent.join("status")) else {
        return false;
    };
    if status_field(&parent_status, "Name:") != Some("sshd")
        || !process_uid_is(&parent_status, 0)
        || !process_arguments(&parent).is_some_and(|args| args == [format!("sshd: {user} [priv]")])
    {
        return false;
    }
    let Ok(cgroup) = fs::read_to_string(proc.join("cgroup")) else {
        return false;
    };
    let Ok(parent_cgroup) = fs::read_to_string(parent.join("cgroup")) else {
        return false;
    };
    let scope = cgroup.trim();
    scope
        .strip_prefix(&format!("0::/user.slice/user-{uid}.slice/session-"))
        .and_then(|s| s.strip_suffix(".scope"))
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        && scope == parent_cgroup.trim()
}

// Linux can hide credential daemons and the user service manager from their owner.
// These services do not run checkout workloads; inspect their child jobs separately.
// Unknown non-dumpable processes still stop cleanup.
pub(super) fn authentication_service(proc: &Path) -> bool {
    let Ok(comm) = fs::read_to_string(proc.join("comm")) else {
        return false;
    };
    let Some(args) = process_arguments(proc) else {
        return false;
    };
    if comm.trim() == "systemd" {
        // Only the fixed user manager occupies init.scope; unit jobs remain
        // independently inspected, including unknown non-dumpable children.
        return verified_user_manager(proc, &args);
    }
    if comm.trim() == "(sd-pam)" {
        if !matches!(args.as_slice(), [name] if name == "(sd-pam)") {
            return false;
        }
        let parent = fs::read_to_string(proc.join("status")).ok().and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PPid:").map(|v| v.trim().to_owned()))
        });
        return parent.is_some_and(|pid| {
            let parent = PathBuf::from(format!("/proc/{pid}"));
            process_arguments(&parent).is_some_and(|args| verified_user_manager(&parent, &args))
                && fs::read_to_string(proc.join("cgroup"))
                    .ok()
                    .zip(fs::read_to_string(parent.join("cgroup")).ok())
                    .is_some_and(|(a, b)| a == b && a.contains("/init.scope"))
        });
    }
    if comm.trim() == "sshd" && args.len() == 1 {
        // SSH transport parents use this title. SFTP and unknown SSH jobs are not exempt.
        let Some(title) = args[0].strip_prefix("sshd: ").map(str::trim_end) else {
            return false;
        };
        let Some((user, channel)) = title.split_once('@') else {
            // Forward-only connections have no channel label. Require their
            // privileged monitor; their child workloads are still inspected.
            return ssh_transport_without_channel(proc, title);
        };
        return !user.is_empty()
            && (channel == "notty"
                || channel
                    .strip_prefix("pts/")
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())));
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
    fn unlabelled_ssh_transport_requires_matching_privileged_monitor() {
        let root = std::env::temp_dir().join(format!("hb-proc-transport-{}", std::process::id()));
        let child = root.join("42");
        let parent = root.join("7");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&parent).unwrap();
        let uid = unsafe { libc::geteuid() };
        let status = format!("Name:\tsshd\nPPid:\t7\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n");
        let scope = format!("0::/user.slice/user-{uid}.slice/session-21317.scope\n");
        let privileged = "Name:\tsshd\nUid:\t0\t0\t0\t0\n";
        fs::write(child.join("comm"), "sshd\n").unwrap();
        fs::write(child.join("cmdline"), "sshd: user\0").unwrap();
        fs::write(child.join("status"), &status).unwrap();
        fs::write(child.join("cgroup"), &scope).unwrap();
        fs::write(parent.join("status"), privileged).unwrap();
        fs::write(parent.join("cmdline"), "sshd: user [priv]\0").unwrap();
        fs::write(parent.join("cgroup"), &scope).unwrap();
        assert!(authentication_service(&child));
        for title in ["sshd: other [priv]\0", "sshd: user [net]\0", "custom-job\0"] {
            fs::write(parent.join("cmdline"), title).unwrap();
            assert!(!authentication_service(&child));
        }
        fs::write(parent.join("cmdline"), "sshd: user [priv]\0").unwrap();
        for invalid in [
            "Name:\tsshd\nUid:\t999\t999\t999\t999\n",
            "Name:\tworker\nUid:\t0\t0\t0\t0\n",
            "Name:\tsshd\n",
        ] {
            fs::write(parent.join("status"), invalid).unwrap();
            assert!(!authentication_service(&child));
        }
        fs::write(parent.join("status"), privileged).unwrap();
        fs::write(parent.join("cgroup"), scope.replace("21317", "21318")).unwrap();
        assert!(!authentication_service(&child));
        fs::write(parent.join("cgroup"), &scope).unwrap();
        for invalid in [
            scope.replace("21317", "job"),
            scope.replace(".scope", ".scope/child"),
        ] {
            fs::write(child.join("cgroup"), &invalid).unwrap();
            fs::write(parent.join("cgroup"), &invalid).unwrap();
            assert!(!authentication_service(&child));
        }
        fs::write(child.join("cgroup"), &scope).unwrap();
        fs::write(parent.join("cgroup"), &scope).unwrap();
        fs::write(
            child.join("status"),
            status.replace(
                &format!("Uid:\t{uid}"),
                &format!("Uid:\t{}", uid.wrapping_add(1)),
            ),
        )
        .unwrap();
        assert!(!authentication_service(&child));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn only_the_verified_systemd_user_manager_identity_is_exempt() {
        let status = "Name:\tsystemd\nPPid:\t1\nUid:\t150124\t150124\t150124\t150124\n";
        let cgroup = "0::/user.slice/user-150124.slice/user@150124.service/init.scope\n";
        let accepts = |args: &[&str], status: &str, cgroup: &str, uid| {
            systemd_user_manager(
                &args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
                status,
                cgroup,
                uid,
            )
        };
        for binary in ["/usr/lib/systemd/systemd", "/lib/systemd/systemd"] {
            assert!(accepts(&[binary, "--user"], status, cgroup, 150124));
        }
        for args in [
            vec!["/tmp/systemd", "--user"],
            vec!["systemd", "--user"],
            vec!["/usr/lib/systemd/systemd"],
            vec!["/usr/lib/systemd/systemd", "--system"],
            vec![
                "/usr/lib/systemd/systemd",
                "--user",
                "--unit=workload.service",
            ],
        ] {
            assert!(!accepts(&args, status, cgroup, 150124));
        }
        let args = ["/usr/lib/systemd/systemd", "--user"];
        for invalid in [
            status.replace("Name:\tsystemd", "Name:\tcodex"),
            status.replace("PPid:\t1", "PPid:\t42"),
            status.replace(
                "150124\t150124\t150124\t150124",
                "150124\t0\t150124\t150124",
            ),
            "PPid:\t1\nUid:\t150124\n".into(),
            "PPid:\t1\n".into(),
        ] {
            assert!(!accepts(&args, &invalid, cgroup, 150124));
        }
        for invalid in [
            "0::/user.slice/user-150124.slice/user@150124.service/app.slice/job.service\n",
            "0::/user.slice/user-150124.slice/user@150124.service/init.scope/child\n",
            "0::/user.slice/user-999.slice/user@999.service/init.scope\n",
            "",
        ] {
            assert!(!accepts(&args, status, invalid, 150124));
        }
        assert!(!accepts(&args, status, cgroup, 999));
    }
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
