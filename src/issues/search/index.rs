use super::{
    embed,
    rank::{self, Field},
};
use crate::issues::{Error, Result, Store};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) const MODEL: &str = "text-embedding-3-small";
pub(super) const DIMENSIONS: usize = 512;
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS documents(project TEXT NOT NULL,number INTEGER NOT NULL,generation TEXT NOT NULL,complete INTEGER NOT NULL DEFAULT 0,after_comment INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(project,number));
CREATE TABLE IF NOT EXISTS passages(id INTEGER PRIMARY KEY,project TEXT NOT NULL,number INTEGER NOT NULL,field TEXT NOT NULL,source INTEGER NOT NULL,part INTEGER NOT NULL,text TEXT NOT NULL,hash TEXT NOT NULL,UNIQUE(project,number,field,source,part));
CREATE INDEX IF NOT EXISTS passage_scope ON passages(project,number);
CREATE INDEX IF NOT EXISTS passage_hash ON passages(hash);
CREATE TABLE IF NOT EXISTS embeddings(hash TEXT PRIMARY KEY,vector BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS queries(query TEXT PRIMARY KEY,vector BLOB NOT NULL,used INTEGER NOT NULL);
CREATE VIRTUAL TABLE IF NOT EXISTS words USING fts5(text,content='passages',content_rowid='id',tokenize='unicode61');
CREATE TRIGGER IF NOT EXISTS passages_insert AFTER INSERT ON passages BEGIN INSERT INTO words(rowid,text) VALUES(NEW.id,NEW.text); END;
CREATE TRIGGER IF NOT EXISTS passages_delete AFTER DELETE ON passages BEGIN INSERT INTO words(words,rowid,text) VALUES('delete',OLD.id,OLD.text); END;
";

pub(super) fn path(source: &Path) -> PathBuf {
    let mut p = source.as_os_str().to_owned();
    p.push(".search.db");
    p.into()
}
pub(super) fn open(source: &Path, write: bool) -> Result<Connection> {
    let path = path(source);
    crate::database::protect_database_paths(source, [&path])?;
    if write {
        Store::create_database_if_missing(&path)?;
    }
    let path = Store::validate_database_path(&path)?;
    let db = Connection::open_with_flags(
        path,
        (if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        }) | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(Duration::from_millis(40))?;
    if write {
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.pragma_update(None, "cache_size", -4096)?;
        db.execute_batch(SCHEMA)?;
        let identity = format!("v1:{MODEL}:{DIMENSIONS}:bytes1800-overlap160");
        let previous: Option<String> = db
            .query_row("SELECT value FROM meta WHERE key='identity'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if previous.as_ref().is_some_and(|p| p != &identity) {
            return Err(Error::new(
                "search_version",
                "Search cache belongs to a different model; rebuild it with the matching version",
            ));
        }
        db.execute(
            "INSERT OR IGNORE INTO meta VALUES('identity',?1)",
            [identity],
        )?;
    }
    Ok(db)
}
pub(super) fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
pub(super) fn bytes(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|n| n.to_le_bytes()).collect()
}
pub(super) fn vector(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() != DIMENSIONS * 4 {
        return None;
    }
    let v: Vec<_> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    if v.iter().all(|n| n.is_finite()) {
        Some(v)
    } else {
        None
    }
}
pub(super) fn put(
    db: &Connection,
    project: &str,
    number: i64,
    field: Field,
    source: i64,
    text: &str,
) -> Result<()> {
    let parts = rank::chunks(text);
    db.execute(
        "DELETE FROM passages WHERE project=?1 AND number=?2 AND field=?3 AND source=?4",
        params![project, number, field.name(), source],
    )?;
    let mut insert=db.prepare_cached("INSERT INTO passages(project,number,field,source,part,text,hash) VALUES(?1,?2,?3,?4,?5,?6,?7)")?;
    for (part, text) in parts.iter().enumerate() {
        insert.execute(params![
            project,
            number,
            field.name(),
            source,
            part as i64,
            text,
            hash(text)
        ])?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct Match {
    pub number: i64,
    pub score: f32,
    pub field: String,
    pub text: String,
    pub source: i64,
    pub semantic: bool,
}
#[derive(Default)]
struct Scores {
    lexical: [f32; 3],
    semantic: [f32; 3],
    best: Option<Match>,
}
fn record(
    scores: &mut BTreeMap<i64, Scores>,
    number: i64,
    field: Field,
    source: i64,
    text: &str,
    query: &str,
    similarity: Option<f32>,
) {
    let entry = scores.entry(number).or_default();
    let strength = if let Some(similarity) = similarity {
        if similarity < 0.3 {
            return;
        }
        ((similarity - 0.3) / 0.7).clamp(0.0, 1.0) * 0.65
    } else {
        rank::lexical(text, query)
    };
    if strength <= 0.0 {
        return;
    }
    let bucket = if similarity.is_some() {
        &mut entry.semantic
    } else {
        &mut entry.lexical
    };
    bucket[field.index()] = bucket[field.index()].max(strength);
    let contribution = strength * field.weight() * if similarity.is_some() { 0.35 } else { 1.0 };
    if entry
        .best
        .as_ref()
        .is_none_or(|best| contribution > best.score)
    {
        entry.best = Some(Match {
            number,
            score: contribution,
            field: field.name().into(),
            text: rank::excerpt(text, query),
            source,
            semantic: similarity.is_some(),
        });
    }
}

/// Eligibility is supplied from a fresh canonical read, never from cached metadata.
pub(crate) fn retrieve(
    source: &Path,
    project: &str,
    query: &str,
    semantic: bool,
    eligible: &HashMap<i64, String>,
    dirty: &HashSet<i64>,
) -> Result<(Vec<Match>, Value)> {
    let mut scores = BTreeMap::new();
    // Live titles keep newly created/edited issues discoverable before indexing.
    for (number, title) in eligible {
        record(&mut scores, *number, Field::Title, 0, title, query, None);
    }
    let exists = path(source).exists();
    let mut status = json!({"enabled":true,"mode":"keywords","indexing":true,"semantic":"pending","pending":null});
    let cached = (|| -> Result<()> {
        if !exists {
            return Ok(());
        }
        let db = open(source, false)?;
        let terms = rank::terms(query);
        if !terms.is_empty() {
            let expression = terms
                .iter()
                .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" OR ");
            // Choose distinct issues before truncating; a long discussion cannot
            // monopolize the candidate budget. Only load text for those passages.
            let selected = serde_json::to_string(
                &eligible
                    .keys()
                    .filter(|n| !dirty.contains(n))
                    .copied()
                    .collect::<Vec<_>>(),
            )?;
            for field in [Field::Title, Field::Description, Field::Comment] {
                let mut sql=db.prepare("WITH hits AS MATERIALIZED (SELECT p.id,p.number,bm25(words) AS rank FROM words JOIN passages p ON p.id=words.rowid WHERE words MATCH ?1 AND p.project=?2 AND p.field=?3 AND p.number IN (SELECT value FROM json_each(?4))), picked AS (SELECT number,min(rank) AS rank FROM hits GROUP BY number ORDER BY rank,number LIMIT 300), ranked AS (SELECT h.id,row_number() OVER(PARTITION BY h.number ORDER BY h.rank,h.id) AS place FROM hits h JOIN picked ON picked.number=h.number) SELECT p.number,p.source,p.text FROM ranked r JOIN passages p ON p.id=r.id WHERE r.place<=3")?;
                let rows =
                    sql.query_map(params![expression, project, field.name(), selected], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    })?;
                for row in rows {
                    let (n, s, t) = row?;
                    if !dirty.contains(&n) {
                        record(&mut scores, n, field, s, &t, query, None);
                    }
                }
            }
        }
        let pending:i64=db.query_row("SELECT count(*) FROM passages p WHERE p.project=?1 AND NOT EXISTS(SELECT 1 FROM embeddings e WHERE e.hash=p.hash)",[project],|r|r.get(0))?;
        let backfill: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key='backfill_done')",
            [],
            |r| r.get(0),
        )?;
        status["pending"] = json!(pending);
        status["indexing"] = json!(!backfill || pending > 0 || !dirty.is_empty());
        let has_vectors:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM passages p JOIN embeddings e ON p.hash=e.hash WHERE p.project=?1)",[project],|r|r.get(0))?;
        if semantic
            && super::enabled(source)
            && has_vectors
            && query.trim().trim_start_matches('#').parse::<i64>().is_err()
        {
            let cached: Option<Vec<u8>> = db
                .query_row("SELECT vector FROM queries WHERE query=?1", [query], |r| {
                    r.get(0)
                })
                .optional()?;
            let query_vector=cached.as_deref().and_then(vector).or_else(|| {
                let now=crate::issues::worker::now();
                let retry:i64=db.query_row("SELECT CAST(value AS INTEGER) FROM meta WHERE key='query_retry_after'",[],|r|r.get(0)).unwrap_or(0);
                if retry>now {return None;}
                let v=match embed::request(&[query.to_owned()],Duration::from_millis(1800)) {
                    Ok(mut values)=>values.pop()?,
                    Err(_)=>{if let Ok(cache)=open(source,true) {let _=cache.execute("INSERT OR REPLACE INTO meta VALUES('query_retry_after',?1)",[(now+30_000).to_string()]);} return None;}
                };
                if let Ok(cache)=open(source,true) {
                    let _=cache.execute("INSERT OR REPLACE INTO queries VALUES(?1,?2,?3)",params![query,bytes(&v),crate::issues::worker::now()]);
                    let _=cache.execute("DELETE FROM queries WHERE query IN (SELECT query FROM queries ORDER BY used DESC LIMIT -1 OFFSET 256)",[]);
                }
                Some(v)
            });
            if let Some(q) = query_vector {
                status["mode"] = json!("hybrid");
                status["semantic"] = json!("ready");
                let selected = serde_json::to_string(
                    &eligible
                        .keys()
                        .filter(|n| !dirty.contains(n))
                        .copied()
                        .collect::<Vec<_>>(),
                )?;
                let mut stmt=db.prepare("SELECT p.id,p.number,p.field,e.vector FROM passages p JOIN embeddings e ON e.hash=p.hash WHERE p.project=?1 AND p.number IN (SELECT value FROM json_each(?2))")?;
                let mut rows = stmt.query(params![project, selected])?;
                let mut best = HashMap::<(i64, usize), (f32, i64)>::new();
                while let Some(row) = rows.next()? {
                    let blob = row.get_ref(3)?.as_blob().map_err(rusqlite::Error::from)?;
                    if blob.len() != DIMENSIONS * 4 {
                        continue;
                    }
                    let similarity = q
                        .iter()
                        .zip(blob.chunks_exact(4))
                        .map(|(a, b)| a * f32::from_le_bytes(b.try_into().unwrap()))
                        .sum::<f32>();
                    if similarity.is_finite() && similarity >= 0.3 {
                        let field =
                            Field::parse(row.get_ref(2)?.as_str().map_err(rusqlite::Error::from)?);
                        let key = (row.get::<_, i64>(1)?, field.index());
                        if best.get(&key).is_none_or(|(score, _)| similarity > *score) {
                            best.insert(key, (similarity, row.get(0)?));
                        }
                    }
                }
                drop(rows);
                drop(stmt);
                let mut totals = HashMap::<i64, f32>::new();
                for ((number, field), (score, _)) in &best {
                    *totals.entry(*number).or_default() += [5.0, 2.0, 0.5][*field] * score;
                }
                let mut candidates: Vec<_> = totals.into_iter().collect();
                candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                let selected: HashSet<_> =
                    candidates.into_iter().take(300).map(|(n, _)| n).collect();
                best.retain(|(number, _), _| selected.contains(number));
                let mut passage =
                    db.prepare("SELECT number,field,source,text FROM passages WHERE id=?1")?;
                for (_, (similarity, id)) in best {
                    passage
                        .query_row([id], |r| {
                            record(
                                &mut scores,
                                r.get(0)?,
                                Field::parse(r.get_ref(1)?.as_str()?),
                                r.get(2)?,
                                r.get_ref(3)?.as_str()?,
                                query,
                                Some(similarity),
                            );
                            Ok(())
                        })
                        .optional()?;
                }
            } else {
                status["semantic"] = json!("unavailable");
            }
        }
        Ok(())
    })();
    if let Err(error) = cached {
        status = json!({"enabled":true,"mode":"keywords","indexing":false,"semantic":"unavailable","index":"unavailable","error":error.code});
    }
    let exact = query.trim().trim_start_matches('#').parse::<i64>().ok();
    if let Some(n) = exact.filter(|n| eligible.contains_key(n)) {
        record(
            &mut scores,
            n,
            Field::Title,
            0,
            &eligible[&n],
            &eligible[&n],
            None,
        );
    }
    let mut found: Vec<_> = scores
        .into_iter()
        .filter_map(|(number, s)| {
            s.best.map(|mut best| {
                best.score = [Field::Title, Field::Description, Field::Comment]
                    .iter()
                    .map(|f| f.weight() * (s.lexical[f.index()] + s.semantic[f.index()] * 0.35))
                    .sum();
                if exact == Some(number) {
                    best.score += 100.0;
                }
                best
            })
        })
        .collect();
    found.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.number.cmp(&b.number)));
    Ok((found, status))
}
