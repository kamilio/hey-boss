//! Connection-scoped history deltas. A full hello establishes the baseline;
//! ordered, reliable stdio frames advance it or reconnect with a fresh hello.
use super::{Result, replica::invalid};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct History {
    runs: HashMap<String, Vec<Value>>,
}
impl History {
    pub fn receive(&mut self, message: &mut Value, negotiated: bool) -> Result<()> {
        if message["worker_history_v1"] == true {
            if !negotiated {
                return Err(invalid("Worker history capability was not negotiated"));
            }
            self.decode(
                message["workers"]
                    .as_array_mut()
                    .ok_or_else(|| invalid("Missing companion workers"))?,
            )?;
        }
        Ok(())
    }
    pub fn new(workers: &[Value]) -> Self {
        Self {
            runs: workers
                .iter()
                .map(|w| {
                    (
                        w["id"].as_str().unwrap().to_owned(),
                        w["runs"].as_array().unwrap().clone(),
                    )
                })
                .collect(),
        }
    }
    pub fn encode(&mut self, workers: &mut [Value]) {
        self.runs
            .retain(|id, _| workers.iter().any(|w| w["id"] == *id));
        for worker in workers {
            let id = worker["id"].as_str().unwrap().to_owned();
            let Some(runs) = worker.as_object_mut().unwrap().remove("runs") else {
                continue;
            };
            let runs = runs.as_array().unwrap();
            let old = self.runs.entry(id).or_default();
            if old != runs {
                worker["run_ids"] = json!(runs.iter().map(|r| &r["id"]).collect::<Vec<_>>());
                worker["runs"] =
                    json!(runs.iter().filter(|r| !old.contains(r)).collect::<Vec<_>>());
                *old = runs.clone();
            }
        }
    }
    pub fn decode(&mut self, workers: &mut [Value]) -> Result<()> {
        for worker in workers.iter_mut() {
            let id = worker["id"]
                .as_str()
                .ok_or_else(|| invalid("Missing worker ID"))?
                .to_owned();
            let old = self.runs.entry(id).or_default();
            if let Some(ids) = worker
                .as_object_mut()
                .ok_or_else(|| invalid("Invalid worker"))?
                .remove("run_ids")
            {
                let changed = worker["runs"]
                    .as_array()
                    .ok_or_else(|| invalid("Missing history delta"))?;
                let mut next = Vec::new();
                for id in ids
                    .as_array()
                    .ok_or_else(|| invalid("Invalid history order"))?
                {
                    let run = changed
                        .iter()
                        .chain(old.iter())
                        .find(|r| r["id"] == *id)
                        .ok_or_else(|| invalid("History delta has no baseline; reconnect"))?;
                    next.push(run.clone());
                }
                *old = next;
            } else if worker.get("runs").is_some() {
                return Err(invalid("History delta is missing its run order"));
            }
            worker["runs"] = json!(old);
        }
        self.runs
            .retain(|id, _| workers.iter().any(|w| w["id"] == *id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_protocol_requires_negotiation_and_accepts_older_full_snapshots() {
        let baseline = vec![json!({"id":"worker","runs":[{"id":"run","summary":"saved"}]})];
        for negotiated in [false, true] {
            let mut history = History::new(&baseline);
            let mut old = json!({"workers":baseline});
            history.receive(&mut old, negotiated).unwrap();
            assert_eq!(old["workers"], json!(baseline));
            let mut new = json!({"worker_history_v1":true,"workers":[{"id":"worker"}]});
            assert_eq!(history.receive(&mut new, negotiated).is_ok(), negotiated);
            if negotiated {
                assert_eq!(new["workers"], json!(baseline));
            }
        }
    }
    #[test]
    fn history_delta_preserves_quiet_history_and_sends_only_one_changed_run() {
        let baseline = (0..40).map(|w|json!({"id":format!("w{w}"),"active":0,"runs":(0..20).map(|r|json!({"id":format!("r{r}"),"summary":"x".repeat(4096)})).collect::<Vec<_>>()})).collect::<Vec<_>>();
        let mut sender = History::new(&baseline);
        let mut receiver = History::new(&baseline);
        let mut quiet = baseline.clone();
        sender.encode(&mut quiet);
        assert!(serde_json::to_vec(&quiet).unwrap().len() < 2_000);
        receiver.decode(&mut quiet).unwrap();
        assert_eq!(quiet, baseline);
        let mut changed = baseline.clone();
        changed[0]["runs"][0]["summary"] = json!("updated");
        let expected = changed.clone();
        sender.encode(&mut changed);
        assert_eq!(changed[0]["runs"].as_array().unwrap().len(), 1);
        assert!(serde_json::to_vec(&changed).unwrap().len() < 3_000);
        receiver.decode(&mut changed).unwrap();
        assert_eq!(changed, expected);
        let mut pruned = expected.clone();
        pruned[0]["runs"].as_array_mut().unwrap().remove(1);
        pruned.remove(1);
        let expected = pruned.clone();
        sender.encode(&mut pruned);
        assert!(pruned[0]["runs"].as_array().unwrap().is_empty());
        receiver.decode(&mut pruned).unwrap();
        assert_eq!(pruned, expected);
        // A new connection starts with the full diagnostic snapshot again.
        let mut reconnect = History::new(&expected);
        let mut quiet = expected.clone();
        History::new(&expected).encode(&mut quiet);
        reconnect.decode(&mut quiet).unwrap();
        assert_eq!(quiet, expected);
        assert!(
            History::default()
                .decode(&mut [json!({"id":"w","run_ids":["unknown"],"runs":[]})])
                .is_err()
        );
    }
}
