//! Standalone-friendly CLI tests: the only git on PATH is a local recording script.
#![cfg(unix)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

static SERIAL: AtomicU64 = AtomicU64::new(0);
// Cross-device copies must be closed in every thread before spawning (Linux ETXTBSY).
static FIXTURES: Mutex<()> = Mutex::new(());

struct Fixture {
    root: PathBuf,
    _guard: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let guard = FIXTURES.lock().unwrap_or_else(|error| error.into_inner());
        let root = std::env::temp_dir().join(format!(
            "hb-utils-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        fs::create_dir(root.join("home")).unwrap();
        // No installation sidecars, and no large binary copies on the usual path.
        let binary = fs::canonicalize(env!("CARGO_BIN_EXE_hey-boss")).unwrap();
        fs::hard_link(&binary, root.join("hey-boss"))
            .or_else(|_| fs::copy(&binary, root.join("hey-boss")).map(|_| ()))
            .unwrap();
        let git = root.join("bin/git");
        fs::write(
            &git,
            br##"#!/bin/sh
printf '%s\0' "$@" > "$FIXTURE_ROOT/args"
printf '%s' "$FAKE_GIT_ENV" > "$FIXTURE_ROOT/env"
pwd -P > "$FIXTURE_ROOT/cwd"
printf '%s' "$$" > "$FIXTURE_ROOT/pid"
case "$FAKE_GIT_MODE" in
  io) /bin/cat; printf 'fake git stderr\n' >&2; exit 37 ;;
  signal) kill -TERM "$$" ;;
esac
"##,
        )
        .unwrap();
        fs::set_permissions(git, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            _guard: guard,
        }
    }

    fn command(&self, action: &str) -> Command {
        let mut command = Command::new(self.root.join("hey-boss"));
        command
            .args(["utils", action])
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", self.root.join("bin"))
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("home/config"))
            .env("XDG_STATE_HOME", self.root.join("home/state"))
            .env("HEY_BOSS_ISSUE_DB", self.root.join("home/issues.db"))
            .env("HEY_BOSS_FLEET_STATE", self.root.join("home/fleet"))
            .env("FIXTURE_ROOT", &self.root);
        command
    }

    fn assert_args(&self, git_action: &str, args: &[impl AsRef<OsStr>]) {
        let mut expected = Vec::new();
        for arg in [OsStr::new(git_action), OsStr::new("--no-verify")]
            .into_iter()
            .chain(args.iter().map(AsRef::as_ref))
        {
            expected.extend_from_slice(arg.as_bytes());
            expected.push(0);
        }
        assert_eq!(fs::read(self.root.join("args")).unwrap(), expected);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn gcn_without_args_maps_to_commit_no_verify() {
    let fixture = Fixture::new();
    assert_success(&fixture.command("gcn").output().unwrap());
    fixture.assert_args("commit", &[] as &[&str]);
}

#[test]
fn gpn_without_args_maps_to_push_no_verify() {
    let fixture = Fixture::new();
    assert_success(&fixture.command("gpn").output().unwrap());
    fixture.assert_args("push", &[] as &[&str]);
}

#[test]
fn forwards_hyphens_spaces_empty_args_and_double_dash() {
    for (alias, action) in [("gcn", "commit"), ("gpn", "push")] {
        for args in [
            vec!["-m", "message with spaces", "", "--", "-filename", "--"],
            vec!["--help", "-h", "--version", "--unknown=value"],
        ] {
            let fixture = Fixture::new();
            assert_success(&fixture.command(alias).args(&args).output().unwrap());
            fixture.assert_args(action, &args);
        }
    }
}

#[test]
fn preserves_even_a_leading_double_dash() {
    for (alias, action) in [("gcn", "commit"), ("gpn", "push")] {
        let fixture = Fixture::new();
        let args = ["--", "--", "path with spaces", "-file"];
        assert_success(&fixture.command(alias).args(args).output().unwrap());
        fixture.assert_args(action, &args);
    }
}

#[test]
fn shell_metacharacters_are_literal_and_never_executed() {
    for (alias, action) in [("gcn", "commit"), ("gpn", "push")] {
        let fixture = Fixture::new();
        let args = [
            "$(printf injected > injected)",
            "`printf injected > injected`",
            "; printf injected > injected;",
            "|",
            "&",
            "*",
            "$HOME",
            "'quoted'",
            "\"quoted\"",
            "line\nbreak",
        ];
        assert_success(&fixture.command(alias).args(args).output().unwrap());
        fixture.assert_args(action, &args);
        assert!(!fixture.root.join("injected").exists());
    }
}

#[test]
fn inherits_cwd_environment_stdio_pid_and_exit_status() {
    for alias in ["gcn", "gpn"] {
        let fixture = Fixture::new();
        let env_value = OsString::from_vec(b"env with spaces\xff".to_vec());
        let input = b"stdin unchanged\n\0\xff";
        let mut child = fixture
            .command(alias)
            .env("FAKE_GIT_MODE", "io")
            .env("FAKE_GIT_ENV", &env_value)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(37));
        assert_eq!(output.stdout, input);
        assert_eq!(output.stderr, b"fake git stderr\n");
        assert_eq!(
            fs::read(fixture.root.join("env")).unwrap(),
            env_value.as_bytes()
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("cwd")).unwrap(),
            format!("{}\n", fixture.root.canonicalize().unwrap().display())
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("pid")).unwrap(),
            pid.to_string()
        );
    }
}

