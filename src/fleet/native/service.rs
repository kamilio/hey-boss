use super::{Result, context::Context, replica::invalid};
use serde_json::json;
#[cfg(target_os = "macos")]
use std::time::Duration;
use std::{fs, path::PathBuf, process::Command};
#[cfg(target_os = "linux")]
fn checked(command: &mut Command) -> Result<()> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "Fleet service installation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}
#[cfg(any(target_os = "macos", test))]
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
#[cfg(any(target_os = "macos", test))]
pub(super) fn mac_definition(ctx: &Context, role: &str) -> String {
    let name = if role == "controller" {
        "supervisor"
    } else {
        "companion"
    };
    let label = format!("local.hey-boss-fleet-{role}");
    let log = xml(&ctx
        .state
        .join(format!("fleet-{role}.log"))
        .to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array><string>{}</string><string>fleet</string><string>{name}</string></array><key>EnvironmentVariables</key><dict><key>HEY_BOSS_FLEET_SUPERVISED</key><string>1</string></dict><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer><key>AbandonProcessGroup</key><true/><key>StandardOutPath</key><string>{log}</string><key>StandardErrorPath</key><string>{log}</string></dict></plist>\n",
        xml(&ctx.binary.to_string_lossy())
    )
}
#[cfg(any(target_os = "linux", test))]
pub(super) fn linux_definition(ctx: &Context, role: &str) -> String {
    let name = if role == "controller" {
        "supervisor"
    } else {
        "companion"
    };
    let binary = ctx
        .binary
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!(
        "[Unit]\nDescription=Hey Boss fleet {name}\n[Service]\nExecStart=\"{binary}\" fleet {name}\nEnvironment=HEY_BOSS_FLEET_SUPERVISED=1\nKillMode=process\nRestart=always\nRestartSec=10\n[Install]\nWantedBy=default.target\n"
    )
}
// Stage only files; service control remains exclusively in install().
fn stage_service(ctx: &Context, role: &str) -> Result<PathBuf> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        #[cfg(target_os = "macos")]
        let (path, definition) = (
            ctx.home
                .join("Library/LaunchAgents")
                .join(format!("local.hey-boss-fleet-{role}.plist")),
            mac_definition(ctx, role),
        );
        #[cfg(target_os = "linux")]
        let (path, definition) = (
            ctx.home
                .join(".config/systemd/user")
                .join(format!("hey-boss-fleet-{role}.service")),
            linux_definition(ctx, role),
        );
        ctx.protect_file(&path)?;
        #[cfg(target_os = "macos")]
        ctx.protect_file(&ctx.state.join(format!("fleet-{role}.log")))?;
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, definition)?;
        Ok(path)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    Err(invalid(
        "Automatic startup requires macOS launchd or Linux systemd",
    ))
}
pub(super) fn install(ctx: &Context, role: &str) -> Result<()> {
    if !matches!(role, "controller" | "agent") {
        return Err(invalid("Unknown fleet service role"));
    }
    let path = stage_service(ctx, role)?;
    #[cfg(target_os = "macos")]
    {
        let label = format!("local.hey-boss-fleet-{role}");
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("{domain}/{label}")])
            .output();
        let mut last = None;
        for delay in [0, 100, 200, 400, 800, 1000, 2000, 2000, 2000] {
            if delay > 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }
            let output = Command::new("launchctl")
                .args(["bootstrap", &domain])
                .arg(&path)
                .output()?;
            if output.status.success() {
                return Ok(());
            }
            let retry = output.status.code() == Some(5);
            last = Some(output);
            if !retry {
                break;
            }
        }
        return Err(format!(
            "Fleet service installation failed: {}",
            String::from_utf8_lossy(&last.unwrap().stderr)
        )
        .into());
    }
    #[cfg(target_os = "linux")]
    {
        checked(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
        checked(
            Command::new("systemctl")
                .args(["--user", "enable", "--now"])
                .arg(path.file_name().unwrap()),
        )?;
        checked(
            Command::new("systemctl")
                .args(["--user", "restart"])
                .arg(path.file_name().unwrap()),
        )?;
        return Ok(());
    }
    #[allow(unreachable_code)]
    Err(invalid(
        "Automatic startup requires macOS launchd or Linux systemd",
    ))
}
pub(super) fn ensure_companion(ctx: &Context) -> Result<()> {
    let path = ctx.state.join("fleet-agent-service.json");
    let build = ctx.build()?;
    if ctx.read_json(&path, json!({}))?["build"] != build {
        install(ctx, "agent")?;
        ctx.atomic_json(&path, &json!({"build":build}))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::context::tests::{assert_sqlite_locked, test_context};
    use super::*;
    #[test]
    fn service_definition_aliases_cannot_truncate_sqlite_files() {
        let (root, ctx, store) = test_context();
        let path = stage_service(&ctx, "agent").unwrap();
        fs::remove_file(&path).unwrap();
        for suffix in ["", "-wal", "-shm"] {
            for symbolic in [false, true] {
                let target = root.join(format!("issues.db{suffix}"));
                if symbolic {
                    std::os::unix::fs::symlink(&target, &path).unwrap();
                } else {
                    fs::hard_link(&target, &path).unwrap();
                }
                let before = fs::metadata(&target).unwrap().len();
                let result = stage_service(&ctx, "agent");
                assert_eq!(
                    fs::metadata(&target).unwrap().len(),
                    before,
                    "Service definition truncated SQLite file {suffix}"
                );
                assert!(result.unwrap_err().to_string().contains("must not alias"));
                assert_sqlite_locked(&ctx.path);
                fs::remove_file(&path).unwrap();
            }
        }
        let path = stage_service(&ctx, "agent").unwrap();
        assert!(fs::read_to_string(path).unwrap().contains("companion"));
        assert_sqlite_locked(&ctx.path);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_log_aliases_are_rejected_before_service_admission() {
        let (root, ctx, store) = test_context();
        let log = ctx.state.join("fleet-agent.log");
        fs::hard_link(&ctx.path, &log).unwrap();
        let result = stage_service(&ctx, "agent");
        assert!(result.unwrap_err().to_string().contains("must not alias"));
        assert_sqlite_locked(&ctx.path);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn service_definitions_keep_workers_outside_service_shutdown() {
        let root = std::env::temp_dir();
        let ctx = Context {
            home: root.clone(),
            state: root.clone(),
            desired: root.join("fleet.json"),
            binary: root.join("hey boss & test"),
            path: root.join("issues.db"),
            node: "fixture".into(),
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        for role in ["controller", "agent"] {
            let mac = mac_definition(&ctx, role);
            assert!(mac.contains("<key>AbandonProcessGroup</key><true/>"));
            assert!(mac.contains(&format!("local.hey-boss-fleet-{role}")));
            assert!(mac.contains("&amp;"));
            let linux = linux_definition(&ctx, role);
            assert!(linux.contains("KillMode=process\n"));
            assert!(linux.contains("Restart=always\n"));
            assert!(linux.contains("ExecStart=\""));
        }
    }
}
