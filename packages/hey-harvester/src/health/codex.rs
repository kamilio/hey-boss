//! Codex is never a TERM/KILL target. Only verified idle interactive sessions get
//! one Ctrl-D through their terminal controller, retaining the normal exit summary.
use super::{
    Item, Observation, Store, now,
    processes::{Process, Table},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) fn is_codex(p: &Process) -> bool {
    let executable = p.executable.to_ascii_lowercase();
    executable.contains("codex")
        || executable.contains("chatgpt.app/")
        || p.arguments.contains("@openai/codex/")
}
fn session_processes(table: &Table) -> BTreeSet<u32> {
    let mut ids: BTreeSet<_> = table
        .values()
        .filter(|p| is_codex(p))
        .map(|p| p.pid)
        .collect();
    loop {
        let before = ids.len();
        for p in table.values() {
            if ids.contains(&p.parent) {
                ids.insert(p.pid);
            }
        }
        if ids.len() == before {
            break;
        }
    }
    ids
}
pub(super) fn family(table: &Table) -> BTreeSet<u32> {
    let mut ids = session_processes(table);
    // Preserve wrappers/ancestors as well; do not expand their unrelated siblings.
    for p in table.values().filter(|p| is_codex(p)) {
        let mut parent = p.parent;
        while parent > 1 && ids.insert(parent) {
            parent = table.get(&parent).map_or(0, |p| p.parent);
        }
    }
    ids
}
fn interactive(p: &Process) -> bool {
    Path::new(&p.executable)
        .file_name()
        .is_some_and(|n| n == "codex")
        && !p.executable.contains(".app/")
        && !p.arguments.split_whitespace().any(|a| {
            matches!(
                a,
                "app-server" | "mcp-server" | "remote-control" | "exec" | "e" | "review"
            )
        })
}
fn idle_session<'a>(p: &Process, agents: &'a [crate::agents::Agent], at: u64) -> Option<&'a str> {
    if !interactive(p) || p.age_seconds < 3600 {
        return None;
    }
    let matches: Vec<_> = agents.iter().filter(|a| a.pid == p.pid).collect();
    if matches.len() != 1 {
        return None;
    }
    let a = matches[0];
    if a.kind != "Codex"
        || a.state != "Idle"
        || a.evidence != "Session file held open by this process"
        || !a.activity_at.is_some_and(|t| at.saturating_sub(t) >= 3600)
        || !a.updated_at.is_some_and(|t| at.saturating_sub(t) >= 300)
    {
        return None;
    }
    a.session_id
        .as_deref()
        .filter(|id| id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
}
fn descendants(p: &Process, table: &Table) -> BTreeSet<u32> {
    let mut ids = BTreeSet::from([p.pid]);
    loop {
        let before = ids.len();
        for p in table.values() {
            if ids.contains(&p.parent) {
                ids.insert(p.pid);
            }
        }
        if before == ids.len() {
            break;
        }
    }
    ids
}

