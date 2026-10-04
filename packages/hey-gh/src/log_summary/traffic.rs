use super::{allowed, field, increment, request_id};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

type ResponseCounts = BTreeMap<String, BTreeMap<String, BTreeMap<String, BTreeMap<String, usize>>>>;
type AttemptKey = (String, u64);

struct Headers {
    at: u64,
    status: u16,
    remaining: Option<u64>,
    used: Option<u64>,
    reset: Option<u64>,
}

#[derive(Default, Serialize)]
pub struct Traffic {
    pub earliest_retained_dispatch_at_ms: Option<u64>,
    pub dispatched_attempts: usize,
    pub response_headers: usize,
    pub responses_without_retained_dispatch: usize,
    pub dispatches_by_endpoint: BTreeMap<String, usize>,
    pub dispatches_by_priority: BTreeMap<String, usize>,
    /// endpoint -> priority -> conditional/unconditional -> status -> count.
    pub responses: ResponseCounts,
    pub quota_windows: Vec<QuotaWindow>,
    pub completed_jobs_cache_decisions: BTreeMap<String, usize>,
}

#[derive(Serialize)]
pub struct QuotaWindow {
    pub auth_scope: String,
    pub instance: String,
    pub resource: String,
    pub reset: u64,
    pub samples: usize,
    pub first_at_ms: u64,
    pub last_at_ms: u64,
    pub minimum_remaining: u64,
    pub maximum_remaining: u64,
    pub minimum_used: Option<u64>,
    pub maximum_used: Option<u64>,
}

struct Dispatch {
    at: u64,
    endpoint: String,
    priority: String,
    conditional: String,
    scope: String,
    instance: String,
    resource: String,
}

#[derive(Default)]
pub(super) struct Records {
    start: u64,
    dispatches: BTreeMap<AttemptKey, Dispatch>,
    headers: BTreeMap<AttemptKey, Headers>,
    cache: BTreeSet<(u64, String)>,
}

