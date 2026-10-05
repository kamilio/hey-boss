use super::*;
use crate::{Client, Freshness, repository::segment};
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    time::Duration,
};

const DIRECT_HEAD_LIMIT: usize = 4;

impl Client {
    /// A bounded, read-only batch through the existing cache and quota scheduler.
    /// Progress cached before a deadline is reused by the next poll.
    pub async fn release_report(&self, request: &Request, freshness: Freshness) -> Result<Batch> {
        crate::report::VALIDATIONS
            .scope(std::cell::RefCell::new(Vec::new()), async {
                let mut batch = self.release_report_inner(request, freshness).await?;
                batch.validations = crate::report::VALIDATIONS.with(|rows| rows.borrow().clone());
                Ok(batch)
            })
            .await
    }
    async fn release_report_inner(&self, request: &Request, freshness: Freshness) -> Result<Batch> {
        request.project.validate()?;
        if request.targets.is_empty() || request.targets.len() > 100 {
            return Err(Error::Invalid("release batch needs 1..100 targets".into()));
        }
        for target in &request.targets {
            validate_target(&request.project, target)?;
        }
        let mut collector = Collector {
            client: self,
            project: &request.project,
            freshness,
            pages: HashMap::new(),
            jobs: HashMap::new(),
            branch_tip: None,
            ancestry: HashSet::new(),
        };
        let deadline = tokio::time::Instant::now() + self.report_timeout();
        let collection_budget =
            self.report_timeout() - Duration::from_secs(20).min(self.report_timeout() / 4);
        let collection_deadline = deadline - (self.report_timeout() - collection_budget);
        // Small batches can use their existing budget; large queues still yield
        // after at most one fifth of it so checked-time ordering rotates fairly.
        let target_budget = collection_budget / request.targets.len().min(5) as u32;
        let mut reports = Vec::new();
        for target in &request.targets {
            if tokio::time::Instant::now() >= collection_deadline {
                break;
            }
            let mut report = Report::new(target);
            let target_deadline =
                collection_deadline.min(tokio::time::Instant::now() + target_budget);
            match crate::client::REQUEST_DEADLINE
                .scope(
                    Some(target_deadline),
                    tokio::time::timeout_at(target_deadline, collector.target(&mut report)),
                )
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => report.errors.push(error.to_string()),
                Err(_) => report
                    .errors
                    .push("release observation deadline; retry uses cached progress".into()),
            }
            if !report.errors.is_empty() {
                report.state = "unknown".into();
            }
            reports.push(report);
        }
        let final_check = crate::client::REQUEST_DEADLINE
            .scope(
                Some(deadline),
                tokio::time::timeout_at(
                    deadline,
                    crate::client::COMPLETION_VALIDATION
                        .scope((), collector.validate_branch(&mut reports)),
                ),
            )
            .await;
        let error = match final_check {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error.to_string()),
            Err(_) => Some("release branch validation deadline; retry uses cached progress".into()),
        };
        if let Some(error) = error {
            for report in reports.iter_mut().filter(|r| !r.gates.is_empty()) {
                report.state = "unknown".into();
                report.errors.push(error.clone());
                for gate in &mut report.gates {
                    gate.satisfied = false;
                    gate.confirmation = None;
                }
            }
        }
        Ok(Batch {
            observed_at_ms: crate::now_ms(),
            reports,
            validations: vec![],
        })
    }
}

pub(super) fn validate_target(project: &Project, target: &str) -> Result<()> {
    let valid = match project.target {
        TargetKind::Commit => {
            (7..=64).contains(&target.len()) && target.bytes().all(|b| b.is_ascii_hexdigit())
        }
        TargetKind::PullRequest => {
            target.bytes().all(|b| b.is_ascii_digit())
                && target.parse::<u64>().is_ok_and(|n| n > 0)
                && !target.starts_with('0')
        }
    };
    if !valid {
        return Err(Error::Invalid(
            "target must match project mode: commit SHA or positive PR number".into(),
        ));
    }
    Ok(())
}

