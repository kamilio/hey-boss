//! An explicit durable queue. No issue-store or agent lifecycle integration.
use super::*;
use rusqlite::{Connection, OptionalExtension, params};
use std::{path::Path, time::Duration};

pub struct Queue(Connection);
#[derive(Debug, Serialize, Deserialize)]
pub struct Confirmation {
    pub gate: String,
    pub purpose: Purpose,
    pub evidence: RunRecord,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Entry {
    pub target: String,
    pub checked_at_ms: Option<u64>,
    pub oldest_source_validation_ms: Option<u64>,
    pub report: Option<Report>,
    /// Previously observed failures survive retries, history truncation and restarts.
    pub failures: Vec<RunRecord>,
    /// Historical confirmations remain facts even when a later poll is unknown.
    pub confirmations: Vec<Confirmation>,
}
impl Queue {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path).map_err(storage)?;
        db.busy_timeout(Duration::from_secs(5)).map_err(storage)?;
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS release_project(id INTEGER PRIMARY KEY CHECK(id=1),config TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS release_targets(target TEXT PRIMARY KEY,created_at INTEGER NOT NULL,checked_at INTEGER,validated_at INTEGER,report TEXT,failures TEXT NOT NULL DEFAULT '[]',confirmations TEXT NOT NULL DEFAULT '[]');").map_err(storage)?;
        Ok(Self(db))
    }
    pub fn add(&mut self, project: &Project, targets: &[String]) -> Result<()> {
        project.validate()?;
        if targets.is_empty() || targets.len() > 100 {
            return Err(Error::Invalid("add 1..100 release targets".into()));
        }
        for target in targets {
            super::collect::validate_target(project, target)?;
        }
        let serialized = serde_json::to_string(project).map_err(storage)?;
        let tx = self.0.transaction().map_err(storage)?;
        let previous: Option<String> = tx
            .query_row("SELECT config FROM release_project WHERE id=1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(storage)?;
        if previous.as_ref().is_some_and(|old| old != &serialized) {
            return Err(Error::Invalid(
                "queue project configuration differs; use a separate queue for the new policy"
                    .into(),
            ));
        }
        tx.execute(
            "INSERT OR IGNORE INTO release_project VALUES(1,?1)",
            [serialized],
        )
        .map_err(storage)?;
        for target in targets {
            tx.execute(
                "INSERT OR IGNORE INTO release_targets(target,created_at) VALUES(?1,?2)",
                params![target, crate::now_ms()],
            )
            .map_err(storage)?;
        }
        tx.commit().map_err(storage)
    }
    pub fn project(&self) -> Result<Project> {
        let value: String = self
            .0
            .query_row("SELECT config FROM release_project WHERE id=1", [], |r| {
                r.get(0)
            })
            .map_err(storage)?;
        let p: Project = serde_json::from_str(&value).map_err(storage)?;
        p.validate()?;
        Ok(p)
    }
    pub fn entries(&self) -> Result<Vec<Entry>> {
        let mut query=self.0.prepare("SELECT target,checked_at,report,failures,validated_at,confirmations FROM release_targets ORDER BY checked_at IS NOT NULL,checked_at,created_at,target").map_err(storage)?;
        let rows = query
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<u64>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<u64>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })
            .map_err(storage)?;
        rows.map(|row| {
            let (
                target,
                checked_at_ms,
                report,
                failures,
                oldest_source_validation_ms,
                confirmations,
            ) = row.map_err(storage)?;
            Ok(Entry {
                target,
                checked_at_ms,
                oldest_source_validation_ms,
                report: report
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .map_err(storage)?,
                failures: serde_json::from_str(&failures).map_err(storage)?,
                confirmations: serde_json::from_str(&confirmations).map_err(storage)?,
            })
        })
        .collect()
    }
    pub fn record(&mut self, batch: &Batch) -> Result<()> {
        let tx = self.0.transaction().map_err(storage)?;
        for report in &batch.reports {
            let prior: Option<(String, Option<u64>, String)> = tx
                .query_row(
                    "SELECT failures,checked_at,confirmations FROM release_targets WHERE target=?1",
                    [&report.target],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_err(storage)?;
            let Some((failures, checked, confirmations)) = prior else {
                continue;
            };
            if checked.is_some_and(|t| t > batch.observed_at_ms) {
                continue;
            }
            let mut failures: Vec<RunRecord> = serde_json::from_str(&failures).map_err(storage)?;
            let mut confirmations: Vec<Confirmation> =
                serde_json::from_str(&confirmations).map_err(storage)?;
            for gate in &report.gates {
                if let Some(evidence) = &gate.confirmation
                    && !confirmations.iter().any(|old| {
                        old.gate == gate.name
                            && old.evidence.id == evidence.id
                            && old.evidence.attempt == evidence.attempt
                            && old.evidence.verdict.state == evidence.verdict.state
                            && old.evidence.verdict.completed_at == evidence.verdict.completed_at
                    })
                {
                    confirmations.push(Confirmation {
                        gate: gate.name.clone(),
                        purpose: gate.purpose,
                        evidence: evidence.clone(),
                    });
                }
            }
            for run in report.gates.iter().flat_map(|g| &g.runs).filter(|r| {
                r.verdict.state == RunState::Failed || !r.verdict.failed_jobs.is_empty()
            }) {
                if !failures.iter().any(|old| {
                    old.id == run.id
                        && old.attempt == run.attempt
                        && old.verdict.failed_jobs == run.verdict.failed_jobs
                }) {
                    failures.push(run.clone());
                }
            }
            let mut report = report.clone();
            if report.state == "verified"
                && failures.iter().any(|r| r.verdict.state == RunState::Failed)
            {
                report.state = "recovered".into();
            }
            tx.execute(
                "UPDATE release_targets SET checked_at=?2,report=?3,failures=?4,validated_at=COALESCE(?5,validated_at),confirmations=?6 WHERE target=?1",
                params![
                    report.target,
                    batch.observed_at_ms,
                    serde_json::to_string(&report).map_err(storage)?,
                    serde_json::to_string(&failures).map_err(storage)?,
                    batch.validations.iter().map(|v|v.validated_at_ms).min(),
                    serde_json::to_string(&confirmations).map_err(storage)?
                ],
            )
            .map_err(storage)?;
        }
        tx.commit().map_err(storage)
    }
    pub fn remove(&self, target: &str) -> Result<()> {
        self.0
            .execute("DELETE FROM release_targets WHERE target=?1", [target])
            .map_err(storage)?;
        Ok(())
    }
    /// Retain prior evidence, but withdraw a success claim on transport errors.
    pub fn record_error(&mut self, targets: &[String], error: &str) -> Result<()> {
        let reports = self
            .entries()?
            .into_iter()
            .filter(|e| targets.contains(&e.target))
            .map(|entry| {
                let mut report = entry.report.unwrap_or_else(|| Report::new(&entry.target));
                report.state = "unknown".into();
                report.errors = vec![error.into()];
                report
            })
            .collect();
        self.record(&Batch {
            observed_at_ms: crate::now_ms(),
            reports,
            validations: vec![],
        })
    }
}
fn storage(error: impl std::fmt::Display) -> Error {
    Error::Storage(error.to_string())
}
