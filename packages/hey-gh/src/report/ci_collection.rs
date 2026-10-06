//! Keep a bounded CI collection moving when independent sources finish early.
use super::{
    CiReport, SourceError, check_size, collect, dedup_id, failed_results, summarize, valid_sha,
};
use crate::{Client, Error, Freshness, Result, client::validate_repository};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    future::{Future, poll_fn},
    pin::Pin,
    task::Poll,
};

#[derive(Clone)]
struct Source {
    sha: String,
    name: &'static str,
    path: String,
    field: &'static str,
}

enum Task {
    Source(usize, Source),
    Jobs(usize, Value),
}

enum Target {
    Source(usize),
    Jobs(usize),
}

type Read<'a> = Pin<Box<dyn Future<Output = (Target, Result<Vec<Value>>)> + Send + 'a>>;

async fn next(active: &mut Vec<Read<'_>>) -> (Target, Result<Vec<Value>>) {
    let (index, result) = poll_fn(|cx| {
        for (index, read) in active.iter_mut().enumerate() {
            if let Poll::Ready(result) = read.as_mut().poll(cx) {
                return Poll::Ready((index, result));
            }
        }
        Poll::Pending
    })
    .await;
    drop(active.swap_remove(index));
    result
}

fn latest_workflows(mut runs: Vec<Value>) -> Vec<Value> {
    dedup_id(&mut runs);
    let mut latest = HashMap::<String, Value>::new();
    for run in runs {
        let key = format!(
            "{}:{}:{}",
            run["workflow_id"], run["event"], run["head_sha"]
        );
        let rank = |r: &Value| {
            (
                r["run_number"].as_u64().unwrap_or(0),
                r["run_attempt"].as_u64().unwrap_or(0),
                r["id"].as_u64().unwrap_or(0),
            )
        };
        if latest.get(&key).is_none_or(|old| rank(&run) > rank(old)) {
            latest.insert(key, run);
        }
    }
    let mut runs: Vec<_> = latest.into_values().collect();
    runs.sort_by_key(|r| r["id"].as_u64().unwrap_or(0));
    runs
}

