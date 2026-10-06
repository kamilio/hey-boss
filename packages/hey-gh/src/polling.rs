//! Short-lived polling ownership, never shared evidence in a credential cache.
use crate::shared_read::Identity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
use tokio::{sync::Notify, time::Instant};

pub(crate) const LEASE: Duration = Duration::from_secs(30);
const LOCAL_DEMAND: Duration = Duration::from_secs(120);
const MAX_DEMANDS: usize = 1024;
pub(crate) const MAX_ROWS: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DemandScope {
    Global,
    Repository(String),
    PullRequest(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Coverage {
    pub identity: Identity,
    pub interval_seconds: u64,
    pub modes: [bool; 3],
    pub rows: BTreeMap<String, String>,
}

#[derive(Default)]
struct State {
    lease: Option<(Instant, Coverage)>,
    local_until: Option<Instant>,
    local_source: Option<&'static str>,
    last_probe: Option<&'static str>,
    scoped: BTreeMap<DemandScope, (Instant, &'static str)>,
}

impl State {
    fn locally_demanded(&self, key: &str, now: Instant) -> bool {
        self.local_until.is_some_and(|until| until > now)
            || self
                .scoped
                .get(&DemandScope::PullRequest(key.into()))
                .is_some_and(|(until, _)| *until > now)
            || key.rsplit_once('/').is_some_and(|(repo, _)| {
                self.scoped
                    .get(&DemandScope::Repository(repo.into()))
                    .is_some_and(|(until, _)| *until > now)
            })
    }
}

#[derive(Default)]
pub(crate) struct Polling {
    state: Mutex<State>,
    changed: [Notify; 3],
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PollingStatus {
    pub state: String,
    pub last_probe: String,
    pub local_demand_seconds: u64,
    pub local_demand_source: Option<String>,
    pub covered_prs: usize,
    #[serde(default)]
    pub local_pr_demands: usize,
    #[serde(default)]
    pub local_repository_demands: usize,
}

pub(crate) fn row(node: &Value) -> Option<(String, String)> {
    let repository = node["repository"]["nameWithOwner"].as_str()?;
    crate::client::validate_repository(repository).ok()?;
    let number = node["number"].as_u64().filter(|n| *n > 0)?;
    node["id"].as_str().filter(|id| !id.is_empty())?;
    node["headRefOid"]
        .as_str()
        .filter(|sha| crate::repository::valid_sha(sha))?;
    Some((
        format!("{}/{number}", repository.to_ascii_lowercase()),
        crate::digest(&serde_json::to_string(node).ok()?),
    ))
}

impl Polling {
    pub fn health(&self) -> PollingStatus {
        let state = self.state.lock().unwrap();
        let now = Instant::now();
        let latest = state
            .scoped
            .values()
            .copied()
            .chain(state.local_until.zip(state.local_source))
            .max_by_key(|(until, _)| *until);
        let remaining = latest
            .map(|(until, _)| until)
            .map_or(Duration::ZERO, |until| until.saturating_duration_since(now));
        let active = state.lease.as_ref().filter(|(until, _)| *until > now);
        let covered_prs = active.map_or(0, |(_, coverage)| {
            coverage
                .rows
                .keys()
                .filter(|key| !state.locally_demanded(key, now))
                .count()
        });
        let last_probe = state.last_probe.unwrap_or("not_started");
        PollingStatus {
            state: if covered_prs > 0 {
                "delegated"
            } else if !remaining.is_zero() {
                "local_demand"
            } else if state.lease.is_some() {
                "expired"
            } else {
                last_probe
            }
            .into(),
            last_probe: last_probe.into(),
            local_demand_seconds: remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0),
            local_demand_source: (!remaining.is_zero())
                .then_some(latest.map(|(_, source)| source))
                .flatten()
                .map(str::to_owned),
            covered_prs,
            local_pr_demands: state
                .scoped
                .iter()
                .filter(|(scope, (until, _))| {
                    matches!(scope, DemandScope::PullRequest(_)) && *until > now
                })
                .count(),
            local_repository_demands: state
                .scoped
                .iter()
                .filter(|(scope, (until, _))| {
                    matches!(scope, DemandScope::Repository(_)) && *until > now
                })
                .count(),
        }
    }
    pub fn renew(
        &self,
        local: &Identity,
        interval: u64,
        started: Instant,
        coverage: Coverage,
    ) -> bool {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        let valid = local.validate().is_ok()
            && coverage.identity.validate().is_ok()
            && local.user_id == coverage.identity.user_id
            && local
                .hostname
                .eq_ignore_ascii_case(&coverage.identity.hostname)
            && local.instance != coverage.identity.instance
            && coverage.interval_seconds > 0
            && coverage.interval_seconds <= interval
            && coverage.modes.iter().any(|mode| *mode)
            && !coverage.rows.is_empty()
            && coverage.rows.len() <= MAX_ROWS
            && coverage.rows.iter().all(|(key, value)| {
                key.len() <= 512
                    && value.len() == 64
                    && value.bytes().all(|b| b.is_ascii_hexdigit())
            })
            && started <= now
            && now < started + LEASE;
        state.last_probe = Some(if valid { "covered" } else { "invalid_coverage" });
        if !valid || state.local_until.is_some_and(|until| until > now) {
            if state.lease.take().is_some() {
                self.wake();
            }
            return false;
        }
        let changed = state
            .lease
            .as_ref()
            .is_none_or(|(until, old)| *until <= now || *old != coverage);
        state.lease = Some((started + LEASE, coverage));
        if changed {
            self.wake();
        }
        true
    }
    fn wake(&self) {
        for notify in &self.changed {
            notify.notify_one();
        }
    }
    pub fn revoke(&self) {
        self.probe_failed("unavailable");
    }
    pub fn probe_failed(&self, code: &'static str) {
        let mut state = self.state.lock().unwrap();
        state.last_probe = Some(code);
        if state.lease.take().is_some() {
            self.wake();
        }
    }
    pub fn demand(&self, source: &'static str) {
        self.demand_scoped(source, DemandScope::Global);
    }
    pub fn demand_scoped(&self, source: &'static str, mut scope: DemandScope) {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        state.scoped.retain(|_, (until, _)| *until > now);
        // Never discard an unexpired consumer hold to admit another one.
        if !state.scoped.contains_key(&scope) && state.scoped.len() >= MAX_DEMANDS {
            scope = DemandScope::Global;
        }
        if scope == DemandScope::Global {
            state.local_until = Some(now + LOCAL_DEMAND);
            state.local_source = Some(source);
            state.scoped.clear();
            if state.lease.take().is_some() {
                self.wake();
            }
            return;
        }
        let already_local = state.local_until.is_some_and(|until| until > now)
            || state.scoped.contains_key(&scope)
            || match &scope {
                DemandScope::PullRequest(key) => state.locally_demanded(key, now),
                _ => false,
            };
        state.scoped.insert(scope, (now + LOCAL_DEMAND, source));
        if !already_local {
            self.wake();
        }
    }
    pub fn active(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .lease
            .as_ref()
            .is_some_and(|(until, _)| *until > Instant::now())
    }
    pub fn covers(&self, mode: &str, node: &Value) -> bool {
        let Some(mode) = mode_index(mode) else {
            return false;
        };
        let state = self.state.lock().unwrap();
        let Some((until, coverage)) = &state.lease else {
            return false;
        };
        if *until <= Instant::now() || !coverage.modes[mode] {
            return false;
        }
        row(node).is_some_and(|(key, fingerprint)| {
            !state.locally_demanded(&key, Instant::now())
                && coverage.rows.get(&key) == Some(&fingerprint)
        })
    }
    pub async fn changed(&self, mode: &str) {
        self.changed[mode_index(mode).expect("account hydration mode")]
            .notified()
            .await;
    }
}

fn mode_index(mode: &str) -> Option<usize> {
    match mode {
        "ci" => Some(0),
        "details" => Some(1),
        "policy" => Some(2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn local() -> Identity {
        Identity {
            hostname: "github.com".into(),
            user_id: 42,
            instance: "a".repeat(32),
        }
    }
    fn node() -> Value {
        json!({"id":"PR_one","number":7,"headRefOid":"a".repeat(40),"repository":{"nameWithOwner":"acme/repo"},"updatedAt":"2026-10-06T00:00:00Z"})
    }
    fn coverage() -> Coverage {
        Coverage {
            identity: Identity {
                instance: "b".repeat(32),
                ..local()
            },
            interval_seconds: 60,
            modes: [true; 3],
            rows: BTreeMap::from([row(&node()).unwrap()]),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn scoped_demand_preserves_other_ownership_across_renewal_and_failure() {
        let polling = Polling::default();
        let mut sibling = node();
        sibling["number"] = json!(8);
        let mut other = node();
        other["repository"]["nameWithOwner"] = json!("acme/other");
        let mut proof = coverage();
        proof
            .rows
            .extend([row(&sibling).unwrap(), row(&other).unwrap()]);
        assert!(polling.renew(&local(), 60, Instant::now(), proof.clone()));
        for mode in ["ci", "details", "policy"] {
            polling.changed(mode).await;
        }
        polling.demand_scoped("ci", DemandScope::PullRequest("acme/repo/7".into()));
        for mode in ["ci", "details", "policy"] {
            assert!(!polling.covers(mode, &node()));
            assert!(polling.covers(mode, &sibling));
            assert!(polling.covers(mode, &other));
            tokio::time::timeout(Duration::from_millis(1), polling.changed(mode))
                .await
                .unwrap();
        }
        polling.demand_scoped("ci", DemandScope::PullRequest("acme/repo/7".into()));
        assert!(
            tokio::time::timeout(Duration::from_millis(1), polling.changed("ci"))
                .await
                .is_err()
        );
        assert_eq!(polling.health().covered_prs, 2);
        polling.revoke();
        assert!(polling.renew(&local(), 60, Instant::now(), proof.clone()));
        assert!(!polling.covers("ci", &node()));
        polling.demand_scoped("repository", DemandScope::Repository("acme/repo".into()));
        assert!(!polling.covers("ci", &sibling));
        assert!(polling.covers("ci", &other));
        assert_eq!(polling.health().covered_prs, 1);
        assert_eq!(polling.health().local_pr_demands, 1);
        assert_eq!(polling.health().local_repository_demands, 1);
        tokio::time::advance(LOCAL_DEMAND).await;
        assert!(polling.renew(&local(), 60, Instant::now(), proof));
        assert!(polling.covers("ci", &node()));
        assert_eq!(polling.health().local_pr_demands, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn scoped_demand_overflow_fails_closed_and_expired_entries_are_pruned() {
        let polling = Polling::default();
        for n in 1..=MAX_DEMANDS {
            polling.demand_scoped("ci", DemandScope::PullRequest(format!("acme/repo/{n}")));
        }
        assert_eq!(polling.state.lock().unwrap().scoped.len(), MAX_DEMANDS);
        polling.demand_scoped("ci", DemandScope::PullRequest("acme/repo/100000".into()));
        assert!(!polling.renew(&local(), 60, Instant::now(), coverage()));
        assert_eq!(polling.health().state, "local_demand");
        assert!(polling.state.lock().unwrap().scoped.len() <= MAX_DEMANDS);
        tokio::time::advance(LOCAL_DEMAND).await;
        polling.demand_scoped("ci", DemandScope::PullRequest("acme/repo/9".into()));
        assert_eq!(polling.state.lock().unwrap().scoped.len(), 1);
        assert!(polling.renew(&local(), 60, Instant::now(), coverage()));
        assert!(polling.covers("ci", &node()));
    }

    #[tokio::test(start_paused = true)]
    async fn health_distinguishes_local_demand_from_coverage_and_expiry() {
        let polling = Polling::default();
        assert_eq!(polling.health().state, "not_started");
        assert!(polling.renew(&local(), 60, Instant::now(), coverage()));
        assert_eq!(polling.health().state, "delegated");
        assert_eq!(polling.health().covered_prs, 1);
        polling.demand("source_feed");
        assert_eq!(polling.health().state, "local_demand");
        assert_eq!(polling.health().local_demand_seconds, 120);
        assert_eq!(
            polling.health().local_demand_source.as_deref(),
            Some("source_feed")
        );
        assert!(!polling.renew(&local(), 60, Instant::now(), coverage()));
        assert_eq!(polling.health().last_probe, "covered");
        tokio::time::advance(LOCAL_DEMAND).await;
        assert_eq!(polling.health().local_demand_seconds, 0);
        assert!(polling.renew(&local(), 60, Instant::now(), coverage()));
        tokio::time::advance(LEASE).await;
        assert_eq!(polling.health().state, "expired");
        assert_eq!(polling.health().covered_prs, 0);
    }

    #[tokio::test]
    async fn delegated_hydration_keeps_local_work_pending_without_minting_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let client = crate::Client::with_token(
            crate::Config {
                cache_path: dir.path().join("cache.sqlite"),
                rest_url: "http://127.0.0.1:9/".parse().unwrap(),
                graphql_url: "http://127.0.0.1:9/graphql".parse().unwrap(),
                report_timeout: Duration::from_secs(1),
                max_attempts: 1,
                ..Default::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let watch = client.save_account_watch(60).await.unwrap();
        let mut sibling = node();
        sibling["number"] = json!(8);
        let mut proof = coverage();
        proof
            .rows
            .insert(row(&sibling).unwrap().0, row(&sibling).unwrap().1);
        client
            .save_derived(
                crate::dashboard::DISCOVERY_CACHE,
                json!({"pulls":[node(), sibling],"validatedAtMs":crate::now_ms()}),
            )
            .await
            .unwrap();
        assert!(client.polling().renew(&local(), 60, Instant::now(), proof));
        let errors = client
            .hydrate_pr_status_ci(crate::Freshness::default())
            .await
            .unwrap();
        assert!(errors.iter().any(|e| e.contains("delegated")), "{errors:?}");
        let cycle = client.account_refresh_cycle(true).await.unwrap().unwrap();
        assert_eq!(cycle.attempted, 0);
        assert_eq!(cycle.deferred, 2);
        assert!(
            client
                .derived("account-status-validated:ci:acme/repo/7")
                .await
                .unwrap()
                .is_none()
        );
        let pending = client
            .derived("account-status-pending:ci")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.data, json!([node(), sibling]));
        assert_eq!(client.watches().await.unwrap()[0].id, watch.id);
        client
            .polling()
            .demand_scoped("ci", DemandScope::PullRequest("acme/repo/7".into()));
        client
            .hydrate_pr_status_ci(crate::Freshness::default())
            .await
            .unwrap();
        assert_eq!(
            client
                .account_refresh_cycle(true)
                .await
                .unwrap()
                .unwrap()
                .attempted,
            1
        );
        assert!(client.polling().covers("ci", &sibling));
        let pending = client
            .derived("account-status-pending:ci")
            .await
            .unwrap()
            .unwrap();
        assert!(pending.data.as_array().unwrap().contains(&sibling));
        assert!(
            client
                .derived("account-status-validated:ci:acme/repo/8")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn lease_covers_only_matching_nodes_and_expires_from_request_start() {
        let polling = Polling::default();
        let started = Instant::now();
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(polling.renew(&local(), 60, started, coverage()));
        assert!(polling.covers("ci", &node()));
        for key in ["id", "headRefOid", "updatedAt"] {
            let mut changed = node();
            changed[key] = json!("changed");
            assert!(!polling.covers("ci", &changed));
        }
        assert!(!polling.covers("discovery", &node()));
        tokio::time::advance(Duration::from_secs(25)).await;
        assert!(!polling.covers("ci", &node()));
        assert!(!polling.active());
    }

    #[tokio::test(start_paused = true)]
    async fn local_demand_revokes_ownership_and_blocks_renewal_without_losing_wakeup() {
        let polling = Polling::default();
        assert!(polling.renew(&local(), 60, Instant::now(), coverage()));
        // Drain initial acquisition notices before testing revocation notices.
        for mode in ["ci", "details", "policy"] {
            polling.changed(mode).await;
        }
        polling.demand("source_feed");
        for mode in ["ci", "details", "policy"] {
            assert!(!polling.covers(mode, &node()));
            tokio::time::timeout(Duration::from_millis(1), polling.changed(mode))
                .await
                .unwrap();
        }
        assert!(!polling.renew(&local(), 60, Instant::now(), coverage()));
        tokio::time::advance(LOCAL_DEMAND).await;
        assert!(polling.renew(&local(), 60, Instant::now(), coverage()));
        polling.revoke();
        assert!(!polling.active());
    }

    #[tokio::test]
    async fn invalid_identity_cadence_modes_or_oversized_proofs_never_delegate() {
        let polling = Polling::default();
        for mutate in [
            |c: &mut Coverage| c.identity.user_id = 99,
            |c: &mut Coverage| c.identity.hostname = "other.example".into(),
            |c: &mut Coverage| c.identity.instance = local().instance,
            |c: &mut Coverage| c.interval_seconds = 61,
            |c: &mut Coverage| c.interval_seconds = 0,
            |c: &mut Coverage| c.modes = [false; 3],
            |c: &mut Coverage| {
                c.rows = (0..=MAX_ROWS)
                    .map(|i| (i.to_string(), "a".repeat(64)))
                    .collect();
            },
            |c: &mut Coverage| {
                c.rows.values_mut().for_each(|v| *v = "invalid".into());
            },
        ] {
            assert!(polling.renew(&local(), 60, Instant::now(), coverage()));
            let mut proof = coverage();
            mutate(&mut proof);
            assert!(!polling.renew(&local(), 60, Instant::now(), proof));
            assert!(!polling.active());
        }
        let mut proof = coverage();
        proof.modes = [true, false, false];
        assert!(polling.renew(&local(), 60, Instant::now(), proof));
        assert!(polling.covers("ci", &node()));
        assert!(!polling.covers("details", &node()));
        assert!(!polling.covers("policy", &node()));
    }
}
