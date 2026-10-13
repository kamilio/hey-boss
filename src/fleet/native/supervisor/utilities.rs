//! Short control requests launch/poll ephemeral runs without blocking fleet sync.
use super::*;

// Control frames and keepalives must leave even while the synchronization
// consumer is preparing a pull or waiting for its database writer.
pub(super) fn forward_controls<W: Write + Send + 'static>(
    output: Arc<Mutex<W>>,
    requests: mpsc::Receiver<Value>,
    failed: mpsc::SyncSender<Result<Option<Value>>>,
    wake: std::thread::Thread,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        loop {
            let message = match requests.recv_timeout(Duration::from_secs(5)) {
                Ok(message) => message,
                Err(mpsc::RecvTimeoutError::Timeout) => json!({"kind":"ping"}),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if let Err(error) = send(&mut *output.lock().unwrap(), message) {
                let _ = failed.try_send(Err(error));
                wake.unpark();
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_and_keepalives_leave_without_a_replication_consumer() {
        let (writer, reader) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(7)))
            .unwrap();
        let (requests, queued) = mpsc::sync_channel(2);
        let (failed, failures) = mpsc::sync_channel(2);
        let forwarding = forward_controls(
            Arc::new(Mutex::new(writer)),
            queued,
            failed,
            std::thread::current(),
        );
        let mut reader = BufReader::new(reader);
        requests
            .send(json!({"kind":"utility","id":"command"}))
            .unwrap();
        assert_eq!(read_frame(&mut reader).unwrap().unwrap()["id"], "command");
        // No synchronization loop runs and no further control traffic is sent.
        assert_eq!(read_frame(&mut reader).unwrap().unwrap()["kind"], "ping");
        drop(requests);
        forwarding.join().unwrap();
        assert!(failures.try_recv().is_err());
    }
}

pub(super) struct Run {
    started: Instant,
    host: String,
    name: String,
    transport: Option<u32>,
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
                if result["ok"] == false {
                    return Err(invalid(
                        result["error"].as_str().unwrap_or("Utility failed"),
                    ));
                }
                return Ok(result);
            }
            if let Some(transport) = run.transport
                && self
                    .state
                    .lock()
                    .unwrap()
                    .connections
                    .get(&run.host)
                    .map(|(pid, _)| *pid)
                    != Some(transport)
            {
                let error = invalid(&format!(
                    "Lost the fleet connection to '{}' after sending utility '{}'; its outcome is unknown. Check the command's effects on '{}' before rerunning it. Execution was not retried.",
                    run.host, run.name, run.host
                ));
                runs.remove(id);
                return Err(error);
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
        let mut utility = config["document"]["utils"][name].clone();
        // Installation policy stays on the supervisor; older CLIs and peers
        // deserialize the execution definition with deny_unknown_fields.
        if let Some(utility) = utility.as_object_mut() {
            utility.remove("global");
        }
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
        let mut transport = None;
        if host != "local" {
            let state = self.state.lock().unwrap();
            let disconnected = || {
                let cause = state
                    .machines
                    .get(host)
                    .and_then(|m| m["error"].as_str())
                    .map(|error| format!("\nLast connection error: {error}"))
                    .unwrap_or_default();
                let error = format!(
                    "Cannot run utility '{name}': '{host}' is disconnected from the internal fleet channel.{cause}\nNothing was sent; it is safe to retry. Reconnection is automatic.\nRecovery: inspect `hey-boss fleet status`, or run `hey-boss fleet setup` on the supervisor to restart the connection, then rerun the utility."
                );
                if request["wait_for_connection"] == true {
                    Ok(json!({"ok":true,"waiting":true,"host":host,"error":error}))
                } else {
                    Err(invalid(&error))
                }
            };
            let Some((pid, outgoing)) = state.connections.get(host) else {
                return disconnected();
            };
            if state
                .machines
                .get(host)
                .is_none_or(|m| m["utils_v1"] != true)
            {
                return Err(invalid(
                    "Upgrade the utility destination with hey-boss upgrade",
                ));
            }
            match outgoing.try_send(message.clone()) {
                Ok(()) => transport = Some(*pid),
                Err(mpsc::TrySendError::Disconnected(_)) => return disconnected(),
                Err(mpsc::TrySendError::Full(_)) => {
                    return Err(invalid(&format!(
                        "Utility '{name}' could not reach '{host}': its fleet channel is busy. Nothing was sent; wait a moment and rerun the utility."
                    )));
                }
            }
        }
        runs.insert(
            id.clone(),
            Run {
                started: Instant::now(),
                host: host.into(),
                name: name.into(),
                transport,
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
