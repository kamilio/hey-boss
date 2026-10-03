#[cfg(not(target_os = "linux"))]
use super::output;
use super::{Config, Item, Observation, now, text_within};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[cfg(target_os = "macos")]
mod scaleft;

mod services;

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

/// Listing every process is the gate for all cleanup. A busy Mac with thousands of
/// leaked browser processes made the 15-second default fail on every run, so nothing
/// was ever harvested and the leak kept growing.
const INVENTORY_TIMEOUT: Duration = Duration::from_secs(90);
#[cfg(not(target_os = "linux"))]
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);

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
    let data = text_within(
        Command::new("ps")
            .env("LC_ALL", "C")
            .args(["-axo", "pid=,ppid=,uid=,etime=,pcpu=,rss=,comm="]),
        INVENTORY_TIMEOUT,
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
    let data = text_within(
        Command::new("ps").args(["-axo", "pid=,ppid=,uid=,etime=,time=,comm="]),
        INVENTORY_TIMEOUT,
    )?;
    let args = text_within(
        Command::new("ps").args(["-axo", "pid=,args="]),
        INVENTORY_TIMEOUT,
    )?;
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
    #[cfg(target_os = "macos")]
    if scaleft::helper(p) {
        return Some(scaleft::CATEGORY);
    }
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
        && miniflare_options(&p.arguments)
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

fn miniflare_options(arguments: &str) -> bool {
    let options: Vec<_> = arguments.split_whitespace().collect();
    if !options
        .iter()
        .any(|s| matches!(*s, "--headless" | "--headless=new"))
        || options.iter().any(|s| s.starts_with("--type="))
    {
        return false;
    }
    let profiles: Vec<_> = options
        .iter()
        .filter_map(|s| s.strip_prefix("--user-data-dir="))
        .collect();
    let [profile] = profiles.as_slice() else {
        return false;
    };
    let path = Path::new(profile);
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }
    let Some(rendering) = path.parent() else {
        return false;
    };
    let Some(instance) = rendering.parent() else {
        return false;
    };
    let Some(temp) = instance.parent() else {
        return false;
    };
    (path.starts_with("/tmp")
        || path.starts_with("/private/tmp")
        || ((path.starts_with("/var/folders") || path.starts_with("/private/var/folders"))
            && temp.file_name().is_some_and(|name| name == "T")))
        && path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with("profile-") && s.len() > 8)
        && rendering
            .file_name()
            .is_some_and(|s| s == "browser-rendering")
        && instance
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with("miniflare-") && s.len() > 10)
}

fn browser(p: &Process) -> bool {
    matches!(
        category(p),
        Some("Cloudflare test browser" | "Selenium test browser" | "Cloudflare crash helper")
    )
}

/// Test fixtures must be nearly frozen; browsers and workers may tick idle timers.
fn strict_cpu(p: &Process) -> bool {
    matches!(
        category(p),
        Some("Poe test proxy" | "Temporary catbot test server" | "ScaleFT SSH helper")
    )
}

/// The final check allows the same 2% CPU budget as the quiet observations. An idle
/// headless browser burns a few hundred milliseconds a minute on timers; with a flat
/// 0.1-second limit every browser failed the final check and was preserved forever.
fn quiet_since(old: &Process, fresh: &Process, strict: bool) -> bool {
    if fresh.age_seconds < old.age_seconds
        || !old.cpu_seconds.is_finite()
        || !fresh.cpu_seconds.is_finite()
        || fresh.cpu_seconds < old.cpu_seconds
    {
        return false;
    }
    let elapsed = (fresh.age_seconds - old.age_seconds) as f64;
    let budget = if strict { 0.1 } else { elapsed * 0.02 + 0.1 };
    fresh.cpu_seconds - old.cpu_seconds <= budget
}

