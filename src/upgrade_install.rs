//! Publish a validated build with rollback, while the caller owns upgrade.lock.
use super::*;

fn desktop_app() -> io::Result<Option<PathBuf>> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let plist = home()?.join("Library/LaunchAgents/local.hey-boss.plist");
    if !plist.exists() {
        return Ok(None);
    }
    let bytes = output(
        Command::new("/usr/bin/plutil")
            .args(["-extract", "ProgramArguments.0", "raw", "-o", "-"])
            .arg(plist),
    )?;
    let daemon = PathBuf::from(String::from_utf8_lossy(&bytes).trim());
    let app = daemon
        .ancestors()
        .nth(3)
        .ok_or_else(|| error("Invalid desktop installation"))?;
    if daemon
        .parent()
        .and_then(Path::file_name)
        .is_none_or(|s| s != "MacOS")
        || app.extension().is_none_or(|s| s != "app")
    {
        return Err(error(
            "Desktop installation needs reinstalling before upgrading",
        ));
    }
    Ok(Some(app.to_owned()))
}
fn restart_hey_gh_daemon() {
    let _ = Command::new("/usr/bin/pkill")
        .args(["-TERM", "-f", "hey-gh serve"])
        .status();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let _ = Command::new("/usr/bin/pkill")
        .args(["-KILL", "-f", "hey-gh serve"])
        .status();
}
fn restart_desktop() -> io::Result<()> {
    restart_hey_gh_daemon();
    output(
        Command::new("/bin/launchctl")
            .args(["kickstart", "-k"])
            .arg(format!("gui/{}/local.hey-boss", unsafe { libc::getuid() })),
    )?;
    Ok(())
}
fn restart_companion() -> io::Result<()> {
    restart_hey_gh_daemon();
    if cfg!(target_os = "macos") {
        output(
            Command::new("/bin/launchctl")
                .args(["kickstart", "-k"])
                .arg(format!("gui/{}/local.hey-boss-broker", unsafe {
                    libc::getuid()
                })),
        )?;
    } else if Command::new("systemctl")
        .args([
            "--user",
            "is-active",
            "--quiet",
            "hey-boss-companion.service",
        ])
        .status()
        .is_ok_and(|s| s.success())
    {
        output(Command::new("systemctl").args([
            "--user",
            "restart",
            "hey-boss-companion.service",
        ]))?;
        output(Command::new("systemctl").args([
            "--user",
            "is-active",
            "--quiet",
            "hey-boss-companion.service",
        ]))?;
    }
    Ok(())
}
fn shortcut(binary: &Path) -> io::Result<()> {
    match symlink(
        binary
            .file_name()
            .ok_or_else(|| error("Invalid binary path"))?,
        binary.with_file_name("hb"),
    ) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

fn companion_bins(binary: &Path, home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for name in ["hey-gh", "hey-harvester"] {
        paths.push(binary.with_file_name(name));
        for directory in [".cargo/bin", ".local/bin"] {
            let existing = home.join(directory).join(name);
            if existing.exists() && !paths.contains(&existing) {
                paths.push(existing);
            }
        }
    }
    paths
}

pub(super) fn publish(
    snapshot: &Path,
    binary: &Path,
    built: &Path,
    state: &Path,
    receipt: &Receipt,
) -> io::Result<()> {
    let companion = binary.with_file_name("hey-boss.companion").exists();
    let app = if companion { None } else { desktop_app()? };
    let skills = [".codex", ".agents", ".claude"]
        .into_iter()
        .map(|root| Ok(home()?.join(root).join("skills/hey-boss")))
        .collect::<io::Result<Vec<_>>>()?;
    publish_to(
        snapshot,
        binary,
        built,
        state,
        receipt,
        &Services {
            app,
            companion,
            skills,
            companion_bins: companion_bins(binary, &home()?),
            harvester: if cfg!(target_os = "macos") {
                let standalone = home()?.join(".local/bin/hey-harvester");
                Some(if standalone.exists() {
                    standalone
                } else {
                    binary.with_file_name("hey-harvester")
                })
            } else {
                None
            },
        },
        hey_harvester::cli::refresh_schedule,
    )
}

struct Services {
    app: Option<PathBuf>,
    companion: bool,
    skills: Vec<PathBuf>,
    companion_bins: Vec<PathBuf>,
    harvester: Option<PathBuf>,
}

fn publish_to(
    snapshot: &Path,
    binary: &Path,
    built: &Path,
    state: &Path,
    receipt: &Receipt,
    services: &Services,
    mut refresh_harvester: impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let Services {
        app,
        companion,
        skills,
        companion_bins,
        harvester,
    } = services;
    let companion = *companion;
    let temp = Temp::new()?;
    let staged_app = temp.0.join("Hey Boss.app");
    if app.is_some() {
        output(
            Command::new("/usr/bin/xcrun")
                .args([
                    "swiftc",
                    "-O",
                    "-whole-module-optimization",
                    "-parse-as-library",
                ])
                .arg(snapshot.join("hey_boss_daemon.swift"))
                .arg("-o")
                .arg(temp.0.join("daemon")),
        )?;
        output(
            Command::new("/usr/bin/swift")
                .arg(snapshot.join("package_hey_boss.swift"))
                .arg(temp.0.join("daemon"))
                .arg(&staged_app),
        )?;
        output(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--strict"])
                .arg(&staged_app),
        )?;
    }
    // Schema changes precede any service restart or binary replacement.
    output(
        Command::new(built)
            .args(["issue", "migrate", "--installation"])
            .arg(binary),
    )?;
    let backup = state.join("upgrade-backups");
    fs::create_dir_all(&backup)?;
    fs::set_permissions(&backup, fs::Permissions::from_mode(0o700))?;
    let previous = backup.join("hey-boss.previous");
    fs::copy(binary, &previous)?;
    // Destination filesystem staging makes the app swap atomic too.
    let adjacent = app.as_ref().map(|a| {
        a.with_file_name(format!(
            "{}.upgrade-new",
            a.file_name().unwrap().to_string_lossy()
        ))
    });
    let app_backup = app.as_ref().map(|a| {
        a.with_file_name(format!(
            "{}.upgrade-previous",
            a.file_name().unwrap().to_string_lossy()
        ))
    });
    if adjacent.as_ref().is_some_and(|p| p.exists())
        || app_backup.as_ref().is_some_and(|p| p.exists())
    {
        return Err(error(
            "Desktop staging or rollback already exists; inspect it before retrying",
        ));
    }
    if let Some(adjacent) = &adjacent {
        copy_tree(&staged_app, adjacent)?;
    }
    let mut companion_backups = Vec::new();
    for (index, destination) in companion_bins.iter().enumerate() {
        let saved = if destination.exists() {
            let saved = backup.join(format!("companion.{index}.previous"));
            fs::copy(destination, &saved)?;
            Some(saved)
        } else {
            None
        };
        companion_backups.push((destination, saved));
    }
    let mut replaced_app = false;
    let mut replaced_binary = false;
    let result = (|| -> io::Result<()> {
        atomic_copy(built, binary, 0o755)?;
        replaced_binary = true;
        for destination in companion_bins {
            atomic_copy(
                &built.with_file_name(
                    destination
                        .file_name()
                        .ok_or_else(|| error("Invalid companion binary path"))?,
                ),
                destination,
                0o755,
            )?;
        }
        if let (Some(app), Some(adjacent), Some(app_backup)) = (&app, &adjacent, &app_backup) {
            fs::rename(app, app_backup)?;
            if let Err(e) = fs::rename(adjacent, app) {
                fs::rename(app_backup, app)?;
                return Err(e);
            }
            replaced_app = true;
            restart_desktop()?;
            let mut healthy = false;
            for _ in 0..10 {
                if output(Command::new(binary).args(["overview", "--json"])).is_ok() {
                    healthy = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            if !healthy {
                return Err(error("Desktop failed health verification"));
            }
        } else if companion {
            restart_companion()?;
        }
        if installed_id(binary).as_deref() != Some(&receipt.source.build) {
            return Err(error("Installed CLI failed build verification"));
        }
        shortcut(binary)?;
        // The running updater can be older than the staged skill bundle.
        let skill_source = snapshot.join("skills/hey-boss");
        let mut skill_files = Vec::new();
        if !skills.is_empty() {
            collect(&skill_source, &skill_source, &mut skill_files)?;
            skill_files.sort_by_key(|path| path == Path::new("SKILL.md"));
        }
        for skill in skills {
            for relative in &skill_files {
                atomic_copy(&skill_source.join(relative), &skill.join(relative), 0o644)?;
            }
        }
        if let Some(harvester) = harvester {
            refresh_harvester(harvester)?;
        }
        // The durable receipt is the last publication step. Failed installations
        // leave the previous generation authoritative.
        write_json(&state.join("upgrade-receipt.json"), receipt)
    })();
    if let Err(e) = result {
        for (destination, saved) in companion_backups {
            if let Some(saved) = saved {
                atomic_copy(&saved, destination, 0o755)?;
            } else if destination.exists() {
                fs::remove_file(destination)?;
            }
        }
        if replaced_binary {
            atomic_copy(&previous, binary, 0o755)?;
        }
        if replaced_app {
            let app = app.as_ref().unwrap();
            fs::remove_dir_all(app)?;
            fs::rename(app_backup.as_ref().unwrap(), app)?;
            let _ = restart_desktop();
        } else if companion && replaced_binary {
            let _ = restart_companion();
        }
        if let Some(adjacent) = adjacent
            && adjacent.exists()
        {
            let _ = fs::remove_dir_all(adjacent);
        }
        if replaced_binary
            && let Some(harvester) = harvester
            && let Err(rollback) = refresh_harvester(harvester)
        {
            return Err(error(format!(
                "{e}; binaries restored but harvester schedule recovery failed: {rollback}"
            )));
        }
        return Err(e);
    }
    if let Some(app_backup) = app_backup {
        fs::remove_dir_all(app_backup)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_standalone_companions_follow_package_upgrades_without_duplicates() {
        let temp = Temp::new().unwrap();
        let home = temp.0.join("home");
        let standalone = home.join(".local/bin/hey-harvester");
        script(&standalone, "echo standalone");
        let homebrew = temp.0.join("homebrew/bin/hey-boss");
        let destinations = companion_bins(&homebrew, &home);
        assert!(destinations.contains(&homebrew.with_file_name("hey-harvester")));
        assert!(
            destinations.contains(&standalone),
            "scheduled standalone binary must be upgraded"
        );
        assert!(
            !destinations.contains(&home.join(".local/bin/hey-gh")),
            "do not create previously absent alternate installations"
        );
        let local_installation = companion_bins(&home.join(".local/bin/hey-boss"), &home);
        assert_eq!(
            local_installation
                .iter()
                .filter(|p| *p == &standalone)
                .count(),
            1
        );
    }

    fn script(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn receipt() -> Receipt {
        Receipt {
            generation: 2,
            source: Source {
                kind: "main".into(),
                repository: Some("repo".into()),
                commit: Some("commit".into()),
                ancestors: Vec::new(),
                build: "1234567890abcdef".into(),
            },
            installed_at: 1,
        }
    }
    #[test]
    fn migration_failure_preserves_binary_and_receipt() {
        let temp = Temp::new().unwrap();
        let bin = temp.0.join("installed/hey-boss");
        let built = temp.0.join("built");
        script(&bin, "echo previous");
        script(&built, "echo migration-failed >&2; exit 1");
        script(&built.with_file_name("hey-gh"), "echo new-hey-gh");
        script(
            &built.with_file_name("hey-harvester"),
            "echo new-hey-harvester",
        );
        script(&bin.with_file_name("hey-gh"), "echo old-hey-gh");
        script(
            &bin.with_file_name("hey-harvester"),
            "echo old-hey-harvester",
        );
        let original = fs::read(&bin).unwrap();
        let state = temp.0.join("state");
        fs::create_dir(&state).unwrap();
        fs::write(state.join("upgrade-receipt.json"), "old-receipt").unwrap();
        let result = publish_to(
            &temp.0,
            &bin,
            &built,
            &state,
            &receipt(),
            &Services {
                app: None,
                companion: false,
                companion_bins: vec![],
                harvester: None,
                skills: Vec::new(),
            },
            |_| panic!("No harvester schedule in this fixture"),
        );
        assert!(result.unwrap_err().to_string().contains("migration-failed"));
        assert_eq!(fs::read(&bin).unwrap(), original);
        assert_eq!(
            fs::read_to_string(bin.with_file_name("hey-harvester")).unwrap(),
            "#!/bin/sh\necho old-hey-harvester\n"
        );
        assert_eq!(
            fs::read_to_string(bin.with_file_name("hey-gh")).unwrap(),
            "#!/bin/sh\necho old-hey-gh\n"
        );
        assert_eq!(
            fs::read_to_string(state.join("upgrade-receipt.json")).unwrap(),
            "old-receipt"
        );
    }
    #[test]
    fn post_install_failure_rolls_back_binary_without_publishing_new_generation() {
        let temp = Temp::new().unwrap();
        let bin = temp.0.join("installed/hey-boss");
        let built = temp.0.join("built");
        script(&bin, "echo previous");
        script(
            &built,
            "if [ \"$1\" = --version ]; then echo 'hey-boss (build 0000000000000000)'; fi; exit 0",
        );
        script(&built.with_file_name("hey-gh"), "echo new-hey-gh");
        script(
            &built.with_file_name("hey-harvester"),
            "echo new-hey-harvester",
        );
        script(&bin.with_file_name("hey-gh"), "echo old-hey-gh");
        script(
            &bin.with_file_name("hey-harvester"),
            "echo old-hey-harvester",
        );
        let original = fs::read(&bin).unwrap();
        let state = temp.0.join("state");
        fs::create_dir(&state).unwrap();
        fs::write(state.join("upgrade-receipt.json"), "old-receipt").unwrap();
        let result = publish_to(
            &temp.0,
            &bin,
            &built,
            &state,
            &receipt(),
            &Services {
                app: None,
                companion: false,
                companion_bins: vec![
                    bin.with_file_name("hey-gh"),
                    bin.with_file_name("hey-harvester"),
                ],
                skills: Vec::new(),
                harvester: None,
            },
            |_| panic!("No harvester schedule in this fixture"),
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("build verification")
        );
        assert_eq!(
            fs::read_to_string(bin.with_file_name("hey-harvester")).unwrap(),
            "#!/bin/sh\necho old-hey-harvester\n"
        );
        assert_eq!(fs::read(&bin).unwrap(), original);
        assert_eq!(
            fs::read_to_string(bin.with_file_name("hey-gh")).unwrap(),
            "#!/bin/sh\necho old-hey-gh\n"
        );
        assert_eq!(
            fs::read_to_string(state.join("upgrade-receipt.json")).unwrap(),
            "old-receipt"
        );
    }
    #[test]
    fn harvester_schedule_refresh_precedes_receipt_and_failure_rolls_back() {
        for failure in ["none", "refresh", "receipt", "recovery"] {
            let temp = Temp::new().unwrap();
            let bin = temp.0.join("installed/hey-boss");
            let built = temp.0.join("built/hey-boss");
            let harvester = bin.with_file_name("hey-harvester");
            script(&bin, "echo previous");
            script(&harvester, "echo previous-harvester");
            script(
                &built,
                "if [ \"$1\" = --version ]; then echo 'hey-boss (build 1234567890abcdef)'; fi",
            );
            script(&built.with_file_name("hey-harvester"), "echo new-harvester");
            let state = temp.0.join("state");
            fs::create_dir_all(&state).unwrap();
            fs::write(state.join("upgrade-receipt.json"), "old-receipt").unwrap();
            let mut registrations = Vec::new();
            let result = publish_to(
                &temp.0,
                &bin,
                &built,
                &state,
                &receipt(),
                &Services {
                    app: None,
                    companion: false,
                    companion_bins: vec![harvester.clone()],
                    skills: Vec::new(),
                    harvester: Some(harvester.clone()),
                },
                |path| {
                    assert_eq!(path, harvester);
                    registrations.push(fs::read_to_string(path).unwrap());
                    if registrations.len() == 1 {
                        assert_eq!(
                            fs::read_to_string(state.join("upgrade-receipt.json")).unwrap(),
                            "old-receipt"
                        );
                        if failure == "receipt" {
                            fs::remove_file(state.join("upgrade-receipt.json")).unwrap();
                            fs::create_dir(state.join("upgrade-receipt.json")).unwrap();
                        }
                        if matches!(failure, "refresh" | "recovery") {
                            return Err(error("registration failed"));
                        }
                    } else if failure == "recovery" {
                        return Err(error("recovery failed"));
                    }
                    Ok(())
                },
            );
            assert_eq!(
                registrations.first().map(String::as_str),
                Some("#!/bin/sh\necho new-harvester\n"),
                "The new binary must be registered before publication"
            );
            if failure != "none" {
                let error = result.unwrap_err().to_string();
                if failure == "recovery" {
                    assert!(error.contains("registration failed"));
                    assert!(error.contains("schedule recovery failed: recovery failed"));
                }
                if failure != "receipt" {
                    assert_eq!(
                        fs::read_to_string(state.join("upgrade-receipt.json")).unwrap(),
                        "old-receipt"
                    );
                }
                assert_eq!(registrations.len(), 2);
                assert_eq!(registrations[1], "#!/bin/sh\necho previous-harvester\n");
                assert_eq!(
                    fs::read_to_string(&harvester).unwrap(),
                    "#!/bin/sh\necho previous-harvester\n"
                );
                assert_eq!(
                    fs::read_to_string(&bin).unwrap(),
                    "#!/bin/sh\necho previous\n"
                );
            } else {
                result.unwrap();
                assert_eq!(registrations.len(), 1);
                assert_eq!(
                    json::<Receipt>(&state.join("upgrade-receipt.json")).unwrap(),
                    receipt()
                );
            }
        }
    }

    #[test]
    fn successful_install_publishes_verified_receipt_shortcut_and_skill() {
        let temp = Temp::new().unwrap();
        let bin = temp.0.join("installed/hey-boss");
        let built = temp.0.join("built");
        script(&bin, "echo previous");
        script(
            &built,
            "if [ \"$1\" = --version ]; then echo 'hey-boss (build 1234567890abcdef)'; fi; exit 0",
        );
        script(&built.with_file_name("hey-gh"), "echo new-hey-gh");
        script(
            &built.with_file_name("hey-harvester"),
            "echo new-hey-harvester",
        );
        for (relative, _) in hey_boss::skill::FILES {
            let source = temp.0.join("skills/hey-boss").join(relative);
            fs::create_dir_all(source.parent().unwrap()).unwrap();
            fs::write(source, format!("staged {relative}")).unwrap();
        }
        fs::write(
            temp.0.join("skills/hey-boss/references/new-workflow.md"),
            "New in this release",
        )
        .unwrap();
        let skill = temp.0.join("deployed");
        let state = temp.0.join("state");
        publish_to(
            &temp.0,
            &bin,
            &built,
            &state,
            &receipt(),
            &Services {
                app: None,
                companion: false,
                companion_bins: vec![
                    bin.with_file_name("hey-gh"),
                    bin.with_file_name("hey-harvester"),
                ],
                skills: vec![skill.clone()],
                harvester: None,
            },
            |_| panic!("No harvester schedule in this fixture"),
        )
        .unwrap();
        assert_eq!(
            json::<Receipt>(&state.join("upgrade-receipt.json")).unwrap(),
            receipt()
        );
        assert_eq!(installed_id(&bin).as_deref(), Some("1234567890abcdef"));
        assert_eq!(
            fs::read_link(bin.with_file_name("hb")).unwrap(),
            PathBuf::from("hey-boss")
        );
        for (relative, _) in hey_boss::skill::FILES {
            assert_eq!(
                fs::read_to_string(skill.join(relative)).unwrap(),
                format!("staged {relative}")
            );
        }
        assert_eq!(
            fs::read_to_string(skill.join("references/new-workflow.md")).unwrap(),
            "New in this release"
        );
        assert_eq!(
            fs::read_to_string(bin.with_file_name("hey-harvester")).unwrap(),
            "#!/bin/sh\necho new-hey-harvester\n"
        );
        assert_eq!(
            fs::read_to_string(bin.with_file_name("hey-gh")).unwrap(),
            "#!/bin/sh\necho new-hey-gh\n"
        );
    }
}
