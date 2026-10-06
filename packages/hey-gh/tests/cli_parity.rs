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
printf '%s' "${GH_CONFIG_DIR:-}" > "$FIXTURE/config-dir"
printf '%s' "${GH_CACHE_DIR:-}" > "$FIXTURE/cache-dir"
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
        self.run_auth(args, input, exit, false)
    }
    fn run_auth(&self, args: &[&str], input: &str, exit: &str, app: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-gh"));
        command
            .args(args)
            .current_dir(self.0.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.0.path().display()))
            .env("FIXTURE", self.0.path())
            .env("GH_EXIT", exit)
            .env("HOME", self.0.path())
            .env("XDG_DATA_HOME", self.0.path())
            .env("GH_TOKEN", "synthetic-user-token")
            .env("GH_ENTERPRISE_TOKEN", "synthetic-enterprise-user-token")
            .env("GH_CONFIG_DIR", self.0.path().join("gh-config"))
            .env_remove("GH_CACHE_DIR")
            .env_remove("HEY_GH_APP_TOKEN")
            .env_remove("GH_HOST")
            .env_remove("GH_REPO")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if app {
            command.env("HEY_GH_APP_TOKEN", "synthetic-app-token");
        }
        let mut child = command.spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        child.wait_with_output().unwrap()
    }
}

#[test]
fn app_transport_preserves_native_streams_exit_status_and_cleans_private_configuration() {
    let f = Fixture::new();
    let original = f.0.path().join("gh-config");
    fs::create_dir(&original).unwrap();
    let config = "git_protocol: ssh\naliases:\n  mine: pr list --author @me\n";
    fs::write(original.join("config.yml"), config).unwrap();
    let args = [
        "--auth",
        "app",
        "pr",
        "list",
        "--author",
        "@me",
        "-R",
        "acme/demo",
    ];
    let result = f.run_auth(&args, "literal @me\n", "8", true);
    assert_eq!(
        result.status.code(),
        Some(8),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"native stdout\n");
    assert_eq!(result.stderr, b"native stderr\n");
    assert_eq!(
        fs::read_to_string(f.0.path().join("args")).unwrap(),
        format!("{}\n", args[2..].join("\n"))
    );
    assert_eq!(
        fs::read_to_string(f.0.path().join("stdin")).unwrap(),
        "literal @me\n"
    );
    assert_eq!(
        fs::read_to_string(f.0.path().join("token")).unwrap(),
        "synthetic-app-token"
    );
    let temporary = fs::read_to_string(f.0.path().join("config-dir")).unwrap();
    assert_ne!(std::path::Path::new(&temporary), original);
    assert!(!std::path::Path::new(&temporary).exists());
    let cache = fs::read_to_string(f.0.path().join("cache-dir")).unwrap();
    assert!(!cache.is_empty());
    assert!(!std::path::Path::new(&cache).exists());
    assert_eq!(
        fs::read_to_string(original.join("config.yml")).unwrap(),
        config
    );
}

#[test]
fn missing_personal_identity_fails_before_any_app_data_request() {
    let f = Fixture::new();
    fs::write(f.0.path().join("gh"), r##"#!/bin/sh
if [ "$1" = api ] && [ "$2" = user ]; then
  printf '%s' "$GH_TOKEN" > "$FIXTURE/lookup-token"
  printf '%s' "$GH_CONFIG_DIR" > "$FIXTURE/lookup-config"
  exit 1
fi
socket=$(sed -n 's/^http_unix_socket: //p' "$GH_CONFIG_DIR/config.yml")
curl --silent --show-error --fail-with-body --unix-socket "$socket" -H 'content-type: application/json' -d '{"query":"query { viewer { login } }"}' http://api.github.com/graphql
"##).unwrap();
    let result = f.run_auth(
        &["--auth", "app", "pr", "status", "-R", "acme/demo"],
        "",
        "0",
        true,
    );
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stdout)
            .contains("Cannot resolve the personal GitHub identity"),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert_eq!(
        fs::read_to_string(f.0.path().join("lookup-token")).unwrap(),
        "synthetic-user-token"
    );
    assert_eq!(
        fs::read_to_string(f.0.path().join("lookup-config")).unwrap(),
        f.0.path().join("gh-config").to_str().unwrap()
    );
}

#[test]
fn app_transport_preserves_signal_termination() {
    use std::os::unix::process::ExitStatusExt;
    let f = Fixture::new();
    fs::write(f.0.path().join("gh"), "#!/bin/sh\nkill -TERM $$\n").unwrap();
    let result = f.run_auth(&["--auth", "app", "repo", "view"], "", "0", true);
    assert_eq!(result.status.signal(), Some(15));
}

#[test]
fn app_aliases_resolve_the_target_host_without_changing_native_execution() {
    let f = Fixture::new();
    let script = fs::read_to_string(f.0.path().join("gh")).unwrap().replacen(
        "#!/bin/sh\n",
        "#!/bin/sh\nif [ \"$1\" = alias ]; then\n  printf '%s\\n' 'myprs: pr list --author @me --repo ghe.example/acme/demo'\n  exit 0\nfi\n",
        1,
    );
    fs::write(f.0.path().join("gh"), script).unwrap();
    let result = f.run_auth(
        &["--auth", "app", "myprs", "--json", "number"],
        "",
        "0",
        true,
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read_to_string(f.0.path().join("args")).unwrap(),
        "myprs\n--json\nnumber\n"
    );
    assert_eq!(fs::read_to_string(f.0.path().join("token")).unwrap(), "");
    assert_eq!(
        fs::read_to_string(f.0.path().join("enterprise-token")).unwrap(),
        "synthetic-app-token"
    );
}

#[test]
fn search_values_cannot_change_the_selected_auth_host() {
    for search in [
        vec!["--search", "https://body.example"],
        vec!["-S", "https://body.example"],
        vec!["-Shttps://body.example"],
    ] {
        let f = Fixture::new();
        let mut args = vec!["--auth", "app", "pr", "list", "-R", "ghe.example/acme/demo"];
        args.extend(search);
        let result = f.run_auth(&args, "", "0", true);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            fs::read_to_string(f.0.path().join("enterprise-token")).unwrap(),
            "synthetic-app-token"
        );
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
