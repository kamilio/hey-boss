#![cfg(unix)]

use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Output, Stdio},
};

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let gh = root.path().join("gh");
        fs::write(&gh, "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$FIXTURE/args\"\ncat > \"$FIXTURE/body\"\nprintf '%s\\n' 'https://github.com/example/repo/pull/7#issuecomment-1'\nexit \"${GH_EXIT:-0}\"\n").unwrap();
        fs::set_permissions(gh, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }

    fn run(&self, args: &[&str], input: &str, exit: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hey-gh"))
            .args(args)
            .env("PATH", format!("{}:/usr/bin:/bin", self.0.path().display()))
            .env("FIXTURE", self.0.path())
            .env("GH_EXIT", exit)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn assert_not_posted(&self) {
        assert!(!self.0.path().join("args").exists());
    }
}

#[test]
fn comment_posts_exact_text_once_without_a_daemon() {
    let f = Fixture::new();
    let body = "Fixed the retry.\n`$HOME` and $(false) stay literal.";
    let output = f.run(
        &[
            "--server",
            "invalid",
            "pr",
            "comment",
            "7",
            "-R",
            "example/repo",
            "--body",
            body,
        ],
        "",
        "0",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(f.0.path().join("body")).unwrap(), body);
    assert_eq!(
        fs::read_to_string(f.0.path().join("args")).unwrap(),
        "pr\ncomment\n7\n--repo\nexample/repo\n--body-file\n-\n"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("#issuecomment-1"));
}

#[test]
fn comment_limits_apply_before_any_github_call() {
    for body in [
        "x".repeat(301),
        "é".repeat(301),
        "a\nb\nc".into(),
        "a\rb\rc".into(),
        "a\u{2028}b\u{2029}c".into(),
        "   ".into(),
    ] {
        let f = Fixture::new();
        let output = f.run(&["pr", "comment", "7", "--body", &body], "", "0");
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Do not sound like a robot."));
        f.assert_not_posted();
    }
}

#[test]
fn comment_file_and_stdin_use_the_same_unicode_limit() {
    for stdin in [false, true] {
        let f = Fixture::new();
        let body = "é".repeat(300);
        let file = f.0.path().join("comment.md");
        fs::write(&file, &body).unwrap();
        let output = f.run(
            &[
                "issue",
                "comment",
                "https://github.com/example/repo/issues/7",
                "--body-file",
                if stdin { "-" } else { file.to_str().unwrap() },
            ],
            if stdin { &body } else { "" },
            "0",
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_to_string(f.0.path().join("body")).unwrap(), body);
    }
    for stdin in [false, true] {
        let f = Fixture::new();
        let body = "x".repeat(301);
        let file = f.0.path().join("comment.md");
        fs::write(&file, &body).unwrap();
        let output = f.run(
            &[
                "pr",
                "comment",
                "7",
                "--body-file",
                if stdin { "-" } else { file.to_str().unwrap() },
            ],
            if stdin { &body } else { "" },
            "0",
        );
        assert!(!output.status.success());
        f.assert_not_posted();
    }
}

#[test]
fn invalid_comment_options_never_post() {
    for args in [
        vec!["pr", "comment", "7"],
        vec!["pr", "comment", "7", "--body", "ok", "--body-file", "-"],
        vec!["pr", "comment", "7", "--body", "ok", "--cached-only"],
        vec!["pr", "comment", "7", "--body", "ok", "--refresh"],
        vec!["pr", "comment", "7", "--body", "ok", "--json", "number"],
        vec!["pr", "comment", "7", "--body", "ok", "--cursor", "cursor"],
        vec!["pr", "comment", "7", "--body", "ok", "--wait", "1"],
        vec!["pr", "comment", "7", "--body", "ok", "--timeout", "1"],
        vec!["pr", "comment", "7", "--body-file", "/missing/comment.md"],
        vec!["issue", "comment", "--body", "ok"],
    ] {
        let f = Fixture::new();
        assert!(!f.run(&args, "", "0").status.success(), "{args:?}");
        f.assert_not_posted();
    }
}

#[test]
fn failed_posts_are_never_retried() {
    let f = Fixture::new();
    let output = f.run(&["pr", "comment", "7", "--body", "Fixed."], "", "1");
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(f.0.path().join("args"))
            .unwrap()
            .matches("comment\n")
            .count(),
        1
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("not retried"));
}

#[test]
fn comment_help_shows_only_write_options_and_shared_guidance() {
    for kind in ["pr", "issue"] {
        let f = Fixture::new();
        let output = f.run(&[kind, "comment", "--help"], "", "0");
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains("--body-file") && help.contains("--repo"));
        assert!(help.contains("Do not sound like a robot."));
        for flag in [
            "--server",
            "--cursor",
            "--timeout",
            "--refresh",
            "--cached-only",
            "--json",
            "--wait",
        ] {
            assert!(!help.contains(flag), "{help}");
        }
        f.assert_not_posted();
    }
    let f = Fixture::new();
    let output = f.run(&["pr", "view", "--help"], "", "0");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--refresh") && help.contains("--cached-only"));
}
