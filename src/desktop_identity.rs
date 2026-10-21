//! Associate launch agents with the installed desktop app, when one exists.
#[cfg(target_os = "macos")]
use std::process::Command;
use std::{
    io,
    path::{Path, PathBuf},
};

pub fn desktop_app(home: &Path) -> io::Result<Option<PathBuf>> {
    #[cfg(target_os = "macos")]
    {
        let registration = home.join("Library/LaunchAgents/local.hey-boss.plist");
        if !registration.try_exists()? {
            return Ok(None);
        }
        let output = Command::new("/usr/bin/plutil")
            .args(["-extract", "ProgramArguments.0", "raw", "-o", "-"])
            .arg(registration)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other("Cannot read desktop registration"));
        }
        let daemon = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        let app = daemon
            .ancestors()
            .nth(3)
            .filter(|app| {
                app.extension().is_some_and(|s| s == "app")
                    && daemon.ends_with("Contents/MacOS/hey-boss-daemon")
            })
            .ok_or_else(|| {
                io::Error::other("Desktop installation needs reinstalling before upgrading")
            })?;
        return Ok(daemon.is_file().then(|| app.to_owned()));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = home;
    #[allow(unreachable_code)]
    Ok(None)
}

pub fn association(home: &Path) -> String {
    if desktop_app(home).ok().flatten().is_some() {
        "<key>AssociatedBundleIdentifiers</key><array><string>local.hey-boss.desktop</string></array>".into()
    } else {
        String::new()
    }
}

/// Refresh legacy registrations as well as newly generated ones. Never reset grants.
pub fn refresh(home: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    if let Some(app) = desktop_app(home)? {
        let output = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
            .arg("-f").arg(app).output()?;
        if !output.status.success() {
            return Err(io::Error::other(
                "Could not register the desktop app with Launch Services",
            ));
        }
        associate_registrations(home)?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = home;
    Ok(())
}

#[cfg(target_os = "macos")]
fn associate_registrations(home: &Path) -> io::Result<()> {
    for label in [
        "local.hey-boss",
        "local.hey-boss-companion",
        "local.hey-boss-broker",
        "local.hey-boss-fleet-controller",
        "local.hey-boss-fleet-agent",
    ] {
        let path = home.join(format!("Library/LaunchAgents/{label}.plist"));
        if !path.try_exists()? {
            continue;
        }
        let output = Command::new("/usr/bin/plutil")
            .args([
                "-replace",
                "AssociatedBundleIdentifiers",
                "-json",
                "[\"local.hey-boss.desktop\"]",
            ])
            .arg(path)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "Could not associate {label} with the desktop app"
            )));
        }
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn launch_agents_only_claim_an_installed_desktop_bundle() {
        let temp = crate::admin::Temporary::new().unwrap();
        assert!(association(&temp.0).is_empty());
        let agents = temp.0.join("Library/LaunchAgents");
        fs::create_dir_all(&agents).unwrap();
        let app = temp.0.join("Hey Boss.app");
        let daemon = app.join("Contents/MacOS/hey-boss-daemon");
        fs::write(agents.join("local.hey-boss.plist"), format!(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>ProgramArguments</key><array><string>{}</string></array></dict></plist>",
            html_escape::encode_text(&daemon.to_string_lossy())
        )).unwrap();
        assert!(
            association(&temp.0).is_empty(),
            "A stale registration is not an installed app"
        );
        fs::create_dir_all(daemon.parent().unwrap()).unwrap();
        fs::write(&daemon, "fixture").unwrap();
        assert_eq!(desktop_app(&temp.0).unwrap(), Some(app));
        assert_eq!(
            association(&temp.0),
            "<key>AssociatedBundleIdentifiers</key><array><string>local.hey-boss.desktop</string></array>"
        );
    }

    #[test]
    fn legacy_registration_refresh_preserves_arguments_and_is_repeatable() {
        let temp = crate::admin::Temporary::new().unwrap();
        let agents = temp.0.join("Library/LaunchAgents");
        fs::create_dir_all(&agents).unwrap();
        let path = agents.join("local.hey-boss-companion.plist");
        fs::write(&path, r#"<?xml version="1.0"?><plist version="1.0"><dict><key>ProgramArguments</key><array><string>/bin/test &amp; cli</string><string>companion</string><string>auto</string></array><key>KeepAlive</key><true/></dict></plist>"#).unwrap();
        for _ in 0..2 {
            associate_registrations(&temp.0).unwrap();
            let output = Command::new("/usr/bin/plutil")
                .args(["-convert", "json", "-o", "-"])
                .arg(&path)
                .output()
                .unwrap();
            assert!(output.status.success());
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                value["AssociatedBundleIdentifiers"],
                serde_json::json!(["local.hey-boss.desktop"])
            );
            assert_eq!(
                value["ProgramArguments"],
                serde_json::json!(["/bin/test & cli", "companion", "auto"])
            );
            assert_eq!(value["KeepAlive"], true);
            assert!(!agents.join("local.hey-boss-broker.plist").exists());
        }
    }
}
