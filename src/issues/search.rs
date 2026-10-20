//! Rebuildable machine-local search. Derived data never enters the fleet journal.
mod embed;
mod index;
mod rank;
mod worker;
pub(crate) use index::retrieve;
pub(crate) use worker::{install, start};
#[cfg(test)]
mod tests;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub query: String,
    #[serde(default = "active")]
    pub state: String,
    #[serde(default)]
    pub mine: bool,
    #[serde(default)]
    pub unassigned: bool,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default = "limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub compact: bool,
    #[serde(default = "yes")]
    pub semantic: bool,
}
fn active() -> String {
    "active".into()
}
fn limit() -> u32 {
    50
}
fn yes() -> bool {
    true
}
impl Options {
    pub(crate) fn list(&self, literal: bool) -> super::Operation {
        super::Operation::List {
            compact: self.compact,
            state: self.state.clone(),
            mine: self.mine,
            unassigned: self.unassigned,
            assignee: self.assignee.clone(),
            labels: self.labels.clone(),
            search: literal.then(|| self.query.clone()),
            limit: self.limit,
            offset: self.offset,
            all: self.all,
        }
    }
}

/// Disabled until explicitly enabled on this machine. Missing or unreadable
/// settings never authorize a provider request.
pub(crate) fn enabled(source: &std::path::Path) -> bool {
    use rusqlite::OptionalExtension;
    index::open(source, false)
        .ok()
        .and_then(|db| {
            db.query_row("SELECT value FROM meta WHERE key='enabled'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .ok()
            .flatten()
        })
        .as_deref()
        == Some("1")
}
pub(crate) fn settings(
    source: &std::path::Path,
    set: Option<bool>,
) -> super::Result<serde_json::Value> {
    if let Some(value) = set {
        let db = index::open(source, true)?;
        db.execute(
            "INSERT OR REPLACE INTO meta VALUES('enabled',?1)",
            [if value { "1" } else { "0" }],
        )?;
    }
    let enabled = enabled(source);
    let mut result = serde_json::json!({"ok":true,"scope":"device","search_index":{"enabled":enabled,"model":index::MODEL,"dimensions":index::DIMENSIONS}});
    if let Ok(db) = index::open(source, false) {
        result["search_index"]["passages"] =
            serde_json::json!(
                db.query_row("SELECT count(*) FROM passages", [], |r| r.get::<_, i64>(0))?
            );
        result["search_index"]["pending"]=serde_json::json!(db.query_row("SELECT count(*) FROM passages p WHERE NOT EXISTS(SELECT 1 FROM embeddings e WHERE e.hash=p.hash)",[],|r|r.get::<_,i64>(0))?);
        result["search_index"]["backfill_complete"]=serde_json::json!(db.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key='backfill_done') AND NOT EXISTS(SELECT 1 FROM documents WHERE complete=0)",[],|r|r.get::<_,bool>(0))?);
    }
    Ok(result)
}
