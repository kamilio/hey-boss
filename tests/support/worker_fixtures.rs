use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command, sync::OnceLock};

// Only test callers build these executables. Cargo supplies the path for the active target.
pub fn binary(name: &str) -> PathBuf {
    static BINARIES: OnceLock<BTreeMap<String, PathBuf>> = OnceLock::new();
    let bins = BINARIES.get_or_init(|| {
        let mut cargo = Command::new(env!("CARGO"));
        // Parent package metadata otherwise changes build-script fingerprints (e.g. ring).
        for (key, _) in std::env::vars_os() {
            if key.as_encoded_bytes().starts_with(b"CARGO_PKG_")
                || key == "CARGO_MANIFEST_DIR"
                || key == "OUT_DIR"
            {
                cargo.env_remove(key);
            }
        }
        let output = cargo
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args([
                "build",
                "--locked",
                "-p",
                "hey-boss",
                "--example",
                "codex-worker",
                "--example",
                "codex-drain",
                "--example",
                "codex-approvals",
                "--example",
                "codex-github-watch",
                "--example",
                "provider-worker",
                "--message-format=json-render-diagnostics",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .filter_map(|line| {
                let v: Value = serde_json::from_str(line).unwrap();
                Some((
                    v["target"]["name"].as_str()?.to_owned(),
                    PathBuf::from(v["executable"].as_str()?),
                ))
            })
            .collect()
    });
    bins[name].clone()
}