struct Collector<'a> {
    client: &'a Client,
    project: &'a Project,
    freshness: Freshness,
    pages: HashMap<String, (Vec<Value>, bool)>,
    jobs: HashMap<(u64, u64), Vec<Value>>,
    branch_tip: Option<String>,
    ancestry: HashSet<(String, String)>,
}
impl Collector<'_> {
    async fn branch_commits(
        &mut self,
        base: &str,
        head: &str,
    ) -> Result<(bool, Option<HashSet<String>>)> {
        if base == head {
            return Ok((true, Some(HashSet::from([base.to_owned()]))));
        }
        let path = format!(
            "repos/{}/compare/{base}...{head}?per_page=100",
            self.project.repository
        );
        let freshness = if matches!(self.freshness, Freshness::CachedOnly) {
            self.freshness
        } else {
            Freshness::MaxAge(Duration::from_secs(86400))
        };
        let mut response = self.client.get(&path, freshness).await?;
        if !contains(base, head, &response.data) {
            return Ok((false, None));
        }
        let Some(total) = response.data["total_commits"]
            .as_u64()
            .filter(|total| (1..=10_000).contains(total))
        else {
            return Ok((true, None));
        };
        // Only a complete immutable comparison can exclude other run SHAs.
        // Bound work to 100 pages and the normal collection byte budget. Cached
        // immutable pages retain progress when the caller's deadline expires.
        let mut commits = Vec::new();
        let mut roster = HashSet::from([base.to_owned()]);
        let mut bytes = 0usize;
        for page in 1..=100 {
            if page > 1 {
                response = self
                    .client
                    .get(&format!("{path}&page={page}"), freshness)
                    .await?;
                if !contains(base, head, &response.data)
                    || response.data["total_commits"].as_u64() != Some(total)
                {
                    return Err(Error::Invalid(
                        "branch comparison changed during pagination".into(),
                    ));
                }
            }
            bytes = bytes.saturating_add(response.data.to_string().len());
            if bytes > self.client.collection_limit() {
                return Err(Error::Invalid(
                    "branch comparison exceeds configured collection byte limit".into(),
                ));
            }
            let Some(rows) = response.data["commits"].as_array() else {
                return Ok((true, None));
            };
            if rows.is_empty() || rows.len() > 100 || commits.len() + rows.len() > total as usize {
                return Ok((true, None));
            }
            for commit in rows {
                let Some(sha) = commit["sha"]
                    .as_str()
                    .filter(|s| crate::repository::valid_sha(s))
                else {
                    return Ok((true, None));
                };
                if !roster.insert(sha.to_owned()) {
                    return Ok((true, None));
                }
                commits.push(commit.clone());
            }
            if commits.len() == total as usize {
                break;
            }
            if rows.len() < 100 {
                return Ok((true, None));
            }
        }
        if commits.len() != total as usize || commits.last().is_none_or(|c| c["sha"] != head) {
            return Ok((true, None));
        }
        // A merged side branch belongs to the tip without containing the target.
        // Traverse parent edges once; reverse-ordered pages must not turn long
        // histories into repeated full-roster scans.
        let mut descendants = HashMap::<&str, Vec<&str>>::new();
        for commit in &commits {
            let sha = commit["sha"].as_str().unwrap();
            for parent in commit["parents"].as_array().into_iter().flatten() {
                if let Some(parent) = parent["sha"].as_str() {
                    descendants.entry(parent).or_default().push(sha);
                }
            }
        }
        let mut proven = HashSet::from([base]);
        let mut pending = VecDeque::from([base]);
        while let Some(parent) = pending.pop_front() {
            for child in descendants.get(parent).into_iter().flatten() {
                if proven.insert(*child) {
                    pending.push_back(*child);
                }
            }
        }
        self.ancestry.extend(
            proven
                .into_iter()
                .map(|head| (base.to_owned(), head.to_owned())),
        );
        Ok((true, Some(roster)))
    }
    async fn ancestor(&self, base: &str, head: &str) -> Result<bool> {
        if base == head || self.ancestry.contains(&(base.to_owned(), head.to_owned())) {
            return Ok(true);
        }
        let response = self
            .client
            .get(
                &format!(
                    "repos/{}/compare/{base}...{head}?per_page=1",
                    self.project.repository
                ),
                if matches!(self.freshness, Freshness::CachedOnly) {
                    self.freshness
                } else {
                    Freshness::MaxAge(Duration::from_secs(86400))
                },
            )
            .await?;
        Ok(contains(base, head, &response.data))
    }
    async fn target(&mut self, report: &mut Report) -> Result<()> {
        let repo = &self.project.repository;
        let (sha, since) = match self.project.target {
            TargetKind::Commit => {
                let value = self
                    .client
                    .get(
                        &format!("repos/{repo}/commits/{}", report.target),
                        self.freshness,
                    )
                    .await?
                    .data;
                (
                    string(&value, "sha")?,
                    string(&value["commit"]["committer"], "date")?,
                )
            }
            TargetKind::PullRequest => {
                let pr = self
                    .client
                    .pull_request(repo, report.target.parse().unwrap(), self.freshness)
                    .await?
                    .data;
                if pr["number"].as_u64() != report.target.parse().ok()
                    || pr["base"]["repo"]["full_name"]
                        .as_str()
                        .is_none_or(|r| !r.eq_ignore_ascii_case(repo))
                    || pr["base"]["ref"] != self.project.branch
                {
                    return Err(Error::Invalid(
                        "PR identity/base branch does not match release project".into(),
                    ));
                }
                if pr["merged"] != true {
                    report.state = if pr["state"] == "open" {
                        "awaiting_merge"
                    } else if pr["state"] == "closed" && pr["merged"] == false {
                        "not_merged"
                    } else {
                        "unknown"
                    }
                    .into();
                    return Ok(());
                }
                (string(&pr, "merge_commit_sha")?, string(&pr, "merged_at")?)
            }
        };
        if !crate::repository::valid_sha(&sha) {
            return Err(Error::Invalid(
                "release target is not an immutable commit".into(),
            ));
        }
        report.commit = Some(sha.clone());
        let date = chrono::DateTime::parse_from_rfc3339(&since)
            .map_err(|_| Error::Invalid("invalid target commit/merge date".into()))?;
        // Include the preceding day to cover clock skew and runs started near merge.
        let since = (date - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let tip = if let Some(tip) = &self.branch_tip {
            tip.clone()
        } else {
            let branch = self
                .client
                .get(
                    &format!("repos/{repo}/branches/{}", segment(&self.project.branch)),
                    self.freshness,
                )
                .await?
                .data;
            let tip = string(&branch["commit"], "sha")?;
            if !crate::repository::valid_sha(&tip) {
                return Err(Error::Invalid("invalid release branch tip".into()));
            }
            self.branch_tip = Some(tip.clone());
            tip
        };
        report.branch_sha = Some(tip.clone());
        let (on_branch, branch_commits) = self.branch_commits(&sha, &tip).await?;
        if !on_branch {
            report.state = "not_on_branch".into();
            return Ok(());
        }
        for gate in &self.project.gates {
            let (runs, history_complete) = if let Some(heads) = branch_commits
                .as_ref()
                .filter(|s| s.len() <= DIRECT_HEAD_LIMIT)
            {
                self.runs_for_heads(&gate.workflow, heads).await?
            } else {
                self.runs(&gate.workflow, &since, branch_commits.as_ref())
                    .await?
            };
            report.gates.push(GateReport {
                name: gate.name.clone(),
                purpose: gate.purpose,
                satisfied: false,
                history_complete: false,
                confirmation: None,
                runs: vec![],
            });
            let observed = report.gates.last_mut().unwrap();
            // Keep collected records if an await is interrupted, but publish
            // no candidate confirmation until the gate's collection finishes.
            let mut confirmation: Option<RunRecord> = None;
            // Latest run per commit wins. A rerun of an older run is not allowed
            // to overwrite a newer run for the same commit.
            let mut latest = BTreeMap::<String, u64>::new();
            for run in &runs {
                let head = string(run, "head_sha")?;
                if !crate::repository::valid_sha(&head) {
                    return Err(Error::Invalid("invalid workflow commit".into()));
                }
                let id = number(run, "id")?;
                latest
                    .entry(head)
                    .and_modify(|old| *old = (*old).max(id))
                    .or_insert(id);
            }
            let mut runs = runs;
            runs.sort_by_key(|r| {
                (
                    r["created_at"].as_str().unwrap_or("").to_owned(),
                    r["id"].as_u64().unwrap_or(0),
                )
            });
            for (index, run) in runs.iter().enumerate() {
                if let Some(confirmation) = &confirmation
                    && run["created_at"]
                        .as_str()
                        .zip(confirmation.verdict.completed_at.as_deref())
                        .is_some_and(|(created, finished)| created > finished)
                {
                    break;
                }
                let head = string(run, "head_sha")?;
                let covered = if let Some(commits) = &branch_commits {
                    // Comparison members belong to the tip, but a merged side
                    // branch can still omit the target. Prove that direction.
                    commits.contains(&head)
                        && (head == sha || head == tip || self.ancestor(&sha, &head).await?)
                } else if head == sha || head == tip {
                    true
                } else {
                    self.ancestor(&sha, &head).await? && self.ancestor(&head, &tip).await?
                };
                if !covered {
                    continue;
                }
                let id = number(run, "id")?;
                let attempt = number(run, "run_attempt")?;
                // Retain failed/cancelled attempts when history starts after a rerun.
                if attempt > 20 {
                    return Err(Error::Invalid(
                        "more than 20 workflow attempts; evidence incomplete".into(),
                    ));
                }
                for n in 1..=attempt {
                    let attempt_run = if n == attempt {
                        run.clone()
                    } else {
                        self.client
                            .get(
                                &format!("repos/{repo}/actions/runs/{id}/attempts/{n}"),
                                self.freshness,
                            )
                            .await?
                            .data
                    };
                    if attempt_run["id"] != id
                        || attempt_run["head_sha"] != head
                        || attempt_run["run_attempt"] != n
                    {
                        return Err(Error::Invalid("workflow attempt identity changed".into()));
                    }
                    if n == attempt && history_complete {
                        self.prefetch_jobs(&runs[index..], &sha, &tip).await?;
                    }
                    let jobs = self.jobs(&attempt_run).await?;
                    let mut verdict = assess(gate, &attempt_run, &jobs);
                    if n == attempt
                        && latest.get(&head) == Some(&id)
                        && matches!(verdict.state, RunState::Passed | RunState::Unchanged)
                    {
                        let query = url::form_urlencoded::Serializer::new(String::new())
                            .append_pair("head_sha", &head)
                            .append_pair("branch", &self.project.branch)
                            .append_pair("per_page", "100")
                            .append_pair("exclude_pull_requests", "true")
                            .finish();
                        let current = self
                            .client
                            .get(
                                &format!(
                                    "repos/{repo}/actions/workflows/{}/runs?{query}",
                                    gate.workflow
                                ),
                                if matches!(self.freshness, Freshness::CachedOnly) {
                                    self.freshness
                                } else {
                                    Freshness::Revalidate
                                },
                            )
                            .await?
                            .data;
                        let current = current["workflow_runs"]
                            .as_array()
                            .and_then(|runs| {
                                runs.iter()
                                    .filter(|r| {
                                        r["head_sha"] == head
                                            && r["head_branch"] == self.project.branch
                                            && r["path"]
                                                == format!(".github/workflows/{}", gate.workflow)
                                            && r["repository"]["full_name"]
                                                .as_str()
                                                .is_some_and(|s| s.eq_ignore_ascii_case(repo))
                                            && matches!(
                                                r["event"].as_str(),
                                                Some("push" | "schedule" | "workflow_dispatch")
                                            )
                                    })
                                    .max_by_key(|r| r["id"].as_u64().unwrap_or(0))
                            })
                            .ok_or_else(|| {
                                Error::Invalid(
                                    "successful workflow is no longer in the current history"
                                        .into(),
                                )
                            })?;
                        if ["id", "run_attempt", "head_sha"]
                            .iter()
                            .any(|key| current[*key] != run[*key])
                        {
                            return Err(Error::Invalid(
                                "successful workflow changed during observation; retry".into(),
                            ));
                        }
                        // Completed selected jobs survive unrelated publication progress
                        // in this attempt; retain the latest raw workflow conclusion.
                        verdict = assess(gate, current, &jobs);
                    }
                    let record = RunRecord {
                        id,
                        attempt: n,
                        sha: head.clone(),
                        url: format!("https://github.com/{repo}/actions/runs/{id}/attempts/{n}"),
                        coverage: if head == sha { "exact" } else { "successor" }.into(),
                        verdict,
                    };
                    if n == attempt
                        && latest.get(&head) == Some(&id)
                        && matches!(record.verdict.state, RunState::Passed | RunState::Unchanged)
                        && confirmation.as_ref().is_none_or(|old| {
                            record.verdict.completed_at < old.verdict.completed_at
                        })
                    {
                        confirmation = Some(record.clone());
                    }
                    observed.runs.push(record);
                }
            }
            observed.satisfied = confirmation.is_some() && history_complete;
            observed.history_complete = history_complete;
            observed.confirmation = confirmation;
        }
        let failed = report
            .gates
            .iter()
            .flat_map(|g| &g.runs)
            .any(|r| r.verdict.state == RunState::Failed);
        report.state = if report.gates.iter().all(|g| g.satisfied) {
            if failed { "recovered" } else { "verified" }
        } else if failed {
            "failed"
        } else {
            "watching"
        }
        .into();
        Ok(())
    }
    async fn validate_branch(&self, reports: &mut [Report]) -> Result<()> {
        // Skip only interrupted, unknown reports with no completed gate or
        // confirmation. A watching/failed report still needs branch validation
        // even when its workflow listing was truncated.
        if !reports.iter().any(|r| {
            r.gates
                .iter()
                .any(|g| r.state != "unknown" || g.history_complete || g.confirmation.is_some())
        }) {
            return Ok(());
        }
        let branch = self
            .client
            .get(
                &format!(
                    "repos/{}/branches/{}",
                    self.project.repository,
                    segment(&self.project.branch)
                ),
                if matches!(self.freshness, Freshness::CachedOnly) {
                    self.freshness
                } else {
                    Freshness::Revalidate
                },
            )
            .await?
            .data;
        let current = string(&branch["commit"], "sha")?;
        let tip = self
            .branch_tip
            .as_deref()
            .ok_or_else(|| Error::Invalid("missing observed release branch".into()))?;
        if !crate::repository::valid_sha(&current) || !self.ancestor(tip, &current).await? {
            return Err(Error::Invalid(
                "release branch changed during observation; retry".into(),
            ));
        }
        // Every target shares the initial tip; one fast-forward proof covers the batch.
        for report in reports.iter_mut().filter(|r| !r.gates.is_empty()) {
            report.branch_sha = Some(current.clone());
        }
        Ok(())
    }
    async fn runs_for_heads(
        &mut self,
        workflow: &str,
        heads: &HashSet<String>,
    ) -> Result<(Vec<Value>, bool)> {
        // Complete branch ancestry bounds the candidate SHAs. Query a small
        // candidate set directly, including every run/attempt regardless of date.
        let mut heads: Vec<_> = heads.iter().collect();
        heads.sort();
        let mut all = Vec::new();
        let mut complete = true;
        for head in heads {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("head_sha", head)
                .append_pair("branch", &self.project.branch)
                .append_pair("per_page", "100")
                .append_pair("exclude_pull_requests", "true")
                .finish();
            let first = format!(
                "repos/{}/actions/workflows/{workflow}/runs?{query}",
                self.project.repository
            );
            if let Some((rows, done)) = self.pages.get(&first) {
                all.extend(rows.clone());
                complete &= done;
                continue;
            }
            let mut path = first.clone();
            let mut rows = Vec::new();
            let mut seen = HashSet::new();
            let mut ids = HashSet::new();
            let mut expected = None;
            let mut done = true;
            let mut bytes = 0;
            loop {
                if seen.len() >= 10 || !seen.insert(path.clone()) {
                    done = false;
                    break;
                }
                let response = self.client.get(&path, self.freshness).await?;
                bytes += response.data.to_string().len();
                if bytes > self.client.collection_limit() {
                    return Err(Error::Invalid(
                        "release head history exceeds collection byte limit".into(),
                    ));
                }
                let total = number_allow_zero(&response.data, "total_count")?;
                if total >= 1000 {
                    done = false;
                    break;
                }
                if expected.is_some_and(|old| old != total) {
                    done = false;
                }
                expected = Some(total);
                for run in response.data["workflow_runs"]
                    .as_array()
                    .ok_or_else(|| Error::Invalid("missing workflow head history".into()))?
                {
                    if run["head_sha"] != *head
                        || run["head_branch"] != self.project.branch
                        || run["path"] != format!(".github/workflows/{workflow}")
                        || run["repository"]["full_name"]
                            .as_str()
                            .is_none_or(|s| !s.eq_ignore_ascii_case(&self.project.repository))
                    {
                        return Err(Error::Invalid(
                            "workflow head history identity mismatch".into(),
                        ));
                    }
                    if !ids.insert(number(run, "id")?) {
                        done = false;
                    }
                    if matches!(
                        run["event"].as_str(),
                        Some("push" | "workflow_dispatch" | "schedule")
                    ) {
                        rows.push(run.clone());
                    }
                }
                match response.link.as_deref().and_then(crate::client::next_link) {
                    Some(next) => path = self.client.pagination_path(&first, &next)?,
                    None => {
                        if ids.len() as u64 != total {
                            done = false;
                        }
                        break;
                    }
                }
            }
            self.pages.insert(first, (rows.clone(), done));
            all.extend(rows);
            complete &= done;
        }
        Ok((all, complete))
    }
    async fn runs(
        &mut self,
        workflow: &str,
        since: &str,
        candidates: Option<&HashSet<String>>,
    ) -> Result<(Vec<Value>, bool)> {
        let mut roster: Vec<_> = candidates.into_iter().flatten().collect();
        roster.sort();
        let key = format!("{workflow}:{since}:{roster:?}");
        if let Some(value) = self.pages.get(&key) {
            return Ok(value.clone());
        }
        let day = chrono::NaiveDate::parse_from_str(since, "%Y-%m-%d")
            .map_err(|_| Error::Invalid("invalid history date".into()))?;
        let start = day.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
        let today = (crate::now_ms() / 1000 / 86400) as i64 * 86400;
        if start > today || today - start > 366 * 86400 {
            return Err(Error::Invalid(
                "release history must start within the last year".into(),
            ));
        }
        // Stable day boundaries preserve page-cache progress between polls.
        // Split dense windows below GitHub's 1000-result Actions search cap.
        let mut windows: Vec<_> = (0..=((today - start) / 86400) as usize)
            .map(|i| {
                let day = start + i as i64 * 86400;
                (day, day + 86399)
            })
            .rev()
            .collect();
        let mut runs = Vec::new();
        let mut seen = HashSet::new();
        let mut complete = true;
        let mut bytes = 0;
        let mut requests = 0;
        let mut archived_heads = HashSet::new();
        let mut archived_runs = Vec::new();
        'windows: while let Some((start, end)) = windows.pop() {
            // GitHub assigns run creation times. Closed windows can discover
            // candidate heads without repeatedly validating unrelated history.
            // They never supply verdicts: known runs or complete head histories
            // get refreshed below. Today's window discovers new dispatches.
            let discovery_only = candidates.is_some() && end < today;
            let at = |s| {
                chrono::DateTime::from_timestamp(s, 0)
                    .unwrap()
                    .format("%Y-%m-%dT%H:%M:%SZ")
                    .to_string()
            };
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("branch", &self.project.branch)
                .append_pair("created", &format!("{}..{}", at(start), at(end)))
                .append_pair("per_page", "100")
                .append_pair("exclude_pull_requests", "true")
                .finish();
            let mut path = format!(
                "repos/{}/actions/workflows/{workflow}/runs?{query}",
                self.project.repository
            );
            let first_path = path.clone();
            let mut expected = None;
            let mut window_ids = HashSet::new();
            loop {
                requests += 1;
                if requests > 100 {
                    complete = false;
                    break 'windows;
                }
                if !seen.insert(path.clone()) {
                    return Err(Error::Invalid("release history pagination cycle".into()));
                }
                let freshness = match self.freshness {
                    Freshness::MaxAge(age) if discovery_only => {
                        Freshness::MaxAge(age.max(Duration::from_secs(86400)))
                    }
                    other => other,
                };
                let mut response = self.client.get(&path, freshness).await?;
                if discovery_only && response.validated_at_ms < (end as u64 + 1) * 1000 {
                    // A page cached while the day was open can omit later runs.
                    // Crossing midnight does not make that snapshot complete.
                    if matches!(self.freshness, Freshness::CachedOnly) {
                        complete = false;
                    } else {
                        response = self.client.get(&path, Freshness::Revalidate).await?;
                    }
                }
                bytes += response.data.to_string().len();
                if bytes > self.client.collection_limit() {
                    return Err(Error::Invalid(
                        "release history exceeds collection byte limit".into(),
                    ));
                }
                let total = number_allow_zero(&response.data, "total_count")?;
                if expected.is_none() && total >= 1000 {
                    if start == end {
                        complete = false;
                        continue 'windows;
                    }
                    let middle = start + (end - start) / 2;
                    windows.push((middle + 1, end));
                    windows.push((start, middle));
                    continue 'windows;
                }
                if expected.is_some_and(|previous| previous != total) {
                    complete = false;
                }
                expected = Some(total);
                let page = response.data["workflow_runs"]
                    .as_array()
                    .ok_or_else(|| Error::Invalid("missing workflow history".into()))?;
                for run in page {
                    if run["path"] != format!(".github/workflows/{workflow}")
                        || run["head_branch"] != self.project.branch
                        || run["repository"]["full_name"]
                            .as_str()
                            .is_none_or(|name| !name.eq_ignore_ascii_case(&self.project.repository))
                    {
                        return Err(Error::Invalid("workflow history identity mismatch".into()));
                    }
                    if !window_ids.insert(number(run, "id")?) {
                        complete = false;
                    }
                    if matches!(
                        run["event"].as_str(),
                        Some("push" | "workflow_dispatch" | "schedule")
                    ) {
                        if discovery_only {
                            let head = string(run, "head_sha")?;
                            if !crate::repository::valid_sha(&head) {
                                return Err(Error::Invalid("invalid workflow commit".into()));
                            }
                            if candidates.is_some_and(|heads| heads.contains(&head)) {
                                archived_heads.insert(head);
                                archived_runs.push(run.clone());
                            }
                        } else {
                            runs.push(run.clone());
                        }
                    }
                }
                match response.link.as_deref().and_then(crate::client::next_link) {
                    Some(next) => path = self.client.pagination_path(&first_path, &next)?,
                    None => {
                        if window_ids.len() as u64 != total {
                            complete = false;
                        }
                        break;
                    }
                }
            }
        }
        if archived_heads.len() > DIRECT_HEAD_LIMIT {
            let refreshed = if complete
                && matches!(self.freshness, Freshness::MaxAge(_))
                && !self.client.ci_uses_installation(&self.project.repository)
            {
                super::workflow_metadata::refresh(
                    self.client,
                    &archived_runs,
                    self.freshness,
                    &mut bytes,
                )
                .await?
            } else {
                None
            };
            let Some(refreshed) = refreshed else {
                // App routing, explicit freshness, incomplete discovery, or an
                // unrepresentable node retains the existing paged REST path.
                return Box::pin(self.runs(workflow, since, None)).await;
            };
            let ids: HashSet<_> = refreshed
                .iter()
                .filter_map(|run| run["id"].as_u64())
                .collect();
            // A newly dispatched run can share an archived head. Replace only
            // known IDs so that today's newer runs still supersede old success.
            runs.retain(|run| run["id"].as_u64().is_none_or(|id| !ids.contains(&id)));
            runs.extend(refreshed);
            archived_heads.clear();
        }
        if !archived_heads.is_empty() {
            // Replace every copy of these heads, including today's listing, so
            // an old page cannot win deduplication over the current attempt.
            runs.retain(|run| {
                run["head_sha"]
                    .as_str()
                    .is_none_or(|head| !archived_heads.contains(head))
            });
            let (current, current_complete) =
                self.runs_for_heads(workflow, &archived_heads).await?;
            runs.extend(current);
            complete &= current_complete;
        }
        // Duplicated rows during shifting pagination cannot fabricate evidence.
        let mut ids = HashSet::new();
        runs.retain(|r| ids.insert((r["id"].as_u64(), r["run_attempt"].as_u64())));
        let value = (runs, complete);
        self.pages.insert(key, value.clone());
        Ok(value)
    }
    async fn prefetch_jobs(&mut self, runs: &[Value], target: &str, tip: &str) -> Result<()> {
        use super::workflow_jobs;
        if !matches!(self.freshness, Freshness::MaxAge(_))
            || self.client.ci_uses_installation(&self.project.repository)
            || !workflow_jobs::eligible(&runs[0])
            || self.jobs.contains_key(&(number(&runs[0], "id")?, 1))
        {
            return Ok(());
        }
        let mut missing = Vec::new();
        let mut bytes = 0usize;
        // Bound speculative work. The current run's ancestry was just proved;
        // later runs must already have a proof from the complete comparison.
        for (index, run) in runs.iter().take(workflow_jobs::BATCH_SIZE).enumerate() {
            let head = string(run, "head_sha")?;
            if !workflow_jobs::eligible(run)
                || (index > 0
                    && head != target
                    && head != tip
                    && !self.ancestry.contains(&(target.to_owned(), head)))
            {
                continue;
            }
            let id = number(run, "id")?;
            if self.jobs.contains_key(&(id, 1)) {
                continue;
            }
            if let Some(jobs) =
                workflow_jobs::cached(self.client, &self.project.repository, run).await?
            {
                bytes = bytes.saturating_add(
                    serde_json::to_vec(&jobs)
                        .map_err(|e| Error::Invalid(e.to_string()))?
                        .len(),
                );
                if bytes > self.client.collection_limit() {
                    return Err(Error::Invalid(
                        "release jobs exceed collection byte limit".into(),
                    ));
                }
                self.jobs.insert((id, 1), jobs);
            } else {
                missing.push(run);
            }
        }
        if missing.len() < 2 {
            return Ok(());
        }
        let collected = workflow_jobs::fetch(self.client, &missing, self.freshness, bytes).await?;
        for (run, jobs) in collected {
            workflow_jobs::save(self.client, &self.project.repository, run, &jobs).await?;
            self.jobs.insert((number(run, "id")?, 1), jobs);
        }
        Ok(())
    }
    async fn jobs(&mut self, run: &Value) -> Result<Vec<Value>> {
        let id = number(run, "id")?;
        let attempt = number(run, "run_attempt")?;
        if let Some(jobs) = self.jobs.get(&(id, attempt)) {
            return Ok(jobs.clone());
        }
        if matches!(self.freshness, Freshness::MaxAge(_))
            && !self.client.ci_uses_installation(&self.project.repository)
            && let Some(jobs) =
                super::workflow_jobs::cached(self.client, &self.project.repository, run).await?
        {
            self.jobs.insert((id, attempt), jobs.clone());
            return Ok(jobs);
        }
        // Even cancelled workflows can have real failures before cancellation.
        let path = format!(
            "repos/{}/actions/runs/{id}/attempts/{attempt}/jobs?per_page=100",
            self.project.repository
        );
        let version = crate::digest(
            &json!([
                id,
                attempt,
                run["status"],
                run["conclusion"],
                run["updated_at"]
            ])
            .to_string(),
        );
        let settled_cancelled = crate::report::settled_cancelled(run);
        let first_path = path.clone();
        let mut path = path;
        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        let mut ids = HashSet::new();
        let mut expected = None;
        let mut bytes = 0;
        loop {
            if seen.len() >= 1000 || !seen.insert(path.clone()) {
                return Err(Error::Invalid(
                    "release jobs pagination limit or cycle".into(),
                ));
            }
            let response = if run["status"] == "completed" {
                self.client
                    .completed_job_page(&path, &version, settled_cancelled, self.freshness)
                    .await?
            } else {
                self.client.get(&path, self.freshness).await?
            };
            bytes += response.data.to_string().len();
            if bytes > self.client.collection_limit() {
                return Err(Error::Invalid(
                    "release jobs exceed collection byte limit".into(),
                ));
            }
            let total = number_allow_zero(&response.data, "total_count")?;
            if expected.is_some_and(|old| old != total) {
                return Err(Error::Invalid(
                    "release job count changed during pagination".into(),
                ));
            }
            expected = Some(total);
            for job in response.data["jobs"]
                .as_array()
                .ok_or_else(|| Error::Invalid("release jobs array missing".into()))?
            {
                if !ids.insert(number(job, "id")?)
                    || job["run_id"] != id
                    || job["run_attempt"] != attempt
                    || job["head_sha"] != run["head_sha"]
                {
                    return Err(Error::Invalid(
                        "duplicate or mismatched release job identity".into(),
                    ));
                }
                rows.push(job.clone());
            }
            if rows.len() > 100_000 {
                return Err(Error::Invalid("release jobs exceed item limit".into()));
            }
            match response.link.as_deref().and_then(crate::client::next_link) {
                Some(next) => path = self.client.pagination_path(&first_path, &next)?,
                None => {
                    if rows.len() as u64 != total {
                        return Err(Error::Invalid("incomplete release job pagination".into()));
                    }
                    break;
                }
            }
        }
        if !matches!(self.freshness, Freshness::CachedOnly)
            && !self.client.ci_uses_installation(&self.project.repository)
        {
            super::workflow_jobs::save(self.client, &self.project.repository, run, &rows).await?;
        }
        self.jobs.insert((id, attempt), rows.clone());
        Ok(rows)
    }
}
fn string(value: &Value, key: &str) -> Result<String> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::Invalid(format!("release evidence lacks {key}")))
}
fn number(value: &Value, key: &str) -> Result<u64> {
    let n = number_allow_zero(value, key)?;
    if n == 0 {
        Err(Error::Invalid(format!("invalid release {key}")))
    } else {
        Ok(n)
    }
}
fn number_allow_zero(value: &Value, key: &str) -> Result<u64> {
    value[key]
        .as_u64()
        .ok_or_else(|| Error::Invalid(format!("release evidence lacks {key}")))
}
