#[cfg(not(target_os = "linux"))]
use super::output;
use super::{Config, Item, Observation, now, text};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub parent: u32,
    pub uid: u32,
    pub age_seconds: u64,
    pub cpu_seconds: f64,
    pub executable: String,
    pub identity: String,
    #[serde(skip)]
    pub arguments: String,
}
pub type Table = BTreeMap<u32, Process>;

/// Read-only UI inventory. These rows never authorize signals or cleanup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayProcess {
    pub pid: u32,
    pub parent: u32,
    pub age_seconds: u64,
    pub cpu_percent: f64,
    pub resident_bytes: u64,
    pub executable: String,
}

pub fn display_inventory() -> io::Result<Vec<DisplayProcess>> {
    // comm, not args: do not expose command-line credentials in the UI or JSON.
    let data = text(
        Command::new("ps")
            .env("LC_ALL", "C")
            .args(["-axo", "pid=,ppid=,uid=,etime=,pcpu=,rss=,comm="]),
    )?;
    let rows = parse_display_inventory(&data, unsafe { libc::geteuid() });
    if rows.is_empty() {
        return Err(io::Error::other("Current-user process list unavailable"));
    }
    Ok(rows)
}

fn parse_display_inventory(data: &str, uid: u32) -> Vec<DisplayProcess> {
    let mut rows: Vec<_> = data
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            if fields.next()?.parse::<u32>().ok()? != uid {
                return None;
            }
            let age_seconds = duration(fields.next()?)? as u64;
            let cpu_percent: f64 = fields.next()?.parse().ok()?;
            if !cpu_percent.is_finite() || cpu_percent < 0.0 {
                return None;
            }
            let resident_bytes = fields.next()?.parse::<u64>().ok()?.checked_mul(1024)?;
            let executable = fields.collect::<Vec<_>>().join(" ");
            if executable.is_empty() {
                return None;
            }
            Some(DisplayProcess {
                pid,
                parent,
                age_seconds,
                cpu_percent,
                resident_bytes,
                executable,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b.resident_bytes
            .cmp(&a.resident_bytes)
            .then(a.pid.cmp(&b.pid))
    });
    rows
}

fn duration(value: &str) -> Option<f64> {
    let (days, time) = value
        .split_once('-')
        .map_or((0.0, value), |(d, t)| (d.parse().unwrap_or(0.0), t));
    time.split(':')
        .try_fold(0.0, |sum, field| {
            Some(sum * 60.0 + field.parse::<f64>().ok()?)
        })
        .map(|s| s + days * 86400.0)
}
pub fn inventory() -> io::Result<Table> {
    let data = text(Command::new("ps").args(["-axo", "pid=,ppid=,uid=,etime=,time=,comm="]))?;
    let args = text(Command::new("ps").args(["-axo", "pid=,args="]))?;
    let arguments: BTreeMap<u32, String> = args
        .lines()
        .filter_map(|s| {
            let s = s.trim_start();
            let (pid, args) = s.split_once(char::is_whitespace)?;
            Some((pid.parse().ok()?, args.trim_start().to_owned()))
        })
        .collect();
    let mut table = Table::new();
    for line in data.lines() {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(parent) = fields.next().and_then(|s| s.parse().ok()) else {
            continue;
        };
        let Some(uid) = fields.next().and_then(|s| s.parse().ok()) else {
            continue;
        };
        let Some(age) = fields.next().and_then(duration) else {
            continue;
        };
        let Some(cpu) = fields.next().and_then(duration) else {
            continue;
        };
        let reported = fields.collect::<Vec<_>>().join(" ");
        let executable = executable(pid).unwrap_or(reported);
        let Some(identity) = identity(pid) else {
            continue;
        };
        table.insert(
            pid,
            Process {
                pid,
                parent,
                uid,
                age_seconds: age as u64,
                cpu_seconds: cpu,
                executable,
                identity,
                arguments: arguments.get(&pid).cloned().unwrap_or_default(),
            },
        );
    }
    if table.is_empty() {
        return Err(io::Error::other(
            "Process inventory unavailable; cleanup skipped",
        ));
    }
    Ok(table)
}