fn minimum_age(p: &Process, config: &Config) -> u64 {
    #[cfg(target_os = "macos")]
    if scaleft::helper(p) {
        return config.process_min_age_seconds.max(3600);
    }
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
        CONNECTION_TIMEOUT,
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
    #[cfg(target_os = "macos")]
    if scaleft::helper(root) {
        return scaleft::disconnected(root.pid);
    }
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
    if category(root) == Some("Cloudflare test browser") {
        return Ok(browser_connections_idle(root.pid, &network, &stdio));
    }
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

#[cfg(not(target_os = "linux"))]
fn browser_connections_idle(root: u32, network: &str, stdio: &str) -> bool {
    #[derive(Default)]
    struct Descriptor<'a> {
        pid: u32,
        fd: &'a str,
        kind: &'a str,
        name: &'a str,
        state: &'a str,
    }
    fn parse(data: &str) -> Option<Vec<Descriptor<'_>>> {
        let mut result = Vec::new();
        let mut pid = 0;
        let mut current: Option<Descriptor<'_>> = None;
        for line in data.lines() {
            let (tag, value) = line.split_at_checked(1)?;
            if tag == "p" || tag == "f" {
                if let Some(record) = current.take() {
                    result.push(record);
                }
                if tag == "p" {
                    pid = value.parse().ok()?;
                } else {
                    current = Some(Descriptor {
                        pid,
                        fd: value,
                        ..Descriptor::default()
                    });
                }
            } else {
                let record = current.as_mut()?;
                match tag {
                    "t" => record.kind = value,
                    "n" => record.name = value,
                    "T" => {
                        if let Some(state) = value.strip_prefix("ST=") {
                            record.state = state;
                        }
                    }
                    _ => return None,
                }
            }
        }
        if let Some(record) = current {
            result.push(record);
        }
        if (!data.is_empty() && result.is_empty())
            || result
                .iter()
                .any(|r| r.pid == 0 || r.fd.is_empty() || r.kind.is_empty() || r.name.is_empty())
        {
            return None;
        }
        Some(result)
    }
    let (Some(network), Some(stdio)) = (parse(network), parse(stdio)) else {
        return false;
    };
    network.iter().all(|r| {
        matches!(r.kind, "IPv4" | "IPv6")
            && r.state == "LISTEN"
            && (r.name.starts_with("127.0.0.1:")
                || r.name.starts_with("[::1]:")
                || r.name.starts_with("::1:"))
    }) && stdio.len() == 3
        && ["0", "1", "2"].iter().all(|fd| {
            stdio
                .iter()
                .filter(|r| {
                    r.pid == root
                        && r.fd == *fd
                        && ((r.kind == "unix" && r.name == "->(none)")
                            || (r.kind == "CHR" && r.name == "/dev/null"))
                })
                .count()
                == 1
        })
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
pub(super) fn signal(p: &Process, signal: i32) -> io::Result<bool> {
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
fn unchanged_browser_family(
    root: &Process,
    ids: &BTreeSet<u32>,
    old: &Table,
    fresh: &Table,
    config: &Config,
) -> bool {
    let Some(current) = fresh.get(&root.pid) else {
        return false;
    };
    category(current) == Some("Cloudflare test browser")
        && current.identity == root.identity
        && tree(current, fresh, root.uid, minimum_age(current, config)).as_ref() == Some(ids)
        && ids.iter().all(|pid| {
            old.get(pid)
                .zip(fresh.get(pid))
                .is_some_and(|(old, fresh)| {
                    old.identity == fresh.identity
                        && old.executable == fresh.executable
                        && old.arguments == fresh.arguments
                        && quiet_since(old, fresh, false)
                })
        })
}

fn terminate_browser(
    root: &Process,
    ids: &BTreeSet<u32>,
    old: &Table,
    config: &Config,
    activity: &Option<String>,
) -> io::Result<usize> {
    // A cwd-only ownership exemption is granted only after the quiet observation
    // window, and renewed with fresh family/connection/ownership evidence at each stage.
    for (kind, root_only, delay) in [
        (libc::SIGTERM, true, 500),
        (libc::SIGTERM, false, 1000),
        (libc::SIGKILL, false, 100),
    ] {
        if identity(root.pid).as_deref() != Some(&root.identity) {
            break;
        }
        let fresh = inventory()?;
        if !unchanged_browser_family(root, ids, old, &fresh, config)
            || !disconnected(&fresh[&root.pid], ids)?
            || activity_fingerprint(ids)? != *activity
            || !super::workload_ownership::protected(&fresh, ids, ids).is_empty()
        {
            break;
        }
        if root_only {
            signal(&fresh[&root.pid], kind)?;
        } else {
            for pid in ids {
                signal(&fresh[pid], kind)?;
            }
        }
        std::thread::sleep(Duration::from_millis(delay));
    }
    Ok(ids
        .iter()
        .filter(|pid| {
            identity(**pid).as_deref() != Some(&old[pid].identity) || executable(**pid).is_none()
        })
        .count())
}

fn terminate(
    root: &Process,
    ids: &BTreeSet<u32>,
    old: &Table,
    config: &Config,
    activity: &Option<String>,
) -> io::Result<usize> {
    #[cfg(target_os = "macos")]
    if scaleft::helper(root) {
        return scaleft::terminate(root, config);
    }
    if category(root) == Some("Cloudflare test browser") {
        return terminate_browser(root, ids, old, config, activity);
    }
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
    let strict = strict_cpu(root);
    for pid in ids {
        if fresh[pid].identity != old[pid].identity || !quiet_since(&old[pid], &fresh[pid], strict)
        {
            return Ok(0);
        }
    }
    if !super::workload_ownership::protected(&fresh, ids, &BTreeSet::new()).is_empty() {
        return Ok(0);
    }
    signal(current, libc::SIGTERM)?;
    std::thread::sleep(Duration::from_millis(500));
    let owned = super::workload_ownership::protected(&fresh, ids, &BTreeSet::new());
    for pid in ids {
        if !owned.contains_key(pid) {
            signal(&old[pid], libc::SIGTERM)?;
        }
    }
    std::thread::sleep(Duration::from_millis(1000));
    let owned = super::workload_ownership::protected(&fresh, ids, &BTreeSet::new());
    for pid in ids {
        if !owned.contains_key(pid) {
            signal(&old[pid], libc::SIGKILL)?;
        }
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
    let candidates: Vec<_> = table
        .values()
        .filter(|root| {
            !config.aggressive
                || matches!(
                    category(root),
                    Some("Cloudflare test browser" | "ScaleFT SSH helper")
                )
        })
        .filter_map(|root| tree(root, table, uid, minimum_age(root, config)).map(|ids| (root, ids)))
        .collect();
    #[cfg(target_os = "macos")]
    let mut scaleft_inspections = scaleft::inspect(
        &candidates
            .iter()
            .filter(|(root, _)| scaleft::helper(root))
            .map(|(root, _)| root.pid)
            .collect::<Vec<_>>(),
        lsof,
    );
    for (root, ids) in candidates {
        let key = format!(
            "orphan:{}",
            ids.iter()
                .map(|pid| format!("{pid}:{}", table[pid].identity))
                .collect::<Vec<_>>()
                .join("/")
        );
        #[cfg(target_os = "macos")]
        let connection = if scaleft::helper(root) {
            scaleft_inspections
                .remove(&root.pid)
                .unwrap()
                .map_err(io::Error::other)
        } else {
            disconnected(root, &ids)
        };
        #[cfg(not(target_os = "macos"))]
        let connection = disconnected(root, &ids);
        let inspection = connection.and_then(|idle| {
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
                    error: None,
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
                    strict_cpu: strict_cpu(root),
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
            error: None,
        });
    }
    observations.retain(|key, _| !key.starts_with("orphan:") || retained.contains(key));
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
    #[cfg(target_os = "macos")]
    #[test]
    fn scaleft_only_recognizes_old_childless_proxycommands() {
        let mut p = worker();
        p.executable = "/Applications/ScaleFT.app/Contents/MacOS/sft".into();
        p.arguments = "/usr/local/bin/sft proxycommand devboxkjopek".into();
        assert_eq!(category(&p), Some("ScaleFT SSH helper"));
        assert!(strict_cpu(&p));
        let config = Config {
            process_min_age_seconds: 0,
            ..Config::default()
        };
        assert_eq!(minimum_age(&p, &config), 3600);
        let mut table = Table::from([(p.pid, p.clone())]);
        assert!(tree(&p, &table, p.uid, 3600).is_some());
        for args in [
            "sft service",
            "sft ssh devbox",
            "sft proxycommand",
            "sft proxycommand devbox extra",
            "sft proxycommand --help",
            "other proxycommand devbox",
        ] {
            let mut other = p.clone();
            other.arguments = args.into();
            assert_eq!(category(&other), None, "{args}");
        }
        let mut child = p.clone();
        child.pid += 1;
        child.parent = p.pid;
        table.insert(child.pid, child);
        assert!(tree(&p, &table, p.uid, 3600).is_none());
        table.remove(&(p.pid + 1));
        for (parent, uid, age) in [(42, p.uid, 7200), (1, p.uid + 1, 7200), (1, p.uid, 3599)] {
            let mut other = p.clone();
            other.parent = parent;
            other.uid = uid;
            other.age_seconds = age;
            assert!(tree(&other, &table, p.uid, 3600).is_none());
        }
        p.executable = "/tmp/sft".into();
        assert_eq!(category(&p), None);
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
    fn final_check_allows_idle_timer_cpu_but_not_work() {
        let old = worker();
        let mut fresh = old.clone();
        fresh.age_seconds += 120;
        fresh.cpu_seconds += 1.5;
        assert!(quiet_since(&old, &fresh, false));
        assert!(!quiet_since(&old, &fresh, true));
        fresh.cpu_seconds = old.cpu_seconds + 4.0;
        assert!(!quiet_since(&old, &fresh, false));
        fresh.age_seconds = old.age_seconds;
        fresh.cpu_seconds = old.cpu_seconds + 0.05;
        assert!(quiet_since(&old, &fresh, true));
        let mut browser = worker();
        browser.executable =
            "/cache/.wrangler/chrome/mac_arm-1/chrome-mac-arm64/Google Chrome for Testing".into();
        browser.arguments =
            "chrome --headless --user-data-dir=/tmp/miniflare-1/browser-rendering/profile-1".into();
        assert_eq!(category(&browser), Some("Cloudflare test browser"));
        assert!(!strict_cpu(&browser));
        assert!(!strict_cpu(&worker()));
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
    // Keep inherited runner descriptors out of this controlled fixture.
    for (int fd = 3; fd < 1024; ++fd) close(fd);
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
        let browser_bin = root.join(".wrangler/chrome/mac/chrome-mac/Google Chrome for Testing");
        std::fs::create_dir_all(browser_bin.parent().unwrap()).unwrap();
        std::fs::copy(&bin, &browser_bin).unwrap();
        let locked = root.join("locked-checkout");
        std::fs::create_dir_all(locked.join(".git")).unwrap();
        std::fs::write(locked.join(".git/locked"), "active worktree").unwrap();
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
        let launch = |owned: bool, browser: bool| {
            let mut child = Command::new(if browser { &browser_bin } else { &bin })
                .current_dir(if browser { &locked } else { &root })
                .args(if browser {
                    [
                        "--headless=new",
                        "--remote-debugging-port=0",
                        "--user-data-dir=/tmp/miniflare-fixture/browser-rendering/profile-1",
                        if owned { "owned" } else { "orphan" },
                    ]
                } else {
                    [
                        "serve",
                        "--control-fd=3",
                        "-",
                        if owned { "owned" } else { "orphan" },
                    ]
                })
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
        let orphan = launch(false, false);
        let connected = launch(false, false);
        let owned = launch(true, false);
        let abandoned_browser = launch(false, true);
        let connected_browser = launch(false, true);
        let _client = TcpStream::connect(("127.0.0.1", connected.port)).unwrap();
        let _browser_client = TcpStream::connect(("127.0.0.1", connected_browser.port)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(orphan.process.parent, 1);
        let table = Table::from(
            [
                orphan.process.clone(),
                connected.process.clone(),
                owned.process.clone(),
                abandoned_browser.process.clone(),
                connected_browser.process.clone(),
            ]
            .map(|p| (p.pid, p)),
        );
        let config = Config {
            process_min_age_seconds: 0,
            browser_min_age_seconds: 0,
            observation_seconds: 0,
            ..Config::default()
        };
        let mut observations = BTreeMap::new();
        let aggressive_browser = launch(false, true);
        let aggressive_config = Config {
            aggressive: true,
            ..config.clone()
        };
        let aggressive_table = Table::from([(
            aggressive_browser.process.pid,
            aggressive_browser.process.clone(),
        )]);
        let (first, count) = aggressive_harvest(
            &aggressive_table,
            "Warning",
            &aggressive_config,
            &mut observations,
            true,
        )
        .unwrap();
        assert_eq!(count, 0, "{first:?}");
        assert!(!first.iter().any(|item| item.eligible));
        let (second, count) = aggressive_harvest(
            &aggressive_table,
            "Warning",
            &aggressive_config,
            &mut observations,
            true,
        )
        .unwrap();
        assert_eq!(count, 1, "{second:?}");
        assert!(identity(aggressive_browser.process.pid).is_none());
        observations.clear();
        assert_eq!(
            harvest(&table, &config, &mut observations, true).unwrap().1,
            0
        );
        // Inspection-only passes must not signal even after an orphan is ready.
        let (items, count) = harvest(&table, &config, &mut observations, false).unwrap();
        assert_eq!(count, 0, "{items:?}");
        assert_eq!(
            identity(orphan.process.pid).as_deref(),
            Some(orphan.process.identity.as_str())
        );
        observations.clear();
        let mut snapshot = super::super::Snapshot::default();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        super::super::during_worktree_scan(
            Duration::from_millis(10),
            move || {
                receive.recv_timeout(Duration::from_secs(60)).unwrap();
            },
            || {
                let table = inventory()?;
                // Only our controlled fixtures: never harvest unrelated host processes.
                let table = table
                    .into_iter()
                    .filter(|(pid, _)| {
                        [
                            orphan.process.pid,
                            connected.process.pid,
                            owned.process.pid,
                            abandoned_browser.process.pid,
                            connected_browser.process.pid,
                        ]
                        .contains(pid)
                    })
                    .collect();
                super::super::inspect_processes(
                    &table,
                    &config,
                    &mut observations,
                    &mut snapshot,
                    true,
                );
                if snapshot.harvested_processes == 2 {
                    let _ = send.try_send(());
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(snapshot.harvested_processes, 2, "{:?}", snapshot.processes);
        assert!(
            snapshot
                .activity
                .iter()
                .filter(|event| event.category == "scan")
                .count()
                >= 2
        );
        assert!(identity(orphan.process.pid).is_none());
        assert!(identity(abandoned_browser.process.pid).is_none());
        assert_eq!(
            identity(connected_browser.process.pid).as_deref(),
            Some(connected_browser.process.identity.as_str())
        );
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

fn essential(p: &Process) -> bool {
    #[cfg(target_os = "macos")]
    if p.executable == scaleft::EXECUTABLE {
        return true;
    }
    if super::codex::is_codex(p) {
        return true;
    }
    let name = Path::new(&p.executable)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "hey-boss" | "hey-harvester" | "hey-proxy" | "sqlite3" | "postgres" | "mysqld"
    ) || name.contains("hables")
        || p.arguments.to_ascii_lowercase().contains("hables")
}
fn expired_kind(p: &Process, pressure: &str) -> Option<&'static str> {
    if essential(p) || p.executable.contains("/.wrangler/chrome/") {
        return None;
    }
    let name = Path::new(&p.executable)
        .file_name()?
        .to_string_lossy()
        .to_ascii_lowercase();
    let runtime = matches!(
        name.as_str(),
        "node"
            | "bun"
            | "python"
            | "python3"
            | "python3.12"
            | "python3.13"
            | "python3.14"
            | "workerd"
            | "deno"
            | "uv"
    );
    let testing = p.executable.contains("ms-playwright")
        || p.executable.contains("chrome-for-testing")
        || p.arguments.contains("--headless")
        || p.arguments.contains("/playwright_");
    if (name.contains("chrome") || name.contains("chromium") || name.contains("firefox"))
        && testing
        && p.age_seconds >= if pressure == "Normal" { 3600 } else { 600 }
    {
        return Some("Expired automated browser");
    }
    if name.contains("crashpad") && p.parent == 1 && p.age_seconds >= 600 {
        return Some("Orphan crash helper");
    }
    if runtime && p.parent == 1 && p.age_seconds >= 3600 {
        return Some("Orphan developer runtime");
    }
    if (runtime || matches!(name.as_str(), "claude" | "cargo" | "rustc")) && p.age_seconds >= 86400
    {
        return Some("Expired developer workload");
    }
    if pressure != "Normal"
        && pressure != "Unavailable"
        && runtime
        && p.age_seconds >= 3600
        && (p.arguments.contains("node_modules/")
            || p.arguments.contains("vitest")
            || p.arguments.contains("playwright")
            || name == "workerd")
    {
        return Some("Developer worker under memory pressure");
    }
    if pressure == "Critical"
        && name.contains("chrome")
        && (name.contains("renderer") || p.arguments.contains("--type=renderer"))
        && p.age_seconds >= 3600
    {
        return Some("Browser renderer under critical memory pressure");
    }
    None
}

pub(super) fn aggressive_harvest(
    table: &Table,
    pressure: &str,
    config: &Config,
    observations: &mut BTreeMap<String, Observation>,
    apply: bool,
) -> io::Result<(Vec<Item>, usize)> {
    let (mut items, killed) = aggressive_expiration(
        table,
        pressure,
        apply,
        |table, refresh| {
            if refresh {
                services::Guard::refresh(table)
            } else {
                services::Guard::read(table)
            }
        },
        signal,
    )?;
    let (browser_items, browser_exited) = harvest(table, config, observations, apply)?;
    items.extend(browser_items);
    let (idle_items, exited) = super::codex::graceful_idle(table, config, observations, apply)?;
    items.extend(idle_items);
    Ok((items, killed + browser_exited + exited))
}

fn aggressive_expiration(
    table: &Table,
    pressure: &str,
    apply: bool,
    mut inspect_services: impl FnMut(&Table, bool) -> io::Result<services::Guard>,
    mut send: impl FnMut(&Process, i32) -> io::Result<bool>,
) -> io::Result<(Vec<Item>, usize)> {
    let uid = unsafe { libc::geteuid() };
    let mut protected = super::codex::family(table);
    let mut pid = std::process::id();
    while pid > 1 && protected.insert(pid) {
        pid = table.get(&pid).map_or(0, |p| p.parent);
    }
    // Wrangler browsers and ScaleFT families bypass age-only expiration. Their
    // orphan helpers must pass the transport and observation checks in harvest.
    for p in table.values().filter(|p| {
        #[cfg(target_os = "macos")]
        if p.executable == scaleft::EXECUTABLE {
            return true;
        }
        p.executable.contains("/.wrangler/chrome/")
    }) {
        protected.extend(descendants(p.pid, table));
    }
    let mut selected = BTreeSet::new();
    let mut labels = BTreeMap::new();
    for p in table.values() {
        if p.uid != uid || protected.contains(&p.pid) {
            continue;
        }
        if let Some(kind) = expired_kind(p, pressure) {
            selected.insert(p.pid);
            labels.insert(p.pid, kind);
        }
    }
    // Stop descendants too, or killing a controller creates tomorrow's orphans.
    loop {
        let before = selected.len();
        for p in table.values() {
            if p.uid == uid
                && selected.contains(&p.parent)
                && !protected.contains(&p.pid)
                && !essential(p)
            {
                selected.insert(p.pid);
                labels.entry(p.pid).or_insert("Expired workload descendant");
            }
        }
        if before == selected.len() {
            break;
        }
    }
    let mut items = Vec::new();
    let services = inspect_services(table, false)?;
    selected.retain(|pid| {
        if let Some(reason) = services.protection(&table[pid]) {
            items.push(Item {
                name: format!("Protected service · PID {pid}"),
                detail: reason.into(),
                eligible: false,
                worktree: None,
                error: None,
            });
            false
        } else {
            true
        }
    });
    for (pid, protection) in
        super::workload_ownership::protected(table, &selected, &BTreeSet::new())
    {
        selected.remove(&pid);
        items.push(Item {
            name: format!("Protected workload · PID {pid}"),
            error: protection.uncertain.then(|| protection.reason.clone()),
            detail: protection.reason,
            eligible: false,
            worktree: None,
        });
    }
    let mut signaled = Vec::new();
    // One bounded inventory for the whole signal batch, refreshed after workload
    // inspection and again before escalation. Failure preserves every candidate.
    let term_services = (apply && !selected.is_empty()).then(|| inspect_services(table, true));
    for pid in selected {
        let p = &table[&pid];
        if let Some(protection) = term_services
            .as_ref()
            .and_then(|guard| services::protection(guard, p))
        {
            items.push(Item {
                name: format!("Protected service · PID {pid}"),
                error: protection.uncertain.then(|| protection.reason.clone()),
                detail: protection.reason,
                eligible: false,
                worktree: None,
            });
            continue;
        }
        let detail = if apply {
            match send(p, libc::SIGTERM) {
                Ok(true) => {
                    signaled.push(p);
                    "Sent TERM; checking exit".into()
                }
                Ok(false) => "Already exited or identity changed".into(),
                Err(e) => format!("Termination failed: {e}"),
            }
        } else {
            "Expired under aggressive policy".into()
        };
        items.push(Item {
            name: format!("{} · PID {}", labels[&pid], pid),
            detail,
            eligible: true,
            worktree: None,
            error: None,
        });
    }
    if !signaled.is_empty() {
        std::thread::sleep(Duration::from_secs(1));
    }
    let pending: BTreeSet<_> = signaled.iter().map(|p| p.pid).collect();
    let mut newly_owned = super::workload_ownership::protected(table, &pending, &BTreeSet::new());
    if !signaled.is_empty() {
        let kill_services = inspect_services(table, true);
        for p in &signaled {
            if let Some(protection) = services::protection(&kill_services, p) {
                newly_owned.insert(p.pid, protection);
            }
        }
    }
    for p in &signaled {
        if !newly_owned.contains_key(&p.pid) {
            send(p, libc::SIGKILL)?;
        }
    }
    if !signaled.is_empty() {
        std::thread::sleep(Duration::from_millis(100));
    }
    let killed = signaled
        .iter()
        .filter(|p| identity(p.pid).as_deref() != Some(&p.identity) || executable(p.pid).is_none())
        .count();
    for item in &mut items {
        if apply && item.detail.starts_with("Sent TERM") {
            if let Some((_, protection)) = newly_owned
                .iter()
                .find(|(pid, _)| item.name.ends_with(&format!("PID {pid}")))
            {
                item.detail = format!("TERM was sent; no KILL sent: {}", protection.reason);
                item.eligible = false;
                item.error = protection.uncertain.then(|| protection.reason.clone());
            } else {
                item.detail = format!(
                    "TERM/KILL completed with identity checks; {killed} processes confirmed gone in this batch"
                );
            }
        }
    }
    Ok((items, killed))
}

#[cfg(test)]
mod abandoned_browser_tests {
    use super::*;

    fn browser() -> Process {
        Process {
            pid: 100,
            parent: 1,
            uid: unsafe { libc::geteuid() },
            age_seconds: 14400,
            cpu_seconds: 20.0,
            executable: "/cache/.wrangler/chrome/mac_arm-1/chrome-mac-arm64/Google Chrome for Testing".into(),
            identity: "browser-start".into(),
            arguments: "chrome --headless=new --remote-debugging-port=0 --user-data-dir=/private/var/folders/test/T/miniflare-123/browser-rendering/profile-abc".into(),
        }
    }

    #[test]
    fn miniflare_signature_requires_real_options_and_a_temporary_profile() {
        let p = browser();
        assert_eq!(category(&p), Some("Cloudflare test browser"));
        for arguments in [
            "chrome --headless=new --user-data-dir=/repo/miniflare-1/browser-rendering/profile-1",
            "chrome --headless=new --type=renderer --user-data-dir=/tmp/miniflare-1/browser-rendering/profile-1",
            "chrome --headless-fake --user-data-dir=/tmp/miniflare-1/browser-rendering/profile-1",
            "chrome --headless=new --user-data-dir=/tmp/personal /miniflare-1/browser-rendering/profile-1",
        ] {
            let mut other = p.clone();
            other.arguments = arguments.into();
            assert_ne!(
                category(&other),
                Some("Cloudflare test browser"),
                "{arguments}"
            );
        }
    }

    #[test]
    fn miniflare_families_never_fall_back_to_aggressive_expiration() {
        let mut p = browser();
        for parent in [1, 42] {
            p.parent = parent;
            for pressure in ["Normal", "Warning", "Critical"] {
                assert!(expired_kind(&p, pressure).is_none());
            }
        }
    }

    #[test]
    fn orphan_observation_pruning_preserves_codex_exit_receipts() {
        let mut observations = BTreeMap::from([(
            "codex-exit:session".into(),
            Observation {
                first_seen: 1,
                last_seen: 1,
                cpu_seconds: 0.0,
                activity_fingerprint: None,
            },
        )]);
        harvest(&Table::new(), &Config::default(), &mut observations, false).unwrap();
        assert!(observations.contains_key("codex-exit:session"));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn browser_connection_proof_requires_complete_disconnected_controller_evidence() {
        let network = "p100\nf53\ntIPv4\nn127.0.0.1:63453\nTST=LISTEN\n";
        let stdio = "p100\nf0\ntunix\nn->(none)\nf1\ntCHR\nn/dev/null\nf2\ntunix\nn->(none)\n";
        assert!(browser_connections_idle(100, network, stdio));
        assert!(browser_connections_idle(100, "", stdio));
        for bad in [
            network.replace("LISTEN", "ESTABLISHED"),
            network.replace("127.0.0.1", "0.0.0.0"),
            network.replace("TST=LISTEN\n", ""),
            format!("{network}p101\nf4\ntIPv4\nn127.0.0.1:40->127.0.0.1:41\nTST=ESTABLISHED\n"),
        ] {
            assert!(!browser_connections_idle(100, &bad, stdio), "{bad}");
        }
        for bad in [
            stdio.replace("n->(none)", "n->0x123"),
            stdio.replace("tunix", "tPIPE"),
            stdio.replace("n/dev/null", "n/dev/ttys001"),
            stdio.replace("f2\ntunix\nn->(none)\n", ""),
            stdio.replace("p100", "p101"),
            stdio.replace("tunix", "tunknown"),
            String::new(),
        ] {
            assert!(!browser_connections_idle(100, network, &bad), "{bad}");
        }
    }

    #[test]
    fn browser_final_validation_rejects_changed_families_or_identity() {
        let root = browser();
        let mut child = root.clone();
        child.pid = 101;
        child.parent = root.pid;
        let old = Table::from([(root.pid, root.clone()), (child.pid, child)]);
        let ids = BTreeSet::from([100, 101]);
        assert!(unchanged_browser_family(
            &root,
            &ids,
            &old,
            &old,
            &Config::default()
        ));
        for change in 0..6 {
            let mut fresh = old.clone();
            match change {
                0 => {
                    fresh.get_mut(&100).unwrap().parent = 42;
                }
                1 => {
                    fresh.get_mut(&101).unwrap().identity = "reused".into();
                }
                2 => {
                    fresh.get_mut(&101).unwrap().executable = "/usr/bin/node".into();
                }
                3 => {
                    fresh.get_mut(&101).unwrap().cpu_seconds += 20.0;
                }
                4 => {
                    fresh.remove(&100);
                }
                _ => {
                    let mut extra = fresh[&101].clone();
                    extra.pid = 102;
                    fresh.insert(102, extra);
                }
            }
            assert!(
                !unchanged_browser_family(&root, &ids, &old, &fresh, &Config::default()),
                "change {change}"
            );
        }
    }

    #[test]
    fn final_quiet_check_rejects_regressing_or_invalid_counters() {
        let old = browser();
        for (age, cpu) in [
            (old.age_seconds - 1, old.cpu_seconds),
            (old.age_seconds + 1, old.cpu_seconds - 1.0),
            (old.age_seconds + 1, f64::NAN),
        ] {
            let mut fresh = old.clone();
            fresh.age_seconds = age;
            fresh.cpu_seconds = cpu;
            assert!(!quiet_since(&old, &fresh, false));
        }
    }
}

#[cfg(test)]
mod aggressive_tests {
    use super::*;
    use std::path::PathBuf;
    #[test]
    fn detached_controller_and_owned_descendant_survive_aggressive_cleanup() {
        use std::io::Read;
        struct Fixture {
            root: PathBuf,
            children: Vec<std::process::Child>,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                for child in &mut self.children {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let _ = std::fs::remove_dir_all(&self.root);
            }
        }
        let root =
            std::env::temp_dir().join(format!("harvester-detached-owner-{}", std::process::id()));
        let mut fixture = Fixture {
            root: root.clone(),
            children: Vec::new(),
        };
        for directory in [
            root.join("free"),
            root.join("work/nested"),
            root.join("admin"),
        ] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(root.join("work/.git"), "gitdir: ../admin\n").unwrap();
        std::fs::write(root.join("admin/locked"), "queued Codex owner").unwrap();
        let open_file = root.join("work/open file\nwith newline");
        std::fs::write(&open_file, "fixture").unwrap();
        let mut table = Table::new();
        for (index, directory) in [
            root.join("free"),
            root.join("work/nested"),
            root.join("free"),
            root.join("free"),
        ]
        .into_iter()
        .enumerate()
        {
            let child = if index == 3 {
                let mut child = Command::new("sh")
                    .args(["-c", "exec 3< \"$1\"; printf x; exec sleep 60", "fixture"])
                    .arg(&open_file)
                    .current_dir(directory)
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                child.stdout.as_mut().unwrap().read_exact(&mut [0]).unwrap();
                child
            } else {
                Command::new("sleep")
                    .arg("60")
                    .current_dir(directory)
                    .spawn()
                    .unwrap()
            };
            let pid = child.id();
            let parent = if index == 1 {
                fixture.children[0].id()
            } else {
                1
            };
            fixture.children.push(child);
            table.insert(
                pid,
                Process {
                    pid,
                    parent,
                    uid: unsafe { libc::geteuid() },
                    age_seconds: 4000,
                    cpu_seconds: 0.0,
                    executable: "/bin/node".into(),
                    identity: identity(pid).unwrap(),
                    arguments: "node vitest".into(),
                },
            );
        }
        let inspect = || {
            aggressive_harvest(
                &table,
                "Warning",
                &Config::default(),
                &mut BTreeMap::new(),
                false,
            )
            .unwrap()
            .0
        };
        let items = inspect();
        for (index, child) in fixture
            .children
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != 2)
        {
            assert!(
                !items.iter().any(
                    |item| item.eligible && item.name.ends_with(&format!("PID {}", child.id()))
                ),
                "Owned process {index} selected: {items:?}"
            );
        }
        let free = fixture.children[2].id();
        assert!(
            items
                .iter()
                .any(|item| item.eligible && item.name.ends_with(&format!("PID {free}"))),
            "Unowned orphan should remain eligible: {items:?}"
        );
        std::fs::remove_file(root.join("admin/locked")).unwrap();
        assert_eq!(inspect().iter().filter(|item| item.eligible).count(), 4);
    }
    #[test]
    fn expires_realistic_leaks_and_preserves_young_work_and_database_services() {
        let mut p = Process {
            pid: 222,
            parent: 1,
            uid: 501,
            age_seconds: 4000,
            cpu_seconds: 900.0,
            executable: "/tmp/bun".into(),
            identity: "fixture".into(),
            arguments: String::new(),
        };
        assert!(expired_kind(&p, "Normal").is_some());
        p.parent = 100;
        p.age_seconds = 100;
        assert!(expired_kind(&p, "Critical").is_none());
        p.age_seconds = 90000;
        p.executable = "/bin/codex".into();
        assert!(expired_kind(&p, "Normal").is_none());
        p.executable = "/bin/hey-boss".into();
        assert!(expired_kind(&p, "Critical").is_none());
        p.executable = "/bin/node".into();
        p.arguments = "node /srv/Hables/server.js".into();
        assert!(expired_kind(&p, "Critical").is_none());
        p.arguments.clear();
        p.executable = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into();
        assert!(expired_kind(&p, "Normal").is_none());
        p.arguments = "--headless".into();
        assert!(expired_kind(&p, "Warning").is_some());
    }
}
