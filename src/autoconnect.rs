use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Config {
    host: String,
    vpn_domain: String,
    enabled: bool,
}
#[derive(Serialize, Deserialize, Default)]
struct Status {
    state: String,
    updated_at: u64,
    retry_at: u64,
    failures: u32,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn directory() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is missing")?;
    let path = PathBuf::from(home).join(".local/share/hey-boss");
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}
fn write_json<T: Serialize>(name: &str, value: &T) -> Result<(), String> {
    let path = directory()?.join(name);
    let temporary = path.with_extension("new");
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}
fn machines_path() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or("HOME is missing")?)
            .join(".hey-boss/config.json"),
    )
}
#[derive(Serialize, Deserialize)]
struct Machines {
    ssh_hosts: Vec<MachineEntry>,
}
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum MachineEntry {
    Host(String),
    Settings {
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vpn_domain: Option<String>,
        #[serde(default = "yes")]
        enabled: bool,
    },
}
fn yes() -> bool {
    true
}
fn resolve_machines(data: &[u8]) -> Result<Vec<Config>, String> {
    if data.len() > 1024 * 1024 {
        return Err("machine config exceeds 1 MiB".into());
    }
    let config: Machines = serde_json::from_slice(data).map_err(|e| e.to_string())?;
    if config.ssh_hosts.len() > 32 {
        return Err("at most 32 machines are supported".into());
    }
    let mut result = Vec::new();
    for entry in config.ssh_hosts {
        let (host, domain, enabled) = match entry {
            MachineEntry::Host(host) => (host, None, true),
            MachineEntry::Settings {
                host,
                vpn_domain,
                enabled,
            } => (host, vpn_domain, enabled),
        };
        let lan = host
            .to_ascii_lowercase()
            .trim_end_matches('.')
            .ends_with(".local");
        let settings = Config {
            host,
            vpn_domain: domain.unwrap_or_else(|| if lan { "local" } else { "quora.net" }.into()),
            enabled,
        };
        validate(&settings)?;
        if settings.vpn_domain == "local" && !lan {
            return Err("VPN bypass requires a .local host".into());
        }
        if result.iter().any(|c: &Config| c.host == settings.host) {
            return Err(format!("duplicate machine: {}", settings.host));
        }
        result.push(settings);
    }
    Ok(result)
}
fn machines() -> Result<Vec<Config>, String> {
    resolve_machines(&std::fs::read(machines_path()?).map_err(|e| e.to_string())?)
}
fn save_machines(configs: &[Config]) -> Result<(), String> {
    let entries = configs
        .iter()
        .map(|c| MachineEntry::Settings {
            host: c.host.clone(),
            vpn_domain: (c.vpn_domain != "local").then(|| c.vpn_domain.clone()),
            enabled: c.enabled,
        })
        .collect();
    let data =
        serde_json::to_vec_pretty(&Machines { ssh_hosts: entries }).map_err(|e| e.to_string())?;
    resolve_machines(&data)?;
    let path = machines_path()?;
    std::fs::create_dir_all(path.parent().ok_or("invalid machine config path")?)
        .map_err(|e| e.to_string())?;
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, path).map_err(|e| e.to_string())
}
fn migrate_machines() -> Result<(), String> {
    if machines_path()?.exists() {
        machines()?;
        return Ok(());
    }
    let legacy = config().ok();
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is missing")?);
    let inventory: serde_json::Value = std::fs::read(home.join(".hey-proxy/config.json"))
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default();
    let mut hosts = Vec::<String>::new();
    if let Some(entries) = inventory.get("ssh_hosts").and_then(|v| v.as_array()) {
        for entry in entries {
            if let Some(host) = entry
                .as_str()
                .or_else(|| entry.get("host").and_then(|h| h.as_str()))
                && !hosts.iter().any(|h| h == host)
            {
                hosts.push(host.to_owned());
            }
        }
    }
    if let Some(c) = &legacy
        && !hosts.contains(&c.host)
    {
        hosts.push(c.host.clone());
    }
    let data = serde_json::to_vec(&Machines {
        ssh_hosts: hosts.into_iter().map(MachineEntry::Host).collect(),
    })
    .map_err(|e| e.to_string())?;
    let mut configs = resolve_machines(&data)?;
    if let Some(old) = legacy
        && let Some(c) = configs.iter_mut().find(|c| c.host == old.host)
    {
        *c = old;
    }
    save_machines(&configs)
}
pub fn setup_machines() -> Result<(), String> {
    migrate_machines()?;
    install_companion_service()?;
    println!("Machines configured in {}", machines_path()?.display());
    Ok(())
}
fn config() -> Result<Config, String> {
    let config: Config = serde_json::from_slice(
        &std::fs::read(directory()?.join("companion.json"))
            .map_err(|e| format!("run companion configure first: {e}"))?,
    )
    .map_err(|e| e.to_string())?;
    validate(&config)?;
    Ok(config)
}
fn validate(c: &Config) -> Result<(), String> {
    if c.host.is_empty()
        || c.host.len() > 253
        || c.host.starts_with('-')
        || c.host
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err("host must be an SSH alias or user@host".into());
    }
    let domain = c.vpn_domain.strip_suffix('.').unwrap_or(&c.vpn_domain);
    if domain.is_empty()
        || domain.len() > 253
        || domain.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err("vpn-domain must be a DNS domain".into());
    }
    Ok(())
}
fn lock(name: &str) -> Result<std::fs::File, String> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(directory()?.join(name))
        .map_err(|e| e.to_string())?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("a companion connector is already running".into());
    }
    Ok(file)
}
fn host_key(host: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    host.hash(&mut hash);
    format!("{:016x}", hash.finish())
}
pub fn control_path(host: &str) -> Result<PathBuf, String> {
    Ok(directory()?.join(format!("ssh-{}.sock", host_key(host))))
}
pub fn connection_lock(host: &str) -> Result<std::fs::File, String> {
    lock(&format!("connection-{}.lock", host_key(host)))
}
fn vpn_present(dns: &str, domain: &str) -> bool {
    dns.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let key = key.trim();
            (key == "domain" || key.starts_with("search domain[")).then_some(value.trim())
        })
        .any(|value| {
            value
                .trim_end_matches('.')
                .eq_ignore_ascii_case(domain.trim_end_matches('.'))
        })
}
fn vpn_ready(domain: &str) -> bool {
    Command::new("/usr/sbin/scutil")
        .arg("--dns")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| vpn_present(&String::from_utf8_lossy(&o.stdout), domain))
}
// Never reset cooldown on a VPN flap. Only a stable connection clears failures.
fn delay(failures: u32, jitter: u64) -> u64 {
    (60_u64.saturating_mul(1_u64 << failures.saturating_sub(1).min(4))).min(900) + jitter % 16
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub fn configure(host: &str, vpn_domain: &str, enabled: bool) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("automatic VPN connection currently requires macOS".into());
    }
    let settings = Config {
        host: host.into(),
        vpn_domain: vpn_domain.into(),
        enabled,
    };
    validate(&settings)?;
    migrate_machines()?;
    let mut configs = machines()?;
    if let Some(c) = configs.iter_mut().find(|c| c.host == host) {
        *c = settings;
    } else {
        configs.push(settings);
    }
    save_machines(&configs)?;
    install_companion_service()?;
    println!("Saved {}", machines_path()?.display());
    Ok(())
}
fn install_companion_service() -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("automatic connections require macOS".into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is missing")?);
    let agent = home.join("Library/LaunchAgents/local.hey-boss-companion.plist");
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &domain])
        .arg(&agent)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(agent.parent().unwrap()).map_err(|e| e.to_string())?;
    let log = directory()?.join("autoconnect.log");
    std::fs::write(&agent, format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>local.hey-boss-companion</string>
<key>ProgramArguments</key><array><string>{}</string><string>companion</string><string>auto</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
<key>ThrottleInterval</key><integer>60</integer>
<key>ExitTimeOut</key><integer>20</integer>
<key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string>
</dict></plist>"#, xml(&executable.to_string_lossy()), xml(&log.to_string_lossy()), xml(&log.to_string_lossy()))).map_err(|e| e.to_string())?;
    bootstrap_agent(std::path::Path::new("/bin/launchctl"), &domain, &agent)?;
    Ok(())
}
fn bootstrap_agent(
    program: &std::path::Path,
    domain: &str,
    agent: &std::path::Path,
) -> Result<(), String> {
    // launchd may return EIO while a just-unloaded job finishes tearing down.
    // A successful bootstrap ends this loop before another instance can start.
    for wait in [0, 100, 200, 400, 800, 1000, 2000, 2000, 2000] {
        std::thread::sleep(Duration::from_millis(wait));
        let output = Command::new(program)
            .args(["bootstrap", domain])
            .arg(agent)
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        if output.status.code() != Some(5) {
            return Err(format!(
                "settings saved, but launchctl could not start the connector ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    Err("settings saved, but launchctl could not start the connector after bounded teardown retries".into())
}
pub fn status() -> Result<(), String> {
    println!(
        "{}",
        std::fs::read_to_string(machines_path()?).map_err(|e| e.to_string())?
    );
    println!(
        "{}",
        std::fs::read_to_string(directory()?.join("connections.json"))
            .unwrap_or_else(|_| "No connection status yet.".into())
    );
    Ok(())
}
struct Worker {
    settings: Config,
    child: Child,
}
fn stop_worker(worker: &mut Worker) {
    unsafe {
        libc::kill(worker.child.id() as i32, libc::SIGTERM);
    }
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if worker.child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = worker.child.kill();
    let _ = worker.child.wait();
}
fn supervise() -> Result<(), String> {
    if !machines_path()?.exists() {
        migrate_machines()?;
    }
    let _lock = lock("auto.lock")?;
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
    }
    let mut workers = std::collections::BTreeMap::<String, Worker>::new();
    let mut retry = std::collections::BTreeMap::<String, Instant>::new();
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut last_error = None;
    while !STOP.load(Ordering::Relaxed) {
        let loaded = machines();
        let error = loaded.as_ref().err().cloned();
        if error != last_error {
            if let Some(e) = &error {
                eprintln!("Machine config: {e}; keeping existing connections");
            }
            last_error = error.clone();
        }
        if let Ok(configs) = loaded {
            let desired: std::collections::BTreeMap<_, _> = configs
                .into_iter()
                .filter(|c| c.enabled)
                .map(|c| (c.host.clone(), c))
                .collect();
            let obsolete: Vec<_> = workers
                .iter_mut()
                .filter_map(|(host, w)| {
                    let exited = w.child.try_wait().ok().flatten().is_some();
                    if exited {
                        retry.insert(host.clone(), Instant::now() + Duration::from_secs(60));
                    }
                    (exited || desired.get(host) != Some(&w.settings)).then(|| host.clone())
                })
                .collect();
            for host in obsolete {
                if let Some(mut w) = workers.remove(&host) {
                    stop_worker(&mut w);
                }
            }
            for (host, settings) in desired {
                if workers.contains_key(&host)
                    || retry.get(&host).is_some_and(|t| *t > Instant::now())
                {
                    continue;
                }
                let mut command = Command::new(&executable);
                command.args([
                    "companion",
                    "auto",
                    "--host",
                    &host,
                    "--vpn-domain",
                    &settings.vpn_domain,
                ]);
                if settings.vpn_domain == "local" {
                    command.arg("--local-network");
                }
                match command.spawn() {
                    Ok(child) => {
                        workers.insert(host, Worker { settings, child });
                    }
                    Err(e) => {
                        eprintln!("Cannot start {host}: {e}");
                        retry.insert(host, Instant::now() + Duration::from_secs(60));
                    }
                }
            }
        }
        let statuses: Vec<_> = workers
            .keys()
            .map(|host| {
                let mut value: serde_json::Value = std::fs::read(
                    directory()
                        .unwrap_or_default()
                        .join(format!("connection-status-{}.json", host_key(host))),
                )
                .ok()
                .and_then(|d| serde_json::from_slice(&d).ok())
                .unwrap_or_else(|| serde_json::json!({"state":"starting"}));
                if !value.is_object() {
                    value = serde_json::json!({"state":"starting"});
                }
                value["host"] = host.clone().into();
                value
            })
            .collect();
        if let Err(e) = write_json(
            "connections.json",
            &serde_json::json!({"machines":statuses,"config_error":error}),
        ) {
            eprintln!("Cannot save connection status: {e}");
        }
        for _ in 0..5 {
            if STOP.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    for (_, mut worker) in workers {
        stop_worker(&mut worker);
    }
    Ok(())
}
struct Session {
    child: Child,
    started: Instant,
    connected: Option<Instant>,
    messages: mpsc::Receiver<String>,
}
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGTERM);
        }
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}
fn start(host: &str) -> Result<Session, String> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    let child = command
        .args(["companion", "connect", host, "--unattended"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;
    let (tx, messages) = mpsc::channel();
    // Retain stdin's pipe: the SSH session must stay open even without a terminal.
    // Own cleanup before any subsequent fallible setup: failure must not leave
    // an unattended SSH process group running outside the retry policy.
    let mut session = Session {
        child,
        started: Instant::now(),
        connected: None,
        messages,
    };
    let output = session
        .child
        .stdout
        .take()
        .ok_or("connector output pipe is missing")?;
    std::thread::Builder::new()
        .name("hey-boss-connection-status".into())
        .spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        })
        .map_err(|e| format!("cannot start connector status reader: {e}"))?;
    Ok(session)
}
pub fn run(
    host: Option<&str>,
    local_network: bool,
    vpn_domain: Option<&str>,
) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("automatic VPN connection currently requires macOS".into());
    }
    if host.is_none() {
        return supervise();
    }
    if local_network
        && !host.is_some_and(|h| {
            h.to_ascii_lowercase()
                .trim_end_matches('.')
                .ends_with(".local")
        })
    {
        return Err("--local-network requires a .local SSH destination".into());
    }
    let explicit = host.map(|host| Config {
        host: host.into(),
        vpn_domain: if local_network {
            "local".into()
        } else {
            vpn_domain
                .map(str::to_owned)
                .map(Ok)
                .unwrap_or_else(|| config().map(|c| c.vpn_domain))
                .unwrap_or_else(|_| "quora.net".into())
        },
        enabled: true,
    });
    if let Some(settings) = &explicit {
        validate(settings)?;
    }
    let _lock = lock(
        &host
            .map(|h| format!("auto-{}.lock", host_key(h)))
            .unwrap_or_else(|| "auto.lock".into()),
    )?;
    let status_file = host
        .map(|h| format!("connection-status-{}.json", host_key(h)))
        .unwrap_or_else(|| "connection-status.json".into());
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
    }
    let mut status: Status = std::fs::read(directory()?.join(&status_file))
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default();
    let mut session: Option<Session> = None;
    let mut ready_samples = 0_u32;
    let mut probe_at = Instant::now();
    let mut vpn = false;
    let mut last_state = String::new();
    let mut previous_settings: Option<(String, String)> = None;
    let mut storage_retry = Instant::now();
    let mut storage_failed = false;
    while !STOP.load(Ordering::Relaxed) {
        // Generated cooldowns never exceed 900 seconds plus 15 seconds of jitter.
        // Recover from damaged state or a backward wall-clock adjustment quietly.
        status.retry_at = status.retry_at.min(now().saturating_add(915));
        let settings = match explicit
            .as_ref()
            .map(|c| {
                Ok(Config {
                    host: c.host.clone(),
                    vpn_domain: c.vpn_domain.clone(),
                    enabled: c.enabled,
                })
            })
            .unwrap_or_else(config)
        {
            Ok(settings) => settings,
            Err(error) => {
                session = None;
                ready_samples = 0;
                previous_settings = None;
                status.state = "config-error".into();
                status.updated_at = now();
                status.retry_at = 0;
                if last_state != status.state {
                    eprintln!("Companion configuration: {error}");
                    last_state = status.state.clone();
                }
                if let Err(error) = write_json(&status_file, &status) {
                    eprintln!("Companion: cannot save config-error status: {error}");
                }
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        if !settings.enabled {
            break;
        }
        let identity = (settings.host.clone(), settings.vpn_domain.clone());
        if previous_settings.as_ref() != Some(&identity) {
            session = None;
            ready_samples = 0;
            probe_at = Instant::now();
            previous_settings = Some(identity);
        }
        if Instant::now() >= probe_at {
            vpn = local_network || vpn_ready(&settings.vpn_domain);
            ready_samples = if vpn {
                ready_samples.saturating_add(1)
            } else {
                0
            };
            probe_at = Instant::now() + Duration::from_secs(15);
        }
        let mut failed = false;
        if let Some(active) = &mut session {
            while let Ok(message) = active.messages.try_recv() {
                if message.starts_with("hey-boss connected.") {
                    active.connected = Some(Instant::now());
                }
            }
            if !vpn
                || active
                    .child
                    .try_wait()
                    .map_err(|e| e.to_string())?
                    .is_some()
                || (active.connected.is_none()
                    && active.started.elapsed() > Duration::from_secs(45))
            {
                failed = true;
            } else if active
                .connected
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(60))
            {
                status.failures = 0;
                status.retry_at = 0;
            }
        }
        if failed {
            session = None;
            status.failures = status.failures.saturating_add(1);
            status.retry_at = now().saturating_add(delay(
                status.failures,
                now() ^ u64::from(std::process::id()),
            ));
        }
        if session.is_none() && ready_samples >= 2 && now() >= status.retry_at {
            match start(&settings.host) {
                Ok(active) => session = Some(active),
                Err(e) => {
                    eprintln!("Auto connection: {e}");
                    status.failures = status.failures.saturating_add(1);
                    status.retry_at = now().saturating_add(delay(status.failures, now()));
                }
            }
        }
        status.state = if !vpn {
            "waiting-for-vpn"
        } else if ready_samples < 2 {
            "waiting-for-stable-vpn"
        } else if let Some(active) = &session {
            if active.connected.is_some() {
                "connected"
            } else {
                "connecting"
            }
        } else {
            "backoff"
        }
        .into();
        if status.state != last_state {
            eprintln!("Companion: {}", status.state);
            if !storage_failed {
                storage_retry = Instant::now();
            }
            last_state = status.state.clone();
        }
        status.updated_at = now();
        // Status is observational: storage failure must not tear down healthy SSH
        // or reset the in-memory cooldown. Retry quietly once per minute.
        if Instant::now() >= storage_retry {
            match write_json(&status_file, &status) {
                Ok(()) => {
                    if storage_failed {
                        eprintln!("Companion: status storage recovered");
                    }
                    storage_failed = false;
                    storage_retry = Instant::now() + Duration::from_secs(15);
                }
                Err(error) => {
                    if !storage_failed {
                        eprintln!("Companion: cannot save status; retaining connection: {error}");
                    }
                    storage_failed = true;
                    storage_retry = Instant::now() + Duration::from_secs(60);
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    drop(session);
    status.state = "stopped".into();
    status.updated_at = now();
    if let Err(error) = write_json(&status_file, &status) {
        eprintln!("Companion: cannot save stopped status: {error}");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn machine_inventory_has_independent_gates_and_rejects_unsafe_configs() {
        let entries = resolve_machines(br#"{"ssh_hosts":["devbox",{"host":"other.local","enabled":true},{"host":"staging","vpn_domain":"corp.example","enabled":false}]}"#).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].vpn_domain, "quora.net");
        assert_eq!(entries[1].vpn_domain, "local");
        assert_eq!(entries[2].vpn_domain, "corp.example");
        assert!(!entries[2].enabled);
        for data in [
            r#"{"ssh_hosts":["devbox","devbox"]}"#,
            r#"{"ssh_hosts":["-oProxyCommand=bad"]}"#,
            r#"{"ssh_hosts":[{"host":"devbox","vpn_domain":"local"}]}"#,
            r#"{"ssh_hosts":[{"host":"devbox","vpn_domain":"bad domain"}]}"#,
            r#"{"ssh_hosts":["bad\u0000host"]}"#,
            "not json",
        ] {
            assert!(
                resolve_machines(data.as_bytes()).is_err(),
                "accepted {data}"
            );
        }
        assert!(resolve_machines(br#"{"ssh_hosts":[]}"#).unwrap().is_empty());
    }
    #[test]
    fn per_host_connectors_have_independent_lock_names() {
        assert_eq!(host_key("devbox"), host_key("devbox"));
        assert_ne!(host_key("devbox"), host_key("mac.local"));
        assert_eq!(host_key("mac.local").len(), 16);
    }
    #[test]
    fn vpn_gate_matches_domains_only_and_ignores_unrelated_text() {
        let dns = "resolver #1\n search domain[0] : Quora.NET\n domain : ad.quora.com.\n nameserver[0] : 10.0.0.1\n";
        assert!(vpn_present(dns, "quora.net"));
        assert!(vpn_present(dns, "ad.quora.com"));
        assert!(!vpn_present(dns, "net"));
        assert!(!vpn_present("options : quora.net", "quora.net"));
        assert!(!vpn_present(
            "search domain[0] : evilquora.net",
            "quora.net"
        ));
    }
    #[test]
    fn retries_are_bounded_and_do_not_spin() {
        assert_eq!(delay(1, 0), 60);
        assert_eq!(delay(2, 0), 120);
        assert_eq!(delay(5, 0), 900);
        assert_eq!(delay(u32::MAX, 15), 915);
    }
    #[test]
    fn connector_bootstrap_recovers_teardown_race_and_does_not_retry_other_errors() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("hb-bootstrap-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let program = root.join("launchctl");
        let attempts = root.join("attempts");
        std::fs::write(&program, "#!/bin/sh\ncountfile=\"$3\"\ncount=0\n[ ! -f \"$countfile\" ] || count=$(cat \"$countfile\")\ncount=$((count + 1))\nprintf '%s' \"$count\" > \"$countfile\"\n[ \"$count\" -ge 3 ] || exit 5\nexit 0\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        bootstrap_agent(&program, "fixture-domain", &attempts).unwrap();
        assert_eq!(std::fs::read_to_string(&attempts).unwrap(), "3");
        std::fs::write(&program, "#!/bin/sh\nprintf 'x' >> \"$3\"\nexit 64\n").unwrap();
        assert!(
            bootstrap_agent(&program, "fixture-domain", &attempts)
                .unwrap_err()
                .contains("64")
        );
        assert_eq!(std::fs::read_to_string(&attempts).unwrap(), "3x");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn config_rejects_unknown_fields_and_invalid_gate() {
        assert!(
            serde_json::from_str::<Config>(
                r#"{"host":"devbox","vpn_domain":"quora.net","enabled":true,"retry":0}"#
            )
            .is_err()
        );
        assert!(
            validate(&Config {
                host: "-bad".into(),
                vpn_domain: "quora.net".into(),
                enabled: true
            })
            .is_err()
        );
        assert!(
            validate(&Config {
                host: "devbox".into(),
                vpn_domain: "".into(),
                enabled: true
            })
            .is_err()
        );
        assert_eq!(xml("a&<b"), "a&amp;&lt;b");
        for domain in [
            ".",
            ".net",
            "quora..net",
            "quora.net..",
            "-bad.net",
            "bad-.net",
            "quora/net",
        ] {
            assert!(
                validate(&Config {
                    host: "devbox".into(),
                    vpn_domain: domain.into(),
                    enabled: true
                })
                .is_err(),
                "{domain}"
            );
        }
        assert!(
            validate(&Config {
                host: "devbox".into(),
                vpn_domain: format!("{}.net", "a".repeat(64)),
                enabled: true
            })
            .is_err()
        );
        assert!(
            validate(&Config {
                host: "devbox".into(),
                vpn_domain: "QUORA.NET.".into(),
                enabled: true
            })
            .is_ok()
        );
    }
}
