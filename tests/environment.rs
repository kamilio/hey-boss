use std::{fs, os::unix::fs::PermissionsExt};
#[path = "support/environment.rs"]
mod support;
use support::Fixture;

#[test]
fn setup_signs_registers_and_is_idempotent_while_check_is_read_only() {
    let f = Fixture::new();
    assert!(!f.cli("check").status.success());
    assert!(!f.root.join(".gitconfig").exists());
    assert!(!f.root.join(".ssh").exists());
    let report = f.success("setup");
    assert_eq!(report["ok"], true);
    assert_eq!(report["email"], "42+octocat@users.noreply.github.com");
    let config = fs::read(f.root.join(".gitconfig")).unwrap();
    let metadata = fs::metadata(f.root.join(".ssh/github_commit_signing")).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    f.success("setup");
    f.success("check");
    assert_eq!(config, fs::read(f.root.join(".gitconfig")).unwrap());
    assert_eq!(
        metadata.modified().unwrap(),
        fs::metadata(f.root.join(".ssh/github_commit_signing"))
            .unwrap()
            .modified()
            .unwrap()
    );
    assert_eq!(
        fs::read_to_string(f.root.join("api-log"))
            .unwrap()
            .matches("POST")
            .count(),
        1
    );
}

#[test]
fn setup_reuses_an_existing_usable_key() {
    let f = Fixture::new();
    fs::create_dir(f.root.join(".ssh")).unwrap();
    let key = f.root.join(".ssh/id_ed25519");
    assert!(
        f.command("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .status()
            .unwrap()
            .success()
    );
    f.success("setup");
    assert!(!f.root.join(".ssh/github_commit_signing").exists());
    assert!(
        fs::read_to_string(f.root.join(".gitconfig"))
            .unwrap()
            .contains("id_ed25519")
    );
}

#[test]
fn repository_override_blocks_check_setup_and_worker_before_start() {
    let f = Fixture::new();
    f.success("setup");
    f.git(&["init", "--quiet"]);
    f.git(&["config", "commit.gpgsign", "false"]);
    for action in ["setup", "check"] {
        let o = f.cli(action);
        assert!(!o.status.success());
        assert!(String::from_utf8_lossy(&o.stdout).contains("commit.gpgsign"));
    }
    let o = f
        .command(env!("CARGO_BIN_EXE_hey-boss"))
        .env("HEY_BOSS_ISSUE_DB", f.root.join("issues.db"))
        .args(["worker", "run", "--json"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("environment"),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
}

#[test]
fn github_permissions_and_registration_readback_are_required() {
    for marker in ["denied", "discard-registration"] {
        let f = Fixture::new();
        fs::write(f.root.join(marker), "").unwrap();
        let o = f.cli("setup");
        assert!(!o.status.success());
        let text = String::from_utf8_lossy(&o.stdout);
        assert!(
            text.contains(if marker == "denied" {
                "permission"
            } else {
                "registration"
            }),
            "{text}"
        );
        assert!(!f.root.join(".gitconfig").exists());
    }
}

#[test]
fn check_fails_when_registered_key_is_removed_or_private_key_is_missing() {
    let f = Fixture::new();
    f.success("setup");
    fs::write(f.root.join("github-keys"), "[]").unwrap();
    assert!(!f.cli("check").status.success());
    f.success("setup");
    fs::remove_file(f.root.join(".ssh/github_commit_signing")).unwrap();
    assert!(!f.cli("check").status.success());
}

#[test]
fn check_honors_conditional_includes_and_does_not_touch_the_checkout() {
    let f = Fixture::new();
    f.success("setup");
    f.git(&["init", "--quiet"]);
    let head = fs::read(f.root.join(".git/HEAD")).unwrap();
    let local = fs::read(f.root.join(".git/config")).unwrap();
    f.success("check");
    assert_eq!(head, fs::read(f.root.join(".git/HEAD")).unwrap());
    assert_eq!(local, fs::read(f.root.join(".git/config")).unwrap());
    assert!(!f.root.join(".git/refs/heads/main").exists());
    fs::write(
        f.root.join("override.gitconfig"),
        "[commit]\n gpgsign = false\n",
    )
    .unwrap();
    f.git(&[
        "config",
        "--global",
        &format!(
            "includeIf.gitdir:{}/.git.path",
            f.root.canonicalize().unwrap().display()
        ),
        f.root.join("override.gitconfig").to_str().unwrap(),
    ]);
    assert!(!f.cli("check").status.success());
}

#[test]
fn probe_isolates_git_dir_but_preserves_identity_environment_overrides() {
    let f = Fixture::new();
    f.success("setup");
    f.git(&["init", "--quiet"]);
    let o = f
        .command(env!("CARGO_BIN_EXE_hey-boss"))
        .env("GIT_DIR", f.root.join(".git"))
        .args(["environment", "check", "--json"])
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{} {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!f.root.join(".git/refs/heads/main").exists());
    let o = f
        .command(env!("CARGO_BIN_EXE_hey-boss"))
        .env("GIT_COMMITTER_EMAIL", "wrong@example.com")
        .args(["environment", "check", "--json"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("wrong@example.com"));
}

#[test]
fn setup_preserves_custom_signing_path_and_valid_repository_overrides() {
    let f = Fixture::new();
    f.success("setup");
    let custom = f.root.join("custom-key");
    fs::rename(f.root.join(".ssh/github_commit_signing"), &custom).unwrap();
    fs::rename(
        f.root.join(".ssh/github_commit_signing.pub"),
        custom.with_extension("pub"),
    )
    .unwrap();
    f.git(&[
        "config",
        "--global",
        "user.signingkey",
        custom.to_str().unwrap(),
    ]);
    f.git(&["init", "--quiet"]);
    f.git(&["config", "user.name", "Repository Name"]);
    let config = fs::read(f.root.join(".gitconfig")).unwrap();
    let report = f.success("setup");
    assert_eq!(
        report["repository_overrides"]["user.name"],
        "Repository Name"
    );
    assert_eq!(config, fs::read(f.root.join(".gitconfig")).unwrap());
    assert!(!f.root.join(".ssh/github_commit_signing").exists());
}

#[test]
fn setup_preserves_a_broken_non_ssh_configuration_for_manual_repair() {
    let f = Fixture::new();
    f.git(&[
        "config",
        "--global",
        "user.signingkey",
        "existing-openpgp-key",
    ]);
    f.git(&["config", "--global", "gpg.program", "/usr/bin/false"]);
    let config = fs::read(f.root.join(".gitconfig")).unwrap();
    let result = f.cli("setup");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("preserved"));
    assert_eq!(config, fs::read(f.root.join(".gitconfig")).unwrap());
    assert!(!f.root.join(".ssh").exists());
}

#[test]
fn probe_inside_checkout_tmpdir_does_not_inherit_that_repository() {
    let f = Fixture::new();
    f.success("setup");
    f.git(&["init", "--quiet"]);
    f.git(&["config", "commit.gpgsign", "false"]);
    let nested = f.root.join("tmp");
    fs::create_dir(&nested).unwrap();
    let o = f
        .command(env!("CARGO_BIN_EXE_hey-boss"))
        .env("TMPDIR", nested)
        .args(["environment", "setup", "--json"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("Repository overrides: commit.gpgsign=false")
    );
}

#[test]
fn setup_preserves_working_registered_openpgp_signing() {
    let f = Fixture::new();
    if f.command("gpg").arg("--version").output().is_err() {
        return;
    }
    fs::create_dir(f.root.join(".gnupg")).unwrap();
    fs::set_permissions(f.root.join(".gnupg"), fs::Permissions::from_mode(0o700)).unwrap();
    let generated = f
        .command("gpg")
        .args([
            "--batch",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "",
            "--quick-gen-key",
            "Octo Cat <42+octocat@users.noreply.github.com>",
            "ed25519",
            "sign",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let keys = f
        .command("gpg")
        .args(["--with-colons", "--list-secret-keys"])
        .output()
        .unwrap();
    let keys = String::from_utf8(keys.stdout).unwrap();
    let fingerprint = keys
        .lines()
        .find(|l| l.starts_with("fpr:"))
        .unwrap()
        .split(':')
        .nth(9)
        .unwrap();
    fs::write(
        f.root.join("gpg-key-id"),
        &fingerprint[fingerprint.len() - 16..],
    )
    .unwrap();
    for (key, value) in [
        ("user.name", "Octo Cat"),
        ("user.email", "42+octocat@users.noreply.github.com"),
        ("user.signingkey", fingerprint),
        ("commit.gpgsign", "true"),
        ("tag.gpgsign", "true"),
    ] {
        f.git(&["config", "--global", key, value]);
    }
    let config = fs::read(f.root.join(".gitconfig")).unwrap();
    assert_eq!(f.success("setup")["format"], "openpgp");
    assert_eq!(config, fs::read(f.root.join(".gitconfig")).unwrap());
    assert!(!f.root.join(".ssh").exists());
}

#[test]
fn service_path_finds_github_cli_without_changing_worker_environment() {
    let f = Fixture::new();
    fs::create_dir_all(f.root.join(".local/bin")).unwrap();
    fs::rename(f.root.join("bin/gh"), f.root.join(".local/bin/gh")).unwrap();
    // Linux runners also install gh in /usr/bin. Keep real Git/signing tools,
    // but exclude every system gh so this specifically exercises the fallback.
    let service_bin = f.root.join("service-bin");
    fs::create_dir(&service_bin).unwrap();
    for tool in ["git", "ssh-keygen", "ssh-add", "hostname", "sh"] {
        let found = std::process::Command::new("which")
            .arg(tool)
            .output()
            .unwrap();
        assert!(found.status.success());
        let path = String::from_utf8(found.stdout).unwrap();
        std::os::unix::fs::symlink(path.trim(), service_bin.join(tool)).unwrap();
    }
    let result = f
        .command(env!("CARGO_BIN_EXE_hey-boss"))
        .env("PATH", service_bin)
        .args(["environment", "setup", "--json"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
