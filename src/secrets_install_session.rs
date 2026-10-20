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
    initialize_with(Path::new("/bin/launchctl"), binary, label)
}

fn initialize_with(launchctl: &Path, binary: &Path, label: &str) -> Result<()> {
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
    let result = Command::new(launchctl)
        .args(["bootstrap", &domain])
        .arg(&path)
        .output()
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
    let _ = Command::new(launchctl)
        .args(["bootout", &format!("{domain}/{job}")])
        .output();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn enrollment_job_preserves_arguments_and_reports_failures() {
        let temp = crate::admin::Temporary::new().unwrap();
        let launchctl = temp.0.join("launchctl");
        // Run exactly the plist argv with a fake launchctl, without creating a real GUI job.
        fs::write(&launchctl, "#!/bin/sh\nif [ \"$1\" = bootout ]; then exit 0; fi\n/usr/bin/osascript -l JavaScript - \"$3\" <<'JXA'\nObjC.import('Foundation');\nfunction run(args) {\n const plist = $.NSDictionary.dictionaryWithContentsOfFile($(args[0]));\n const argv = ObjC.deepUnwrap(plist.objectForKey('ProgramArguments'));\n const task = $.NSTask.alloc.init;\n task.launchPath = argv.shift(); task.arguments = argv;\n task.launch; task.waitUntilExit;\n}\nJXA\n").unwrap();
        fs::set_permissions(&launchctl, fs::Permissions::from_mode(0o700)).unwrap();
        let binary = temp.0.join("binary with ' & < chars");
        for exit in [0, 7] {
            fs::write(&binary, format!("#!/bin/sh\ntest \"$1\" = secrets && test \"$2\" = init && test \"$3\" = --label && test \"$4\" = 'Mac & <one>' || exit 19\nexit {exit}\n")).unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(
                initialize_with(&launchctl, &binary, "Mac & <one>").is_ok(),
                exit == 0
            );
        }
    }
}
