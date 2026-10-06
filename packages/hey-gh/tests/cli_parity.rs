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
        fs::write(
            &gh,
            r##"#!/bin/sh
printf '%s\n' "$@" > "$FIXTURE/args"
printf '%s' "${GH_TOKEN:-}" > "$FIXTURE/token"
printf '%s' "${GH_ENTERPRISE_TOKEN:-}" > "$FIXTURE/enterprise-token"
cat > "$FIXTURE/stdin"
printf 'native stdout\n'
printf 'native stderr\n' >&2
exit "${GH_EXIT:-0}"
"##,
        )
        .unwrap();
        fs::set_permissions(gh, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }
    fn run(&self, args: &[&str], input: &str, exit: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hey-gh"))
            .args(args)
            .current_dir(self.0.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.0.path().display()))
            .env("FIXTURE", self.0.path())
            .env("GH_EXIT", exit)
            .env("HOME", self.0.path())
            .env("XDG_DATA_HOME", self.0.path())
            .env("GH_TOKEN", "synthetic-user-token")
            .env_remove("HEY_GH_APP_TOKEN")
            .env_remove("GH_HOST")
            .env_remove("GH_REPO")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        child.wait_with_output().unwrap()
    }
}

#[test]
fn native_commands_preserve_arguments_streams_and_exit_codes() {
    let cases: &[&[&str]] = &[
        &[
            "pr",
            "create",
            "--title",
            "release --cached-only",
            "--body-file",
            "-",
        ],
        &[
            "pr", "list", "--state", "closed", "--author", "someone", "--json", "number", "--jq",
            ".[0]",
        ],
        &["pr", "view", "7", "--json", "number,title"],
        &["pr", "checks", "7", "--watch", "--fail-fast"],
        &["pr", "status"],
        &["issue", "edit", "7", "--add-label", "bug"],
        &["pr", "--repo", "acme/demo", "view", "7", "--json", "number"],
        &["pr", "list", "--jq", "--cached-only", "--json", "number"],
        &["repo", "view"],
        &["release", "create", "v1"],
        &["status"],
        &["api", "graphql", "-f", "query=query { viewer { login } }"],
        &["run", "watch", "123"],
        &["workflow", "run", "build.yml"],
        &["auth", "status"],
        &["co", "7"],
        &["stack", "list"],
        &["future-command", "--a-new-flag", "--", "--cached-only"],
        &["-R", "acme/demo", "pr", "merge", "7", "--auto"],
    ];
    for args in cases {
        let f = Fixture::new();
        let result = f.run(args, "literal $HOME\n$(false)\n", "8");
        assert_eq!(
            result.status.code(),
            Some(8),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stdout, b"native stdout\n");
        assert_eq!(result.stderr, b"native stderr\n");
        assert_eq!(
            fs::read_to_string(f.0.path().join("args")).unwrap(),
            format!("{}\n", args.join("\n"))
        );
        assert_eq!(
            fs::read_to_string(f.0.path().join("stdin")).unwrap(),
            "literal $HOME\n$(false)\n"
        );
        assert_eq!(
            fs::read_to_string(f.0.path().join("token")).unwrap(),
            "synthetic-user-token"
        );
    }
}

#[test]
fn user_auth_is_an_explicit_override_and_does_not_rewrite_comment_body() {
    let f = Fixture::new();
    let args = [
        "--auth",
        "user",
        "pr",
        "comment",
        "7",
        "--edit-last",
        "--create-if-none",
        "--body-file",
        "-",
    ];
    let body = format!("  {}\nline two\nline three\n", "é".repeat(400));
    let result = f.run(&args, &body, "0");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read_to_string(f.0.path().join("args")).unwrap(),
        format!("{}\n", args[2..].join("\n"))
    );
    assert_eq!(fs::read_to_string(f.0.path().join("stdin")).unwrap(), body);
}

#[test]
fn comment_help_is_native_and_needs_no_app_credentials() {
    let f = Fixture::new();
    let result = f.run(&["pr", "comment", "--help"], "", "0");
    assert!(result.status.success());
    assert_eq!(result.stdout, b"native stdout\n");
}

#[test]
fn comments_never_fall_back_to_user_auth() {
    for args in [
        vec![
            "pr",
            "comment",
            "https://github.com/acme/demo/pull/7",
            "--body",
            "ok",
        ],
        vec!["issue", "comment", "7", "-Racme/demo", "--body", "ok"],
        vec![
            "pr",
            "review",
            "7",
            "--comment",
            "--body",
            "ok",
            "-R",
            "acme/demo",
        ],
        vec!["pr", "--repo", "acme/demo", "comment", "7", "--body", "ok"],
        vec!["-Racme/demo", "issue", "comment", "7", "--body", "ok"],
        vec!["api", "repos/acme/demo/issues/7/comments", "-f", "body=ok"],
        vec![
            "api",
            "repos/acme/demo/pulls/comments/8",
            "-X",
            "PATCH",
            "-f",
            "body=ok",
        ],
    ] {
        let f = Fixture::new();
        let result = f.run(&args, "", "0");
        assert!(!result.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("GitHub App"),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !f.0.path().join("args").exists(),
            "must fail before invoking gh without App configuration"
        );
    }
}

#[test]
fn native_process_preserves_signal_termination() {
    use std::os::unix::process::ExitStatusExt;
    let f = Fixture::new();
    fs::write(f.0.path().join("gh"), "#!/bin/sh\nkill -TERM $$\n").unwrap();
    let result = f.run(&["repo", "view"], "", "0");
    assert_eq!(result.status.signal(), Some(15));
}
