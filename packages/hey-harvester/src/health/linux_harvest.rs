//! Narrow Linux test-fixture rules. Never classify a generic Python/Node service.
use super::linux::{authentication_service, descriptors};
use super::processes::{Process, Table, identity};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn temporary(path: &Path) -> bool {
    path.parent() == Some(Path::new("/tmp"))
        && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            n.starts_with("tmp")
                && n.len() > 3
                && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}
fn python(p: &Process) -> bool {
    Path::new(&p.executable)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| {
            n.strip_prefix("python")
                .is_some_and(|suffix| suffix.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
        })
}
fn module_args(p: &Process) -> Vec<&str> {
    let mut args: Vec<_> = p.arguments.split_whitespace().skip(1).collect();
    if args.first() == Some(&"-u") {
        args.remove(0);
    }
    args
}
fn port(s: &str) -> bool {
    s.parse::<u16>().is_ok_and(|n| n > 0)
}
fn app_server(p: &Process) -> bool {
    let a = module_args(p);
    python(p)
        && a.len() == 5
        && a[0] == "-m"
        && a[1] == "aipoe"
        && port(a[2])
        && a[3] == "--per-process-port"
        && port(a[4])
}
pub(super) fn category(p: &Process) -> Option<&'static str> {
    let args: Vec<_> = p.arguments.split_whitespace().collect();
    if p.executable == "/opt/google/chrome/chrome"
        && args
            .iter()
            .any(|s| *s == "--headless" || s.starts_with("--headless="))
        && args.contains(&"--test-type=webdriver")
        && args.iter().any(|s| {
            s.strip_prefix("--user-data-dir=/tmp/.com.google.Chrome.")
                .is_some_and(|suffix| {
                    !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
                })
        })
    {
        return Some("Selenium test browser");
    }
    if !python(p) {
        return None;
    }
    let a = module_args(p);
    if a.len() == 4 && a[0] == "-m" && a[1] == "poe_proxy.unix_server" && a[3] == "--test-mode" {
        let socket = Path::new(a[2]);
        if socket.file_name().is_some_and(|s| s == "poeproxy.sock")
            && socket.parent().is_some_and(temporary)
        {
            return Some("Poe test proxy");
        }
    }
    if args.len() == 4
        && args[2] == "--port"
        && port(args[3])
        && Path::new(args[1]).ends_with("scripts/catbot.py")
        && Path::new(args[0]).ends_with("venv/bin/python")
        && Path::new(args[0]).ancestors().nth(3).is_some_and(temporary)
    {
        return Some("Temporary catbot test server");
    }
    None
}
fn cwd(p: &Process) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{}/cwd", p.pid)).ok()
}

pub(super) fn fixture_family(root: &Process, table: &Table) -> Option<BTreeSet<u32>> {
    let directory = cwd(root)?;
    if !directory.ends_with("poe/graphql-server") {
        return None;
    }
    let mut ids = BTreeSet::from([root.pid]);
    if category(root) == Some("Temporary catbot test server") {
        let script = root.arguments.split_whitespace().nth(1)?;
        if Path::new(script) != directory.parent()?.parent()?.join("scripts/catbot.py") {
            return None;
        }
    } else {
        // The application server has no test flag. Only include it alongside its
        // explicit test-mode proxy, same executable/cwd and launch time.
        let companions: Vec<_> = table
            .values()
            .filter(|p| {
                p.uid == root.uid
                    && p.parent == 1
                    && p.executable == root.executable
                    && p.age_seconds.abs_diff(root.age_seconds) <= 2
                    && app_server(p)
                    && cwd(p).as_ref() == Some(&directory)
            })
            .collect();
        if companions.len() > 1 {
            return None;
        }
        ids.extend(companions.iter().map(|p| p.pid));
    }
    if table.values().any(|p| ids.contains(&p.parent)) {
        return None;
    }
    Some(ids)
}

pub(super) fn activity_fingerprint(ids: &BTreeSet<u32>) -> io::Result<String> {
    let mut result = String::new();
    for pid in ids {
        result.push_str(&format!(
            "{pid}:{}\n",
            fs::read_to_string(format!("/proc/{pid}/io"))?
        ));
    }
    Ok(result)
}

