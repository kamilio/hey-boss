use super::{Actor, Error, Project, Result, identifier};
use std::path::Path;

pub fn machine() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let mut id = [0_u8; 16];
        let timeout = libc::timespec {
            tv_sec: 2,
            tv_nsec: 0,
        };
        if unsafe { libc::gethostuuid(id.as_mut_ptr(), &timeout) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(id.iter().map(|b| format!("{b:02x}")).collect())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let value = std::fs::read_to_string("/etc/machine-id")
            .or_else(|_| std::fs::read_to_string("/var/lib/dbus/machine-id"))?;
        let value = value.trim();
        identifier(value, "machine ID", 256)?;
        Ok(value.into())
    }
}

pub fn host() -> String {
    let mut bytes = [0_u8; 256];
    if unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } == 0 {
        String::from_utf8_lossy(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())])
            .into()
    } else {
        "unknown".into()
    }
}

pub fn project(cwd: &Path, machine: &str) -> Result<Project> {
    let cwd = cwd.canonicalize()?;
    let directory = cwd
        .to_str()
        .ok_or_else(|| Error::invalid("Project path must be UTF-8"))?;
    if let Some(git) = crate::agents::git_info(directory) {
        return Ok(project_from_git(&git, machine));
    }
    Ok(Project {
        id: format!("local:{machine}:{directory}"),
        name: cwd
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("directory")
            .into(),
    })
}

pub(crate) fn is_home_project(project: &Project) -> bool {
    let Some((_, directory)) = project
        .id
        .strip_prefix("local:")
        .and_then(|id| id.split_once(':'))
    else {
        return false;
    };
    let directory = Path::new(directory);
    // RPCs and fleet replicas can describe another machine's home directory.
    if directory == Path::new("/root")
        || directory
            .parent()
            .is_some_and(|parent| parent == Path::new("/home") || parent == Path::new("/Users"))
    {
        return true;
    }
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let home = std::path::PathBuf::from(home);
    let home = home.canonicalize().unwrap_or(home);
    directory == home
}

/// Temporary agent checkouts are not durable projects. Inspect the path encoded
/// in local identities so the same rule works for disconnected fleet machines.
/// Git remotes and explicitly named projects remain independent of their paths.
pub(crate) fn is_temporary_project(id: &str) -> bool {
    let Some((_, directory)) = id.strip_prefix("local:").and_then(|id| id.split_once(':')) else {
        return false;
    };
    let directory = Path::new(directory);
    if ["/tmp", "/private/tmp", "/var/tmp", "/private/var/tmp"]
        .iter()
        .any(|root| directory.starts_with(root))
    {
        return true;
    }
    // macOS per-user temporary roots have two variable components before T.
    for root in ["/var/folders", "/private/var/folders"] {
        if let Ok(relative) = directory.strip_prefix(root) {
            let mut components = relative.components();
            if components.next().is_some()
                && components.next().is_some()
                && components.next().is_some_and(|c| c.as_os_str() == "T")
            {
                return true;
            }
        }
    }
    directory.starts_with(std::env::temp_dir())
}

pub(crate) fn is_git_metadata_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == ".git")
}

pub(crate) fn is_git_metadata_project(id: &str) -> bool {
    id.strip_prefix("local:")
        .and_then(|id| id.split_once(':'))
        .is_some_and(|(_, path)| is_git_metadata_path(Path::new(path)))
}

