//! Installation-time permissions. Never rewrite Codex's config.toml or default.rules.
use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const HEADER: &str = "# Managed by hey-boss configure-agents.\n";
const RULE_START: &str = "prefix_rule(pattern = [";
const RULE_END: &str = "], decision = \"allow\")";
const LIMIT: u64 = 4 * 1024 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

// Borrow raw values so unrelated settings, formatting and numeric precision remain
// byte-for-byte intact. Reject duplicate keys in the objects we actually modify.
struct Object<'a>(BTreeMap<String, &'a RawValue>);
impl<'de> Deserialize<'de> for Object<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = Object<'de>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an object with unique keys")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut result = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, &'de RawValue>()? {
                    if result.insert(key.clone(), value).is_some() {
                        return Err(serde::de::Error::custom(format!("duplicate key: {key}")));
                    }
                }
                Ok(Object(result))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

fn set_property(source: &str, object: &Object<'_>, key: &str, value: &str) -> String {
    let mut result = source.to_owned();
    if let Some(old) = object.0.get(key) {
        let start = old.get().as_ptr() as usize - source.as_ptr() as usize;
        result.replace_range(start..start + old.get().len(), value);
    } else {
        let start = source.find('{').unwrap() + 1;
        let comma = if object.0.is_empty() { "" } else { "," };
        result.insert_str(start, &format!("\n  {key:?}: {value}{comma}\n"));
    }
    result
}

fn claude_settings(source: &str, commands: &[String]) -> io::Result<String> {
    let root: Object<'_> = serde_json::from_str(source)?;
    let permissions = root.0.get("permissions").map_or("{}", |raw| raw.get());
    let object: Object<'_> = serde_json::from_str(permissions)?;
    for key in ["deny", "ask"] {
        if let Some(raw) = object.0.get(key) {
            serde_json::from_str::<Vec<String>>(raw.get())?;
        }
    }
    let allow = object.0.get("allow").map_or("[]", |raw| raw.get());
    let existing: Vec<String> = serde_json::from_str(allow)?;
    let mut additions = Vec::new();
    for command in commands {
        let rule = format!("Bash({command} *)");
        if !existing.contains(&rule) && !existing.contains(&format!("Bash({command}:*)")) {
            additions.push(serde_json::to_string(&rule)?);
        }
    }
    if additions.is_empty() {
        return Ok(source.to_owned());
    }
    let mut updated = allow.to_owned();
    let comma = if existing.is_empty() { "" } else { "," };
    updated.insert_str(
        allow.rfind(']').unwrap(),
        &format!("{comma}{}", additions.join(", ")),
    );
    let permissions = set_property(permissions, &object, "allow", &updated);
    let result = set_property(source, &root, "permissions", &permissions);
    // Validate the final document before any file is changed.
    serde_json::from_str::<&RawValue>(&result)?;
    Ok(result)
}

fn codex_rules(source: Option<&str>, commands: &[String]) -> io::Result<String> {
    if let Some(source) = source {
        let body = source.strip_prefix(HEADER).ok_or_else(|| {
            invalid("hey-boss.rules already exists and is not managed by hey-boss; move it aside before retrying")
        })?;
        for line in body.lines() {
            let encoded = line
                .strip_prefix(RULE_START)
                .and_then(|s| s.strip_suffix(RULE_END))
                .ok_or_else(|| {
                    invalid("hey-boss.rules has custom edits; move it aside before retrying")
                })?;
            let command: String = serde_json::from_str(encoded)?;
            validate_command(&command)?;
        }
    }
    let mut result = HEADER.to_owned();
    for command in commands {
        result.push_str(&format!(
            "{RULE_START}{}{RULE_END}\n",
            serde_json::to_string(command)?
        ));
    }
    Ok(result)
}

fn validate_command(command: &str) -> io::Result<()> {
    let path = Path::new(command);
    if command != "hey-boss"
        && (!path.is_absolute() || path.file_name().is_none_or(|name| name != "hey-boss"))
    {
        return Err(invalid(
            "--binary must be an absolute path ending in hey-boss",
        ));
    }
    if command.chars().any(|c| c.is_control() || c == '*') {
        return Err(invalid(
            "executable path cannot contain control characters or wildcard '*'",
        ));
    }
    Ok(())
}