fn opaque(line: &str, key: &str, len: usize) -> String {
    field(line, key)
        .filter(|v| v.len() == len && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or("unknown")
        .to_owned()
}

impl Records {
    pub fn observe(&mut self, line: &str, at: u64, start: u64) {
        self.start = start;
        if line.contains("GitHub derived cache decision") && at >= start {
            self.cache.insert((at, line.to_owned()));
        }
        let number = |name| field(line, name).and_then(|v| v.parse::<u64>().ok());
        let Some((id, attempt)) = request_id(line).zip(number("attempt")) else {
            return;
        };
        let key = (id.to_owned(), attempt);
        if line.contains("GitHub request dispatched") {
            self.dispatches.insert(
                key,
                Dispatch {
                    at,
                    endpoint: super::endpoint(line),
                    priority: match field(line, "foreground") {
                        Some("true") => "foreground",
                        Some("false") => "background",
                        _ => "unknown",
                    }
                    .into(),
                    conditional: match field(line, "conditional") {
                        Some("true") => "conditional",
                        Some("false") => "unconditional",
                        _ => "unknown",
                    }
                    .into(),
                    scope: opaque(line, "auth_scope", 64),
                    instance: opaque(line, "instance", 32),
                    resource: allowed(
                        field(line, "resource"),
                        &["core", "search", "graphql", "app_auth"],
                    ),
                },
            );
        } else if line.contains("GitHub response headers")
            && at >= start
            && let Some(status) = number("http_status").filter(|n| (100..=599).contains(n))
        {
            self.headers.insert(
                key,
                Headers {
                    at,
                    status: status as u16,
                    remaining: number("remaining"),
                    used: number("used"),
                    reset: number("reset"),
                },
            );
        }
    }

    pub fn summarize(self) -> Traffic {
        let mut result = Traffic {
            earliest_retained_dispatch_at_ms: self.dispatches.values().map(|d| d.at).min(),
            ..Traffic::default()
        };
        for d in self.dispatches.values().filter(|d| d.at >= self.start) {
            result.dispatched_attempts += 1;
            increment(&mut result.dispatches_by_endpoint, d.endpoint.clone());
            increment(&mut result.dispatches_by_priority, d.priority.clone());
        }
        let mut windows = BTreeMap::<(String, String, String, u64), QuotaWindow>::new();
        for (
            key,
            Headers {
                at,
                status,
                remaining,
                used,
                reset,
            },
        ) in self.headers
        {
            result.response_headers += 1;
            let Some(d) = self.dispatches.get(&key) else {
                result.responses_without_retained_dispatch += 1;
                continue;
            };
            increment(
                result
                    .responses
                    .entry(d.endpoint.clone())
                    .or_default()
                    .entry(d.priority.clone())
                    .or_default()
                    .entry(d.conditional.clone())
                    .or_default(),
                status.to_string(),
            );
            if let Some((remaining, reset)) = remaining.zip(reset) {
                let w = windows
                    .entry((
                        d.scope.clone(),
                        d.instance.clone(),
                        d.resource.clone(),
                        reset,
                    ))
                    .or_insert_with(|| QuotaWindow {
                        auth_scope: d.scope.clone(),
                        instance: d.instance.clone(),
                        resource: d.resource.clone(),
                        reset,
                        samples: 0,
                        first_at_ms: at,
                        last_at_ms: at,
                        minimum_remaining: remaining,
                        maximum_remaining: remaining,
                        minimum_used: used,
                        maximum_used: used,
                    });
                w.samples += 1;
                w.first_at_ms = w.first_at_ms.min(at);
                w.last_at_ms = w.last_at_ms.max(at);
                w.minimum_remaining = w.minimum_remaining.min(remaining);
                w.maximum_remaining = w.maximum_remaining.max(remaining);
                if let Some(used) = used {
                    w.minimum_used = Some(w.minimum_used.map_or(used, |old| old.min(used)));
                    w.maximum_used = Some(w.maximum_used.map_or(used, |old| old.max(used)));
                }
            }
        }
        result.quota_windows = windows.into_values().collect();
        for (_, line) in self.cache {
            increment(
                &mut result.completed_jobs_cache_decisions,
                allowed(
                    field(&line, "outcome"),
                    &["hit", "miss", "refresh", "in_progress"],
                ),
            );
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_attempts_not_jobs_and_correlates_across_window_boundary() {
        let mut r = Records::default();
        let id = "a".repeat(32);
        let dispatch = format!(
            "GitHub request dispatched request_id={id} attempt=1 endpoint=check_runs foreground=false conditional=true auth_scope={} instance={} resource=core",
            "b".repeat(64),
            "c".repeat(32)
        );
        let response = format!(
            "GitHub response headers request_id={id} attempt=1 http_status=304 remaining=800 used=4200 reset=100"
        );
        r.observe(&dispatch, 9, 10);
        r.observe(&response, 11, 10);
        r.observe(&response, 11, 10); // rotation duplicate
        r.observe(&dispatch.replace("attempt=1", "attempt=2"), 12, 10);
        r.observe(
            &response
                .replace("attempt=1", "attempt=2")
                .replace("304", "200")
                .replace("800", "798")
                .replace("4200", "4202"),
            13,
            10,
        );
        let s = r.summarize();
        assert_eq!(s.dispatched_attempts, 1);
        assert_eq!(s.response_headers, 2);
        assert_eq!(s.responses_without_retained_dispatch, 0);
        assert_eq!(
            s.responses["check_runs"]["background"]["conditional"]["304"],
            1
        );
        assert_eq!(s.quota_windows[0].minimum_remaining, 798);
        assert_eq!(s.quota_windows[0].maximum_used, Some(4202));
    }

    #[test]
    fn rejects_private_labels_and_separates_reset_windows() {
        let mut r = Records::default();
        for (attempt, reset) in [(1, 100), (2, 200)] {
            r.observe(&format!("GitHub request dispatched request_id={} attempt={attempt} endpoint=private-path auth_scope=private-token instance=private-host resource=private-resource foreground=secret conditional=secret", "a".repeat(32)),10,10);
            r.observe(&format!("GitHub response headers request_id={} attempt={attempt} http_status=200 remaining=5000 reset={reset}", "a".repeat(32)),11,10);
        }
        let s = r.summarize();
        assert_eq!(s.quota_windows.len(), 2);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("private"));
        assert!(!json.contains("secret"));
    }
}