pub(crate) fn project_from_git(git: &crate::agents::GitInfo, machine: &str) -> Project {
    let id = git
        .origin
        .clone()
        .unwrap_or_else(|| format!("local:{machine}:{}", git.repository_root));
    let name = git
        .origin
        .as_deref()
        .and_then(|s| s.rsplit('/').next())
        .or_else(|| {
            Path::new(&git.repository_root)
                .file_name()
                .and_then(|s| s.to_str())
        })
        .unwrap_or("repository")
        .to_owned();
    Project { id, name }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

pub fn resolve(explicit: Option<&str>, machine: &str, cwd: &Path) -> Result<Actor> {
    let origin = crate::Origin::capture();
    let mut actor = Actor {
        id: String::new(),
        kind: "explicit".into(),
        session_id: None,
        machine: machine.into(),
        host: host(),
        pid: None,
        process_start: None,
        cwd: cwd.into(),
        source: String::new(),
        invocation: None,
        creation_run: None,
    };
    let configured = explicit
        .map(str::to_owned)
        .or_else(|| env("HEY_BOSS_AGENT_ID"));
    if let Some(id) = configured {
        identifier(&id, "agent ID", 512)?;
        actor.id = id;
        actor.source = if explicit.is_some() {
            "--agent"
        } else {
            "HEY_BOSS_AGENT_ID"
        }
        .into();
    } else if let Some(id) = env("CODEX_THREAD_ID") {
        identifier(&id, "Codex thread ID", 256)?;
        actor.id = format!("codex:{id}");
        actor.kind = "codex".into();
        actor.session_id = Some(id);
        actor.source = "CODEX_THREAD_ID".into();
    } else {
        // Only a uniquely matched session belonging to an ancestor is evidence of
        // the caller. In particular, never pick the newest transcript in the cwd.
        let snapshot = crate::agents::scan();
        if let Some(found) = ancestor_session(&origin.launchers, &snapshot.agents) {
            let kind = found.kind.to_ascii_lowercase();
            let session = found.session_id.clone().unwrap();
            actor.id = format!("{kind}:{session}");
            actor.kind = kind;
            actor.session_id = Some(session);
            actor.pid = Some(found.pid);
            actor.source = "verified ancestor session".into();
        }
        if actor.id.is_empty() {
            return Err(Error::new(
                "identity_unavailable",
                "Cannot identify this agent session. Set HEY_BOSS_AGENT_ID to a stable session ID or pass --agent ID (for a terminal, use --agent human:NAME).",
            ));
        }
    }
    // A launcher is diagnostic metadata, never the durable owner ID. Explicit
    // identities do not imply that their parent shell is an agent process.
    if actor.kind == "codex" && actor.pid.is_none() {
        actor.pid = origin
            .launchers
            .iter()
            .find(|p| p.executable.file_name().is_some_and(|n| n == "codex"))
            .map(|p| p.pid);
    }
    actor.process_start = actor.pid.and_then(crate::agents::process_identity);
    // Stable explicit Codex identities also carry their actual session. Human
    // callers never inherit the agent environment that launched their terminal.
    if actor.session_id.is_none() && actor.id.starts_with("codex:") {
        actor.session_id = actor.id.strip_prefix("codex:").map(str::to_owned);
        actor.kind = "codex".into();
    }
    Ok(actor)
}

/// Capture only for creations, on the caller's machine before remote transport.
pub fn creation_context(actor: &mut Actor) {
    if actor.session_id.is_none() {
        return;
    }
    actor.invocation = (actor.kind == "codex")
        .then(|| {
            actor
                .session_id
                .as_deref()
                .and_then(crate::agent_conversations::invocation)
        })
        .flatten();
    if let Ok(path) = super::database_path()
        && let Ok(db) =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    {
        let _ = db.busy_timeout(std::time::Duration::from_millis(100));
        actor.creation_run = super::provenance::source_run(&db, actor, super::worker::now())
            .ok()
            .flatten();
    }
}

fn ancestor_session<'a>(
    launchers: &[crate::Launcher],
    agents: &'a [crate::agents::Agent],
) -> Option<&'a crate::agents::Agent> {
    for launcher in launchers {
        let candidates: Vec<_> = agents.iter().filter(|a| a.pid == launcher.pid).collect();
        if candidates.len() == 1 {
            let found = candidates[0];
            // An unresolved child must never inherit its parent agent's identity.
            // Cached or merely requested sessions are insufficient evidence.
            return (found.session_id.is_some()
                && (found.evidence == "Session file held open by this process"
                    || found.evidence
                        == "Live PID-specific Claude session metadata (process start verified)"))
                .then_some(found);
        }
        if candidates.len() > 1
            || launcher
                .executable
                .file_name()
                .is_some_and(|n| n == "codex" || n == "claude")
        {
            return None;
        }
    }
    None
}