fn config_home(variable: &str, fallback: &str) -> io::Result<PathBuf> {
    if let Some(value) = std::env::var_os(variable).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err(invalid(format!("{variable} must be an absolute path")));
        }
        return Ok(path);
    }
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| invalid("HOME is missing"))?;
    let path = PathBuf::from(home).join(fallback);
    if !path.is_absolute() {
        return Err(invalid("HOME must be an absolute path"));
    }
    Ok(path)
}

struct Snapshot {
    bytes: Vec<u8>,
    metadata: fs::Metadata,
}

fn snapshot(path: &Path) -> io::Result<Option<Snapshot>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(invalid("config must be a regular file with no hard links"));
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(invalid("config exceeds 4 MiB; left unchanged"));
    }
    Ok(Some(Snapshot { bytes, metadata }))
}

struct ConfigFile {
    requested: PathBuf,
    path: PathBuf,
    original: Option<Snapshot>,
    _lock: File,
}

impl ConfigFile {
    fn open(requested: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(requested.parent().unwrap())?;
        // Follow an existing dotfiles symlink without replacing the link itself.
        let path = match fs::symlink_metadata(&requested) {
            Ok(_) => requested.canonicalize()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => requested
                .parent()
                .unwrap()
                .canonicalize()?
                .join(requested.file_name().unwrap()),
            Err(error) => return Err(error),
        };
        let lock_path = path.with_file_name(format!(
            ".{}.hey-boss.lock",
            path.file_name().unwrap().to_string_lossy()
        ));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(lock_path)?;
        if !lock.metadata()?.is_file() || lock.metadata()?.nlink() != 1 {
            return Err(invalid(
                "config lock must be a regular file with no hard links",
            ));
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = snapshot(&path)?;
        Ok(Self {
            requested,
            path,
            original,
            _lock: lock,
        })
    }

    fn text(&self) -> io::Result<Option<&str>> {
        self.original
            .as_ref()
            .map(|s| std::str::from_utf8(&s.bytes).map_err(|_| invalid("config is not UTF-8")))
            .transpose()
    }

    fn unchanged(&self) -> io::Result<()> {
        let resolved = if fs::symlink_metadata(&self.requested).is_ok() {
            self.requested.canonicalize()?
        } else {
            self.requested
                .parent()
                .unwrap()
                .canonicalize()?
                .join(self.requested.file_name().unwrap())
        };
        let current = snapshot(&self.path)?;
        let same = match (&self.original, current) {
            (None, None) => true,
            (Some(old), Some(new)) => {
                old.bytes == new.bytes
                    && old.metadata.ino() == new.metadata.ino()
                    && old.metadata.dev() == new.metadata.dev()
                    && old.metadata.mode() == new.metadata.mode()
                    && old.metadata.mtime() == new.metadata.mtime()
                    && old.metadata.mtime_nsec() == new.metadata.mtime_nsec()
            }
            _ => false,
        };
        if resolved != self.path || !same {
            return Err(invalid(
                "config changed during installation; retry to merge the latest version",
            ));
        }
        Ok(())
    }

    fn write(&self, content: &str) -> io::Result<()> {
        if self.text()? == Some(content) {
            return Ok(());
        }
        self.unchanged()?;
        let (temporary, mut file) = unique_file(&self.path, "tmp")?;
        let result = (|| {
            file.write_all(content.as_bytes())?;
            let mode = self
                .original
                .as_ref()
                .map_or(0o600, |s| s.metadata.mode() & 0o777);
            file.set_permissions(fs::Permissions::from_mode(mode))?;
            file.sync_all()?;
            if let Some(original) = &self.original {
                let (backup, mut file) = unique_file(&self.path, "bak")?;
                file.write_all(&original.bytes)?;
                file.sync_all()?;
                eprintln!("Backup: {}", backup.display());
            }
            self.unchanged()?;
            fs::rename(&temporary, &self.path)?;
            File::open(self.path.parent().unwrap())?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

fn unique_file(path: &Path, suffix: &str) -> io::Result<(PathBuf, File)> {
    loop {
        let name = format!(
            "{}.hey-boss-{}-{}.{}",
            path.file_name().unwrap().to_string_lossy(),
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed),
            suffix
        );
        let path = path.with_file_name(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

/// Homebrew stages this manifest in its sandbox; user config writes happen only
/// when the installed CLI runs in the user's normal environment.
pub fn configure_pending(executable: &Path) -> io::Result<bool> {
    let path = executable.with_file_name("hey-boss.agent-permissions.json");
    if !path.exists() {
        return Ok(false);
    }
    let pending = ConfigFile::open(path.clone())?;
    // Another first-run process may have consumed the manifest while we waited.
    let Some(source) = pending.text()? else {
        return Ok(false);
    };
    let binaries: Vec<PathBuf> = serde_json::from_str(source)?;
    if binaries.is_empty() {
        return Err(invalid(format!(
            "{}: no installed executable paths",
            path.display()
        )));
    }
    configure(&binaries)?;
    pending.unchanged()?;
    fs::remove_file(path)?;
    Ok(true)
}

pub fn configure(binaries: &[PathBuf]) -> io::Result<()> {
    let paths = if binaries.is_empty() {
        vec![std::env::current_exe()?]
    } else {
        binaries.to_vec()
    };
    let mut commands = vec!["hey-boss".to_owned()];
    for path in paths {
        let command = path
            .to_str()
            .ok_or_else(|| invalid("executable path is not UTF-8"))?;
        validate_command(command)?;
        commands.push(command.to_owned());
    }
    commands.sort();
    commands.dedup();
    let codex_path = config_home("CODEX_HOME", ".codex")?.join("rules/hey-boss.rules");
    let claude_path = config_home("CLAUDE_CONFIG_DIR", ".claude")?.join("settings.json");
    // Validate both configurations before writing either one. Take locks in a
    // consistent order; other editors are also checked immediately before rename.
    let contextual = |path: &Path, error: io::Error| {
        io::Error::new(error.kind(), format!("{}: {error}", path.display()))
    };
    let codex = ConfigFile::open(codex_path.clone()).map_err(|e| contextual(&codex_path, e))?;
    // Avoid locking the same inode twice if the two paths alias one another.
    if claude_path.canonicalize().ok().as_ref() == Some(&codex.path) {
        return Err(invalid("Codex and Claude configs resolve to the same file"));
    }
    let claude = ConfigFile::open(claude_path.clone()).map_err(|e| contextual(&claude_path, e))?;
    let source = codex.text().map_err(|e| contextual(&codex_path, e))?;
    let rules = codex_rules(source, &commands).map_err(|e| contextual(&codex_path, e))?;
    // Also cover quoted absolute invocations when the install directory has spaces
    // or shell punctuation. Never use wildcard paths or allow a shell/interpreter.
    let mut claude_commands = commands.clone();
    for command in &commands {
        if command
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && !"/_-.".contains(c))
        {
            claude_commands.push(format!("'{}'", command.replace('\'', "'\\''")));
            claude_commands.push(format!(
                "\"{}\"",
                command
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('$', "\\$")
                    .replace('`', "\\`")
            ));
        }
    }
    let source = claude.text().map_err(|e| contextual(&claude_path, e))?;
    let settings = claude_settings(source.unwrap_or("{}\n"), &claude_commands)
        .map_err(|e| contextual(&claude_path, e))?;
    codex.unchanged().map_err(|e| contextual(&codex_path, e))?;
    claude
        .unchanged()
        .map_err(|e| contextual(&claude_path, e))?;
    codex
        .write(&rules)
        .map_err(|e| contextual(&codex_path, e))?;
    claude
        .write(&settings)
        .map_err(|e| contextual(&claude_path, e))?;
    eprintln!(
        "Global hey-boss permissions configured:\n  {}\n  {}\nRestart Codex to load the rules. Existing deny/ask rules and managed policies still apply.",
        codex_path.display(),
        claude_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_editor_changes_are_not_overwritten() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("out")
            .join(format!("config-editor-race-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        fs::write(&path, "{}").unwrap();
        let config = ConfigFile::open(path.clone()).unwrap();
        fs::write(&path, "{\"theme\":\"dark\"}").unwrap();
        assert!(
            config
                .write("{\"permissions\":{}}")
                .unwrap_err()
                .to_string()
                .contains("changed during")
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "{\"theme\":\"dark\"}");
        assert!(!fs::read_dir(&root).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "bak")
        }));
        fs::remove_dir_all(root).unwrap();
    }
}
