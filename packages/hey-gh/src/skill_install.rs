use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

const SKILL: &str = include_str!("../skills/hey-gh/SKILL.md");

pub fn install(mut roots: Vec<PathBuf>) -> io::Result<Vec<PathBuf>> {
    if roots.is_empty() {
        let home = dirs::home_dir()
            .ok_or_else(|| io::Error::other("cannot determine home directory; use --skills-dir"))?;
        let codex_home = std::env::var_os("CODEX_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        roots = vec![codex_home.join("skills"), home.join(".agents/skills")];
    }
    if roots.iter().any(|root| root.as_os_str().is_empty()) {
        return Err(io::Error::other("--skills-dir must not be empty"));
    }
    let mut installed = Vec::new();
    for root in roots {
        let directory = root.join("hey-gh");
        fs::create_dir_all(&directory)?;
        let target = directory.join("SKILL.md");
        if installed.contains(&target) {
            continue;
        }
        write_atomic(&target)?;
        installed.push(target);
    }
    Ok(installed)
}

fn write_atomic(target: &Path) -> io::Result<()> {
    // Stage beside the destination so replacement never exposes a partial card.
    let mut name = OsString::from(".SKILL.md-");
    name.push(format!("{:016x}.tmp", fastrand::u64(..)));
    let temporary = target.with_file_name(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(SKILL.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