pub(super) fn graceful_idle(
    table: &Table,
    config: &super::Config,
    observations: &mut BTreeMap<String, Observation>,
    apply: bool,
) -> io::Result<(Vec<Item>, usize)> {
    let candidates: Vec<_> = table
        .values()
        .filter(|p| interactive(p) && p.age_seconds >= 3600 && p.uid == unsafe { libc::geteuid() })
        .collect();
    if candidates.is_empty() {
        return Ok((vec![], 0));
    }
    let agents = crate::agents::scan();
    if !agents.warnings.is_empty() {
        return Ok((
            vec![Item {
                name: "Idle Codex cleanup".into(),
                detail: "Session inventory incomplete; Codex preserved".into(),
                eligible: false,
                worktree: None,
                error: None,
            }],
            0,
        ));
    }
    let at = now();
    let mut items = Vec::new();
    let mut stopped = 0;
    let mut self_ancestors = BTreeSet::new();
    let mut pid = std::process::id();
    while pid > 1 && self_ancestors.insert(pid) {
        pid = table.get(&pid).map_or(0, |p| p.parent);
    }
    for p in candidates {
        if self_ancestors.contains(&p.pid) {
            continue;
        }
        let Some(session) = idle_session(p, &agents.agents, at) else {
            continue;
        };
        let key = format!("codex-idle:{}:{}:{session}", p.pid, p.identity);
        let requested = format!("codex-exit:{}:{}:{session}", p.pid, p.identity);
        if observations.contains_key(&requested) {
            continue;
        }
        let ids = descendants(p, table);
        let cpu: f64 = ids.iter().map(|pid| table[pid].cpu_seconds).sum();
        let fingerprint = ids
            .iter()
            .map(|pid| format!("{pid}:{}", table[pid].identity))
            .collect::<Vec<_>>()
            .join("/");
        let previous = observations.get(&key).filter(|o| {
            at.saturating_sub(o.last_seen) <= config.interval_seconds.saturating_mul(3)
                && o.activity_fingerprint.as_ref() == Some(&fingerprint)
                && cpu - o.cpu_seconds < 0.1
        });
        let first = previous.map_or(at, |o| o.first_seen);
        observations.insert(
            key,
            Observation {
                first_seen: first,
                last_seen: at,
                cpu_seconds: cpu,
                activity_fingerprint: Some(fingerprint),
            },
        );
        let ready = at.saturating_sub(first) >= 300;
        let mut detail = if ready {
            "Verified idle for an hour; graceful exit eligible"
        } else {
            "Idle transcript; observing five minutes of unchanged process-tree CPU"
        }
        .to_owned();
        if ready && apply {
            // A fresh transcript check immediately before the only signal. An
            // unresponsive or uncertain session is left running, never escalated.
            let fresh = crate::agents::scan();
            if !fresh.warnings.is_empty() || idle_session(p, &fresh.agents, now()) != Some(session)
            {
                continue;
            }
            let command = format!("codex resume {session}");
            Store::standard()?.save(&format!("codex-resume-{session}.json"),&serde_json::json!({"session_id":session,"resume_command":command,"pid":p.pid,"requested_at":at}))?;
            let delivered = match terminal_exit(p) {
                Ok(value) => value,
                Err(error) => {
                    items.push(Item {
                        name: format!("Idle Codex · PID {}", p.pid),
                        detail: format!("Normal exit unavailable; session preserved: {error}"),
                        eligible: false,
                        worktree: None,
                        error: None,
                    });
                    continue;
                }
            };
            if delivered {
                observations.insert(
                    requested,
                    Observation {
                        first_seen: at,
                        last_seen: at,
                        cpu_seconds: cpu,
                        activity_fingerprint: None,
                    },
                );
                std::thread::sleep(Duration::from_millis(500));
                if super::processes::identity(p.pid).as_deref() != Some(&p.identity) {
                    stopped += 1;
                }
                detail = format!(
                    "Requested normal terminal exit (Ctrl-D); resume receipt saved: {command}; no signals or forced termination"
                );
            } else {
                detail = format!(
                    "No supported terminal controller; session preserved. Resume receipt: {command}"
                );
            }
        }
        items.push(Item {
            name: format!("Idle Codex · PID {}", p.pid),
            detail,
            eligible: ready,
            worktree: None,
            error: None,
        });
    }
    observations.retain(|key, _| {
        !key.starts_with("codex-")
            || table
                .values()
                .any(|p| key.contains(&format!(":{}:{}:", p.pid, p.identity)))
    });
    Ok((items, stopped))
}

