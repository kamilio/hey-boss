//! Indexed visible-range history and bounded projections using the scheduler's evaluator.
use super::*;
use crate::jobs::calendar::{Cursor, Query};
use serde::Serialize;
use std::collections::BTreeMap;

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

#[derive(Default)]
struct Group {
    count: usize,
    states: BTreeMap<String, usize>,
    first: i64,
    last: i64,
    run_id: Option<String>,
    snapshot: Option<Snapshot>,
}
impl Group {
    fn add(&mut self, state: &str, count: usize, first: i64, last: i64) {
        if self.count == 0 {
            self.first = first;
            self.last = last;
        } else {
            self.first = self.first.min(first);
            self.last = self.last.max(last);
        }
        self.count += count;
        *self.states.entry(state.into()).or_default() += count;
    }
}

fn summary_day(
    db: &Connection,
    project: &str,
    q: &Query,
    day: &crate::jobs::calendar::Day,
    jobs: &[Job],
    schedules: &[Schedule],
    now: i64,
) -> Result<Value> {
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let history_table = history_table(db)?;
    // Aggregate in SQLite; deserialize at most SAMPLE history snapshots, even
    // when a minute schedule has thousands of executions in the visible range.
    let history = db.query_collect(
        &format!("SELECT job_id,state,COUNT(*),MIN(scheduled_at),MAX(scheduled_at),MIN(id) FROM {history_table} WHERE project_id=?1 AND scheduled_at>=?2 AND scheduled_at<?3 AND (?4 IS NULL OR job_id=?4) GROUP BY job_id,state"),
        params![project, day.start, day.end, q.job_id],
        |r| -> rusqlite::Result<_> { Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?)) },
    )?;
    for (id, state, count, first, last, run_id) in history {
        let group = groups.entry(id).or_default();
        group.add(&state, count as usize, first, last);
        group.run_id.get_or_insert(run_id);
    }
    for (job, schedule) in jobs.iter().zip(schedules) {
        let mut after = (day.start - 1).max(now).max(job.schedule_from);
        while after < day.end - 1 {
            let dates = schedule.preview(after, day.end - 1, 1000)?;
            if dates.is_empty() {
                break;
            }
            after = *dates.last().unwrap();
            let group = groups.entry(job.id.clone()).or_default();
            group.add("scheduled", dates.len(), dates[0], after);
            group.snapshot = Some(job.snapshot.clone());
            if dates.len() < 1000 {
                break;
            }
        }
    }
    let count: usize = groups.values().map(|g| g.count).sum();
    let item_count = groups.len();
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_unstable_by(|(a_id, a), (b_id, b)| (a.first, a_id).cmp(&(b.first, b_id)));
    let mut entries = Vec::new();
    for (_, group) in groups.into_iter().take(SAMPLE) {
        let entry = if let Some(snapshot) = &group.snapshot {
            Entry::projection(snapshot, group.first)
        } else {
            Entry::run(observed_run(db, group.run_id.as_deref().unwrap())?)
        };
        let mut entry = serde_json::to_value(entry)?;
        entry["scheduled_at"] = json!(group.first);
        entry["last_at"] = json!(group.last);
        entry["occurrence_count"] = json!(group.count);
        entry["state_counts"] = json!(group.states);
        entries.push(entry);
    }
    Ok(
        json!({"date":day.date,"start_at":day.start,"end_at":day.end,"count":count,"item_count":item_count,"entries":entries}),
    )
}
pub(super) fn read(
    db: &Connection,
    project: &str,
    q: &Query,
    now: i64,
    detail: bool,
) -> Result<Value> {
    let bounds = q.bounds()?;
    let history_table = history_table(db)?;
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
        if !detail {
            days.push(summary_day(db, project, q, day, &jobs, &schedules, now)?);
            continue;
        }
        let limit = PAGE + 1;
        let mut entries = Vec::new();
        let lower = q.cursor.as_ref().map_or(day.start, |c| c.at.max(day.start));
        let runs=db.query_collect(&format!("SELECT {RUN_COLUMNS} FROM {history_table} WHERE project_id=?1 AND scheduled_at>=?2 AND scheduled_at<?3 AND (?4 IS NULL OR job_id=?4) AND (scheduled_at>?5 OR (scheduled_at=?5 AND 'run:'||id>?6)) ORDER BY scheduled_at,id LIMIT ?7"),params![project,lower,day.end,q.job_id,q.cursor.as_ref().map_or(i64::MIN,|c|c.at),q.cursor.as_ref().map_or("",|c|c.key.as_str()),limit],run_row)?;
        entries.extend(runs.into_iter().map(Entry::run));
        for (job, schedule) in jobs.iter().zip(&schedules) {
            let mut after = (day.start - 1).max(now).max(job.schedule_from);
            after = after.max(lower.saturating_sub(1));
            if after >= day.end - 1 {
                continue;
            }
            let dates = schedule.preview(after, day.end - 1, PAGE + 2)?;
            entries.extend(
                dates
                    .iter()
                    .map(|&at| Entry::projection(&job.snapshot, at))
                    .filter(|e| e.after(&q.cursor)),
            );
            trim(&mut entries, limit);
        }
        trim(&mut entries, limit);
        let cursor = (entries.len() > PAGE).then(|| Cursor {
            at: entries[PAGE - 1].scheduled_at,
            key: entries[PAGE - 1].key.clone(),
        });
        entries.truncate(PAGE);
        return Ok(
            json!({"entries":entries,"next_cursor":cursor,"start_at":start,"end_at":end,"timezone":q.timezone}),
        );
    }
    Ok(json!({"days":days,"start_at":start,"end_at":end,"timezone":q.timezone}))
}
