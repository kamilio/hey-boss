#![cfg(unix)]
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Output, Stdio},
};

struct Fixture(tempfile::TempDir);
impl Fixture {
    fn new(host: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let gh = root.path().join("gh");
        fs::write(&gh, r##"#!/bin/sh
if [ "$1" = alias ]; then
  printf '%s\n' 'say: pr comment $1 --repo acme/demo --body-file -' 'shellsay: "!gh pr comment 7 --body hi"'
  exit 0
fi
printf '%s\n' "$@" > "$FIXTURE/args"
printf '%s' "${GH_TOKEN:-}" > "$FIXTURE/token"
printf '%s' "${GH_ENTERPRISE_TOKEN:-}" > "$FIXTURE/enterprise-token"
printf '%s' "${GITHUB_TOKEN:-}${GITHUB_ENTERPRISE_TOKEN:-}${HEY_GH_APP_PRIVATE_KEY:-}" > "$FIXTURE/other-tokens"
cat > "$FIXTURE/body"
printf 'https://github.com/acme/demo/pull/7#issuecomment-1\n'
exit "${GH_EXIT:-0}"
"##).unwrap();
        fs::set_permissions(gh, fs::Permissions::from_mode(0o700)).unwrap();
        let data = if cfg!(target_os = "macos") {
            root.path().join("Library/Application Support")
        } else {
            root.path().to_owned()
        };
        let apps = data.join("hey-gh/apps");
        fs::create_dir_all(&apps).unwrap();
        let credentials = apps.join(format!("{host}.json"));
        fs::write(
            &credentials,
            serde_json::to_vec(&serde_json::json!({
                "client_id":"test", "installation_id":42, "repositories":["acme/demo"],
                "private_key":include_str!("fixtures/github-app-test-key.pem")
            }))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(credentials, fs::Permissions::from_mode(0o600)).unwrap();
        Self(root)
    }
    fn run(&self, args: &[&str], input: &str, fail: bool) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hey-gh"))
            .args(args)
            .current_dir(self.0.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.0.path().display()))
            .env("FIXTURE", self.0.path())
            .env(
                "HEY_GH_APP_TOKEN",
                if fail {
                    "invalid\ntoken"
                } else {
                    "synthetic-app-token"
                },
            )
            .env("HOME", self.0.path())
            .env("XDG_DATA_HOME", self.0.path())
            .env("GH_TOKEN", "synthetic-personal")
            .env("GH_ENTERPRISE_TOKEN", "synthetic-enterprise")
            .env("GITHUB_TOKEN", "synthetic-other-personal")
            .env("GITHUB_ENTERPRISE_TOKEN", "synthetic-other-enterprise")
            .env("HEY_GH_APP_PRIVATE_KEY", "synthetic-env-key")
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
    fn read(&self, file: &str) -> String {
        fs::read_to_string(self.0.path().join(file)).unwrap()
    }
}

#[test]
fn comments_select_app_auth_and_preserve_native_arguments_and_body() {
    for args in [
        vec!["pr", "comment", "7", "-R", "acme/demo", "--body-file", "-"],
        vec![
            "issue",
            "comment",
            "https://github.com/acme/demo/issues/7",
            "--edit-last",
            "--create-if-none",
            "--body-file=-",
        ],
        vec![
            "issue",
            "comment",
            "7",
            "--repo=acme/demo",
            "--delete-last",
            "--yes",
        ],
        vec![
            "pr",
            "review",
            "7",
            "-Racme/demo",
            "--comment",
            "--body-file",
            "-",
        ],
        vec![
            "api",
            "repos/acme/demo/issues/7/comments",
            "-XPOST",
            "--input",
            "-",
        ],
        vec![
            "api",
            "--hostname",
            "github.com",
            "repos/acme/demo/pulls/comments/8",
            "-X",
            "DELETE",
        ],
        vec![
            "api",
            "graphql",
            "-f",
            "query=mutation { posted: addComment(input: {subjectId: \"x\", body: \"text\"}) { clientMutationId } }",
        ],
        vec!["--auth", "app", "api", "graphql", "--input", "-"],
        vec!["say", "7"],
        vec![
            "pr",
            "comment",
            "7",
            "-R",
            "acme/demo",
            "--help=false",
            "--body",
            "ok",
        ],
        vec![
            "pr",
            "comment",
            "7",
            "-R",
            "acme/demo",
            "-h=false",
            "--web=false",
            "--body",
            "ok",
        ],
        vec![
            "api",
            "https://api.github.com/repos/acme/demo/issues/7/comments",
            "-fbody=ok",
        ],
    ] {
        let f = Fixture::new("github.com");
        let body = format!("  {}\nline two\nline three\n", "é".repeat(400));
        let result = f.run(&args, &body, false);
        assert!(
            result.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(f.read("token"), "synthetic-app-token");
        assert_eq!(f.read("enterprise-token"), "");
        assert_eq!(f.read("other-tokens"), "");
        assert_eq!(f.read("body"), body);
        let expected = if args[0] == "--auth" {
            &args[2..]
        } else {
            &args[..]
        };
        assert_eq!(f.read("args"), format!("{}\n", expected.join("\n")));
        assert!(String::from_utf8_lossy(&result.stdout).contains("#issuecomment-1"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("synthetic-app-token"));
    }
}

#[test]
fn enterprise_host_comes_from_selector_before_repo_and_never_from_body() {
    let f = Fixture::new("git.example.com");
    let result = f.run(
        &[
            "pr",
            "comment",
            "--body",
            "https://wrong.example/body",
            "https://git.example.com/acme/demo/pull/7",
            "-R",
            "github.com/acme/demo",
        ],
        "",
        false,
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(f.read("token"), "");
    assert_eq!(f.read("enterprise-token"), "synthetic-app-token");
}

#[test]
fn invalid_supplied_app_token_never_posts_or_retries_as_user() {
    let f = Fixture::new("github.com");
    let result = f.run(
        &["pr", "comment", "7", "-R", "acme/demo", "--body", "ok"],
        "",
        true,
    );
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("HEY_GH_APP_TOKEN"));
    assert!(!String::from_utf8_lossy(&result.stderr).contains("synthetic"));
    assert!(!f.0.path().join("args").exists());
}

#[test]
fn browser_comments_cannot_silently_use_personal_identity() {
    let f = Fixture::new("github.com");
    let result = f.run(
        &["pr", "comment", "7", "--repo", "acme/demo", "--web"],
        "",
        false,
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("browser"));
    assert!(!f.0.path().join("mint-args").exists());
}

#[test]
fn opaque_graphql_input_requires_explicit_auth_without_consuming_stdin() {
    let f = Fixture::new("github.com");
    let result = f.run(&["api", "graphql", "--input", "-"], "{}", false);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("explicit --auth"));
    assert!(!f.0.path().join("mint-args").exists());
}

#[test]
fn shell_aliases_require_auth_and_explicit_user_keeps_native_execution() {
    let f = Fixture::new("github.com");
    let result = f.run(&["shellsay"], "", false);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("shell aliases require explicit"));
    let result = f.run(&["--auth", "user", "shellsay"], "", false);
    assert!(result.status.success());
    assert_eq!(f.read("token"), "synthetic-personal");
}