/// Active Codex working directories cannot be retired with their running session.
pub(super) fn working_paths(table: &Table) -> io::Result<Vec<PathBuf>> {
    // Process ancestry can include login/session services (even owned ones). They do
    // not own this user's worktrees and procfs correctly refuses their cwd.
    let uid = unsafe { libc::geteuid() };
    let ids: BTreeSet<_> = session_processes(table)
        .into_iter()
        .filter(|pid| table.get(pid).is_some_and(|p| p.uid == uid))
        .collect();
    if ids.is_empty() {
        return Ok(vec![]);
    }
    #[cfg(target_os = "linux")]
    {
        let mut paths = Vec::new();
        for pid in ids {
            match std::fs::read_link(format!("/proc/{pid}/cwd")) {
                Ok(p) => paths.push(p),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(paths)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let list = ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let out = super::output(
            std::process::Command::new("/usr/sbin/lsof")
                .args(["-nP", "-a", "-p", &list, "-d", "cwd", "-Fn"]),
            Duration::from_secs(20),
        )?;
        if !out.status.success() && !out.stderr.is_empty() {
            return Err(io::Error::other("Cannot verify Codex working directories"));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.strip_prefix('n'))
            .map(PathBuf::from)
            .collect())
    }
}

fn terminal_exit(p: &Process) -> io::Result<bool> {
    use std::{
        fs::OpenOptions,
        os::{
            fd::AsRawFd,
            unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
        },
        process::Command,
    };
    #[cfg(target_os = "linux")]
    let terminal = std::fs::read_link(format!("/proc/{}/fd/0", p.pid))?;
    #[cfg(not(target_os = "linux"))]
    let terminal = {
        let out = super::output(
            Command::new("/usr/sbin/lsof").args([
                "-nP",
                "-a",
                "-p",
                &p.pid.to_string(),
                "-d",
                "0",
                "-Fn",
            ]),
            Duration::from_secs(5),
        )?;
        let paths: Vec<_> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.strip_prefix("n/dev/"))
            .map(|p| PathBuf::from(format!("/dev/{p}")))
            .collect();
        if paths.len() != 1 {
            return Ok(false);
        }
        paths[0].clone()
    };
    if !terminal.starts_with("/dev") {
        return Ok(false);
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NOCTTY | libc::O_NONBLOCK)
        .open(&terminal)?;
    let m = file.metadata()?;
    let group = unsafe { libc::getpgid(p.pid as i32) };
    if !m.file_type().is_char_device()
        || m.uid() != p.uid
        || group <= 1
        || unsafe { libc::isatty(file.as_raw_fd()) } != 1
        || unsafe { libc::tcgetpgrp(file.as_raw_fd()) } != group
        || super::processes::identity(p.pid).as_deref() != Some(&p.identity)
    {
        return Ok(false);
    }
    // Writing /dev/tty sends output, not input; TIOCSTI is blocked on modern OSes.
    // Use the terminal's real input controller, addressing only the matched TTY.
    if let Ok(out) = super::output(
        Command::new("tmux").args(["list-panes", "-a", "-F", "#{pane_id} #{pane_tty}"]),
        Duration::from_secs(3),
    ) {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some((pane, tty)) = line.split_once(' ')
                && tty == terminal.to_string_lossy()
                && pane.starts_with('%')
                && pane[1..].bytes().all(|b| b.is_ascii_digit())
            {
                let result = super::output(
                    Command::new("tmux").args(["send-keys", "-t", pane, "C-d"]),
                    Duration::from_secs(3),
                )?;
                return Ok(result.status.success());
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        const SCRIPT: &str = r#"on run argv
if application id "com.googlecode.iterm2" is running then
 tell application id "com.googlecode.iterm2"
  repeat with w in windows
   repeat with t in tabs of w
    repeat with s in sessions of t
     if tty of s is (item 1 of argv) then
      tell s to write text (ASCII character 4) newline NO
      return "sent"
     end if
    end repeat
   end repeat
  end repeat
 end tell
end if
return "unavailable"
end run"#;
        let out = super::output(
            Command::new("/usr/bin/osascript")
                .args(["-e", SCRIPT, "--"])
                .arg(&terminal),
            Duration::from_secs(5),
        )?;
        Ok(out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "sent")
    }
    #[cfg(not(target_os = "macos"))]
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_codex_and_its_children_and_wrapper_are_never_force_candidates() {
        let codex = Process {
            pid: 100,
            parent: 90,
            uid: 501,
            age_seconds: 999999,
            cpu_seconds: 1.0,
            executable: "/bin/codex".into(),
            identity: "id".into(),
            arguments: "codex".into(),
        };
        let mut child = codex.clone();
        child.pid = 101;
        child.parent = 100;
        child.executable = "/bin/node".into();
        let mut wrapper = child.clone();
        wrapper.pid = 90;
        wrapper.parent = 1;
        let mut unrelated = child.clone();
        unrelated.pid = 102;
        unrelated.parent = 1;
        let table = Table::from([(100, codex), (101, child), (90, wrapper), (102, unrelated)]);
        assert_eq!(family(&table), BTreeSet::from([90, 100, 101]));
        assert_eq!(session_processes(&table), BTreeSet::from([100, 101]));
    }
    #[test]
    fn requires_live_idle_evidence_and_never_expires_an_app_server() {
        let mut p = Process {
            pid: 100,
            parent: 1,
            uid: 501,
            age_seconds: 999999,
            cpu_seconds: 1.0,
            executable: "/bin/codex".into(),
            identity: "id".into(),
            arguments: "codex".into(),
        };
        let mut a:crate::agents::Agent=serde_json::from_value(serde_json::json!({"id":"fixture","pid":100,"kind":"Codex","session_id":"11111111-1111-1111-1111-111111111111","state":"Working","evidence":"Session file held open by this process","activity_at":1,"updated_at":1})).unwrap();
        assert!(idle_session(&p, &[a.clone()], 99999).is_none());
        a.state = "Idle".into();
        assert!(idle_session(&p, &[a.clone()], 99999).is_some());
        a.evidence = "Resume target".into();
        assert!(idle_session(&p, &[a.clone()], 99999).is_none());
        a.evidence = "Session file held open by this process".into();
        a.activity_at = Some(99000);
        assert!(idle_session(&p, &[a.clone()], 99999).is_none());
        a.activity_at = Some(1);
        p.arguments = "codex app-server".into();
        assert!(idle_session(&p, &[a], 99999).is_none());
    }
}
