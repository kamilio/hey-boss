use std::{fs, path::PathBuf, process::Command};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("hey-harvester-cli-{}-{name}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-harvester"));
        command
            .env("HOME", &self.0)
            .env("HEY_BOSS_HEALTH_DIR", self.0.join("health"));
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn reads_existing_inventory_and_writes_existing_health_configuration() {
    let f = Fixture::new("config");
    fs::create_dir_all(f.0.join(".hey-boss")).unwrap();
    fs::create_dir_all(f.0.join(".local/share/hey-boss")).unwrap();
    fs::write(
        f.0.join(".hey-boss/config.json"),
        r#"{"ssh_hosts":["devbox",{"host":"mac"}]}"#,
    )
    .unwrap();
    fs::write(f.0.join(".local/share/hey-boss/connections.json"), r#"{"machines":[{"host":"connected","state":"connected"},{"host":"disconnected","state":"offline"}]}"#).unwrap();
    let output = f.command().args(["hosts", "--json"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Vec<String>>(&output.stdout).unwrap(),
        ["connected", "devbox", "mac"]
    );
    let output = f
        .command()
        .args([
            "configure",
            "--worktrees",
            "false",
            "--logs",
            "true",
            "--aggressive",
            "true",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let store = hey_harvester::health::Store::new(f.0.join("health")).unwrap();
    assert!(!store.config().unwrap().clean_worktrees);
    assert!(store.config().unwrap().trim_worker_logs);
    assert!(store.config().unwrap().aggressive);
    assert_eq!(store.config().unwrap().worktree_min_age_days, 1);
    assert!(!store.config().unwrap().automatic);
}

#[test]
fn installation_preserves_disabled_schedule_and_existing_observations() {
    let f = Fixture::new("install");
    let store = hey_harvester::health::Store::new(f.0.join("health")).unwrap();
    let config = hey_harvester::health::Config {
        clean_caches: false,
        automatic: false,
        ..Default::default()
    };
    store.save("config.json", &config).unwrap();
    fs::write(f.0.join("health/state.json"), "existing observations").unwrap();
    let output = f.command().arg("install").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(f.0.join(".local/bin/hey-harvester")).unwrap(),
        fs::read(env!("CARGO_BIN_EXE_hey-harvester")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(f.0.join("health/state.json")).unwrap(),
        "existing observations"
    );
    assert!(!store.config().unwrap().automatic);
    assert!(!store.config().unwrap().clean_caches);
    assert!(!f.0.join("Library/LaunchAgents").exists());
    assert!(!f.0.join(".config/systemd").exists());
}
