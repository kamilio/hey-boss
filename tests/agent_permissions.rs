use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("out")
            .join(format!(
                "agent-permissions-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(root.join(".codex/rules")).unwrap();
        fs::create_dir_all(root.join(".claude")).unwrap();
        Self(root)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        command
            .env("HOME", &self.0)
            .env_remove("CODEX_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .args(["configure-agents", "--binary", "/opt/hey-boss/bin/hey-boss"]);
        command
    }
    fn success(&self) -> Output {
        let output = self.command().output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn settings(&self) -> PathBuf {
        self.0.join(".claude/settings.json")
    }
    fn rules(&self) -> PathBuf {
        self.0.join(".codex/rules/hey-boss.rules")
    }
    fn backups(&self) -> Vec<PathBuf> {
        [self.0.join(".claude"), self.0.join(".codex/rules")]
            .iter()
            .flat_map(|dir| fs::read_dir(dir).unwrap())
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "bak"))
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn new_configs_are_global_scoped_and_idempotent() {
    let fixture = Fixture::new();
    let untouched =
        "# My existing policy\nprefix_rule(pattern = [\"git\"], decision = \"prompt\")\n";
    fs::write(fixture.0.join(".codex/rules/default.rules"), untouched).unwrap();
    fs::write(fixture.0.join(".codex/config.toml"), "model = 'custom'\n").unwrap();
    fixture.success();
    let settings = fs::read(fixture.settings()).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&settings).unwrap();
    assert_eq!(
        parsed["permissions"]["allow"],
        serde_json::json!(["Bash(/opt/hey-boss/bin/hey-boss *)", "Bash(hey-boss *)"])
    );
    let rules = fs::read_to_string(fixture.rules()).unwrap();
    assert!(rules.contains("pattern = [\"hey-boss\"], decision = \"allow\""));
    assert!(rules.contains("pattern = [\"/opt/hey-boss/bin/hey-boss\"]"));
    assert_eq!(
        fs::read_to_string(fixture.0.join(".codex/rules/default.rules")).unwrap(),
        untouched
    );
    assert_eq!(
        fs::read_to_string(fixture.0.join(".codex/config.toml")).unwrap(),
        "model = 'custom'\n"
    );
    let inode = fs::metadata(fixture.settings()).unwrap().ino();
    fixture.success();
    assert_eq!(fs::read(fixture.settings()).unwrap(), settings);
    assert_eq!(fs::metadata(fixture.settings()).unwrap().ino(), inode);
    assert!(fixture.backups().is_empty());
    assert_eq!(
        fs::metadata(fixture.settings())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn merge_preserves_unknown_values_formatting_restrictions_and_legacy_entries() {
    let fixture = Fixture::new();
    let original = concat!(
        "{\r\n  \"unknown\" : {\"large\": 1234567890123456789012345678901234567890, \"exponent\": 1e999},\r\n",
        "  \"permissions\" : {\"allow\": [\"Bash(git status)\", \"Bash(hey-boss:*)\"],\r\n",
        "    \"deny\": [\"Bash(hey-boss agent-control *)\"], \"ask\": [\"Bash(hey-boss companion *)\"], \"defaultMode\": \"default\"},\r\n",
        "  \"hooks\": {\"Stop\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"echo hello\"}]}]}}\r\n"
    );
    fs::write(fixture.settings(), original).unwrap();
    fs::set_permissions(fixture.settings(), fs::Permissions::from_mode(0o640)).unwrap();
    fixture.success();
    let updated = fs::read_to_string(fixture.settings()).unwrap();
    assert_eq!(
        updated,
        original.replacen(
            "\"Bash(hey-boss:*)\"]",
            "\"Bash(hey-boss:*)\",\"Bash(/opt/hey-boss/bin/hey-boss *)\"]",
            1
        )
    );
    let backups = fixture.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(&backups[0]).unwrap(), original);
    assert_eq!(
        fs::metadata(&backups[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(fixture.settings())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    fixture.success();
    assert_eq!(fixture.backups().len(), 1);
}

#[test]
fn malformed_or_ambiguous_settings_leave_both_configs_untouched() {
    for input in [
        "",
        "{",
        "null",
        "[]",
        "{\"permissions\":null}",
        "{\"permissions\":{\"allow\":null}}",
        "{\"permissions\":{\"allow\":[1]}}",
        "{\"permissions\":{\"deny\":\"Bash\"}}",
        "{\"permissions\":{\"ask\":false}}",
        "{\"permissions\":{},\"permissions\":{}}",
        "{\"permissions\":{\"allow\":[],\"allow\":[]}}",
        "{\"permissions\":{\"allow\":[],}}",
        "{/*comment*/}",
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.settings(), input).unwrap();
        let output = fixture.command().output().unwrap();
        assert!(!output.status.success(), "accepted {input}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("settings.json"));
        assert_eq!(fs::read_to_string(fixture.settings()).unwrap(), input);
        assert!(!fixture.rules().exists());
        assert!(fixture.backups().is_empty());
    }
}

#[test]
fn unowned_or_modified_rules_are_not_overwritten() {
    for input in [
        "# personal rules\n",
        "# Managed by hey-boss configure-agents.\n# custom addition\n",
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.rules(), input).unwrap();
        assert!(!fixture.command().output().unwrap().status.success());
        assert_eq!(fs::read_to_string(fixture.rules()).unwrap(), input);
        assert!(!fixture.settings().exists());
        assert!(fixture.backups().is_empty());
    }
}

#[test]
fn overrides_and_symlinked_dotfiles_are_respected() {
    let fixture = Fixture::new();
    let codex = fixture.0.join("custom codex");
    let claude = fixture.0.join("custom claude");
    fs::create_dir_all(&claude).unwrap();
    let target = fixture.0.join("dotfiles.json");
    fs::write(&target, "{\"theme\":\"dark\"}\n").unwrap();
    symlink(&target, claude.join("settings.json")).unwrap();
    let output = fixture
        .command()
        .env("CODEX_HOME", &codex)
        .env("CLAUDE_CONFIG_DIR", &claude)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::symlink_metadata(claude.join("settings.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let settings: serde_json::Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
    assert_eq!(settings["theme"], "dark");
    assert_eq!(
        settings["permissions"]["allow"].as_array().unwrap().len(),
        2
    );
    assert!(codex.join("rules/hey-boss.rules").exists());
    assert!(!fixture.rules().exists());
    assert!(!fixture.settings().exists());
}

#[test]
fn dangling_symlinks_and_hard_links_are_not_replaced() {
    let fixture = Fixture::new();
    symlink(fixture.0.join("missing"), fixture.settings()).unwrap();
    assert!(!fixture.command().output().unwrap().status.success());
    assert!(
        fs::symlink_metadata(fixture.settings())
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!fixture.rules().exists());
    fs::remove_file(fixture.settings()).unwrap();
    let target = fixture.0.join("shared.json");
    fs::write(&target, "{}").unwrap();
    fs::hard_link(&target, fixture.settings()).unwrap();
    assert!(!fixture.command().output().unwrap().status.success());
    assert_eq!(fs::read_to_string(target).unwrap(), "{}");
    assert!(!fixture.rules().exists());
}

#[test]
fn concurrent_installers_merge_once() {
    let fixture = Fixture::new();
    fs::write(fixture.settings(), "{\"theme\":\"dark\"}").unwrap();
    let children: Vec<_> = (0..6)
        .map(|_| {
            fixture
                .command()
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let parsed: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.settings()).unwrap()).unwrap();
    assert_eq!(parsed["permissions"]["allow"].as_array().unwrap().len(), 2);
    assert_eq!(fixture.backups().len(), 1);
}

#[test]
fn installed_paths_are_escaped_and_wildcards_rejected() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["--binary", "/tmp/Boss's \"tools\"/hey-boss"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rules = fs::read_to_string(fixture.rules()).unwrap();
    assert!(rules.contains(r#"["/tmp/Boss's \"tools\"/hey-boss"]"#));
    let before = fs::read(fixture.settings()).unwrap();
    for path in [
        "/tmp/*/hey-boss",
        "relative/hey-boss",
        "/tmp/other-command",
        "/tmp/\n/hey-boss",
    ] {
        assert!(
            !fixture
                .command()
                .args(["--binary", path])
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(fs::read(fixture.settings()).unwrap(), before);
    }
}

#[test]
fn homebrew_manifest_is_applied_by_explicit_configuration_and_first_use() {
    for arguments in [vec!["configure-agents"], vec!["status", "missing-task"]] {
        let fixture = Fixture::new();
        let executable = fixture.0.join("bin/hey-boss");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &executable).unwrap();
        let manifest = executable.with_file_name("hey-boss.agent-permissions.json");
        fs::write(
            &manifest,
            r#"["/opt/homebrew/bin/hey-boss","/opt/homebrew/opt/hey-boss/libexec/bin/hey-boss"]"#,
        )
        .unwrap();
        let mut command = Command::new(&executable);
        command
            .env("HOME", &fixture.0)
            .env_remove("CODEX_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .args(arguments);
        // A malformed user config keeps the manifest available for a retry.
        fs::write(fixture.settings(), "{").unwrap();
        assert!(!command.output().unwrap().status.success());
        assert!(manifest.exists());
        assert!(!fixture.rules().exists());
        fs::write(fixture.settings(), "{}").unwrap();
        let output = command.output().unwrap();
        // The status invocation has no daemon/state, but config setup runs first.
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("Global hey-boss permissions configured")
        );
        assert!(!manifest.exists());
        let rules = fs::read_to_string(fixture.rules()).unwrap();
        assert!(rules.contains("/opt/homebrew/bin/hey-boss"));
        assert!(rules.contains("/opt/homebrew/opt/hey-boss/libexec/bin/hey-boss"));
        assert!(!rules.contains(fixture.0.to_str().unwrap()));
    }
}

#[test]
fn valid_partial_settings_and_non_utf8_failures_are_handled() {
    for original in [
        "{}",
        "{\"permissions\":{}}",
        "{\"permissions\":{\"allow\":[]}}",
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.settings(), original).unwrap();
        fixture.success();
        let parsed: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.settings()).unwrap()).unwrap();
        assert_eq!(parsed["permissions"]["allow"].as_array().unwrap().len(), 2);
    }
    let fixture = Fixture::new();
    fs::write(fixture.settings(), [0xff]).unwrap();
    let output = fixture.command().output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("settings.json: config is not UTF-8"));
    assert_eq!(fs::read(fixture.settings()).unwrap(), [0xff]);
    assert!(!fixture.rules().exists());
}