impl Client {
    pub(super) async fn collect_ci_report(
        &self,
        repository: &str,
        head: &str,
        merge: Option<&str>,
        freshness: Freshness,
        include_workflows: bool,
    ) -> Result<CiReport> {
        validate_repository(repository)?;
        if !valid_sha(head) || merge.is_some_and(|s| !valid_sha(s)) {
            return Err(Error::Invalid(
                "CI refs must be immutable commit SHAs".into(),
            ));
        }
        let mut sources = Vec::new();
        for sha in std::iter::once(head).chain(merge.filter(|s| *s != head)) {
            sources.push(Source {
                sha: sha.into(),
                name: "check_runs",
                path: format!(
                    "repos/{repository}/commits/{sha}/check-runs?filter=latest&per_page=100"
                ),
                field: "check_runs",
            });
            sources.push(Source {
                sha: sha.into(),
                name: "commit_statuses",
                path: format!("repos/{repository}/commits/{sha}/status?per_page=100"),
                field: "statuses",
            });
            if include_workflows {
                sources.push(Source {
                    sha: sha.into(),
                    name: "workflow_runs",
                    path: format!("repos/{repository}/actions/runs?head_sha={sha}&per_page=100"),
                    field: "workflow_runs",
                });
            }
        }
        // Resolve the dependencies for job pages early. Otherwise repeated
        // limited reads can spend every turn revalidating mutable commit
        // sources, never filling the missing completed-attempt cache.
        let mut order: Vec<_> = (0..sources.len()).collect();
        let mut missing = vec![false; sources.len()];
        if crate::client::interactive_read()
            && matches!(freshness, Freshness::MaxAge(age) if !age.is_zero())
        {
            let candidates: Vec<_> = order
                .iter()
                .copied()
                .filter(|&index| sources[index].name != "workflow_runs")
                .collect();
            let paths: Vec<_> = candidates
                .iter()
                .map(|&index| sources[index].path.as_str())
                .collect();
            // At most four indexed keys. Presence is only an ordering hint;
            // normal source reads still enforce freshness, shape, and identity.
            // A hint failure retains normal ordering and normal source errors.
            if let Ok(present) = self.ci_source_presence(repository, &paths).await {
                for (index, present) in candidates.into_iter().zip(present) {
                    missing[index] = !present;
                }
            }
        }
        order.sort_by_key(|&index| (sources[index].name != "workflow_runs", !missing[index]));
        let mut pending: VecDeque<_> = order
            .into_iter()
            .map(|index| Task::Source(index, sources[index].clone()))
            .collect();
        let mut results: Vec<Option<Result<Vec<Value>>>> =
            (0..sources.len()).map(|_| None).collect();
        let mut sources_left = sources.len();
        let mut sources_collected = false;
        let (mut checks, mut statuses, mut errors) = (Vec::new(), Vec::new(), Vec::new());
        let mut workflow_lists = sources.iter().filter(|s| s.name == "workflow_runs").count();
        let mut workflows = Vec::new();
        let mut job_results: Vec<Option<Result<Vec<Value>>>> = Vec::new();
        let mut active: Vec<Read<'_>> = Vec::new();
        let mut active_jobs = 0;
        let width = if self.status().queue_capacity >= 32 {
            3
        } else {
            1
        };
        while !pending.is_empty() || !active.is_empty() {
            while active.len() < width {
                // A job may still be preparing its request in the cache. Do
                // not let later commit reads overtake it in the HTTP queue.
                if active_jobs > 0 && matches!(pending.front(), Some(Task::Source(..))) {
                    break;
                }
                let Some(task) = pending.pop_front() else {
                    break;
                };
                if matches!(&task, Task::Jobs(..)) {
                    active_jobs += 1;
                }
                // Poll in the caller's task so validation, entity fences and
                // collection budgets remain shared. No private request queue.
                active.push(Box::pin(async move {
                    match task {
                        Task::Source(index, source) => (
                            Target::Source(index),
                            self.ci_source(
                                repository,
                                &source.sha,
                                source.name,
                                &source.path,
                                source.field,
                                freshness,
                            )
                            .await,
                        ),
                        Task::Jobs(index, run) => (
                            Target::Jobs(index),
                            self.workflow_jobs(
                                repository,
                                run["id"].as_u64().unwrap(),
                                run["run_attempt"].as_u64().unwrap(),
                                &run,
                                freshness,
                            )
                            .await,
                        ),
                    }
                }));
            }
            let (target, mut result) = next(&mut active).await;
            let workflows_ready = match target {
                Target::Source(index) => {
                    if sources[index].name == "commit_statuses"
                        && let Ok(values) = &mut result
                    {
                        for value in values {
                            value["observed_sha"] = json!(sources[index].sha);
                        }
                    }
                    results[index] = Some(result);
                    sources_left -= 1;
                    if sources[index].name == "workflow_runs" {
                        workflow_lists -= 1;
                        workflow_lists == 0
                    } else {
                        false
                    }
                }
                Target::Jobs(index) => {
                    active_jobs -= 1;
                    job_results[index] = Some(result);
                    false
                }
            };
            check_size(
                results
                    .iter()
                    .filter_map(|r| r.as_ref()?.as_ref().ok())
                    .flatten(),
                self.collection_limit(),
            )?;
            if workflows_ready {
                // Deduplicate both refs before fetching jobs. A late list can
                // contain the same run, or supersede an earlier cancelled run.
                workflows = latest_workflows(
                    sources
                        .iter()
                        .zip(&results)
                        .filter(|(s, _)| s.name == "workflow_runs")
                        .filter_map(|(_, r)| r.as_ref()?.as_ref().ok())
                        .flatten()
                        .cloned()
                        .collect(),
                );
                job_results = (0..workflows.len()).map(|_| None).collect();
                // Keep active sources running, but admit newly ready jobs
                // before the remaining commit sources. Reverse insertion keeps
                // workflow order stable and does not expand the read window.
                for (index, run) in workflows.iter().enumerate().rev() {
                    if run["id"].as_u64().is_some() && run["run_attempt"].as_u64().is_some() {
                        pending.push_front(Task::Jobs(index, run.clone()));
                    }
                }
            }
            if sources_left == 0 && !sources_collected {
                // Preserve the original, unfiltered source budget above, then
                // release superseded workflow data before counting job detail.
                // Stable output order does not depend on completion order.
                for (source, result) in sources.iter().zip(&mut results) {
                    let values = collect(
                        result.take().expect("completed CI source"),
                        &format!("{}:{}", source.name, source.sha),
                        &mut errors,
                    );
                    match source.name {
                        "check_runs" => checks.extend(values),
                        "commit_statuses" => statuses.extend(values),
                        "workflow_runs" => {}
                        _ => unreachable!("fixed CI source list"),
                    }
                }
                dedup_id(&mut checks);
                dedup_id(&mut statuses);
                sources_collected = true;
            }
            check_size(
                checks.iter().chain(&statuses).chain(&workflows).chain(
                    job_results
                        .iter()
                        .filter_map(|r| r.as_ref()?.as_ref().ok())
                        .flatten(),
                ),
                self.collection_limit(),
            )?;
        }
        let mut jobs = Vec::new();
        for (run, result) in workflows.iter().zip(job_results) {
            if let (Some(id), Some(attempt)) = (run["id"].as_u64(), run["run_attempt"].as_u64()) {
                jobs.extend(collect(
                    result.expect("completed jobs source"),
                    &format!("jobs:{id}:{attempt}"),
                    &mut errors,
                ));
            } else {
                errors.push(SourceError {
                    source: "workflow_runs".into(),
                    message: "workflow run lacks id or attempt".into(),
                });
            }
        }
        check_size(
            checks
                .iter()
                .chain(&statuses)
                .chain(&workflows)
                .chain(&jobs),
            self.collection_limit(),
        )?;
        let summary = summarize(&checks, &statuses, &workflows, &jobs, !errors.is_empty());
        let failures = failed_results(&checks, &statuses, &workflows, &jobs);
        Ok(CiReport {
            head_sha: head.into(),
            merge_sha: merge.map(str::to_owned),
            check_runs: checks,
            commit_statuses: statuses,
            workflow_runs: workflows,
            jobs,
            summary,
            failures,
            errors,
        })
    }
}
