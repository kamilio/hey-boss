//! Best-effort tab naming, matching poe-code's terminal-name behavior.
use super::text;
use serde_json::Value;
use std::{
    io::{self, IsTerminal, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, PartialEq, Eq)]
enum Target {
    Tmux(String),
    ITerm,
}

fn target(
    tmux: Option<&str>,
    pane: Option<&str>,
    program: Option<&str>,
    tty: bool,
) -> Option<Target> {
    if tmux.is_some_and(|value| !value.is_empty()) {
        pane.filter(|value| !value.is_empty())
            .map(|value| Target::Tmux(value.into()))
    } else if program == Some("iTerm.app") && tty {
        Some(Target::ITerm)
    } else {
        None
    }
}

pub(super) fn set(name: &str) {
    let title = text(&Value::String(name.into()));
    let title = title.trim();
    if title.is_empty() {
        return;
    }
    let tmux = std::env::var("TMUX").ok();
    let pane = std::env::var("TMUX_PANE").ok();
    let program = std::env::var("TERM_PROGRAM").ok();
    match target(
        tmux.as_deref(),
        pane.as_deref(),
        program.as_deref(),
        io::stdout().is_terminal(),
    ) {
        Some(Target::Tmux(pane)) => {
            // Arguments are literal; never rename a different pane's window.
            let Ok(mut child) = Command::new("tmux")
                .args(["rename-window", "-t", &pane, "--", title])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                return;
            };
            let deadline = Instant::now() + Duration::from_millis(500);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        Some(Target::ITerm) => {
            // OSC 1 names the tab without replacing the window title.
            let mut stdout = io::stdout().lock();
            let _ = write!(stdout, "\x1b]1;{title}\x07");
            let _ = stdout.flush();
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_targets_only_the_original_pane_and_takes_precedence_over_iterm() {
        assert_eq!(
            target(Some("socket"), Some("%42"), Some("iTerm.app"), true),
            Some(Target::Tmux("%42".into()))
        );
        assert_eq!(target(Some("socket"), None, Some("iTerm.app"), true), None);
        assert_eq!(target(Some("socket"), Some(""), None, true), None);
    }

    #[test]
    fn only_interactive_iterm_output_receives_a_tab_title() {
        assert_eq!(
            target(None, None, Some("iTerm.app"), true),
            Some(Target::ITerm)
        );
        assert_eq!(
            target(Some(""), None, Some("iTerm.app"), true),
            Some(Target::ITerm)
        );
        assert_eq!(target(None, None, Some("iTerm.app"), false), None);
        assert_eq!(target(None, None, Some("other"), true), None);
    }
}
