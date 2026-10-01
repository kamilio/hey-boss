//! A closed SSH transport, not the persistent ScaleFT agent IPC, proves abandonment.
use super::*;

pub(super) const EXECUTABLE: &str = "/Applications/ScaleFT.app/Contents/MacOS/sft";
pub(super) const CATEGORY: &str = "ScaleFT SSH helper";

pub(super) fn helper(p: &Process) -> bool {
    if p.executable != EXECUTABLE {
        return false;
    }
    let mut args = p.arguments.split_whitespace();
    matches!((args.next(), args.next(), args.next(), args.next()),
        (Some(bin), Some("proxycommand"), Some(host), None)
        if matches!(bin, "sft" | "/usr/local/bin/sft" | EXECUTABLE)
            && !host.starts_with('-'))
}

// Two lsof queries per bounded batch, rather than two subprocesses per orphan.
// Final authorization always queries the individual process again.
pub(super) fn inspect(
    pids: &[u32],
    mut query: impl FnMut(&[&str]) -> io::Result<String>,
) -> BTreeMap<u32, Result<bool, String>> {
    let mut result = BTreeMap::new();
    for chunk in pids.chunks(128) {
        let list = chunk
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let inspection = (|| {
            let stdio = query(&["-a", "-p", &list, "-d", "0,1,2", "-F", "pftn"])?;
            let network = query(&["-a", "-p", &list, "-i", "-F", "p"])?;
            closed_transports(chunk, &stdio, &network)
        })();
        for pid in chunk {
            result.insert(
                *pid,
                match &inspection {
                    Ok(closed) => Ok(closed.contains(pid)),
                    Err(error) => Err(error.to_string()),
                },
            );
        }
    }
    result
}

fn closed_transports(pids: &[u32], stdio: &str, network: &str) -> io::Result<BTreeSet<u32>> {
    let invalid =
        || io::Error::other("Incomplete ScaleFT descriptor inspection; preserving helpers");
    let expected: BTreeSet<_> = pids.iter().copied().collect();
    let mut connected = BTreeSet::new();
    for line in network.lines() {
        // lsof always emits an f marker, even when only process fields were requested.
        if let Some(fd) = line.strip_prefix('f') {
            if connected.is_empty() || fd.parse::<u32>().is_err() {
                return Err(invalid());
            }
            continue;
        }
        let pid = line
            .strip_prefix('p')
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(invalid)?;
        if !expected.contains(&pid) || !connected.insert(pid) {
            return Err(invalid());
        }
    }
    let mut records: BTreeMap<u32, Vec<&str>> = BTreeMap::new();
    let mut current = None;
    for line in stdio.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            let pid = pid.parse().map_err(|_| invalid())?;
            if !expected.contains(&pid) || records.insert(pid, Vec::new()).is_some() {
                return Err(invalid());
            }
            current = Some(pid);
        } else {
            records
                .get_mut(&current.ok_or_else(invalid)?)
                .ok_or_else(invalid)?
                .push(line);
        }
    }
    // Require positive evidence from detached stderr. Missing records, open stdin
    // or stdout (even /dev/null), partial output and unknown descriptor shapes fail closed.
    Ok(records
        .into_iter()
        .filter_map(|(pid, fields)| {
            (!connected.contains(&pid)
                && fields.len() == 3
                && fields[0] == "f2"
                && fields.contains(&"tunix")
                && fields.contains(&"n->(none)"))
            .then_some(pid)
        })
        .collect())
}

pub(super) fn disconnected(pid: u32) -> io::Result<bool> {
    inspect(&[pid], lsof)
        .remove(&pid)
        .unwrap()
        .map_err(io::Error::other)
}

fn unchanged(old: &Process, fresh: &Process, table: &Table, config: &Config) -> bool {
    helper(fresh)
        && old.identity == fresh.identity
        && old.executable == fresh.executable
        && old.arguments == fresh.arguments
        && old.uid == fresh.uid
        && quiet_since(old, fresh, true)
        && tree(
            fresh,
            table,
            unsafe { libc::geteuid() },
            minimum_age(fresh, config),
        ) == Some(BTreeSet::from([old.pid]))
}