#[cfg(target_os = "macos")]
fn executable(pid: u32) -> Option<String> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let count =
        unsafe { libc::proc_pidpath(pid as i32, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if count <= 0 {
        return None;
    }
    let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
    String::from_utf8(buffer[..end].to_vec()).ok()
}
#[cfg(not(target_os = "macos"))]
fn executable(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
pub fn identity(pid: u32) -> Option<String> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    if unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    } != size
    {
        return None;
    }
    let info = unsafe { info.assume_init() };
    Some(format!(
        "{}:{}:{}",
        info.pbi_uid, info.pbi_start_tvsec, info.pbi_start_tvusec
    ))
}
#[cfg(not(target_os = "macos"))]
pub fn identity(pid: u32) -> Option<String> {
    let data = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tick = data.rsplit_once(')')?.1.split_whitespace().nth(19)?;
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(format!("{}:{tick}", boot.trim()))
}

fn category(p: &Process) -> Option<&'static str> {
    #[cfg(target_os = "linux")]
    if let Some(kind) = super::linux_harvest::category(p) {
        return Some(kind);
    }
    let name = Path::new(&p.executable).file_name()?.to_str()?;
    // No Codex, Claude, shell, generic Node/Python, GUI app, or service targeting.
    if name == "workerd"
        && p.executable.contains("/node_modules/")
        && p.executable.contains("/workerd-")
        && p.arguments.contains(" serve ")
        && p.arguments.contains(" --control-fd=3 ")
    {
        Some("Cloudflare test worker")
    } else if p.executable.contains("/.wrangler/chrome/")
        && (name == "Google Chrome for Testing" || name == "chrome")
        && p.arguments.contains(" --headless")
        && p.arguments.contains("/miniflare-")
        && p.arguments.contains("/browser-rendering/profile-")
        && p.arguments.contains(" --user-data-dir=")
    {
        Some("Cloudflare test browser")
    } else if p.executable.contains("/.wrangler/chrome/") && name == "chrome_crashpad_handler" {
        Some("Cloudflare crash helper")
    } else if name == "dns-sd" && p.arguments.ends_with(" -B _ucp._tcp local.") {
        Some("Abandoned discovery command")
    } else if matches!(
        name,
        "Python" | "python" | "python3" | "python3.14" | "python3.13" | "python3.12"
    ) && p.arguments.contains("/hey-boss-test-")
        && p.arguments.contains("/action-http.py ")
    {
        Some("Hey Boss HTTP test fixture")
    } else {
        None
    }
}

fn browser(p: &Process) -> bool {
    matches!(
        category(p),
        Some("Cloudflare test browser" | "Selenium test browser" | "Cloudflare crash helper")
    )
}

fn minimum_age(p: &Process, config: &Config) -> u64 {
    if browser(p) {
        config.browser_min_age_seconds
    } else {
        config.process_min_age_seconds
    }
}

fn descendants(root: u32, table: &Table) -> BTreeSet<u32> {
    let mut result = BTreeSet::from([root]);
    loop {
        let children: Vec<u32> = table
            .values()
            .filter(|p| result.contains(&p.parent))
            .map(|p| p.pid)
            .collect();
        let old = result.len();
        result.extend(children);
        if old == result.len() {
            return result;
        }
    }
}
fn tree(root: &Process, table: &Table, uid: u32, min_age: u64) -> Option<BTreeSet<u32>> {
    if root.uid != uid
        || root.parent != 1
        || root.age_seconds < min_age
        || root.pid == std::process::id()
    {
        return None;
    }
    let category = category(root)?;
    #[cfg(target_os = "linux")]
    if matches!(category, "Poe test proxy" | "Temporary catbot test server") {
        return super::linux_harvest::fixture_family(root, table);
    }
    let ids = descendants(root.pid, table);
    for pid in &ids {
        let p = &table[pid];
        if p.uid != uid
            || (*pid != root.pid
                && !(category == "Selenium test browser"
                    && matches!(
                        p.executable.as_str(),
                        "/opt/google/chrome/chrome" | "/usr/bin/cat"
                    ))
                && !(category == "Cloudflare test browser"
                    && p.executable.contains("/.wrangler/chrome/")
                    && p.executable.starts_with(
                        root.executable
                            .split("/chrome-mac")
                            .next()
                            .unwrap_or(&root.executable),
                    )))
        {
            return None;
        }
    }
    Some(ids)
}

#[cfg(target_os = "linux")]
fn disconnected(root: &Process, ids: &BTreeSet<u32>) -> io::Result<bool> {
    if matches!(
        category(root),
        Some(
            "Cloudflare test browser"
                | "Selenium test browser"
                | "Poe test proxy"
                | "Temporary catbot test server"
        )
    ) {
        super::linux_harvest::disconnected(ids, !browser(root))
    } else {
        super::linux::disconnected(ids)
    }
}

