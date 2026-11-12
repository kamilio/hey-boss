//! Run after packaging the app, so the checks exercise the shipped manifest and daemon path.
#![cfg(target_os = "macos")]

use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn read_plist(path: &Path) -> Value {
    let result = Command::new("plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output()
        .unwrap();
    assert!(result.status.success(), "Could not read {}", path.display());
    serde_json::from_slice(&result.stdout).unwrap()
}

#[test]
#[ignore = "requires the packaged out/Hey Boss.app"]
fn packaged_transport_allows_http_only_for_the_local_issues_hostname() {
    let info = read_plist(Path::new("out/Hey Boss.app/Contents/Info.plist"));
    assert_eq!(
        info["NSAppTransportSecurity"],
        json!({"NSExceptionDomains":{
            "hey-boss.test":{"NSExceptionAllowsInsecureHTTPLoads":true}
        }})
    );
}

#[test]
#[ignore = "requires the packaged out/Hey Boss.app"]
fn reinstall_preserves_desktop_and_companion_listeners() {
    let daemon = Path::new("out/Hey Boss.app/Contents/MacOS/hey-boss-daemon")
        .canonicalize()
        .unwrap();
    let root = std::env::current_dir()
        .unwrap()
        .join("out")
        .join(format!("setup-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    // Run the installer against private paths. Never change a running launchd job.
    let source = include_str!("../setup_hey_boss.swift");
    assert_eq!(source.matches("\"/bin/launchctl\"").count(), 1);
    let script = root.join("setup.swift");
    fs::write(
        &script,
        source.replace("\"/bin/launchctl\"", "\"/usr/bin/true\""),
    )
    .unwrap();
    let setup = root.join("setup");
    assert!(
        Command::new("xcrun")
            .args(["swiftc", "-module-cache-path"])
            .arg(root.join("cache"))
            .arg(script)
            .arg("-o")
            .arg(&setup)
            .status()
            .unwrap()
            .success()
    );
    for companion in [false, true] {
        let dir = root.join(if companion { "companion" } else { "desktop" });
        let bin = dir.join("bin");
        let state = dir.join("state");
        let agents = dir.join("agents");
        fs::create_dir_all(&bin).unwrap();
        let marker = bin.join("hey-boss.companion");
        if companion {
            fs::write(&marker, "existing companion").unwrap();
        }
        for _ in 0..2 {
            assert!(
                Command::new(&setup)
                    .args([&state, &bin, &agents, &daemon])
                    .status()
                    .unwrap()
                    .success()
            );
            let plist = read_plist(&agents.join("local.hey-boss.plist"));
            let socket = if companion {
                "desktop.sock"
            } else {
                "daemon.sock"
            };
            assert_eq!(
                plist["Sockets"]["Listener"]["SockPathName"],
                json!(state.join(socket))
            );
            assert_eq!(plist["ProgramArguments"], json!([daemon]));
            assert_eq!(
                plist["EnvironmentVariables"]["HEY_BOSS_CLI_PATH"],
                json!(bin.join("hey-boss"))
            );
            assert_eq!(
                fs::read_to_string(bin.join("hey-boss.state")).unwrap(),
                state.to_str().unwrap()
            );
            if companion {
                assert_eq!(fs::read_to_string(&marker).unwrap(), "existing companion");
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}
