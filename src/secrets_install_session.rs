//! An SSH login has a separate Keychain session from the logged-in desktop.
//! Only enrollment runs here; secret reads still require an unlocked local session.
use hey_boss_secrets::{Error, Result, backend};
use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn xml(value: &str) -> String {
    html_escape::encode_text(value).into_owned()
}

pub(super) fn initialize(binary: &Path, label: &str) -> Result<()> {
    initialize_with(binary, label, Command::output)
}

fn initialize_with(
    binary: &Path,
    label: &str,
    run: impl Fn(&mut Command) -> std::io::Result<std::process::Output>,
) -> Result<()> {
    let temp = crate::admin::Temporary::new().map_err(|_| Error::StorageFailure)?;
    let job = format!(
        "local.hey-boss.secrets-enroll-{}",
        temp.0.file_name().unwrap().to_string_lossy()
    );
    let domain = format!("gui/{}", unsafe { libc::geteuid() });
    let status = temp.0.join("status");
    // Values never enter this job. Arguments are separate plist strings, not shell source.
    let script = "\"$1\" secrets init --label \"$2\" >/dev/null 2>/dev/null; result=$?; printf '%s' \"$result\" > \"$3\"";
    let arguments = [
        "/bin/sh",
        "-c",
        script,
        "secrets-enroll",
        binary.to_str().ok_or(Error::InvalidInput)?,
        label,
        status.to_str().ok_or(Error::InvalidInput)?,
    ];
    let database = crate::issues::database_path().map_err(|_| Error::StorageFailure)?;
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array>{}</array><key>EnvironmentVariables</key><dict><key>HEY_BOSS_ISSUE_DB</key><string>{}</string></dict><key>RunAtLoad</key><true/></dict></plist>",
        xml(&job),
        arguments
            .iter()
            .map(|a| format!("<string>{}</string>", xml(a)))
            .collect::<String>(),
        xml(database.to_str().ok_or(Error::InvalidInput)?)
    );
    let path = temp.0.join("job.plist");
    backend::write_private(&path, plist.as_bytes())?;
    let result = run(Command::new("/bin/launchctl")
        .args(["bootstrap", &domain])
        .arg(&path))
    .map_err(|_| Error::StoreUnavailable)?;
    if !result.status.success() {
        return Err(Error::StoreUnavailable);
    }
    let deadline = Instant::now() + Duration::from_secs(45);
    let result = loop {
        if let Ok(code) = fs::read_to_string(&status) {
            // An empty file may be observed between creation and the short write.
            if !code.is_empty() {
                break if code == "0" {
                    Ok(())
                } else {
                    Err(Error::StoreUnavailable)
                };
            }
        }
        if Instant::now() >= deadline {
            break Err(Error::StoreUnavailable);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = run(Command::new("/bin/launchctl").args(["bootout", &format!("{domain}/{job}")]));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn enrollment_job_preserves_arguments_and_reports_failures() {
        let temp = crate::admin::Temporary::new().unwrap();
        // Execute launchd's actual argv without registering a job on the user's desktop.
        let run = |command: &mut Command| {
            let mut args = command.get_args();
            if args.next().unwrap() == "bootout" {
                return Command::new("/usr/bin/true").output();
            }
            let plist = Command::new("/usr/bin/plutil")
                .args(["-convert", "json", "-o", "-", "--"])
                .arg(args.nth(1).unwrap())
                .output()?;
            assert!(plist.status.success());
            let job: serde_json::Value = serde_json::from_slice(&plist.stdout).unwrap();
            let mut argv = job["ProgramArguments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap());
            Command::new(argv.next().unwrap()).args(argv).output()
        };
        let binary = temp.0.join("binary with ' & < chars");
        for exit in [0, 7] {
            fs::write(&binary, format!("#!/bin/sh\ntest \"$1\" = secrets && test \"$2\" = init && test \"$3\" = --label && test \"$4\" = 'Mac & <one>' || exit 19\nexit {exit}\n")).unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(
                initialize_with(&binary, "Mac & <one>", run).is_ok(),
                exit == 0
            );
        }
    }
}
