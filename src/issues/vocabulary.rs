//! Per-word fleet settings. Explicit edits always override imported defaults.
use super::{Error, Result};
use crate::database::Connection;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use unicode_normalization::UnicodeNormalization;

pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS dictation_vocabulary(
    source INTEGER NOT NULL CHECK(source IN (0,1)),
    term TEXT NOT NULL,
    spelling TEXT NOT NULL,
    active INTEGER NOT NULL CHECK(active IN (0,1)),
    PRIMARY KEY(source,term)
);";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Show,
    Initialize { words: Vec<String> },
    Add { words: Vec<String> },
    Remove { words: Vec<String> },
}

fn term(value: &str) -> Result<String> {
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, '/' | ':'))
    {
        return Err(Error::invalid(
            "Vocabulary terms cannot contain control characters, slashes, or colons",
        ));
    }
    let word: String = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .nfc()
        .collect();
    if word.is_empty() || word.len() > 256 {
        return Err(Error::invalid(
            "Use a vocabulary word or phrase of 1–256 bytes",
        ));
    }
    Ok(word)
}

fn words(db: &Connection) -> Result<Vec<String>> {
    let mut entries = BTreeMap::new();
    // Source 0 is a one-time import; source 1 contains the user's later edits.
    // A late/offline import cannot revive an explicit removal on another Mac.
    let mut statement = db.prepare(
        "SELECT term,spelling,active FROM dictation_vocabulary WHERE term<>'' ORDER BY source,term",
    )?;
    for row in statement.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, bool>(2)?,
        ))
    })? {
        let (key, word, active) = row?;
        if active {
            entries.insert(key, word);
        } else {
            entries.remove(&key);
        }
    }
    let mut words: Vec<_> = entries.into_values().collect();
    words.sort();
    Ok(words)
}

pub(crate) fn execute(db: &Connection, operation: &Operation) -> Result<Value> {
    let initialized: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM dictation_vocabulary WHERE source=0 AND term='')",
        [],
        |r| r.get(0),
    )?;
    let (input, source, active) = match operation {
        Operation::Show => (None, 0, false),
        Operation::Initialize { words } if !initialized => (Some(words), 0, true),
        Operation::Initialize { .. } => (None, 0, false),
        Operation::Add { words } => (Some(words), 1, true),
        Operation::Remove { words } => (Some(words), 1, false),
    };
    if let Some(input) = input {
        let previous_count = words(db)?.len();
        if input.len() > 256 || (source == 1 && input.is_empty()) {
            return Err(Error::invalid("Use 1–256 vocabulary words or phrases"));
        }
        let normalized = input
            .iter()
            .map(|word| term(word))
            .collect::<Result<Vec<_>>>()?;
        for word in normalized {
            db.execute("INSERT INTO dictation_vocabulary(source,term,spelling,active) VALUES(?1,?2,?3,?4)
                ON CONFLICT(source,term) DO UPDATE SET spelling=excluded.spelling,active=excluded.active
                WHERE dictation_vocabulary.spelling IS NOT excluded.spelling OR dictation_vocabulary.active IS NOT excluded.active",
                params![source,word.to_lowercase(),word,active])?;
        }
        if source == 0 {
            db.execute(
                "INSERT OR IGNORE INTO dictation_vocabulary VALUES(0,'','',0)",
                [],
            )?;
        }
        let count = words(db)?.len();
        if source == 1 && active && count > 256 && count > previous_count {
            return Err(Error::invalid("Keep vocabulary to 256 words or phrases"));
        }
    }
    let mut vocabulary = words(db)?;
    let total = vocabulary.len();
    // Independent offline additions can exceed the limit when they merge.
    // Keep all entries in storage and use a deterministic bounded inference list.
    vocabulary.truncate(256);
    Ok(
        json!({"ok":true,"scope":"global","vocabulary":vocabulary,"total":total,
        "initialized":initialized || matches!(operation,Operation::Initialize { .. })}),
    )
}
