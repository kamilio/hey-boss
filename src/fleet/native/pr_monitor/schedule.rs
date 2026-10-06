use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Schedule {
    pub cooldown_until: i64,
    entries: BTreeMap<String, Entry>,
}

#[derive(Default, Serialize, Deserialize)]
struct Entry {
    attempted_at: i64,
    next_at: i64,
    failures: u32,
}

impl Schedule {
    pub fn next_at(&self, url: &str) -> i64 {
        self.cooldown_until
            .max(self.entries.get(url).map_or(0, |entry| entry.next_at))
    }

    pub fn watch_due(
        &mut self,
        prs: &[crate::issues::TrackedPullRequest],
        requested: &std::collections::BTreeSet<String>,
        now: i64,
    ) -> Vec<String> {
        let mut due = self.due(prs, now);
        if now < self.cooldown_until {
            return due;
        }
        // Manual requests join the same bounded queue and cannot bypass quota.
        for pr in prs {
            if requested.contains(pr.url.trim_end_matches('/')) && !due.contains(&pr.url) {
                due.push(pr.url.clone());
            }
        }
        due.sort_by_key(|url| {
            (
                !requested.contains(url.trim_end_matches('/')),
                self.entries.get(url).map_or(0, |entry| entry.attempted_at),
                url.clone(),
            )
        });
        due.truncate(20);
        due
    }

    pub fn watch_success(&mut self, url: &str, now: i64) {
        self.entries.insert(
            url.into(),
            Entry {
                attempted_at: now,
                next_at: now + 30_000,
                failures: 0,
            },
        );
    }

    pub fn defer_details(&mut self, url: &str, now: i64) {
        self.entries.insert(
            url.into(),
            Entry {
                attempted_at: 1,
                next_at: now,
                failures: 0,
            },
        );
    }
    pub fn due(&mut self, prs: &[crate::issues::TrackedPullRequest], now: i64) -> Vec<String> {
        self.select(prs, now, 20, 300_000)
    }

    pub fn lifecycle_due(
        &mut self,
        prs: &[crate::issues::TrackedPullRequest],
        now: i64,
    ) -> Vec<String> {
        self.select(prs, now, 200, 60_000)
    }

    pub fn lifecycle_started(&mut self, url: &str, now: i64) {
        let entry = self.entries.entry(url.into()).or_default();
        entry.attempted_at = now;
        entry.next_at = now.saturating_add(30_000);
    }

    pub fn lifecycle_success(&mut self, url: &str, now: i64, validated_at: i64, closed: bool) {
        self.entries.insert(
            url.into(),
            Entry {
                attempted_at: now,
                next_at: validated_at
                    .saturating_add(if closed { 1_800_000 } else { 60_000 })
                    .max(now),
                failures: 0,
            },
        );
    }