fn activity_fingerprint(ids: &BTreeSet<u32>) -> io::Result<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        super::linux_harvest::activity_fingerprint(ids).map(Some)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = ids;
        Ok(None)
    }
}

#[cfg(not(target_os = "linux"))]
fn lsof(args: &[&str]) -> io::Result<String> {
    let o = output(
        Command::new("lsof").args(["-nP"]).args(args),
        Duration::from_secs(15),
    )?;
    // lsof exits 1 for an empty selection. Any diagnostic is uncertainty, not idle.
    if !o.stderr.is_empty() || !(o.status.success() || o.status.code() == Some(1)) {
        return Err(io::Error::other(
            "Cannot verify process connections; preserving processes",
        ));
    }
    String::from_utf8(o.stdout).map_err(io::Error::other)
}
#[cfg(not(target_os = "linux"))]
fn disconnected(root: &Process, ids: &BTreeSet<u32>) -> io::Result<bool> {
    let list = ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let network = lsof(&["-a", "-p", &list, "-i", "-F", "pftnT"])?;
    if network.lines().any(|s| s == "tIPv4" || s == "tIPv6") {
        // Only listening TCP sockets or fully closed ones are idle; UDP is protected.
        for record in network
            .split('\n')
            .collect::<Vec<_>>()
            .split(|s| s.starts_with('f'))
        {
            if record
                .iter()
                .any(|s| s.starts_with('t') && (s.contains("IPv4") || s.contains("IPv6")))
                && !record
                    .iter()
                    .any(|s| matches!(*s, "TST=LISTEN" | "TST=CLOSED"))
            {
                return Ok(false);
            }
        }
    }
    if category(root) == Some("Cloudflare crash helper") {
        // Crash helpers are independent processes. Never associate them by launch time:
        // another browser started simultaneously may still own a connected helper.
        let ipc = lsof(&["-a", "-p", &root.pid.to_string(), "-U", "-F", "pftn"])?;
        if ipc
            .lines()
            .any(|s| s.starts_with("n->") && s != "n->(none)")
        {
            return Ok(false);
        }
    }
    let descriptors = if category(root) == Some("Cloudflare test worker") {
        "0,1,2,3"
    } else {
        "0,1,2"
    };
    let stdio = lsof(&[
        "-a",
        "-p",
        &root.pid.to_string(),
        "-d",
        descriptors,
        "-F",
        "pftn",
    ])?;
    if stdio.is_empty() {
        return Ok(false);
    }
    // Connected controller sockets, terminals and unnamed pipes can still carry work.
    if stdio.lines().any(|s| {
        s.starts_with("n->") && s != "n->(none)"
            || s.starts_with("n/dev/tt")
            || s == "tPIPE"
            || s == "tFIFO"
    }) {
        return Ok(false);
    }
    Ok(true)
}

struct QuietWindow {
    grace: u64,
    max_gap: u64,
    strict_cpu: bool,
}

fn observed(
    map: &mut BTreeMap<String, Observation>,
    key: &str,
    cpu: f64,
    at: u64,
    activity: Option<String>,
    window: QuietWindow,
) -> bool {
    let previous = map.get(key);
    let continuous = previous.is_some_and(|p| {
        at >= p.last_seen
            && at - p.last_seen <= window.max_gap
            && cpu >= p.cpu_seconds
            && p.activity_fingerprint == activity
            && cpu - p.cpu_seconds
                <= if window.strict_cpu {
                    0.1
                } else {
                    (at - p.last_seen) as f64 * 0.02 + 0.1
                }
    });
    let first = if continuous {
        previous.unwrap().first_seen
    } else {
        at
    };
    map.insert(
        key.into(),
        Observation {
            first_seen: first,
            last_seen: at,
            cpu_seconds: cpu,
            activity_fingerprint: activity,
        },
    );
    continuous && at.saturating_sub(first) >= window.grace
}

