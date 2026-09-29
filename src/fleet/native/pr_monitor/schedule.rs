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
    pub fn due(&mut self, prs: &[crate::issues::TrackedPullRequest], now: i64) -> Vec<String> {
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
                let next = entry.map(|e| e.next_at).unwrap_or_else(|| {
                    pr.checked_at
                        .map_or(0, |at| at.saturating_add(interval(pr.closed)))
                });
                (now >= next).then_some((
                    entry.map(|e| e.attempted_at).or(pr.checked_at).unwrap_or(0),
                    pr.url.clone(),
                ))
            })
            .collect::<Vec<_>>();
        due.sort();
        due.into_iter().take(20).map(|(_, url)| url).collect()
    }

    pub fn success(&mut self, url: &str, now: i64, validated_at: i64, closed: bool) {
        self.entries.insert(
            url.into(),
            Entry {
                attempted_at: now,
                // A shared cache hit is only as fresh as its upstream validation.
                next_at: validated_at.saturating_add(interval(closed)).max(now),
                failures: 0,
            },
        );
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
            _ => 60_000 * (1_i64 << entry.failures.min(5)),
        };
        entry.attempted_at = now;
        entry.next_at = now.saturating_add(delay);
    }
}

fn interval(closed: bool) -> i64 {
    if closed { 1_800_000 } else { 300_000 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::issues::TrackedPullRequest;

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
            schedule.success(url, now, now, false);
        }
        let next = schedule.due(&prs, now + 60_000);
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
        let validated_at = now - 240_000;
        schedule.success("cached", now, validated_at, false);
        let prs = [pr("cached", Some(validated_at), false)];
        assert!(schedule.due(&prs, now + 59_999).is_empty());
        assert_eq!(schedule.due(&prs, now + 60_000), vec!["cached"]);
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
}