    fn select(
        &mut self,
        prs: &[crate::issues::TrackedPullRequest],
        now: i64,
        limit: usize,
        open_interval: i64,
    ) -> Vec<String> {
        let active = prs
            .iter()
            .map(|pr| pr.url.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        self.entries.retain(|url, _| active.contains(url.as_str()));
        if now < self.cooldown_until {
            return Vec::new();
        }
        let mut due = prs
            .iter()
            .filter_map(|pr| {
                let entry = self.entries.get(&pr.url);
                let next = entry
                    .map(|e| {
                        if !pr.closed && e.failures == 0 {
                            // A cadence change can shorten successful idle intervals,
                            // never a failed read's backoff or the shared cooldown.
                            e.next_at.min(e.attempted_at.saturating_add(open_interval))
                        } else {
                            e.next_at
                        }
                    })
                    .unwrap_or_else(|| {
                        pr.checked_at.map_or(0, |at| {
                            at.saturating_add(if pr.closed { 1_800_000 } else { open_interval })
                        })
                    });
                (now >= next).then_some((
                    entry.map(|e| e.attempted_at).or(pr.checked_at).unwrap_or(0),
                    pr.url.clone(),
                ))
            })
            .collect::<Vec<_>>();
        due.sort();
        due.into_iter().take(limit).map(|(_, url)| url).collect()
    }

    pub fn failure(&mut self, url: &str, now: i64, error: &hey_gh::Error) {
        let entry = self.entries.entry(url.into()).or_default();
        entry.failures = entry.failures.saturating_add(1);
        let delay = match error {
            hey_gh::Error::RateLimited {
                retry_after_seconds,
            } => {
                let delay = i64::try_from(*retry_after_seconds)
                    .unwrap_or(i64::MAX)
                    .saturating_mul(1000)
                    .max(1000);
                self.cooldown_until = now.saturating_add(delay);
                delay
            }
            hey_gh::Error::GitHub {
                status: 403 | 404, ..
            } => 1_800_000,
            hey_gh::Error::Deadline | hey_gh::Error::QueueFull | hey_gh::Error::Invalid(_) => {
                15_000 * (1_i64 << entry.failures.min(2))
            }
            _ => 60_000 * (1_i64 << entry.failures.min(5)),
        };
        entry.attempted_at = now;
        entry.next_at = now.saturating_add(delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::issues::TrackedPullRequest;

    #[test]
    fn lifecycle_sweeps_are_bounded_and_cancelled_admissions_rotate_after_restart() {
        let mut schedule = Schedule::default();
        let prs = (0..250)
            .map(|n| pr(&format!("pr-{n:03}"), None, false))
            .collect::<Vec<_>>();
        let first = schedule.lifecycle_due(&prs, 1000);
        assert_eq!(first.len(), 200);
        for url in first {
            schedule.lifecycle_started(&url, 1000);
        }
        let mut restored: Schedule =
            serde_json::from_slice(&serde_json::to_vec(&schedule).unwrap()).unwrap();
        let next = restored.lifecycle_due(&prs, 1001);
        assert_eq!(next.len(), 50);
        assert_eq!(next[0], "pr-200");
        assert_eq!(restored.lifecycle_due(&prs, 31_000)[0], "pr-200");
        restored.failure(
            "pr-200",
            1001,
            &hey_gh::Error::RateLimited {
                retry_after_seconds: 600,
            },
        );
        assert!(restored.lifecycle_due(&prs, 500_000).is_empty());
    }

    #[test]
    fn shorter_lifecycle_cadence_preserves_failed_and_closed_backoff() {
        let mut schedule: Schedule =
            serde_json::from_value(serde_json::json!({"cooldown_until":0,"entries":{
                "open":{"attempted_at":1000,"next_at":301000,"failures":0},
                "failed":{"attempted_at":1000,"next_at":301000,"failures":1},
                "closed":{"attempted_at":1000,"next_at":1801000,"failures":0}
            }}))
            .unwrap();
        let prs = [
            pr("open", Some(1000), false),
            pr("failed", Some(1000), false),
            pr("closed", Some(1000), true),
        ];
        assert_eq!(schedule.lifecycle_due(&prs, 61_000), vec!["open"]);
        schedule.cooldown_until = 100_000;
        assert!(schedule.lifecycle_due(&prs, 99_999).is_empty());
    }

    #[test]
    fn manual_fetch_bypasses_local_backoff_but_obeys_quota_and_batch_limit() {
        let mut schedule = Schedule::default();
        let prs = (0..25)
            .map(|n| pr(&format!("pr-{n}"), None, false))
            .collect::<Vec<_>>();
        for pr in &prs {
            schedule.watch_success(&pr.url, 1000);
        }
        let requested = prs.iter().map(|pr| pr.url.clone()).collect();
        assert!(schedule.due(&prs, 2000).is_empty());
        assert_eq!(schedule.watch_due(&prs, &requested, 2000).len(), 20);
        schedule.cooldown_until = 3000;
        assert!(schedule.watch_due(&prs, &requested, 2000).is_empty());
        assert_eq!(schedule.next_at("pr-0"), 31000);
    }

    fn pr(url: &str, checked_at: Option<i64>, closed: bool) -> TrackedPullRequest {
        TrackedPullRequest {
            url: url.into(),
            checked_at,
            closed,
        }
    }

    #[test]
    fn bounded_fair_queue_prioritizes_unchecked_and_skips_fresh_or_closed_prs() {
        let mut schedule = Schedule::default();
        let now = 10_000_000;
        let mut prs = (0..30)
            .map(|n| pr(&format!("old-{n:02}"), Some(1), false))
            .collect::<Vec<_>>();
        prs.push(pr("new", None, false));
        prs.push(pr("fresh", Some(now - 60_000), false));
        prs.push(pr("closed", Some(now - 600_000), true));
        let due = schedule.due(&prs, now);
        assert_eq!(due.len(), 20);
        assert_eq!(due[0], "new");
        assert!(!due.iter().any(|u| u == "fresh" || u == "closed"));
        for url in &due {
            schedule.lifecycle_success(url, now, now, false);
        }
        let next = schedule.due(&prs, now + 30_000);
        assert_eq!(next.len(), 11);
        assert!(next.iter().all(|url| !due.contains(url)));
        assert!(
            schedule
                .due(&[pr("new", None, false)], now + 300_000)
                .contains(&"new".into())
        );
    }

    #[test]
    fn cached_reads_do_not_extend_the_validation_interval() {
        let mut schedule = Schedule::default();
        let now = 10_000_000;
        let validated_at = now - 40_000;
        schedule.lifecycle_success("cached", now, validated_at, false);
        let prs = [pr("cached", Some(validated_at), false)];
        assert!(schedule.due(&prs, now + 19_999).is_empty());
        assert_eq!(schedule.due(&prs, now + 20_000), vec!["cached"]);
    }

    #[test]
    fn cooldown_and_failure_backoff_survive_restart_without_starving_other_prs() {
        let mut schedule = Schedule::default();
        let prs = vec![pr("a", None, false), pr("b", None, false)];
        schedule.failure(
            "a",
            1_000,
            &hey_gh::Error::RateLimited {
                retry_after_seconds: 600,
            },
        );
        let mut restored: Schedule =
            serde_json::from_slice(&serde_json::to_vec(&schedule).unwrap()).unwrap();
        assert!(restored.due(&prs, 600_999).is_empty());
        assert_eq!(restored.due(&prs, 601_000)[0], "b");
        restored.failure(
            "a",
            601_000,
            &hey_gh::Error::GitHub {
                status: 404,
                message: "missing".into(),
            },
        );
        assert_eq!(restored.due(&prs, 661_000), vec!["b"]);
        assert!(restored.due(&prs, 2_401_000).contains(&"a".into()));
    }

    #[test]
    fn deadline_and_stale_validation_errors_retry_quickly_instead_of_stalling_for_half_an_hour() {
        let mut schedule = Schedule::default();
        let prs = vec![pr("a", None, false)];
        schedule.failure("a", 1_000, &hey_gh::Error::Deadline);
        assert!(schedule.due(&prs, 30_999).is_empty());
        assert_eq!(schedule.due(&prs, 31_000), vec!["a"]);
        schedule.failure(
            "a",
            31_000,
            &hey_gh::Error::Invalid("GitHub CI evidence is stale".into()),
        );
        assert!(schedule.due(&prs, 90_999).is_empty());
        assert_eq!(schedule.due(&prs, 91_000), vec!["a"]);
    }
}