#[test]
fn preserves_signal_termination() {
    for alias in ["gcn", "gpn"] {
        let fixture = Fixture::new();
        let output = fixture
            .command(alias)
            .env("FAKE_GIT_MODE", "signal")
            .output()
            .unwrap();
        assert_eq!(output.status.signal(), Some(15));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn forwards_non_utf8_arguments() {
    for (alias, action) in [("gcn", "commit"), ("gpn", "push")] {
        let fixture = Fixture::new();
        let args = [
            OsString::from_vec(b"-\xff".to_vec()),
            OsString::from_vec(b"path \xfe\x80".to_vec()),
        ];
        assert_success(&fixture.command(alias).args(&args).output().unwrap());
        fixture.assert_args(action, &args);
    }
}

#[test]
fn needs_no_daemon_database_or_installation_setup() {
    for alias in ["gcn", "gpn"] {
        let fixture = Fixture::new();
        assert_success(&fixture.command(alias).output().unwrap());
        assert_eq!(fs::read_dir(fixture.root.join("home")).unwrap().count(), 0);
        for name in [
            "hey-boss.state",
            "hey-boss.setup.lock",
            "daemon.sock",
            "history.db",
        ] {
            assert!(!fixture.root.join(name).exists(), "unexpected {name}");
        }
    }
}

#[test]
fn missing_git_uses_normal_main_error_handling() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.root.join("bin/git")).unwrap();
    for alias in ["gcn", "gpn"] {
        let output = fixture.command(alias).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.starts_with(b"hey-boss: "), "{:?}", output);
        assert!(!fixture.root.join("args").exists());
    }
}

#[test]
fn utils_help_lists_shortcuts_and_clipboard_without_running_git() {
    let fixture = Fixture::new();
    let output = fixture.command("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for name in ["gcn", "gpn", "copy", "paste", "pbcopy", "pbpaste"] {
        assert!(help.contains(name), "missing {name}");
    }
    assert!(!fixture.root.join("args").exists());
}

#[test]
fn clipboard_help_and_invalid_arguments_need_no_connection() {
    for alias in ["copy", "paste", "pbcopy", "pbpaste"] {
        let fixture = Fixture::new();
        let output = fixture.command(alias).arg("--help").output().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("clipboard"));
        let output = fixture.command(alias).arg("unexpected").output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(!fixture.root.join("args").exists());
        assert_eq!(fs::read_dir(fixture.root.join("home")).unwrap().count(), 0);
    }
}
