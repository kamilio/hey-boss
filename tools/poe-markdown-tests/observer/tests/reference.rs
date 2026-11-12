use serde_json::{Value, json};
use std::{env, fs, path::Path, process::Command};

#[test]
fn observe_upstream_suite() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("markdown-observer-")
        .tempdir_in(base.join("node_modules"))
        .unwrap();
    let package = temp.path().file_name().unwrap().to_str().unwrap();
    let addon = temp.path().join("capture.node");
    let deps = env::current_exe().unwrap();
    fs::copy(
        deps.parent().unwrap().join(format!(
            "{}poe_markdown_observer{}",
            env::consts::DLL_PREFIX,
            env::consts::DLL_SUFFIX
        )),
        &addon,
    )
    .unwrap();
    // Vitest inlines setup files. Resolve the setup alias to an external package
    // so Node loads the binary instead of Vite parsing it as source.
    let alias = base.join("reference-observer");
    let mut config: Value =
        serde_json::from_slice(&fs::read(base.join("vitest.json")).unwrap()).unwrap();
    config["test"]["globals"] = true.into();
    config["test"]["setupFiles"] = json!([alias]);
    config["test"]["server"]["deps"]["external"] = json!([package]);
    config["resolve"]["alias"] = json!({(alias.to_str().unwrap()): addon});
    let config_path = temp.path().join("vitest.json");
    fs::write(&config_path, config.to_string()).unwrap();
    let result = Command::new("node")
        .current_dir(base)
        .env(
            "NODE_OPTIONS",
            format!(
                "{} --experimental-addon-modules",
                env::var("NODE_OPTIONS").unwrap_or_default()
            ),
        )
        .env("FORCE_COLOR", "1")
        .args(["node_modules/vitest/vitest.mjs", "run", "--config"])
        .arg(config_path)
        .args([
            "--reporter=default",
            "--reporter=json",
            "--outputFile=report.json",
        ])
        .status()
        .unwrap();
    assert!(result.success(), "upstream Vitest suite failed: {result}");
}
