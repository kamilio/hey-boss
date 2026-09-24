//! Capacity and activity use the exact worker set represented by the tab.
use serde_json::Value;

pub(super) fn summary(workers: &[&Value]) -> String {
    let mut active = 0;
    let mut slots = 0;
    let mut available = 0;
    let mut draining = 0;
    for worker in workers {
        let busy = worker["active"].as_u64().unwrap_or(0);
        active += busy;
        if worker["intent"] == "drain" {
            draining += 1;
        } else if !worker["pid"].is_null() && worker["config"]["enabled"] == true {
            let capacity = worker["config"]["concurrency"].as_u64().unwrap_or(0);
            slots += capacity;
            available += capacity.saturating_sub(busy);
        }
    }
    let mut result = format!(
        " {} worker{} · {active} active · {available} available / {slots} slots",
        workers.len(),
        if workers.len() == 1 { "" } else { "s" },
    );
    if draining > 0 {
        result.push_str(&format!(" · {draining} draining"));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reports_all_workers_in_the_view_instead_of_the_selected_workers_capacity() {
        let worker = json!({"pid":42,"active":5,"config":{"enabled":true,"concurrency":5}});
        assert_eq!(
            summary(&[&worker, &worker, &worker]),
            " 3 workers · 15 active · 0 available / 15 slots"
        );
        assert_eq!(
            summary(&[&worker]),
            " 1 worker · 5 active · 0 available / 5 slots"
        );
    }

    #[test]
    fn draining_and_paused_workers_do_not_offer_new_capacity() {
        let running = json!({"pid":42,"active":3,"config":{"enabled":true,"concurrency":5}});
        let draining = json!({"pid":43,"active":5,"intent":"drain","config":{"enabled":false,"concurrency":5}});
        let paused = json!({"pid":44,"active":0,"config":{"enabled":false,"concurrency":5}});
        assert_eq!(
            summary(&[&running, &draining, &paused]),
            " 3 workers · 8 active · 2 available / 5 slots · 1 draining"
        );
    }
}