pub(super) fn terminate(root: &Process, config: &Config) -> io::Result<usize> {
    let fresh = inventory()?;
    let Some(current) = fresh.get(&root.pid) else {
        return Ok(0);
    };
    let ids = BTreeSet::from([root.pid]);
    if !unchanged(root, current, &fresh, config)
        || !super::super::workload_ownership::protected(&fresh, &ids, &BTreeSet::new()).is_empty()
        || !disconnected(root.pid)?
    {
        return Ok(0);
    }
    // Do not escalate. A helper that ignores TERM must earn a new quiet window.
    if !signal(current, libc::SIGTERM)? {
        return Ok(0);
    }
    std::thread::sleep(Duration::from_millis(500));
    Ok(usize::from(
        identity(root.pid).as_deref() != Some(&root.identity),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy() -> Process {
        Process {
            pid: 100,
            parent: 1,
            uid: unsafe { libc::geteuid() },
            age_seconds: 7200,
            cpu_seconds: 2.0,
            executable: EXECUTABLE.into(),
            identity: "original".into(),
            arguments: "/usr/local/bin/sft proxycommand devboxkjopek".into(),
        }
    }

    #[test]
    fn final_authorization_rejects_identity_command_owner_family_and_activity_changes() {
        let old = proxy();
        let table = Table::from([(old.pid, old.clone())]);
        let config = Config::default();
        assert!(unchanged(&old, &old, &table, &config));
        let mut variants = vec![old.clone(); 8];
        variants[0].identity = "reused-pid".into();
        variants[1].executable = "/tmp/sft".into();
        variants[2].arguments = "sft proxycommand another-host".into();
        variants[3].uid += 1;
        variants[4].parent = 42;
        variants[5].cpu_seconds += 0.2;
        variants[6].age_seconds = 3599;
        variants[7].cpu_seconds = f64::NAN;
        for fresh in variants {
            let table = Table::from([(fresh.pid, fresh.clone())]);
            assert!(!unchanged(&old, &fresh, &table, &config), "{fresh:?}");
        }
        let mut child = old.clone();
        child.pid += 1;
        child.parent = old.pid;
        let table = Table::from([(old.pid, old.clone()), (child.pid, child)]);
        assert!(!unchanged(&old, &old, &table, &config));
        for args in ["sft service", "sft proxycommand devbox", "sft --headless"] {
            let mut p = old.clone();
            p.arguments = args.into();
            assert!(essential(&p));
            for pressure in ["Normal", "Warning", "Critical"] {
                assert_eq!(expired_kind(&p, pressure), None);
            }
        }
    }

    #[test]
    fn aggressive_pressure_preserves_scaleft_services_and_their_descendants() {
        for args in ["sft service", "sft proxycommand devbox"] {
            let mut root = proxy();
            root.arguments = args.into();
            root.age_seconds = 172800;
            let mut child = root.clone();
            child.pid += 1;
            child.parent = root.pid;
            child.executable = "/usr/local/bin/node".into();
            child.arguments = "node worker.js".into();
            assert!(expired_kind(&child, "Critical").is_some());
            let table = Table::from([(root.pid, root), (child.pid, child)]);
            let config = Config {
                aggressive: true,
                ..Config::default()
            };
            for pressure in ["Normal", "Warning", "Critical"] {
                let (items, killed) =
                    aggressive_harvest(&table, pressure, &config, &mut BTreeMap::new(), false)
                        .unwrap();
                assert!(items.is_empty(), "{args}: {pressure}: {items:?}");
                assert_eq!(killed, 0);
            }
        }
    }

    #[test]
    fn live_descriptors_and_both_policies_preserve_transport_and_recheck_real_executable() {
        use std::io::Read;
        use std::process::Stdio;
        let directory =
            std::env::temp_dir().join(format!("harvester-scaleft-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        struct Directory(std::path::PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _directory = Directory(directory.clone());
        let source = directory.join("fixture.c");
        let binary = directory.join("fixture");
        std::fs::write(
            &source,
            r#"
#include <unistd.h>
#include <stdio.h>
#include <stdlib.h>
#include <fcntl.h>
#include <sys/socket.h>
#include <netinet/in.h>
int main(int argc, char **argv) {
    for (int fd=3; fd<1024; ++fd) close(fd);
    pid_t pid=fork(); if(pid<0) return 2; if(pid>0) return 0;
    setsid(); alarm(120);
    printf("%d\n", getpid()); fflush(stdout);
    int pair[2]; if(socketpair(AF_UNIX, SOCK_STREAM, 0, pair)) return 3;
    if(dup2(pair[0], 2)<0) return 4; close(pair[0]); close(pair[1]);
    // The live ScaleFT agent IPC is allowed: it is not the SSH transport.
    int agent[2]; if(socketpair(AF_UNIX, SOCK_STREAM, 0, agent)) return 5;
    int mode=atoi(argv[1]);
    if(mode==1) { int input=open("/dev/null", O_RDWR); dup2(input,0); close(input); }
    else close(0);
    close(1);
    if(mode==2) {
        int fd=socket(AF_INET,SOCK_STREAM,0); struct sockaddr_in a={0};
        a.sin_family=AF_INET; a.sin_addr.s_addr=htonl(INADDR_LOOPBACK);
        if(bind(fd,(struct sockaddr*)&a,sizeof(a)) || listen(fd,4)) return 6;
    }
    for(;;) pause();
}
"#,
        )
        .unwrap();
        assert!(
            Command::new("cc")
                .arg(&source)
                .arg("-o")
                .arg(&binary)
                .status()
                .unwrap()
                .success()
        );
        struct Fixture(Process);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = signal(&self.0, libc::SIGKILL);
            }
        }
        let mut fixtures = Vec::new();
        for mode in 0..3 {
            let mut child = Command::new(&binary)
                .arg(mode.to_string())
                .current_dir(&directory)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut response = String::new();
            child
                .stdout
                .take()
                .unwrap()
                .read_to_string(&mut response)
                .unwrap();
            assert!(child.wait().unwrap().success());
            let pid: u32 = response.trim().parse().unwrap();
            // Keep cleanup possible even if the inventory assertion fails.
            let mut p = proxy();
            p.pid = pid;
            p.identity = identity(pid).unwrap();
            p.executable = executable(pid).unwrap();
            fixtures.push(Fixture(p));
        }
        let pids: Vec<_> = fixtures.iter().map(|f| f.0.pid).collect();
        let results = inspect(&pids, lsof);
        assert_eq!(results[&pids[0]], Ok(true), "{results:?}");
        assert_eq!(results[&pids[1]], Ok(false));
        assert_eq!(results[&pids[2]], Ok(false));
        let table: Table = fixtures
            .iter()
            .map(|f| {
                let mut p = f.0.clone();
                p.executable = EXECUTABLE.into();
                (p.pid, p)
            })
            .collect();
        for aggressive in [false, true] {
            let config = Config {
                aggressive,
                observation_seconds: 0,
                ..Config::default()
            };
            let mut observations = BTreeMap::new();
            let (first, count) = harvest(&table, &config, &mut observations, false).unwrap();
            assert_eq!(count, 0);
            assert_eq!(first.len(), 3);
            assert!(first.iter().all(|item| !item.eligible));
            let (second, count) = harvest(&table, &config, &mut observations, false).unwrap();
            assert_eq!(count, 0);
            assert_eq!(second.iter().filter(|item| item.eligible).count(), 1);
            // A forged/stale inventory must never authorize a signal to the fixture.
            let (_, count) = harvest(&table, &config, &mut observations, true).unwrap();
            assert_eq!(count, 0);
            for f in &fixtures {
                assert_eq!(identity(f.0.pid).as_deref(), Some(f.0.identity.as_str()));
            }
        }
    }

    #[test]
    fn only_observed_closed_transport_without_any_ip_socket_qualifies() {
        let closed = "p10\nf2\ntunix\nn->(none)\n";
        assert_eq!(
            closed_transports(&[10], closed, "").unwrap(),
            BTreeSet::from([10])
        );
        assert!(
            closed_transports(&[10], closed, "p10\nf9\nf10\n")
                .unwrap()
                .is_empty()
        );
        for stdio in [
            "",
            "p10\n",
            "p10\nf2\ntunix\n",
            "p10\nf2\ntunix\nn->0x123\n",
            "p10\nf2\ntCHR\nn/dev/null\n",
            "p10\nf0\ntPIPE\nnpipe\nf2\ntunix\nn->(none)\n",
            "p10\nf1\ntCHR\nn/dev/null\nf2\ntunix\nn->(none)\n",
            "p10\nf2\ntunix\nn->(none)\nn->(none)\n",
        ] {
            assert!(
                closed_transports(&[10], stdio, "").unwrap().is_empty(),
                "{stdio}"
            );
        }
        for (stdio, network) in [
            ("f2\ntunix\nn->(none)\n", ""),
            (closed, "garbage\n"),
            (closed, "p11\n"),
            ("p10\np10\n", ""),
            ("p11\nf2\ntunix\nn->(none)\n", ""),
        ] {
            assert!(closed_transports(&[10], stdio, network).is_err());
        }
    }

    #[test]
    fn batches_hundreds_of_helpers_and_keeps_connections_and_failures_separate() {
        let pids: Vec<_> = (1..=878).collect();
        let mut queries = 0;
        let results = inspect(&pids, |args| {
            queries += 1;
            let ids: Vec<u32> = args[2].split(',').map(|s| s.parse().unwrap()).collect();
            if ids.contains(&257) {
                return Err(io::Error::other("lsof diagnostic"));
            }
            if args.contains(&"-i") {
                return Ok(if ids.contains(&878) {
                    "p878\n".into()
                } else {
                    String::new()
                });
            }
            Ok(ids
                .iter()
                .map(|pid| format!("p{pid}\nf2\ntunix\nn->(none)\n"))
                .collect())
        });
        assert_eq!(queries, 13);
        assert_eq!(results.len(), 878);
        assert_eq!(results[&1], Ok(true));
        assert!(results[&257].is_err());
        assert_eq!(results[&877], Ok(true));
        assert_eq!(results[&878], Ok(false));
    }
}
