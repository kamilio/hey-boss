use serde_json::Value;
use std::process::Command;

#[test]
fn release_cli_persists_queue_and_reports_transport_errors_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("project.json");
    let state = dir.path().join("queue.sqlite");
    std::fs::write(
        &config,
        include_str!("../src/release/profiles/poe-code.json"),
    )
    .unwrap();
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_hey-gh"))
            .args(["--server", "http://127.0.0.1:1", "release"])
            .args(args)
            .output()
            .unwrap()
    };
    let added = invoke(&[
        "add",
        "--config",
        config.to_str().unwrap(),
        "--state",
        state.to_str().unwrap(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let failed = invoke(&["poll", "--state", state.to_str().unwrap()]);
    assert!(!failed.status.success());
    let data: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(data["entries"][0]["report"]["state"], "unknown");
    assert!(
        !data["entries"][0]["report"]["errors"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let status = invoke(&["status", "--state", state.to_str().unwrap()]);
    assert!(status.status.success());
    let data: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(data[0]["report"]["state"], "unknown");
    let removed = invoke(&[
        "remove",
        "--state",
        state.to_str().unwrap(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ]);
    assert!(removed.status.success());
    let status = invoke(&["status", "--state", state.to_str().unwrap()]);
    assert_eq!(
        serde_json::from_slice::<Value>(&status.stdout).unwrap(),
        serde_json::json!([])
    );
}
