//! Short control requests launch/poll ephemeral runs without blocking fleet sync.
use super::*;

pub(super) struct Run {
    started: Instant,
    host: String,
    result: Option<Value>,
}
impl Supervisor {
    pub(super) fn utility_request(&self, request: &Value) -> Result<Value> {
        let mut runs = self.utilities.lock().unwrap();
        runs.retain(|_, run| {
            run.started.elapsed() < crate::utilities::TIMEOUT + Duration::from_secs(60)
        });
        if request["kind"] == "utils_poll" {
            let id = request["id"]
                .as_str()
                .ok_or_else(|| invalid("Missing utility run ID"))?;
            let run = runs
                .get(id)
                .ok_or_else(|| invalid("Utility result unavailable; execution was not retried"))?;
            if let Some(result) = &run.result {
                let result = result.clone();
                runs.remove(id);
                return Ok(result);
            }
            return Ok(json!({"ok":true,"done":false}));
        }
        drop(runs);
        let name = request["name"]
            .as_str()
            .ok_or_else(|| invalid("Missing utility name"))?;
        let config = configuration::load(&self.ctx)?;
        if let Some(error) = config["error"].as_str() {
            return Err(invalid(error));
        }
        let utility = &config["document"]["utils"][name];
        if request["kind"] == "utils_resolve" {
            return Ok(json!({"ok":true,"utility":utility}));
        }
        let definition: crate::utilities::Definition = serde_json::from_value(utility.clone())
            .map_err(|_| invalid("Unknown utility; add it to utils in fleet.yaml"))?;
        definition.validate()?;
        crate::utilities::arguments(request)?;
        crate::utilities::decode(&request["stdin"])?;
        let host = definition
            .destination
            .as_deref()
            .ok_or_else(|| invalid("Utility no longer has a destination; run it again"))?;
        let id = id()?;
        let message = json!({"kind":"utility","id":id,"utility":utility,"args":request["args"],"stdin":request["stdin"]});
        let mut runs = self.utilities.lock().unwrap();
        if runs.len() >= 32 {
            return Err(invalid(
                "Utilities are busy; wait for an active run to finish",
            ));
        }
        if host != "local" {
            let state = self.state.lock().unwrap();
            let (_, outgoing) = state.connections.get(host).ok_or_else(|| {
                invalid("Utility destination is disconnected; nothing was queued")
            })?;
            if state
                .machines
                .get(host)
                .is_none_or(|m| m["utils_v1"] != true)
            {
                return Err(invalid(
                    "Upgrade the utility destination with hey-boss upgrade",
                ));
            }
            outgoing
                .try_send(message.clone())
                .map_err(|_| invalid("Utility destination is busy; nothing was queued"))?;
        }
        runs.insert(
            id.clone(),
            Run {
                started: Instant::now(),
                host: host.into(),
                result: None,
            },
        );
        if host == "local" {
            let runs = self.utilities.clone();
            let run_id = id.clone();
            std::thread::spawn(move || {
                let result = crate::utilities::execute(&message);
                if let Some(run) = runs.lock().unwrap().get_mut(&run_id) {
                    run.result = Some(result);
                }
            });
        }
        Ok(json!({"ok":true,"id":id}))
    }
    pub(super) fn utility_reply(&self, host: &str, message: &Value) {
        if let Some(run) = self
            .utilities
            .lock()
            .unwrap()
            .get_mut(message["id"].as_str().unwrap_or(""))
            && run.host == host
            && run.result.is_none()
        {
            run.result = Some(message["result"].clone());
        }
    }
}