/// Signal only a freshly reidentified process. Never signal a process group.
fn signal(p: &Process, signal: i32) -> io::Result<bool> {
    #[cfg(target_os = "linux")]
    {
        super::linux_harvest::signal(p, signal)
    }
    #[cfg(not(target_os = "linux"))]
    {
        if identity(p.pid).as_deref() != Some(&p.identity)
            || executable(p.pid).as_deref() != Some(&p.executable)
        {
            return Ok(false);
        }
        if unsafe { libc::kill(p.pid as i32, signal) } == 0 {
            Ok(true)
        } else {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ESRCH) {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}
fn terminate(
    root: &Process,
    ids: &BTreeSet<u32>,
    old: &Table,
    config: &Config,
    activity: &Option<String>,
) -> io::Result<usize> {
    let fresh = inventory()?;
    let Some(current) = fresh.get(&root.pid) else {
        return Ok(0);
    };
    if current.identity != root.identity
        || tree(current, &fresh, root.uid, minimum_age(current, config)).as_ref() != Some(ids)
        || !disconnected(current, ids)?
        || activity_fingerprint(ids)? != *activity
    {
        return Ok(0);
    }
    for pid in ids {
        if fresh[pid].identity != old[pid].identity
            || fresh[pid].cpu_seconds - old[pid].cpu_seconds > 0.1
        {
            return Ok(0);
        }
    }
    signal(current, libc::SIGTERM)?;
    std::thread::sleep(Duration::from_millis(500));
    for pid in ids {
        signal(&old[pid], libc::SIGTERM)?;
    }
    std::thread::sleep(Duration::from_millis(1000));
    for pid in ids {
        signal(&old[pid], libc::SIGKILL)?;
    }
    std::thread::sleep(Duration::from_millis(100));
    Ok(ids
        .iter()
        .filter(|pid| {
            identity(**pid).as_deref() != Some(&old[pid].identity) || executable(**pid).is_none()
        })
        .count())
}

pub fn harvest(
    table: &Table,
    config: &Config,
    observations: &mut BTreeMap<String, Observation>,
    apply: bool,
) -> io::Result<(Vec<Item>, usize)> {
    let uid = unsafe { libc::geteuid() };
    let at = now();
    let mut retained = BTreeSet::new();
    let mut items = Vec::new();
    let mut killed = 0;
    for root in table.values() {
        let Some(ids) = tree(root, table, uid, minimum_age(root, config)) else {
            continue;
        };
        let key = ids
            .iter()
            .map(|pid| format!("{pid}:{}", table[pid].identity))
            .collect::<Vec<_>>()
            .join("/");
        let inspection = disconnected(root, &ids).and_then(|idle| {
            Ok((
                idle,
                if idle {
                    activity_fingerprint(&ids)?
                } else {
                    None
                },
            ))
        });
        let (idle, activity) = match inspection {
            Ok(result) => result,
            Err(error) => {
                observations.remove(&key);
                items.push(Item {
                    name: format!("{} · PID {}", category(root).unwrap(), root.pid),
                    detail: format!("Inspection incomplete; preserved: {error}"),
                    eligible: false,
                    worktree: None,
                });
                continue;
            }
        };
        let cpu = ids.iter().map(|pid| table[pid].cpu_seconds).sum();
        let ready = idle
            && observed(
                observations,
                &key,
                cpu,
                at,
                activity.clone(),
                QuietWindow {
                    grace: config.observation_seconds,
                    max_gap: config.interval_seconds.saturating_mul(3),
                    strict_cpu: matches!(
                        category(root),
                        Some("Poe test proxy" | "Temporary catbot test server")
                    ),
                },
            );
        if idle {
            retained.insert(key.clone());
        }
        let mut detail = if !idle {
            "Connected or in use; preserved"
        } else if ready {
            "Owner exited; no clients; quiet across repeated checks"
        } else {
            "Owner exited; observing before cleanup"
        }
        .to_owned();
        if ready && apply {
            match terminate(root, &ids, table, config, &activity) {
                Ok(count) => {
                    killed += count;
                    detail = if count == 0 {
                        "Changed during final check; preserved".into()
                    } else {
                        format!("Stopped {count} of {} processes", ids.len())
                    };
                }
                Err(error) => {
                    detail = format!("Cleanup interrupted: {error}");
                }
            }
            observations.remove(&key);
        }
        items.push(Item {
            name: format!("{} · PID {}", category(root).unwrap(), root.pid),
            detail,
            eligible: ready,
            worktree: None,
        });
    }
    observations.retain(|key, _| retained.contains(key));
    Ok((items, killed))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_includes_normal_apps_without_making_them_candidates() {
        let rows = parse_display_inventory(
            "10 1 501 2-03:04:05 120.5 8192 /Applications/Codex Helper (Renderer)\n11 1 0 10:00 0.0 99999 system-service\n12 10 501 01:02 0.0 1024 /bin/zsh\nmalformed\n",
            501,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pid, 10);
        assert_eq!(rows[0].age_seconds, 183845);
        assert_eq!(rows[0].resident_bytes, 8 * 1024 * 1024);
        assert_eq!(rows[0].cpu_percent, 120.5);
        assert_eq!(rows[0].executable, "/Applications/Codex Helper (Renderer)");
        assert_eq!(rows[1].age_seconds, 62);
        let live = display_inventory().unwrap();
        assert!(live.iter().any(|p| p.pid == std::process::id()));
    }
    fn worker() -> Process {
        Process {
            pid: 100,
            parent: 1,
            uid: 501,
            age_seconds: 7200,
            cpu_seconds: 2.0,
            executable: "/repo/node_modules/@cloudflare/workerd-darwin-arm64/bin/workerd".into(),
            identity: "501:100:10".into(),
            arguments: "workerd serve --control-fd=3 -".into(),
        }
    }
    #[test]
    fn only_known_orphans_without_unexpected_children_are_candidates() {
        let mut p = worker();
        let mut table = Table::from([(p.pid, p.clone())]);
        assert!(tree(&p, &table, 501, 3600).is_some());
        p.parent = 42;
        assert!(tree(&p, &table, 501, 3600).is_none());
        p.parent = 1;
        assert!(tree(&p, &table, 999, 3600).is_none());
        assert!(tree(&p, &table, 501, 9000).is_none());
        let mut child = p.clone();
        child.pid = 101;
        child.parent = 100;
        child.executable = "/usr/bin/git".into();
        table.insert(101, child);
        assert!(tree(&p, &table, 501, 3600).is_none());
        for name in [
            "/usr/bin/python3",
            "/bin/zsh",
            "/bin/codex",
            "/Applications/Codex.app/Contents/MacOS/Codex",
            "/usr/bin/node",
        ] {
            p.executable = name.into();
            assert!(category(&p).is_none());
        }
    }
    #[test]
    fn requires_continuous_quiet_observations_and_resets_after_activity_or_restart() {
        let mut map = BTreeMap::new();
        assert!(!observed(
            &mut map,
            "pid:start",
            1.0,
            1000,
            None,
            QuietWindow {
                grace: 300,
                max_gap: 900,
                strict_cpu: false
            }
        ));
        assert!(observed(
            &mut map,
            "pid:start",
            1.2,
            1300,
            None,
            QuietWindow {
                grace: 300,
                max_gap: 900,
                strict_cpu: false
            }
        ));
        assert!(!observed(
            &mut map,
            "pid:start",
            50.0,
            1600,
            None,
            QuietWindow {
                grace: 300,
                max_gap: 900,
                strict_cpu: false
            }
        ));
        assert!(!observed(
            &mut map,
            "pid:new",
            0.0,
            1900,
            None,
            QuietWindow {
                grace: 300,
                max_gap: 900,
                strict_cpu: false
            }
        ));
        assert!(!observed(
            &mut map,
            "pid:start",
            50.0,
            4000,
            None,
            QuietWindow {
                grace: 300,
                max_gap: 900,
                strict_cpu: false
            }
        ));
    }
    #[test]
    fn reused_identity_cannot_signal_a_live_child() {
        let mut child = Command::new("sleep").arg("20").spawn().unwrap();
        let mut p = worker();
        p.uid = unsafe { libc::geteuid() };
        p.pid = child.id();
        p.identity = "wrong-start-time".into();
        assert!(!signal(&p, libc::SIGTERM).unwrap());
        assert!(child.try_wait().unwrap().is_none());
        p.identity = identity(child.id()).unwrap();
        p.executable = executable(child.id()).unwrap();
        assert!(signal(&p, libc::SIGTERM).unwrap());
        child.wait().unwrap();
    }
    #[test]
    fn elapsed_and_cpu_parsing() {
        assert_eq!(duration("2-03:04:05"), Some(183845.0));
        assert_eq!(duration("1:02.50"), Some(62.5));
    }

    #[test]
    fn browsers_have_shorter_age_but_io_and_cpu_activity_reset_the_grace_period() {
        let config = Config::default();
        let mut p = worker();
        assert_eq!(minimum_age(&p, &config), 3600);
        p.executable = "/cache/.wrangler/chrome/mac/chrome-mac/Google Chrome for Testing".into();
        p.arguments =
            "chrome --headless --user-data-dir=/tmp/miniflare-abc/browser-rendering/profile-1"
                .into();
        assert_eq!(minimum_age(&p, &config), 600);
        p.age_seconds = 599;
        assert!(
            tree(
                &p,
                &Table::from([(p.pid, p.clone())]),
                p.uid,
                minimum_age(&p, &config)
            )
            .is_none()
        );
        p.age_seconds = 600;
        assert!(
            tree(
                &p,
                &Table::from([(p.pid, p.clone())]),
                p.uid,
                minimum_age(&p, &config)
            )
            .is_some()
        );
        let mut observations = BTreeMap::new();
        let window = || QuietWindow {
            grace: 300,
            max_gap: 900,
            strict_cpu: true,
        };
        assert!(!observed(
            &mut observations,
            "p",
            1.0,
            1000,
            Some("read:1".into()),
            window()
        ));
        assert!(!observed(
            &mut observations,
            "p",
            1.0,
            1300,
            Some("read:2".into()),
            window()
        ));
        assert!(observed(
            &mut observations,
            "p",
            1.0,
            1600,
            Some("read:2".into()),
            window()
        ));
        assert!(!observed(
            &mut observations,
            "p",
            1.5,
            1900,
            Some("read:2".into()),
            window()
        ));
    }

    #[test]
    fn real_orphan_is_reaped_but_connected_and_owned_workers_survive() {
        use std::io::Read;
        use std::net::TcpStream;
        use std::process::Stdio;
        let root = std::env::temp_dir().join(format!("hb-health-process-{}", std::process::id()));
        let bin = root.join("node_modules/@cloudflare/workerd-darwin-arm64/bin/workerd");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        let source = root.join("worker.c");
        std::fs::write(&source, r#"
#include <unistd.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <fcntl.h>
#include <signal.h>
#include <sys/socket.h>
#include <netinet/in.h>
int main(int argc, char **argv) {
    int owned = argc > 4 && strcmp(argv[4], "owned") == 0;
    if (!owned) { pid_t p = fork(); if (p < 0) return 2; if (p > 0) return 0; setsid(); }
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in address = {0}; address.sin_family = AF_INET; address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (bind(fd, (struct sockaddr *)&address, sizeof(address)) || listen(fd, 4)) return 3;
    socklen_t len = sizeof(address); getsockname(fd, (struct sockaddr *)&address, &len);
    printf("%d %d\n", getpid(), ntohs(address.sin_port)); fflush(stdout);
    int null = open("/dev/null", O_RDWR); dup2(null, 0); dup2(null, 1); dup2(null, 2); close(null);
    // Do not put the listening socket on the workerd control descriptor.
    if (fd == 3) { int moved = dup(fd); close(fd); fd = moved; }
    alarm(90);
    int connection = accept(fd, NULL, NULL);
    (void)connection;
    for (;;) pause();
}
"#).unwrap();
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
            process: Process,
            child: std::process::Child,
            port: u16,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = signal(&self.process, libc::SIGKILL);
                let _ = self.child.wait();
            }
        }
        let launch = |owned: bool| {
            let mut child = Command::new(&bin)
                .args([
                    "serve",
                    "--control-fd=3",
                    "-",
                    if owned { "owned" } else { "orphan" },
                ])
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
            let mut fields = response.split_whitespace();
            let pid = fields.next().unwrap().parse::<u32>().unwrap();
            let port = fields.next().unwrap().parse().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            let p = inventory().unwrap().remove(&pid).unwrap();
            Fixture {
                process: p,
                child,
                port,
            }
        };
        let orphan = launch(false);
        let connected = launch(false);
        let owned = launch(true);
        let _client = TcpStream::connect(("127.0.0.1", connected.port)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(orphan.process.parent, 1);
        let table = Table::from(
            [
                orphan.process.clone(),
                connected.process.clone(),
                owned.process.clone(),
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
        assert!(identity(orphan.process.pid).is_none());
        assert_eq!(
            identity(connected.process.pid).as_deref(),
            Some(connected.process.identity.as_str())
        );
        assert_eq!(
            identity(owned.process.pid).as_deref(),
            Some(owned.process.identity.as_str())
        );
        drop(orphan);
        drop(connected);
        drop(owned);
        std::fs::remove_dir_all(root).unwrap();
    }
}