fn closed_controller_pipes(ids: &BTreeSet<u32>, pipes: &BTreeSet<String>) -> io::Result<bool> {
    if pipes.is_empty() {
        return Ok(true);
    }
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if ids.contains(&pid) {
            continue;
        }
        let inspect: io::Result<bool> = (|| {
            if entry.metadata()?.uid() != unsafe { libc::geteuid() } {
                return Ok(false);
            }
            Ok(descriptors(pid)?
                .iter()
                .any(|(_, path)| pipes.contains(path.to_string_lossy().as_ref())))
        })();
        match inspect {
            Ok(true) => return Ok(false),
            Ok(false) => {}
            Err(e) => {
                let exited = fs::read_to_string(entry.path().join("stat")).is_ok_and(|s| {
                    s.rsplit_once(')')
                        .and_then(|(_, v)| v.split_whitespace().next())
                        .is_some_and(|s| matches!(s, "Z" | "X"))
                });
                if e.kind() == io::ErrorKind::NotFound
                    || exited
                    || e.kind() == io::ErrorKind::PermissionDenied
                        && authentication_service(&entry.path())
                {
                    continue;
                }
                return Err(e);
            }
        }
    }
    Ok(true)
}

fn tcp_idle(
    data: &str,
    sockets: &BTreeSet<String>,
    fixtures: bool,
    found: &mut BTreeSet<String>,
) -> bool {
    let entries: Vec<_> = data
        .lines()
        .skip(1)
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .collect();
    let listening: BTreeSet<_> = entries
        .iter()
        .filter(|f| f.len() >= 10 && sockets.contains(f[9]) && f[3] == "0A")
        .filter_map(|f| f[1].split_once(':').map(|(_, port)| port))
        .collect();
    for f in entries
        .iter()
        .filter(|f| f.len() >= 10 && sockets.contains(f[9]))
    {
        found.insert(f[9].into());
        if f[3] == "0A" && f[4] != "00000000:00000000" {
            return false;
        }
        if matches!(f[3], "0A" | "07") {
            continue;
        }
        // Test-only upstream connections may stay open after the controller dies.
        // Incoming clients still protect the fixture; CPU and I/O must stay quiet.
        if !fixtures
            || !matches!(f[3], "01" | "08")
            || f[1]
                .split_once(':')
                .is_none_or(|(_, port)| listening.contains(port))
            || f[3] == "01" && f[4] != "00000000:00000000"
        {
            return false;
        }
    }
    true
}
fn unix_idle(data: &str, sockets: &BTreeSet<String>, found: &mut BTreeSet<String>) -> bool {
    for line in data.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        if f.len() < 8 || !sockets.contains(f[5]) {
            continue;
        }
        found.insert(f[5].into());
        if f[2] != "0"
            || f[1] != "LISTEN" && f[3] != "0"
            || !matches!(f[1], "LISTEN" | "ESTAB" | "UNCONN")
            || f[1] == "ESTAB" && !sockets.contains(f[7])
        {
            return false;
        }
    }
    true
}

pub(super) fn disconnected(ids: &BTreeSet<u32>, fixtures: bool) -> io::Result<bool> {
    let mut sockets = BTreeSet::new();
    let mut pipes = BTreeSet::new();
    for pid in ids {
        let fds = descriptors(*pid)?;
        if fixtures
            && (fds
                .iter()
                .find(|(fd, _)| *fd == 0)
                .is_none_or(|(_, path)| path != Path::new("/dev/null"))
                || [1, 2].iter().any(|n| {
                    fds.iter()
                        .find(|(fd, _)| fd == n)
                        .is_none_or(|(_, p)| !p.to_string_lossy().starts_with("pipe:["))
                }))
        {
            return Ok(false);
        }
        for (fd, path) in fds {
            let s = path.to_string_lossy();
            if let Some(inode) = s.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')) {
                sockets.insert(inode.to_owned());
            }
            if s.starts_with("pipe:[") {
                pipes.insert(s.into_owned());
            } else if fd <= 2 && s.starts_with("/dev/") && path != Path::new("/dev/null") {
                return Ok(false);
            }
        }
    }
    if !closed_controller_pipes(ids, &pipes)? {
        return Ok(false);
    }
    let mut found = BTreeSet::new();
    for pid in ids {
        for name in ["tcp", "tcp6"] {
            let data = fs::read_to_string(format!("/proc/{pid}/net/{name}"))?;
            if !tcp_idle(&data, &sockets, fixtures, &mut found) {
                return Ok(false);
            }
        }
    }
    let unix = super::text(Command::new("ss").args(["-xapnH"]))?;
    if !unix_idle(&unix, &sockets, &mut found) {
        return Ok(false);
    }
    // Unknown/UDP sockets and other network namespaces fail closed.
    Ok(sockets.is_subset(&found))
}

