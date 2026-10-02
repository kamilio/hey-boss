//! Provider-neutral public conversation records, separate from private RPC traffic.
use crate::{
    agent_runtime::{Event, SessionRef},
    issues::{Error, Result},
};
use serde_json::json;
use std::{
    fs::{DirBuilder, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub(crate) fn location(database: &Path, provider: &str, session: &str) -> Result<PathBuf> {
    if !matches!(provider, "claude" | "pi")
        || session.is_empty()
        || session.len() > 128
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Error::invalid("Invalid agent transcript identity"));
    }
    Ok(database
        .with_added_extension("agent-sessions")
        .join(format!("{provider}-{session}.jsonl")))
}
pub(crate) struct Transcript {
    file: File,
    provider: &'static str,
}
impl Transcript {
    pub(crate) fn open(database: &Path, reference: &SessionRef) -> Result<Self> {
        let path = location(database, reference.provider.name(), &reference.id)?;
        let directory = path.parent().unwrap();
        match DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        if !std::fs::symlink_metadata(directory)?.is_dir() {
            return Err(Error::invalid(
                "Agent transcript directory is not a directory",
            ));
        }
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(Error::invalid("Agent transcript is not a file"));
        }
        Ok(Self {
            file,
            provider: reference.provider.name(),
        })
    }
    pub(crate) fn append(&mut self, event: &Event) -> Result<()> {
        // Callers only persist public messages and tool activity. Never store
        // raw provider controls, approval payloads or private thinking events.
        if !matches!(
            event,
            Event::Message { .. } | Event::ToolStarted { .. } | Event::ToolCompleted { .. }
        ) {
            return Ok(());
        }
        let mut bytes = serde_json::to_vec(
            &json!({"type":"agent_event","provider":self.provider,"event":event}),
        )?;
        bytes.push(b'\n');
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(Error::invalid("Agent transcript entry exceeds 8 MiB"));
        }
        self.file.write_all(&bytes)?;
        Ok(())
    }
}
