//! Branch-tip reconciliation through the same queue, cache, and cursor feed.
use crate::client::validate_repository;
use crate::{Client, Error, Freshness, Result, SourceError, Watch, WatchKind, digest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BranchTransition {
    /// baseline, created, updated, or deleted.
    pub kind: String,
    pub old_sha: Option<String>,
    pub new_sha: Option<String>,
    /// forward, rewind, rewritten, or unknown (not inferred from timestamps).
    pub ancestry: String,
    pub comparison_complete: bool,
    pub commits: Vec<Value>,
    pub compare_url: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BranchReport {
    pub repository: String,
    pub branch: String,
    pub sha: Option<String>,
    pub tip_commit: Option<Value>,
    pub transition: BranchTransition,
    pub errors: Vec<SourceError>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepositoryReport {
    pub repository: String,
    pub default_branch: String,
    pub branches: Vec<BranchReport>,
    pub errors: Vec<SourceError>,
    /// Refs whose observed SHA changed in this collection (including removals).
    pub changed_branches: Vec<String>,
    pub cursor: String,
}

pub(crate) fn segment(value: &str) -> String {
    let mut url = url::Url::parse("http://localhost/").expect("static URL");
    url.path_segments_mut().expect("HTTP URL").push(value);
    url.path().trim_start_matches('/').to_owned()
}
pub(crate) fn validate_branch(branch: &str) -> Result<()> {
    if branch.is_empty()
        || branch.len() > 1024
        || branch == "@"
        || branch.starts_with('/')
        || branch.ends_with('/')
        || branch.ends_with('.')
        || branch.contains("..")
        || branch.contains("@{")
        || branch.contains("//")
        || branch
            .chars()
            .any(|c| c.is_control() || " ~^:?*[\\".contains(c))
        || branch
            .split('/')
            .any(|part| part.starts_with('.') || part.ends_with(".lock"))
    {
        return Err(Error::Invalid("invalid Git branch name".into()));
    }
    Ok(())
}
pub(crate) fn valid_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}
fn error(source: &str, error: impl std::fmt::Display) -> SourceError {
    SourceError {
        source: source.into(),
        message: error.to_string(),
    }
}

impl Client {
    pub async fn save_repository_watch(
        &self,
        repository: &str,
        mut branches: Vec<String>,
        all_branches: bool,
        interval_seconds: u64,
    ) -> Result<Watch> {
        validate_repository(repository)?;
        if !(10..=86400).contains(&interval_seconds) || branches.len() > 1000 {
            return Err(Error::Invalid(
                "interval must be 10..86400 and at most 1000 explicit branches are supported"
                    .into(),
            ));
        }
        for branch in &branches {
            validate_branch(branch)?;
        }
        branches.sort();
        branches.dedup();
        let watch = Watch {
            id: digest(&format!("branches:{repository}")),
            repository: repository.into(),
            pull_number: 0,
            interval_seconds,
            kind: WatchKind::Branches,
            branches,
            all_branches,
        };
        self.persist_watch(&watch).await?;
        Ok(watch)
    }

    /// Reconcile selected refs, always including the current default branch.
    /// All-branch mode detects removals from the last successfully observed roster.
    pub async fn repository_report(
        &self,
        repository: &str,
        branches: &[String],
        all_branches: bool,
        freshness: Freshness,
    ) -> Result<RepositoryReport> {
        validate_repository(repository)?;
        if branches.len() > 1000 {
            return Err(Error::Invalid("too many branches".into()));
        }
        for branch in branches {
            validate_branch(branch)?;
        }
        let lock = self.report_lock(&format!("repository:{repository}"));
        tokio::time::timeout(self.report_timeout(), async {
            let _guard = lock.lock().await;
            let metadata = self.get(&format!("repos/{repository}"), freshness).await?;
            let default_branch = metadata.data["default_branch"]
                .as_str()
                .ok_or_else(|| Error::Invalid("repository has no default branch".into()))?
                .to_owned();
            validate_branch(&default_branch)?;
            let roster_resource = format!("branches://{}/{repository}", self.hostname());
            let mut selected: BTreeSet<String> = branches.iter().cloned().collect();
            selected.insert(default_branch.clone());
            let mut roster = None;
            let previous_roster = self.stored_snapshot(&roster_resource).await?;
            let known_roster = previous_roster.is_some();
            if all_branches {
                let listed = self
                    .pages(
                        &format!("repos/{repository}/branches?per_page=100"),
                        None,
                        freshness,
                    )
                    .await?;
                let mut names = BTreeSet::new();
                for branch in listed {
                    let name = branch["name"]
                        .as_str()
                        .ok_or_else(|| Error::Invalid("branch listing lacks name".into()))?;
                    validate_branch(name)?;
                    names.insert(name.to_owned());
                }
                selected.extend(names.iter().cloned());
                if let Some(previous) = previous_roster
                    && let Some(old) = previous["names"].as_array()
                {
                    selected.extend(old.iter().filter_map(Value::as_str).map(str::to_owned));
                }
                roster = Some(json!({"names":names}));
            }
            // Include the local repository refs involved in registered PR watches.
            let mut errors = Vec::new();
            for watch in self.watches().await? {
                if watch.kind != WatchKind::PullRequests
                    || !watch.repository.eq_ignore_ascii_case(repository)
                {
                    continue;
                }
                let pulls = if watch.pull_number == 0 {
                    self.my_pull_requests(repository, freshness).await
                } else {
                    self.pull_request(repository, watch.pull_number, freshness)
                        .await
                        .map(|r| vec![r.data])
                };
                match pulls {
                    Ok(pulls) => {
                        for pr in pulls {
                            for field in ["base", "head"] {
                                let same_repo = pr[field]["repo"]["full_name"]
                                    .as_str()
                                    .is_some_and(|name| name.eq_ignore_ascii_case(repository));
                                if (field == "base" || same_repo)
                                    && let Some(name) = pr[field]["ref"].as_str()
                                {
                                    validate_branch(name)?;
                                    selected.insert(name.into());
                                }
                            }
                        }
                    }
                    Err(e) => errors.push(error("watched_pr_branches", e)),
                }
            }
            if selected.len() > 10000 {
                return Err(Error::Invalid("branch selection exceeds 10000 refs".into()));
            }
            let mut changed_branches = Vec::new();
            let mut reports = Vec::new();
            let mut observations = vec![(
                format!("repository://{}/{repository}", self.hostname()),
                json!({"default_branch":default_branch}),
            )];
            let mut bytes = 0usize;
            for branch in selected {
                match self
                    .collect_branch(repository, &branch, all_branches && known_roster, freshness)
                    .await
                {
                    Ok(mut report) => {
                        let resource = format!(
                            "branch://{}/{repository}/{}",
                            self.hostname(),
                            segment(&branch)
                        );
                        let old = self.stored_snapshot(&resource).await?;
                        if old.as_ref().map(|v| &v["sha"]) != Some(&json!(report.sha)) {
                            changed_branches.push(branch.clone());
                        }
                        for commit in &mut report.transition.commits {
                            *commit = commit_metadata(commit);
                        }
                        errors.extend(report.errors.clone());
                        let mut branch_observations = Vec::new();
                        for commit in &report.transition.commits {
                            let sha = commit["sha"].as_str().filter(|s| valid_sha(s)).ok_or_else(
                                || Error::Invalid("comparison commit lacks immutable SHA".into()),
                            )?;
                            branch_observations.push((
                                format!("commit://{}/{repository}/{sha}", self.hostname()),
                                commit.clone(),
                            ));
                        }
                        let value = serde_json::to_value(&report)
                            .map_err(|e| Error::Invalid(e.to_string()))?;
                        bytes = bytes.saturating_add(value.to_string().len());
                        if bytes > self.collection_limit() {
                            return Err(Error::Invalid(
                                "repository report exceeds collection byte limit".into(),
                            ));
                        }
                        branch_observations.push((
                            format!(
                                "branch://{}/{repository}/{}",
                                self.hostname(),
                                segment(&branch)
                            ),
                            value,
                        ));
                        self.observe_many(&branch_observations).await?;
                        reports.push(report);
                    }
                    Err(e) => errors.push(error(&format!("branch:{branch}"), e)),
                }
            }
            // Preserve deleted-ref discovery until every selected ref was observed.
            if errors.is_empty()
                && let Some(roster) = roster
            {
                observations.push((roster_resource, roster));
            }
            let cursor = self.observe_many(&observations).await?;
            Ok(RepositoryReport {
                repository: repository.into(),
                default_branch,
                branches: reports,
                errors,
                changed_branches,
                cursor,
            })
        })
        .await
        .map_err(|_| Error::Deadline)?
    }

    pub(crate) async fn refresh_affected_prs(
        &self,
        repository: &str,
        changed: &[String],
    ) -> Result<()> {
        if changed.is_empty() {
            return Ok(());
        }
        let mut numbers = BTreeSet::new();
        for watch in self.watches().await? {
            if watch.kind != WatchKind::PullRequests
                || !watch.repository.eq_ignore_ascii_case(repository)
            {
                continue;
            }
            if watch.pull_number != 0 {
                numbers.insert(watch.pull_number);
            } else {
                numbers.extend(
                    self.my_pull_requests(repository, Freshness::Revalidate)
                        .await?
                        .iter()
                        .filter_map(|p| p["number"].as_u64()),
                );
            }
        }
        let mut failures = Vec::new();
        for number in numbers {
            let pr = self
                .pull_request(repository, number, Freshness::Revalidate)
                .await?;
            if ["base", "head"].iter().any(|field| {
                pr.data[*field]["ref"]
                    .as_str()
                    .is_some_and(|r| changed.iter().any(|name| name == r))
            }) {
                match self
                    .ci_for_pr(repository, number, Freshness::Revalidate)
                    .await
                {
                    Ok(ci) if ci.complete => {}
                    Ok(_) => failures.push(format!("PR {number}: incomplete CI refresh")),
                    Err(e) => failures.push(format!("PR {number}: {e}")),
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(failures.join("; ")))
        }
    }

    async fn collect_branch(
        &self,
        repository: &str,
        branch: &str,
        discovered: bool,
        freshness: Freshness,
    ) -> Result<BranchReport> {
        let resource = format!(
            "branch://{}/{repository}/{}",
            self.hostname(),
            segment(branch)
        );
        let previous: Option<BranchReport> = self
            .stored_snapshot(&resource)
            .await?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| Error::Storage(e.to_string()))?;
        if matches!(freshness, Freshness::CachedOnly) {
            return previous.ok_or(Error::CacheMiss);
        }
        let immutable_policy = if matches!(freshness, Freshness::Revalidate)
            || previous.as_ref().is_some_and(|p| !p.errors.is_empty())
        {
            Freshness::Revalidate
        } else {
            Freshness::MaxAge(Duration::from_secs(86400))
        };
        let path = format!("repos/{repository}/branches/{}", segment(branch));
        for attempt in 0..2 {
            let policy = if attempt == 0 {
                freshness
            } else {
                Freshness::Revalidate
            };
            let tip = match self.get(&path, policy).await {
                Ok(r) => Some(r),
                Err(Error::GitHub { status: 404, .. }) => None,
                Err(e) => return Err(e),
            };
            let sha = tip
                .as_ref()
                .map(|r| {
                    r.data["commit"]["sha"]
                        .as_str()
                        .filter(|s| valid_sha(s))
                        .map(str::to_owned)
                        .ok_or_else(|| Error::Invalid("branch lacks immutable tip SHA".into()))
                })
                .transpose()?;
            if let Some(previous) = &previous
                && previous.sha == sha
                && previous.errors.is_empty()
            {
                return Ok(previous.clone());
            }
            let old_sha = previous.as_ref().and_then(|p| {
                if p.errors.is_empty() {
                    p.sha.clone()
                } else {
                    p.transition.old_sha.clone()
                }
            });
            let kind = match (&previous, &sha) {
                (None, Some(_)) if discovered => "created",
                (None, Some(_)) => "baseline",
                (None, None) => "missing",
                (_, None) => "deleted",
                (Some(p), Some(_)) if p.sha.is_none() => "created",
                _ => "updated",
            }
            .to_owned();
            let mut report = BranchReport {
                repository: repository.into(),
                branch: branch.into(),
                sha: sha.clone(),
                tip_commit: None,
                transition: BranchTransition {
                    kind,
                    old_sha: old_sha.clone(),
                    new_sha: sha.clone(),
                    ancestry: "unknown".into(),
                    comparison_complete: true,
                    commits: Vec::new(),
                    compare_url: None,
                },
                errors: Vec::new(),
            };
            if let Some(sha) = &sha {
                match self
                    .get(
                        &format!("repos/{repository}/commits/{sha}"),
                        immutable_policy,
                    )
                    .await
                {
                    Ok(commit) if commit.data["sha"] == *sha => {
                        report.tip_commit = Some(commit.data)
                    }
                    Ok(_) => report
                        .errors
                        .push(error("tip_commit", "commit response SHA mismatch")),
                    Err(e) => report.errors.push(error("tip_commit", e)),
                }
                if let Some(old) = old_sha.as_ref().filter(|old| *old != sha) {
                    let compare_path =
                        format!("repos/{repository}/compare/{old}...{sha}?per_page=100");
                    let comparison = self.get(&compare_path, immutable_policy).await;
                    match comparison {
                        Ok(compare) => {
                            report.transition.ancestry = match compare.data["status"].as_str() {
                                Some("ahead") => "forward",
                                Some("behind") => "rewind",
                                Some("diverged") => "rewritten",
                                _ => "unknown",
                            }
                            .into();
                            if report.transition.ancestry == "unknown" {
                                report.errors.push(error(
                                    "compare",
                                    "comparison lacks a supported ancestry status",
                                ));
                            }
                            report.transition.compare_url =
                                compare.data["html_url"].as_str().map(str::to_owned);
                            match self
                                .pages(
                                    &compare_path,
                                    Some("commits"),
                                    Freshness::MaxAge(Duration::from_secs(1)),
                                )
                                .await
                            {
                                Ok(commits) => {
                                    let total = compare.data["total_commits"].as_u64();
                                    if total != Some(commits.len() as u64) {
                                        report.errors.push(error(
                                            "compare",
                                            "comparison commit count is incomplete",
                                        ));
                                    }
                                    report.transition.commits = commits;
                                }
                                Err(e) => report.errors.push(error("compare", e)),
                            }
                        }
                        Err(e) => report.errors.push(error("compare", e)),
                    }
                } else if let Some(commit) = &report.tip_commit {
                    report.transition.commits.push(commit.clone());
                }
                // Always publish the tip, even when comparison failed, so the new
                // commit is visible while the missing history remains retryable.
                if let Some(commit) = &report.tip_commit
                    && !report.transition.commits.iter().any(|c| c["sha"] == *sha)
                {
                    report.transition.commits.push(commit.clone());
                }
            }
            report.transition.comparison_complete = report.errors.is_empty();
            let confirmed = match self.get(&path, Freshness::Revalidate).await {
                Ok(r) => Some(
                    r.data["commit"]["sha"]
                        .as_str()
                        .ok_or_else(|| Error::Invalid("branch confirmation lacks SHA".into()))?
                        .to_owned(),
                ),
                Err(Error::GitHub { status: 404, .. }) => None,
                Err(e) => return Err(e),
            };
            if confirmed == sha {
                return Ok(report);
            }
        }
        Err(Error::Invalid(
            "branch tip changed repeatedly during collection; retry".into(),
        ))
    }
}

fn commit_metadata(commit: &Value) -> Value {
    json!({"sha":commit["sha"],"html_url":commit["html_url"],"author":commit["author"],"committer":commit["committer"],
        "commit":{"message":commit["commit"]["message"],"author":commit["commit"]["author"],"committer":commit["commit"]["committer"],"tree":commit["commit"]["tree"]},
        "parents":commit["parents"].as_array().map(|parents|parents.iter().map(|p|json!({"sha":p["sha"]})).collect::<Vec<_>>()).unwrap_or_default()})
}
