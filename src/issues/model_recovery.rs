//! Bounded, device-local repair of display metadata; never part of a list read.
use crate::agent_conversations::{model_name, recovered_model, valid_session};
use crate::database::Connection;
use serde_json::{Value, json};
use std::{collections::HashMap, fs::ReadDir, path::Path};

#[derive(Default)]
pub(crate) struct Recovery {
    cursor: String,
    pending: HashMap<String, Vec<(String, String)>>,
    directories: Vec<ReadDir>,
    last_page: bool,
    scanning: bool,
}

impl Recovery {
    /// At most 128 agent rows, 256 directory entries and four 8 MiB tails per
    /// step. No Codex database, subprocess, request-path work or write-held I/O.
    /// Returns true after one pass; the daemon retries unavailable evidence later.
    pub(crate) fn step(
        &mut self,
        db: &Connection,
        machine: &str,
        home: &Path,
    ) -> super::Result<bool> {
        if !self.scanning {
            let rows = db.query_collect(
                "SELECT id,substr(metadata,1,65536) FROM agents WHERE id>?1 ORDER BY id LIMIT 128",
                [&self.cursor],
                |r| -> rusqlite::Result<_> { Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)) },
            )?;
            self.last_page = rows.len() < 128;
            for (id, raw) in rows {
                self.cursor = id.clone();
                let Ok(metadata) = serde_json::from_str::<Value>(&raw) else {
                    continue;
                };
                if metadata["machine"] != machine
                    || metadata["kind"] != "codex"
                    || metadata["model"].as_str().and_then(model_name).is_some()
                {
                    continue;
                }
                let Some(session) = metadata["session_id"].as_str().filter(|s| valid_session(s))
                else {
                    continue;
                };
                if id
                    .strip_prefix("codex:")
                    .filter(|s| valid_session(s))
                    .is_some_and(|s| s != session)
                {
                    continue;
                }
                self.pending
                    .entry(session.into())
                    .or_default()
                    .push((id, raw));
            }
            // Accumulate a bounded batch so thousands of historical agents do
            // not each rescan the session tree. Directory traversal is streamed.
            if !self.last_page && self.pending.values().map(Vec::len).sum::<usize>() < 1024 {
                return Ok(false);
            }
            self.scanning = true;
            for name in ["archived_sessions", "sessions"] {
                if let Ok(directory) = std::fs::read_dir(home.join(name)) {
                    self.directories.push(directory);
                }
            }
        }
        let mut reads = 0;
        for _ in 0..256 {
            let Some(directory) = self.directories.last_mut() else {
                break;
            };
            let Some(entry) = directory.next() else {
                self.directories.pop();
                continue;
            };
            let Ok(entry) = entry else { continue };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            // Never follow symlinks; cap nesting and simultaneously open handles.
            if kind.is_dir() && self.directories.len() < 8 {
                if let Ok(directory) = std::fs::read_dir(entry.path()) {
                    self.directories.push(directory);
                }
            } else if kind.is_file() {
                let name = entry.file_name();
                let Some(name) = name.to_str().and_then(|n| n.strip_suffix(".jsonl")) else {
                    continue;
                };
                let Some(session) = name.get(name.len().saturating_sub(36)..) else {
                    continue;
                };
                let Some(actors) = self.pending.get(session) else {
                    continue;
                };
                reads += 1;
                if let Some(model) = recovered_model(&entry.path(), session) {
                    for (id, original) in actors {
                        // A concurrent live capture wins. Modify display metadata
                        // only; keep last_seen, saved identities and all history.
                        db.execute("UPDATE agents SET metadata=json_set(metadata,'$.model',?1) WHERE id=?2 AND metadata=?3",
                            rusqlite::params![model, id, original])?;
                    }
                    self.pending.remove(session);
                }
                if reads == 4 {
                    break;
                }
            }
        }
        if self.pending.is_empty() {
            self.directories.clear();
        }
        if self.directories.is_empty() {
            self.pending.clear();
            self.scanning = false;
        }
        Ok(self.directories.is_empty() && self.last_page)
    }
}

