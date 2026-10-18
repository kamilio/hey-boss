//! Bounded database timing evidence. Never retain SQL values or result rows.
use super::wire::Command;
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
    sync::Mutex,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const LOG_LIMIT: u64 = 64 * 1024;

/// Keep evidence readable even when the service's journal needs root access.
/// One owner serializes records; diagnostics must never fail a database request.
#[derive(Default)]
pub(super) struct Log(Option<Mutex<PathBuf>>);

impl Log {
    pub fn new(path: PathBuf) -> Self {
        Self(Some(Mutex::new(path)))
    }

    pub fn record(&self, mut event: serde_json::Value) {
        event["at_ms"] = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64)
            .into();
        event["pid"] = std::process::id().into();
        eprintln!("{event}");
        let _ = self.append(&format!("{event}\n"));
    }

    fn append(&self, record: &str) -> std::io::Result<()> {
        let Some(path) = &self.0 else {
            return Ok(());
        };
        let path = path
            .lock()
            .map_err(|_| std::io::Error::other("Diagnostic log poisoned"))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&*path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::getuid() }
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || record.len() as u64 > LOG_LIMIT
        {
            return Err(std::io::Error::other("Unsafe diagnostic log"));
        }
        if metadata.len() + record.len() as u64 > LOG_LIMIT {
            file.set_len(0)?;
        }
        file.write_all(record.as_bytes())
    }
}

#[derive(Clone)]
pub(super) struct Operation {
    pub name: &'static str,
    pub sql: String,
}

impl Operation {
    pub fn new(command: &Command) -> Self {
        let (name, sql) = match command {
            Command::Hello => ("hello", ""),
            Command::CheckSchema => ("schema", ""),
            Command::ExclusiveSession => ("exclusive session", ""),
            Command::ReadTransaction => ("read transaction", ""),
            Command::Prepare { sql } => ("prepare", sql.as_str()),
            Command::Execute { sql, .. } => ("execute", sql.as_str()),
            Command::Query { sql, .. } => ("query", sql.as_str()),
            Command::Batch { sql } => ("batch", sql.as_str()),
            Command::Backup { .. } => ("backup", ""),
        };
        Self {
            name,
            sql: redact(sql),
        }
    }
}

fn redact(sql: &str) -> String {
    let mut output = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if output.len() >= 192 {
            output.push('…');
            break;
        }
        match c {
            '\'' | '"' | '`' | '[' => {
                let end = if c == '[' { ']' } else { c };
                while let Some(next) = chars.next() {
                    if next == end {
                        if chars.peek() == Some(&end) {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
                output.push('?');
            }
            '-' if chars.peek() == Some(&'-') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
                output.push(' ');
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
                output.push(' ');
            }
            '0'..='9' => {
                while chars
                    .peek()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
                {
                    chars.next();
                }
                output.push('?');
            }
            c if c.is_whitespace() || c.is_control() => {
                if !output.ends_with(' ') {
                    output.push(' ');
                }
            }
            c => output.push(c),
        }
    }
    output
}

pub(super) struct Activity {
    pub session: usize,
    pub acquired: Instant,
    pub started: Instant,
    pub operation: Operation,
    pub executing: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_file_is_bounded_private_and_rejects_links() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = std::env::temp_dir().join(format!(
            "hb-diagnostics-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("evidence.jsonl");
        let log = Log::new(path.clone());
        let record = format!("{}\n", "x".repeat(1023));
        for _ in 0..80 {
            log.append(&record).unwrap();
        }
        let metadata = std::fs::metadata(&path).unwrap();
        assert!(metadata.len() <= LOG_LIMIT);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert!(log.append(&"x".repeat(LOG_LIMIT as usize + 1)).is_err());
        std::fs::remove_file(&path).unwrap();
        let target = root.join("untouched");
        std::fs::write(&target, "original").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &path).unwrap();
        assert!(log.append("unsafe").is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::hard_link(&target, &path).unwrap();
        assert!(log.append("unsafe").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diagnostics_exclude_values_comments_paths_and_control_characters() {
        let operation = Operation::new(&Command::Query {
            sql: "SELECT id FROM work WHERE body='private''text' AND n=1234 /* private comment */ -- private line\nAND name=\"private identifier\" AND x=`private name` AND y=[private bracket]".into(),
            values: vec![super::super::wire::SqlValue::Text("private binding".into())],
        });
        assert_eq!(operation.name, "query");
        assert!(!operation.sql.contains("private"));
        assert!(!operation.sql.contains("1234"));
        assert!(!operation.sql.contains('\n'));
        assert!(operation.sql.contains("SELECT id FROM work WHERE body=?"));
        assert!(
            Operation::new(&Command::Backup {
                path: "/private/path".into()
            })
            .sql
            .is_empty()
        );
        assert!(redact(&"SELECT data FROM work ".repeat(100)).len() <= 199);
    }
}
