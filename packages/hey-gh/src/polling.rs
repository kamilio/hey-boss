//! Short-lived polling ownership, never shared evidence in a credential cache.
use crate::shared_read::Identity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
use tokio::{sync::Notify, time::Instant};

pub(crate) const LEASE: Duration = Duration::from_secs(30);
const LOCAL_DEMAND: Duration = Duration::from_secs(120);
pub(crate) const MAX_ROWS: usize = 10_000;

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
}

#[derive(Default)]
pub(crate) struct Polling {
    state: Mutex<State>,
    changed: [Notify; 3],
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
            && now < started + LEASE
            && state.local_until.is_none_or(|until| until <= now);
        if !valid {
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
        if self.state.lock().unwrap().lease.take().is_some() {
            self.wake();
        }
    }
    pub fn demand(&self) {
        let mut state = self.state.lock().unwrap();
        state.local_until = Some(Instant::now() + LOCAL_DEMAND);
        if state.lease.take().is_some() {
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
        row(node).is_some_and(|(key, fingerprint)| coverage.rows.get(&key) == Some(&fingerprint))
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
        client
            .save_derived(
                crate::dashboard::DISCOVERY_CACHE,
                json!({"pulls":[node()],"validatedAtMs":crate::now_ms()}),
            )
            .await
            .unwrap();
        assert!(
            client
                .polling()
                .renew(&local(), 60, Instant::now(), coverage())
        );
        let errors = client
            .hydrate_pr_status_ci(crate::Freshness::default())
            .await
            .unwrap();
        assert!(errors.iter().any(|e| e.contains("delegated")), "{errors:?}");
        let cycle = client.account_refresh_cycle(true).await.unwrap().unwrap();
        assert_eq!(cycle.attempted, 0);
        assert_eq!(cycle.deferred, 1);
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
        assert_eq!(pending.data, json!([node()]));
        assert_eq!(client.watches().await.unwrap()[0].id, watch.id);
        client.polling().revoke();
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
        polling.demand();
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