pub(super) fn signal(p: &Process, signal: i32) -> io::Result<bool> {
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, p.pid, 0) };
    if raw < 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(false)
        } else {
            Err(error)
        };
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
    if identity(p.pid).as_deref() != Some(&p.identity)
        || fs::read_link(format!("/proc/{}/exe", p.pid))
            .ok()
            .as_deref()
            != Some(Path::new(&p.executable))
        || fs::metadata(format!("/proc/{}", p.pid))?.uid() != p.uid
    {
        return Ok(false);
    }
    if unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    } == 0
    {
        Ok(true)
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(false)
        } else {
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::Config;
    use super::super::processes::{harvest, inventory};
    use super::*;
    use std::collections::BTreeMap;
    use std::net::{TcpListener, TcpStream};
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    fn mock(args: &str) -> Process {
        Process {
            pid: 99,
            parent: 1,
            uid: unsafe { libc::geteuid() },
            age_seconds: 7200,
            cpu_seconds: 0.0,
            identity: "fake".into(),
            executable: "/usr/bin/python3.12".into(),
            arguments: args.into(),
        }
    }
    #[test]
    fn fixtures_need_exact_test_flags_and_temporary_paths() {
        assert_eq!(
            category(&mock(
                "python3 -u -m poe_proxy.unix_server /tmp/tmpabc/poeproxy.sock --test-mode"
            )),
            Some("Poe test proxy")
        );
        for args in [
            "python3 -m poe_proxy.unix_server /tmp/tmpabc/poeproxy.sock",
            "python3 -m poe_proxy.unix_server /srv/poeproxy.sock --test-mode",
            "python3 -c '-m poe_proxy.unix_server /tmp/tmpabc/poeproxy.sock --test-mode'",
            "python3 -u -m aipoe 1234 --per-process-port 4567",
        ] {
            assert_eq!(category(&mock(args)), None, "{args}");
        }
        assert_eq!(
            category(&mock(
                "/tmp/tmpabc/venv/bin/python /repo/scripts/catbot.py --port 1234"
            )),
            Some("Temporary catbot test server")
        );
        assert_eq!(
            category(&mock(
                "/repo/venv/bin/python /repo/scripts/catbot.py --port 1234"
            )),
            None
        );
        let mut browser = mock(
            "chrome --headless --test-type=webdriver --user-data-dir=/tmp/.com.google.Chrome.abc",
        );
        browser.executable = "/opt/google/chrome/chrome".into();
        assert_eq!(category(&browser), Some("Selenium test browser"));
        browser.arguments = "chrome --headless --user-data-dir=/home/user/personal".into();
        assert_eq!(category(&browser), None);
    }
    #[test]
    fn external_clients_and_unknown_sockets_are_protected() {
        let sockets = BTreeSet::from(["111".into(), "222".into()]);
        let mut found = BTreeSet::new();
        let listening = "header\n0: 0100007F:1234 00000000:0000 0A 00000000:00000000 x x x x 111\n";
        let incoming =
            format!("{listening}1: 0100007F:1234 0100007F:5678 01 00000000:00000000 x x x x 222\n");
        assert!(!tcp_idle(&incoming, &sockets, true, &mut found));
        let outgoing =
            format!("{listening}1: 0100007F:5678 0100007F:9999 01 00000000:00000000 x x x x 222\n");
        assert!(tcp_idle(&outgoing, &sockets, true, &mut found));
        assert!(!tcp_idle(&outgoing, &sockets, false, &mut found));
        assert!(!unix_idle(
            "u_str ESTAB 0 0 * 111 * 333",
            &sockets,
            &mut found
        ));
        assert!(unix_idle(
            "u_str LISTEN 0 100 /tmp/socket 111 * 0",
            &sockets,
            &mut found
        ));
        assert!(!unix_idle(
            "u_str LISTEN 1 100 /tmp/socket 111 * 0",
            &sockets,
            &mut found
        ));
        assert!(unix_idle(
            "u_str ESTAB 0 0 * 111 * 222",
            &sockets,
            &mut found
        ));
        assert!(!unix_idle(
            "u_str ESTAB 1 0 * 111 * 222",
            &sockets,
            &mut found
        ));
    }
    #[test]
    fn real_dead_pipes_are_reaped_but_clients_controllers_and_io_survive() {
        let root = std::env::temp_dir().join(format!("tmphbfixture{}", std::process::id()));
        let directory = root.join("poe/graphql-server");
        fs::create_dir_all(&directory).unwrap();
        let bin = root.join("python3");
        let source = root.join("fixture.c");
        fs::write(
            &source,
            r#"
#include <unistd.h>
#include <stdio.h>
#include <stdlib.h>
#include <signal.h>
#include <sys/socket.h>
#include <netinet/in.h>
int main(void) {
    for (int fd = 3; fd < 1024; ++fd) close(fd);
    pid_t p = fork(); if(p<0) return 2; if(p>0) return 0;
    setsid(); signal(SIGPIPE,SIG_IGN); alarm(45);
    int fd=socket(AF_INET,SOCK_STREAM,0); struct sockaddr_in a={0};
    a.sin_family=AF_INET; a.sin_addr.s_addr=htonl(INADDR_LOOPBACK);
    if(bind(fd,(struct sockaddr*)&a,sizeof(a)) || listen(fd,4)) return 3;
    socklen_t len=sizeof(a); getsockname(fd,(struct sockaddr*)&a,&len);
    if(getenv("UPSTREAM")) {
        int out=socket(AF_INET,SOCK_STREAM,0); struct sockaddr_in b=a;
        b.sin_port=htons(atoi(getenv("UPSTREAM")));
        if(connect(out,(struct sockaddr*)&b,sizeof(b))) return 4;
    }
    FILE *f=fopen(getenv("INFO"),"w"); fprintf(f,"%d %d",getpid(),ntohs(a.sin_port)); fclose(f);
    if(getenv("BUSY")) { for(;;) { write(1,"x",1); usleep(10000); } }
    accept(fd,0,0); for(;;) pause();
}
"#,
        )
        .unwrap();
        assert!(
            Command::new("cc")
                .arg(&source)
                .arg("-o")
                .arg(&bin)
                .status()
                .unwrap()
                .success()
        );
        struct Fixture {
            p: Process,
            child: Child,
            port: u16,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = signal(&self.p, libc::SIGKILL);
                let _ = self.child.wait();
            }
        }
        let launch = |name: &str, keep_reader: bool, upstream: Option<u16>, busy: bool| {
            let info = root.join(name);
            let mut cmd = Command::new(&bin);
            cmd.args([
                "-u",
                "-m",
                "poe_proxy.unix_server",
                root.join("poeproxy.sock").to_str().unwrap(),
                "--test-mode",
            ])
            .current_dir(&directory)
            .env("INFO", &info)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
            if let Some(port) = upstream {
                cmd.env("UPSTREAM", port.to_string());
            }
            if busy {
                cmd.env("BUSY", "1");
            }
            let mut child = cmd.spawn().unwrap();
            child.wait().unwrap();
            if !keep_reader {
                drop(child.stdout.take());
                drop(child.stderr.take());
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            let fields = loop {
                if let Ok(s) = fs::read_to_string(&info) {
                    let parts: Vec<_> = s.split_whitespace().map(str::to_owned).collect();
                    if parts.len() == 2 {
                        break parts;
                    }
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(20));
            };
            let pid = fields[0].parse().unwrap();
            let p = inventory().unwrap().remove(&pid).unwrap();
            Fixture {
                p,
                child,
                port: fields[1].parse().unwrap(),
            }
        };
        let quiet = launch("quiet", false, None, false);
        let controlled = launch("controlled", true, None, false);
        let connected = launch("connected", false, None, false);
        let _client = TcpStream::connect(("127.0.0.1", connected.port)).unwrap();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let outgoing = launch(
            "outgoing",
            false,
            Some(upstream.local_addr().unwrap().port()),
            false,
        );
        let _upstream_client = upstream.accept().unwrap();
        let busy = launch("busy", false, None, true);
        let ids = |p: &Process| BTreeSet::from([p.pid]);
        assert!(disconnected(&ids(&quiet.p), true).unwrap());
        assert!(!disconnected(&ids(&controlled.p), true).unwrap());
        assert!(!disconnected(&ids(&connected.p), true).unwrap());
        assert!(disconnected(&ids(&outgoing.p), true).unwrap());
        assert!(!disconnected(&ids(&outgoing.p), false).unwrap());
        let before = activity_fingerprint(&ids(&busy.p)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_ne!(before, activity_fingerprint(&ids(&busy.p)).unwrap());
        let table = Table::from(
            [
                quiet.p.clone(),
                controlled.p.clone(),
                connected.p.clone(),
                busy.p.clone(),
            ]
            .map(|p| (p.pid, p)),
        );
        let config = Config {
            process_min_age_seconds: 0,
            observation_seconds: 0,
            ..Config::default()
        };
        let mut observations = BTreeMap::new();
        assert_eq!(
            harvest(&table, &config, &mut observations, true).unwrap().1,
            0
        );
        let (items, count) = harvest(&table, &config, &mut observations, true).unwrap();
        assert_eq!(count, 1, "{items:?}");
        assert!(fs::read_link(format!("/proc/{}/exe", quiet.p.pid)).is_err());
        assert!(fs::read_link(format!("/proc/{}/exe", controlled.p.pid)).is_ok());
        assert!(fs::read_link(format!("/proc/{}/exe", connected.p.pid)).is_ok());
        assert!(fs::read_link(format!("/proc/{}/exe", busy.p.pid)).is_ok());
        drop((quiet, controlled, connected, outgoing, busy));
        fs::remove_dir_all(root).unwrap();
    }
}