/// Process liveness is advisory. A stopped process can be a resumable session;
/// it never causes an automatic release. Remote processes remain unknown.
pub fn presence(actor: &Actor, local_machine: &str) -> &'static str {
    if actor.machine != local_machine {
        return "unknown";
    }
    match (actor.pid, actor.process_start.as_deref()) {
        (Some(pid), Some(start)) => match crate::agents::process_identity(pid) {
            Some(current) if current == start => "running",
            Some(_) => "stale",
            None => {
                if unsafe { libc::kill(pid as i32, 0) } == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    "stale"
                } else {
                    "unknown"
                }
            }
        },
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn git_metadata_is_an_exact_local_path_component() {
        for path in [
            "/workspace/.git",
            "/workspace/.git/objects",
            "/workspace/.git/worktrees/topic",
        ] {
            assert!(is_git_metadata_project(&format!("local:remote:{path}")));
        }
        for id in [
            "local:remote:/workspace/project",
            "local:remote:/workspace/.github",
            "local:remote:/workspace/repo.git",
            "named:.git",
            "github.com/example/.git",
        ] {
            assert!(!is_git_metadata_project(id));
        }
    }
    #[test]
    fn home_projects_are_ignored_across_machines_but_checkouts_are_kept() {
        for path in ["/home/remote", "/Users/remote", "/root"] {
            assert!(is_home_project(&Project {
                id: format!("local:remote:{path}"),
                name: "Home".into()
            }));
        }
        for id in [
            "local:remote:/home/remote/project",
            "local:remote:/Users/remote/project",
            "local:remote:/root/project",
            "github.com/example/home",
            "named:Home",
        ] {
            assert!(!is_home_project(&Project {
                id: id.into(),
                name: "Project".into()
            }));
        }
    }
    fn agent(pid: u32, session: Option<&str>, evidence: &str) -> crate::agents::Agent {
        crate::agents::Agent {
            id: format!("pid-{pid}"),
            pid,
            kind: "Codex".into(),
            cwd: None,
            session_id: session.map(str::to_owned),
            task: None,
            title: None,
            activity: None,
            state: "Session open".into(),
            updated_at: None,
            evidence: evidence.into(),
            update: None,
            activity_at: None,
            git: None,
        }
    }
    #[test]
    fn unresolved_child_and_ambiguous_sessions_never_borrow_parent_identity() {
        let launchers = [
            crate::Launcher {
                pid: 10,
                executable: "/bin/sh".into(),
            },
            crate::Launcher {
                pid: 20,
                executable: "/bin/codex".into(),
            },
            crate::Launcher {
                pid: 30,
                executable: "/bin/codex".into(),
            },
        ];
        let live = "Session file held open by this process";
        let parent = agent(30, Some("parent"), live);
        assert!(
            ancestor_session(
                &launchers,
                &[agent(20, None, "unavailable"), parent.clone()]
            )
            .is_none()
        );
        assert!(
            ancestor_session(
                &launchers,
                &[
                    agent(20, Some("a"), live),
                    agent(20, Some("b"), live),
                    parent.clone()
                ]
            )
            .is_none()
        );
        assert!(
            ancestor_session(
                &launchers,
                &[
                    agent(20, Some("old"), "Previously observed transcript"),
                    parent.clone()
                ]
            )
            .is_none()
        );
        assert!(ancestor_session(&launchers, &[parent]).is_none());
        let agents = [agent(20, Some("child"), live)];
        assert_eq!(
            ancestor_session(&launchers, &agents)
                .unwrap()
                .session_id
                .as_deref(),
            Some("child")
        );
    }
    #[test]
    fn presence_is_advisory_and_rejects_reused_pids() {
        let pid = std::process::id();
        let mut actor = Actor {
            id: "session".into(),
            kind: "explicit".into(),
            session_id: None,
            machine: "local".into(),
            host: "host".into(),
            pid: Some(pid),
            process_start: crate::agents::process_identity(pid),
            cwd: "/".into(),
            source: "test".into(),
            invocation: None,
            creation_run: None,
        };
        assert!(actor.process_start.is_some());
        assert_eq!(presence(&actor, "local"), "running");
        assert_eq!(presence(&actor, "another-machine"), "unknown");
        actor.process_start = Some("different-start".into());
        assert_eq!(presence(&actor, "local"), "stale");
        actor.pid = None;
        assert_eq!(presence(&actor, "local"), "unknown");
    }
}
