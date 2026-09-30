//! Fleet-wide inventory and explicitly chosen skill versions. Network work never blocks page reads.
use super::{atomic_write, is_valid_skill_name};
use crate::issues::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const TRANSPORT: &str = include_str!("transport.py");
const DEFAULT_WORDS: usize = 400;

#[derive(Clone, Default, Serialize, Deserialize)]
struct State {
    revision: u64,
    #[serde(default)]
    machines: Vec<Value>,
    #[serde(default)]
    choices: BTreeMap<String, String>,
    #[serde(default = "default_words")]
    max_words: usize,
    #[serde(skip)]
    busy: bool,
    #[serde(default)]
    message: String,
}
fn default_words() -> usize {
    DEFAULT_WORDS
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| Error::invalid("HOME is missing"))
}
fn state_path(home: &Path) -> PathBuf {
    home.join(".hey-boss/skill-manager.json")
}
fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| {
        Mutex::new(
            home()
                .ok()
                .and_then(|h| fs::read(state_path(&h)).ok())
                .and_then(|s| serde_json::from_slice(&s).ok())
                .unwrap_or_else(|| State {
                    max_words: DEFAULT_WORDS,
                    ..State::default()
                }),
        )
    })
}
fn persist(home: &Path, state: &State) -> Result<()> {
    atomic_write(&state_path(home), &serde_json::to_vec(state)?)?;
    fs::set_permissions(state_path(home), fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Concrete portability findings, never a claim that arbitrary commands were tested.
pub fn lint(name: &str, text: &str, files: &[Value], max_words: usize) -> Vec<Value> {
    let mut out = Vec::new();
    let mut warn = |kind: &str, message: String, line: usize| {
        out.push(json!({"kind":kind,"message":message,"line":line}))
    };
    let body = text
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---"));
    let metadata = body.and_then(|(front, _)| serde_yaml_ng::from_str::<Value>(front).ok());
    match metadata {
        Some(ref meta) if meta.is_object() => {
            if meta["name"].as_str() != Some(name) {
                warn(
                    "metadata",
                    format!("Set name to {name} so both agents identify the same skill."),
                    1,
                );
            }
            if meta["description"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
            {
                warn(
                    "metadata",
                    "Add a description that says when to use this skill.".into(),
                    1,
                );
            }
            for key in [
                "allowed-tools",
                "context",
                "agent",
                "disable-model-invocation",
            ] {
                if meta.get(key).is_some() {
                    warn(
                        "agent_specific",
                        format!(
                            "{key} is agent-specific metadata; keep required behavior in the instructions too."
                        ),
                        1,
                    );
                }
            }
        }
        _ => warn(
            "metadata",
            "Add valid YAML frontmatter with name and description for Codex and Claude.".into(),
            1,
        ),
    }
    let words = text.split_whitespace().count();
    if words > max_words {
        warn(
            "too_long",
            format!(
                "{words} words exceeds your {max_words}-word budget. Move advanced details into references."
            ),
            1,
        );
    }
    for (index, line) in text.lines().enumerate() {
        if ["/Users/", "/home/", "C:\\Users\\"]
            .iter()
            .any(|s| line.contains(s))
        {
            warn(
                "machine_path",
                "Replace this machine-specific path with a relative path or a discovered location."
                    .into(),
                index + 1,
            );
        }
        if [
            "functions.exec",
            "functions.apply_patch",
            "multi_tool_use",
            "mcp__",
            "TodoWrite",
            "AskUserQuestion",
            "request_user_input",
            "spawn_agent",
        ]
        .iter()
        .any(|s| line.contains(s))
        {
            warn("agent_tool", "This names an agent-specific tool. Describe the capability and provide a CLI or other fallback.".into(), index + 1);
        }
    }
    let known: BTreeSet<&str> = files.iter().filter_map(|f| f["path"].as_str()).collect();
    for event in pulldown_cmark::Parser::new(text) {
        if let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link { dest_url, .. }) = event {
            let dest = dest_url
                .split(['#', '?'])
                .next()
                .unwrap_or("")
                .trim_start_matches("./");
            if !dest.is_empty()
                && !dest.contains(':')
                && !dest.starts_with('/')
                && !known.contains(dest)
            {
                warn(
                    "broken_reference",
                    format!("Bundle is missing linked file {dest}."),
                    0,
                );
            }
        }
    }
    out
}
fn annotate(machine: &mut Value, max_words: usize) {
    for copy in machine["copies"].as_array_mut().into_iter().flatten() {
        let text = copy["text"].as_str().unwrap_or("").replace("\r\n", "\n");
        copy["warnings"] = json!(lint(
            copy["name"].as_str().unwrap_or(""),
            &text,
            copy["files"].as_array().unwrap_or(&vec![]),
            max_words
        ));
        copy["word_count"] = json!(text.split_whitespace().count());
        let description = text
            .strip_prefix("---\n")
            .and_then(|s| s.split_once("\n---"))
            .and_then(|(front, _)| serde_yaml_ng::from_str::<Value>(front).ok())
            .and_then(|meta| meta["description"].as_str().map(str::to_owned))
            .unwrap_or_else(|| super::extract_description(&text));
        copy["description"] = json!(description);
    }
}

fn hosts(home: &Path) -> Result<Vec<String>> {
    let mut hosts = BTreeSet::new();
    let state_dir = std::env::var_os("HEY_BOSS_FLEET_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share/hey-boss"));
    if let Ok(text) = fs::read_to_string(state_dir.join("companion-hosts")) {
        hosts.extend(text.lines().map(str::to_owned));
    }
    let config = std::env::var_os("HEY_BOSS_FLEET_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".hey-boss/config.json"));
    if config.exists() {
        let value: Value = serde_json::from_slice(&fs::read(config)?)?;
        for entry in value["ssh_hosts"].as_array().into_iter().flatten() {
            if let Some(host) = entry.as_str().or_else(|| entry["host"].as_str()) {
                hosts.insert(host.to_owned());
            }
        }
    }
    let desired = std::env::var_os("HEY_BOSS_FLEET_DESIRED")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".hey-boss/fleet.yaml"));
    if desired.exists() {
        let value: Value = serde_yaml_ng::from_slice(&fs::read(desired)?)
            .map_err(|e| Error::invalid(format!("Cannot read fleet machines: {e}")))?;
        hosts.extend(
            value["machines"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(h, _)| h.clone()),
        );
    }
    hosts.remove("local");
    if hosts.len() > 64 || hosts.iter().any(|h| !crate::health::remote::valid_host(h)) {
        return Err(Error::invalid("Invalid machine inventory"));
    }
    Ok(std::iter::once("local".into()).chain(hosts).collect())
}
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
fn transport(home: &Path, host: &str, input: &Value) -> Result<Value> {
    let temp = crate::admin::Temporary::new()?;
    fs::write(temp.0.join("input"), serde_json::to_vec(input)?)?;
    let mut command = if host == "local" {
        let mut c = Command::new("python3");
        c.args(["-c", TRANSPORT]).env("HOME", home);
        c
    } else {
        let mut c = Command::new("ssh");
        c.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=6",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            "--",
            host,
        ]);
        c.arg(format!("python3 -c {}", quote(TRANSPORT)));
        c
    };
    let mut child = command
        .process_group(0)
        .stdin(fs::File::open(temp.0.join("input"))?)
        .stdout(fs::File::create(temp.0.join("output"))?)
        .stderr(fs::File::create(temp.0.join("error"))?)
        .spawn()?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > Duration::from_secs(35)
            || fs::metadata(temp.0.join("output"))?.len() > 96 * 1024 * 1024
        {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            return Err(Error::invalid(
                "Machine timed out or inventory exceeded 96 MiB; scan again when available",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        let mut message = String::new();
        fs::File::open(temp.0.join("error"))?
            .take(4000)
            .read_to_string(&mut message)?;
        return Err(Error::invalid(if message.trim().is_empty() {
            "Skill transfer failed"
        } else {
            message.trim()
        }));
    }
    let mut bytes = Vec::new();
    fs::File::open(temp.0.join("output"))?
        .take(96 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 96 * 1024 * 1024 {
        return Err(Error::invalid("Inventory too large"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn public_report(state: &State, selected: BTreeSet<String>) -> Value {
    let machines: Vec<Value> = state
        .machines
        .iter()
        .map(|machine| {
            let mut result: serde_json::Map<String, Value> = machine
                .as_object()
                .unwrap()
                .iter()
                .filter(|(key, _)| key.as_str() != "copies")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            result.insert(
                "copies".into(),
                Value::Array(
                    machine["copies"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|copy| {
                            Value::Object(
                                copy.as_object()
                                    .unwrap()
                                    .iter()
                                    .filter(|(key, _)| key.as_str() != "files")
                                    .map(|(key, value)| (key.clone(), value.clone()))
                                    .collect(),
                            )
                        })
                        .collect(),
                ),
            );
            Value::Object(result)
        })
        .collect();
    json!({"ok":true,"revision":state.revision,"busy":state.busy,"message":state.message,"machines":machines,"selected":selected,"choices":state.choices,"max_words":state.max_words})
}
pub fn report() -> Result<Value> {
    let home = home()?;
    let mut current = state().lock().unwrap();
    if !current.busy
        && let Ok(bytes) = fs::read(state_path(&home))
    {
        let saved: State = serde_json::from_slice(&bytes)?;
        if saved.revision > current.revision {
            *current = saved;
        }
    }
    Ok(public_report(&current, super::selected_skills(&home)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    action: String,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    selected: Vec<String>,
    #[serde(default)]
    choices: BTreeMap<String, String>,
    #[serde(default = "default_words")]
    max_words: usize,
}
fn chosen_sources(
    state: &State,
    selected: &[String],
    choices: &BTreeMap<String, String>,
) -> Result<Vec<Value>> {
    let mut sources = Vec::new();
    for name in selected {
        if !is_valid_skill_name(name) {
            return Err(Error::invalid("Invalid skill name"));
        }
        let candidates: Vec<&Value> = state
            .machines
            .iter()
            .flat_map(|m| m["copies"].as_array().into_iter().flatten())
            .filter(|c| c["name"] == *name && c["scope"] == "global")
            .collect();
        let unique: BTreeSet<&str> = candidates
            .iter()
            .filter_map(|c| c["digest"].as_str())
            .collect();
        let choice = choices.get(name).map(String::as_str).or_else(|| {
            if unique.len() == 1 {
                unique.first().copied()
            } else {
                None
            }
        });
        let source = candidates.iter().find(|c| c["digest"].as_str() == choice);
        match source {
            Some(source) => sources.push((*source).clone()),
            None if name == "hey-boss" && candidates.is_empty() => {}
            _ => {
                return Err(Error::conflict(format!(
                    "Choose a source version for {name} before distributing"
                )));
            }
        }
    }
    Ok(sources)
}
struct LibraryStage(PathBuf);
impl LibraryStage {
    fn new(parent: &Path) -> Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let path = parent.join(format!(
            ".pending-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for LibraryStage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn save_library(home: &Path, sources: &[Value]) -> Result<()> {
    for source in sources {
        let parent = home.join(".hey-boss/skills");
        fs::create_dir_all(&parent)?;
        let target = parent.join(source["name"].as_str().unwrap());
        let stage = LibraryStage::new(&parent)?;
        let bundle = stage.0.join("bundle");
        fs::create_dir(&bundle)?;
        for file in source["files"].as_array().unwrap() {
            let relative = Path::new(file["path"].as_str().unwrap());
            if relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err(Error::invalid("Invalid skill file path"));
            }
            let path = bundle.join(relative);
            let bytes = STANDARD
                .decode(file["data"].as_str().unwrap())
                .map_err(|_| Error::invalid("Invalid bundle"))?;
            atomic_write(&path, &bytes)?;
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(if file["executable"] == true {
                    0o755
                } else {
                    0o644
                }),
            )?;
        }
        // The library owns this directory. Agent copies are backed up by the installer.
        let previous = stage.0.join("previous");
        if target.exists() {
            fs::rename(&target, &previous)?;
        }
        if let Err(error) = fs::rename(bundle, &target) {
            if previous.exists() {
                let _ = fs::rename(previous, target);
            }
            return Err(error.into());
        }
    }
    Ok(())
}

pub fn act(action: Action) -> Result<Value> {
    let home = home()?;
    fs::create_dir_all(home.join(".hey-boss"))?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join(".hey-boss/skill-manager.lock"))?;
    use std::os::fd::AsRawFd;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(Error::conflict(
            "Another skill manager is scanning or distributing. Retry shortly.",
        ));
    }
    let mut state = state().lock().unwrap();
    if state.busy {
        return Err(Error::conflict(
            "A skill scan or distribution is already running",
        ));
    }
    if !["scan", "distribute"].contains(&action.action.as_str()) {
        return Err(Error::invalid("Unknown skill action"));
    }
    if let Ok(bytes) = fs::read(state_path(&home)) {
        let saved: State = serde_json::from_slice(&bytes)?;
        if saved.revision > state.revision {
            *state = saved;
        }
    }
    let hosts = hosts(&home)?;
    let sources = if action.action == "distribute" {
        if action.revision != state.revision {
            return Err(Error::conflict(
                "Inventory changed. Refresh before distributing; your selection is preserved.",
            ));
        }
        if !(50..=2000).contains(&action.max_words) {
            return Err(Error::invalid("Word budget must be between 50 and 2000"));
        }
        let sources = chosen_sources(&state, &action.selected, &action.choices)?;
        save_library(&home, &sources)?;
        super::set_selected_skills(&home, &action.selected)?;
        state.choices = sources
            .iter()
            .map(|s| {
                (
                    s["name"].as_str().unwrap().to_owned(),
                    s["digest"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        state.max_words = action.max_words;
        sources
    } else {
        vec![]
    };
    state.revision += 1;
    state.busy = true;
    state.message = if action.action == "scan" {
        "Discovering skills across your machines…"
    } else {
        "Distributing selected versions…"
    }
    .into();
    let snapshot = state.clone();
    if let Err(error) = persist(&home, &state) {
        state.busy = false;
        return Err(error);
    }
    let initial = public_report(&state, super::selected_skills(&home));
    std::thread::spawn(move || {
        let _lock = lock;
        let outcome = std::panic::catch_unwind(|| {
            run(
                &home,
                &hosts,
                &snapshot,
                &sources,
                action.action == "distribute",
            )
        });
        let mut current = self::state().lock().unwrap();
        current.busy = false;
        current.revision += 1;
        if outcome.is_err() {
            current.message =
                "Skill operation interrupted. Scan again to verify machine state.".into();
        }
        if let Err(error) = persist(&home, &current) {
            current.message = format!("Could not save inventory: {error}");
        }
    });
    Ok(initial)
}
fn run(home: &Path, hosts: &[String], snapshot: &State, sources: &[Value], distribute: bool) {
    for batch in hosts.chunks(4) {
        std::thread::scope(|scope| {
            let handles: Vec<_> = batch.iter().map(|host| scope.spawn(move || {
                let previous = snapshot.machines.iter().find(|m| m["host"] == *host);
                let expected = previous.map(|m| m["copies"].clone()).unwrap_or(json!([]));
                // Never overwrite an unseen machine. First scan its copies for review.
                let input = if distribute {
                    if previous.is_none() { return (host.clone(), Err(Error::conflict("New machine: scan before distributing"))); }
                    json!({"action":"install","sources":sources,"expected":expected})
                } else { json!({"action":"scan", "project": if host == "local" { std::env::current_dir().ok() } else { None }}) };
                (host.clone(), transport(home, host, &input))
            })).collect();
            for handle in handles {
                let (host, result) = handle.join().unwrap();
                let mut current = state().lock().unwrap();
                let previous = current.machines.iter().position(|m| m["host"] == host);
                let mut machine = match result {
                    Ok(mut value) => {
                        if distribute && let Some(index) = previous {
                            let project_copies: Vec<Value> = current.machines[index]["copies"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter(|c| c["scope"] == "project")
                                .cloned()
                                .collect();
                            value["copies"]
                                .as_array_mut()
                                .unwrap()
                                .extend(project_copies);
                        }
                        value["state"] = json!(if distribute { "synced" } else { "online" });
                        value["scanned_at"] = json!(
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs()
                        );
                        value["error"] = Value::Null;
                        value
                    }
                    Err(error) => {
                        let mut value = previous
                            .map(|i| current.machines[i].clone())
                            .unwrap_or(json!({"copies":[],"errors":[]}));
                        value["state"] = json!("attention");
                        value["error"] = json!(error.to_string());
                        value
                    }
                };
                machine["host"] = json!(host);
                annotate(&mut machine, current.max_words);
                if let Some(i) = previous {
                    current.machines[i] = machine;
                } else {
                    current.machines.push(machine);
                }
            }
        });
    }
    let mut current = state().lock().unwrap();
    let failures = current
        .machines
        .iter()
        .filter(|m| {
            m["state"] == "attention" || m["errors"].as_array().is_some_and(|a| !a.is_empty())
        })
        .count();
    current.message = if failures > 0 {
        format!(
            "{} finished · {failures} machine(s) need attention. Review results and retry.",
            if distribute { "Distribution" } else { "Scan" }
        )
    } else if distribute {
        "Selected skills distributed. Codex and Claude may need a new session to reload them."
            .into()
    } else {
        "Inventory refreshed. Choose the skills and versions you want everywhere.".into()
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lint_finds_portability_issues_and_broken_links() {
        let text = "---\nname: example\ndescription: Work\nallowed-tools: Bash\n---\nUse functions.exec in /Users/alex/repo. Read [guide](references/missing.md).";
        let warnings = lint("example", text, &[], 400);
        for kind in [
            "agent_specific",
            "agent_tool",
            "machine_path",
            "broken_reference",
        ] {
            assert!(warnings.iter().any(|w| w["kind"] == kind), "{kind}");
        }
        assert!(
            lint(
                "example",
                "---\nname: example\ndescription: Work\n---\nUse git status.",
                &[],
                400
            )
            .is_empty()
        );
    }
    #[test]
    fn lint_honors_word_budget_and_resolves_bundled_references() {
        let text = "---\nname: short\ndescription: A concise workflow\n---\nRead [details](references/nested/details.md#usage).";
        let files = vec![json!({"path":"references/nested/details.md"})];
        assert!(lint("short", text, &files, 400).is_empty());
        assert!(
            lint("short", text, &files, 5)
                .iter()
                .any(|w| w["kind"] == "too_long")
        );
        assert!(
            lint("short", "---\nname: [broken\n---\n", &[], 400)
                .iter()
                .any(|w| w["kind"] == "metadata")
        );
    }

    #[test]
    fn conflicting_versions_require_an_explicit_choice_including_remote_only_skills() {
        let state = State {
            machines: vec![
                json!({"host":"a","copies":[{"name":"stacked-prs","scope":"global","digest":"one"}]}),
                json!({"host":"b","copies":[{"name":"stacked-prs","scope":"global","digest":"two"},{"name":"remote-only","scope":"global","digest":"three"}]}),
            ],
            ..State::default()
        };
        assert!(chosen_sources(&state, &["stacked-prs".into()], &BTreeMap::new()).is_err());
        let chosen = chosen_sources(
            &state,
            &["stacked-prs".into(), "remote-only".into()],
            &BTreeMap::from([("stacked-prs".into(), "two".into())]),
        )
        .unwrap();
        assert_eq!(chosen[0]["digest"], "two");
        assert_eq!(chosen[1]["digest"], "three");
        assert!(
            chosen_sources(
                &state,
                &["stacked-prs".into()],
                &BTreeMap::from([("stacked-prs".into(), "stale".into())])
            )
            .is_err()
        );
    }
}
