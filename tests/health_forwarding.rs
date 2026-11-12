//! Set HEY_BOSS_TEST_BINARY to exercise an installed CLI instead of the Cargo build.
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

#[test]
fn health_forwards_host_arguments_and_harvester_exit_status() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("out");
    fs::create_dir_all(&out).unwrap();
    let home = tempfile::tempdir_in(out).unwrap();
    let bin = home.path().join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let harvester = bin.join("hey-harvester");
    fs::write(&harvester, "#!/bin/sh\nprintf '%s\\n' \"$@\"\nexit 23\n").unwrap();
    fs::set_permissions(&harvester, fs::Permissions::from_mode(0o700)).unwrap();
    let binary = std::env::var_os("HEY_BOSS_TEST_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_hey-boss").into());
    let result = Command::new(fs::canonicalize(binary).unwrap())
        .args(["health", "--host", "fixture.invalid", "status", "--json"])
        .env_clear()
        .env("HOME", home.path())
        .current_dir(home.path())
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"--host\nfixture.invalid\nstatus\n--json\n");
}
