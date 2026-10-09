//! Indexed visible-range history and bounded projections using the scheduler's evaluator.
use super::*;
use crate::jobs::calendar::{Cursor, Query};
use serde::Serialize;

const PAGE: usize = 100;
const SAMPLE: usize = 3;
// Refuse oversized projects explicitly instead of silently dropping schedules.
const MAX_JOBS: usize = 100;
#[derive(Serialize)]
struct Entry {
    key: String,
    scheduled_at: i64,
    state: String,
    snapshot: Snapshot,
    run: Option<Run>,
}
impl Entry {
    fn projection(snapshot: &Snapshot, at: i64) -> Self {
        Self {
            key: format!("job:{}", snapshot.job_id),
            scheduled_at: at,
            state: "scheduled".into(),
            snapshot: snapshot.clone(),
            run: None,
        }
    }
    fn run(run: Run) -> Self {
        Self {
            key: format!("run:{}", run.id),
            scheduled_at: run.scheduled_at,
            state: run.state.clone(),
            snapshot: run.snapshot.clone(),
            run: Some(run),
        }
    }
    fn after(&self, cursor: &Option<Cursor>) -> bool {
        cursor
            .as_ref()
            .is_none_or(|c| (self.scheduled_at, self.key.as_str()) > (c.at, c.key.as_str()))
    }
}
fn trim(entries: &mut Vec<Entry>, limit: usize) {
    entries.sort_unstable_by(|a, b| (a.scheduled_at, &a.key).cmp(&(b.scheduled_at, &b.key)));
    entries.truncate(limit);
}
pub(super) fn read(
    db: &Connection,
    project: &str,
    q: &Query,
    now: i64,
    detail: bool,
) -> Result<Value> {
    let bounds = q.bounds()?;
    let start = bounds[0].start;
    let end = bounds.last().unwrap().end;
    if let Some(id) = &q.job_id {
        get_job(db, project, id)?;
    }
    let jobs=db.query_collect(&format!("SELECT {JOB_COLUMNS} FROM scheduled_jobs WHERE project_id=?1 AND enabled=1 AND deleted_at IS NULL AND (?2 IS NULL OR id=?2) ORDER BY id LIMIT ?3"),params![project,q.job_id,MAX_JOBS+1],job_row)?;
    if jobs.len() > MAX_JOBS {
        return Err(Error::invalid(
            "This project has more than 100 active schedules. Select a job to view its calendar.",
        ));
    }
    let schedules = jobs
        .iter()
        .map(|j| Schedule::parse(&j.snapshot.definition.cron, &j.snapshot.definition.timezone))
        .collect::<Result<Vec<_>>>()?;
    let mut days = Vec::new();
    for day in &bounds {
        let mut count = 0usize;
        let limit = if detail { PAGE + 1 } else { SAMPLE };
        let mut entries = Vec::new();
        let lower = q.cursor.as_ref().map_or(day.start, |c| c.at.max(day.start));
        let runs=db.query_collect(&format!("SELECT {RUN_COLUMNS} FROM scheduled_job_runs WHERE project_id=?1 AND scheduled_at>=?2 AND scheduled_at<?3 AND (?4 IS NULL OR job_id=?4) AND (scheduled_at>?5 OR (scheduled_at=?5 AND 'run:'||id>?6)) ORDER BY scheduled_at,id LIMIT ?7"),params![project,lower,day.end,q.job_id,q.cursor.as_ref().map_or(i64::MIN,|c|c.at),q.cursor.as_ref().map_or("",|c|c.key.as_str()),limit],run_row)?;
        if !detail {
            count=db.query_row("SELECT COUNT(*) FROM scheduled_job_runs WHERE project_id=?1 AND scheduled_at>=?2 AND scheduled_at<?3 AND (?4 IS NULL OR job_id=?4)",params![project,day.start,day.end,q.job_id],|r|r.get::<_,i64>(0))? as usize;
        }
        entries.extend(runs.into_iter().map(Entry::run));
        for (job, schedule) in jobs.iter().zip(&schedules) {
            let mut after = (day.start - 1).max(now).max(job.schedule_from);
            if detail {
                after = after.max(lower.saturating_sub(1));
            }
            if after >= day.end - 1 {
                continue;
            }
            loop {
                let dates =
                    schedule.preview(after, day.end - 1, if detail { PAGE + 2 } else { 1000 })?;
                if dates.is_empty() {
                    break;
                }
                count += dates.len();
                after = *dates.last().unwrap();
                entries.extend(
                    dates
                        .iter()
                        .take(if detail { PAGE + 2 } else { SAMPLE })
                        .map(|&at| Entry::projection(&job.snapshot, at))
                        .filter(|e| e.after(&q.cursor)),
                );
                trim(&mut entries, limit);
                if detail || dates.len() < 1000 {
                    break;
                }
            }
        }
        trim(&mut entries, limit);
        if detail {
            let cursor = (entries.len() > PAGE).then(|| Cursor {
                at: entries[PAGE - 1].scheduled_at,
                key: entries[PAGE - 1].key.clone(),
            });
            entries.truncate(PAGE);
            return Ok(
                json!({"entries":entries,"next_cursor":cursor,"start_at":start,"end_at":end,"timezone":q.timezone}),
            );
        }
        days.push(json!({"date":day.date,"start_at":day.start,"end_at":day.end,"count":count,"entries":entries}));
    }
    Ok(json!({"days":days,"start_at":start,"end_at":end,"timezone":q.timezone}))
}