/// A delayed recovery journal entry is a fill-only patch, never a replacement
/// for newer agent identity/activity metadata captured by the supervisor.
pub(crate) fn merge_recovered(
    old: &Value,
    before: &Value,
    after: &Value,
    node: &str,
) -> super::Result<Value> {
    // SQLite JSON subtypes can make trigger payloads embed metadata as an
    // object, while snapshots and ordinary row reads carry its encoded text.
    let parse = |row: &Value| -> super::Result<Value> {
        Ok(match &row["metadata"] {
            Value::String(raw) => serde_json::from_str(raw)?,
            Value::Object(_) => row["metadata"].clone(),
            _ => json!({}),
        })
    };
    let incoming = parse(after)?;
    let previous = parse(before)?;
    let mut incoming_identity = incoming.clone();
    let mut previous_identity = previous.clone();
    for metadata in [&mut incoming_identity, &mut previous_identity] {
        if let Some(fields) = metadata.as_object_mut() {
            fields.remove("model");
        }
    }
    // Recovery changes only a missing model, with no new activity timestamp.
    // Recognize that patch from the journal itself; Actor metadata stays within
    // the existing wire schema so old peers and trace links keep working.
    let recovery = !before.is_null()
        && before["last_seen"] == after["last_seen"]
        && incoming_identity == previous_identity
        && previous["model"].as_str().and_then(model_name).is_none()
        && incoming["model"].as_str().and_then(model_name).is_some();
    if !recovery {
        let known = parse(old)?;
        let mut merged = after.clone();
        if incoming["model"].as_str().and_then(model_name).is_none()
            && known["model"].as_str().and_then(model_name).is_some()
        {
            let mut metadata = incoming;
            metadata["model"] = known["model"].clone();
            merged["metadata"] = json!(metadata.to_string());
        }
        return Ok(merged);
    }
    let session = incoming["session_id"].as_str().filter(|s| valid_session(s));
    if session.is_none()
        || incoming["machine"] != node
        || incoming["model"].as_str().and_then(model_name).is_none()
    {
        return Err(super::Error::invalid("Invalid recovered model identity"));
    }
    if old.is_null() {
        return Ok(after.clone());
    }
    let mut metadata = parse(old)?;
    if metadata["machine"] != node || metadata["session_id"].as_str() != session {
        return Err(super::Error::invalid(
            "Recovered model no longer matches the saved agent",
        ));
    }
    if metadata["model"].as_str().and_then(model_name).is_none() {
        metadata["model"] = incoming["model"].clone();
    }
    let mut merged = old.clone();
    merged["metadata"] = json!(metadata.to_string());
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SESSION: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    #[test]
    fn recovery_merge_rejects_foreign_evidence_and_preserves_newer_capture() {
        let row = |model: Value| json!({"id":"codex:alias","last_seen":1,"metadata":json!({"machine":"local","session_id":SESSION,"model":model}).to_string()});
        let before = row(Value::Null);
        let recovered = row(json!("recovered"));
        assert!(merge_recovered(&before, &before, &recovered, "remote").is_err());
        let known = row(json!("known"));
        assert_eq!(
            merge_recovered(&known, &before, &recovered, "local").unwrap(),
            known
        );
        let incoming = merge_recovered(&known, &before, &before, "local").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(incoming["metadata"].as_str().unwrap()).unwrap()["model"],
            "known"
        );
        let mut other_session = before.clone();
        other_session["metadata"] = json!(
            json!({"machine":"local","session_id":"ffffffff-bbbb-cccc-dddd-eeeeeeeeeeee"})
                .to_string()
        );
        assert!(merge_recovered(&other_session, &before, &recovered, "local").is_err());
    }

    #[test]
    fn recovery_pages_past_unverifiable_agents_and_bounds_directory_work() {
        let root =
            std::env::temp_dir().join(format!("model-recovery-bounds-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        for i in 0..600 {
            std::fs::write(root.join(format!("sessions/unrelated-{i}")), "").unwrap();
        }
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE agents(id TEXT PRIMARY KEY,metadata TEXT,last_seen INTEGER);
            WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<130) INSERT INTO agents SELECT printf('a%03d',x),'{}',0 FROM n;").unwrap();
        db.execute(
            "INSERT INTO agents VALUES('z',?1,0)",
            [json!({"kind":"codex","machine":"local","session_id":SESSION}).to_string()],
        )
        .unwrap();
        let mut recovery = Recovery::default();
        assert!(!recovery.step(&db, "local", &root).unwrap());
        assert_eq!(recovery.cursor, "a128");
        assert!(!recovery.step(&db, "local", &root).unwrap());
        assert!(!recovery.step(&db, "local", &root).unwrap());
        assert!(recovery.step(&db, "local", &root).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovers_exact_local_sessions_and_preserves_identity_history_and_known_metadata() {
        let root = std::env::temp_dir().join(format!("model-recovery-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sessions/2026/09/24")).unwrap();
        let path = root.join(format!("sessions/2026/09/24/rollout-{SESSION}.jsonl"));
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                json!({"type":"session_meta","payload":{"id":SESSION}}),
                json!({"type":"turn_context","payload":{"model":"gpt-6-astra"}})
            ),
        )
        .unwrap();
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE agents(id TEXT PRIMARY KEY,metadata TEXT,last_seen INTEGER);
          CREATE TABLE events(data TEXT); INSERT INTO events VALUES('{\"actor_model\":\"original-event\"}');
          CREATE TABLE issues(origin TEXT); INSERT INTO issues VALUES('{\"model\":\"original-creator\"}');").unwrap();
        let cases = [
            (format!("codex:{SESSION}"), SESSION, "local", None),
            ("codex:verified-custom".into(), SESSION, "local", None),
            (
                format!("codex:{SESSION}:unverified"),
                "invalid:alias",
                "local",
                None,
            ),
            ("codex:remote".into(), SESSION, "remote", None),
            (
                "codex:known".into(),
                SESSION,
                "local",
                Some("recorded-model"),
            ),
            (
                "codex:missing".into(),
                "ffffffff-bbbb-cccc-dddd-eeeeeeeeeeee",
                "local",
                None,
            ),
        ];
        for (id, session, machine, model) in &cases {
            db.execute("INSERT INTO agents VALUES(?1,?2,123)", rusqlite::params![id,json!({"id":id,"kind":"codex","machine":machine,"session_id":session,"model":model,"host":"saved-host","source":"--agent"}).to_string()]).unwrap();
        }
        let before = db
            .query_collect("SELECT metadata FROM agents ORDER BY id", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap();
        let mut recovery = Recovery::default();
        for _ in 0..100 {
            if recovery.step(&db, "local", &root).unwrap() {
                break;
            }
        }
        for (index, (id, _, _, model)) in cases.iter().enumerate() {
            let (metadata, seen): (String, i64) = db
                .query_row(
                    "SELECT metadata,last_seen FROM agents WHERE id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            let metadata: Value = serde_json::from_str(&metadata).unwrap();
            assert_eq!(
                metadata["model"],
                json!(if index < 2 {
                    Some("gpt-6-astra")
                } else {
                    *model
                }),
                "{id}"
            );
            assert_eq!(metadata["id"], *id);
            assert_eq!(metadata["session_id"], cases[index].1);
            assert_eq!(seen, 123);
        }
        assert_eq!(
            db.query_row("SELECT data FROM events", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "{\"actor_model\":\"original-event\"}"
        );
        assert_eq!(
            db.query_row("SELECT origin FROM issues", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "{\"model\":\"original-creator\"}"
        );
        let after = db
            .query_collect("SELECT metadata FROM agents ORDER BY id", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap();
        assert_eq!(before.iter().zip(&after).filter(|(a, b)| a != b).count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
