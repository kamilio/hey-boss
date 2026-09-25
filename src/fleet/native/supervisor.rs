use super::{
    Result, authority,
    context::{Context, encode_frame, hash, id, now, read_frame, send},
    control, conversation, pull,
    replica::{self, invalid},
    takeover,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};

struct State {
    machines: BTreeMap<String, Value>,
    local: Vec<Value>,
    local_updated: f64,
    chief_ownership: Vec<crate::chief_ownership::Assignment>,
    events: VecDeque<Value>,
    sequence: u64,
    epoch: String,
    build: String,
    desired_build: Value,
    connections: BTreeMap<String, (u32, SyncSender<Value>)>,
    waiters: BTreeMap<String, SyncSender<Value>>,
    deploying: bool,
    machines_dirty: bool,
}
pub(super) struct Supervisor {
    pub ctx: Context,
    state: Mutex<State>,
    // Serialize disk snapshots without serializing in-memory peer progress.
    persistence: Mutex<()>,
    // Configuration and signal ordering must not hold the liveness mutex.
    configuration: Mutex<()>,
}
impl Supervisor {
    pub fn new(ctx: Context) -> Result<Arc<Self>> {
        ctx.rpc(json!({"action":"whoami"}))?;
        let db = ctx.db()?;
        replica::install_capture(&db, "controller", &ctx.node)?;
        let saved = replica::state_get(&db, "machines", json!({}))?;
        let mut machines: BTreeMap<String, Value> = serde_json::from_value(saved)?;
        for m in machines.values_mut() {
            m["state"] = json!("disconnected");
            if m["deployment"] == "updating" {
                m["deployment"] = json!("outdated");
            }
        }
        let chief_ownership = crate::chief_ownership::read(&db)?;
        let local = ctx.workers()?;
        if !ctx.state.join("fleet-main.json").exists() {
            ctx.atomic_json(
                &ctx.state.join("fleet-main.json"),
                &json!({"role":"controller","workers":definitions(&local)}),
            )?;
        }
        let build = Context::running_build().to_owned();
        Ok(Arc::new(Self {
            ctx,
            persistence: Mutex::new(()),
            configuration: Mutex::new(()),
            state: Mutex::new(State {
                machines,
                local,
                local_updated: now(),
                chief_ownership,
                events: VecDeque::new(),
                sequence: 0,
                epoch: id()?,
                build,
                desired_build: Value::Null,
                connections: BTreeMap::new(),
                waiters: BTreeMap::new(),
                deploying: false,
                machines_dirty: false,
            }),
        }))
    }
    fn event(&self, host: &str, kind: &str, detail: &str) {
        let mut state = self.state.lock().unwrap();
        state.sequence += 1;
        let event = json!({"id":state.sequence,"epoch":state.epoch,"at":now(),"host":host,"kind":kind,"detail":detail});
        state.events.push_back(event);
        while state.events.len() > 200 {
            state.events.pop_front();
        }
    }
    fn update(&self, host: &str, fields: Value) -> Result<()> {
        let fields = fields
            .as_object()
            .ok_or_else(|| invalid("Invalid fleet state update"))?;
        {
            let mut state = self.state.lock().unwrap();
            let m = state
                .machines
                .entry(host.into())
                .or_insert_with(|| json!({"host":host}));
            let changed = fields.iter().any(|(key, value)| {
                !matches!(key.as_str(), "heartbeat" | "last_sync") && m.get(key) != Some(value)
            });
            m.as_object_mut().unwrap().extend(fields.clone());
            state.machines_dirty |= changed;
            // Liveness and sync progress are current in memory. Substantive
            // changes save both latest timestamps with the durable snapshot.
        }
        // Pure liveness never waits for SQLite, including after a failed save.
        if fields
            .keys()
            .any(|key| !matches!(key.as_str(), "heartbeat" | "last_sync"))
        {
            self.save_machines()?;
        }
        Ok(())
    }
    fn save_machines(&self) -> Result<()> {
        let Ok(_saving) = self.persistence.try_lock() else {
            return Ok(());
        };
        let machines = {
            let mut state = self.state.lock().unwrap();
            if !state.machines_dirty {
                return Ok(());
            }
            // New substantive updates during this write remain dirty. Only the
            // serialized writer can clear the dirty bit before taking a snapshot.
            state.machines_dirty = false;
            json!(state.machines)
        };
        let result = (|| replica::state_set(&self.ctx.db()?, "machines", &machines))();
        if result.is_err() {
            self.state.lock().unwrap().machines_dirty = true;
        }
        result
    }
    fn machine(&self, host: &str) -> Value {
        self.state
            .lock()
            .unwrap()
            .machines
            .get(host)
            .cloned()
            .unwrap_or(json!({}))
    }
    pub fn status(&self) -> Result<Value> {
        self.snapshot(None)
    }
    pub fn overview(&self) -> Result<Value> {
        let visible = replica::rows(
            &self.ctx.db()?,
            "SELECT id FROM projects WHERE hidden_at IS NULL",
            &[],
        )?
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect::<BTreeSet<_>>();
        self.snapshot(Some(&visible))
    }
    // A visible-project scope selects the compact projection. Project before
    // cloning: discarded event arrays must never enter the response allocation.
    fn snapshot(&self, visible: Option<&BTreeSet<String>>) -> Result<Value> {
        let db = self.ctx.db()?;
        let signals = replica::rows(
            &db,
            "SELECT * FROM fleet_signals ORDER BY created_at DESC LIMIT 100",
            &[],
        )?;
        let conflicts = replica::rows(
            &db,
            if visible.is_some() {
                "SELECT id,node,seq,table_name,reason,created_at FROM fleet_conflicts WHERE resolved=0 ORDER BY created_at DESC LIMIT 100"
            } else {
                "SELECT id,node,seq,table_name,reason,created_at,substr(data,1,8192) AS saved_change FROM fleet_conflicts WHERE resolved=0 ORDER BY created_at DESC LIMIT 100"
            },
            &[],
        )?;
        let state = self.state.lock().unwrap();
        let mut local = json!({"host":"local","hostname":crate::issues::identity::host(),"node":self.ctx.node,"role":"supervisor","state":"connected","heartbeat":state.local_updated,"pending":0,"build":state.build});
        local["workers"] = Value::Array(match visible {
            Some(projects) => state
                .local
                .iter()
                .map(|w| overview_worker(w, projects))
                .collect(),
            None => state.local.clone(),
        });
        let mut machines = vec![local];
        for (host, machine) in &state.machines {
            if host == "local" {
                continue;
            }
            let mut machine = match visible {
                Some(projects) => {
                    let mut projected = Value::Object(
                        machine
                            .as_object()
                            .into_iter()
                            .flatten()
                            .filter(|(key, _)| key.as_str() != "workers")
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect(),
                    );
                    if let Some(workers) = machine["workers"].as_array() {
                        projected["workers"] = Value::Array(
                            workers
                                .iter()
                                .map(|w| overview_worker(w, projects))
                                .collect(),
                        );
                    } else if let Some(workers) = machine.get("workers") {
                        projected["workers"] = workers.clone();
                    }
                    projected
                }
                None => machine.clone(),
            };
            if machine["role"] == "agent" {
                machine["role"] = json!("companion");
            }
            machines.push(machine);
        }
        let mut result = json!({"ok":true,"supervisor":self.ctx.node,"controller":self.ctx.node,"epoch":state.epoch,"sequence":state.sequence,"desired_build":state.desired_build});
        result["machines"] = Value::Array(machines);
        result["events"] = Value::Array(if visible.is_some() {
            vec![]
        } else {
            state.events.iter().cloned().collect()
        });
        result["signals"] = Value::Array(signals);
        result["conflicts"] = Value::Array(conflicts);
        Ok(result)
    }
    fn conversation(&self, request: &Value) -> Result<Value> {
        let taking_over = request["kind"] == "takeover";
        let steering = request["kind"] == "steer";
        let mutation = taking_over || steering;
        let host = request["host"]
            .as_str()
            .ok_or_else(|| invalid("Missing device"))?;
        let run_id = request["run"]
            .as_str()
            .ok_or_else(|| invalid("Missing agent"))?;
        let window: crate::agent_conversations::Window = serde_json::from_value(request.clone())?;
        let cursor = serde_json::to_value(&window)?;
        let status = self.status()?;
        let machine = status["machines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["host"] == host || (!mutation && m["hostname"] == host))
            .ok_or_else(|| invalid("This device is no longer available"))?;
        let run = machine["workers"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|w| {
                ["runs", "chiefs"]
                    .into_iter()
                    .flat_map(move |key| w[key].as_array().into_iter().flatten())
            })
            .find(|r| r["id"] == run_id)
            .cloned();
        let run = if let Some(run) = run {
            run
        } else if !mutation {
            let db = self.ctx.db()?;
            let origin = crate::issues::provenance::referenced(&db, host, run_id)?
                .ok_or_else(|| invalid("This agent is no longer available"))?;
            if run_id.starts_with("session:") {
                crate::issues::provenance::saved_run(&db, run_id)?
                    .ok_or_else(|| invalid("This conversation is no longer available"))?
            } else {
                origin["run"].clone()
            }
        } else {
            return Err(invalid("This agent is no longer available"));
        };
        if mutation && run["kind"] == "chief" {
            return Err(invalid("Chief conversations are read-only"));
        }
        let host = machine["host"]
            .as_str()
            .ok_or_else(|| invalid("Missing device"))?;
        let project = replica::rows(
            &self.ctx.db()?,
            "SELECT hidden_at FROM projects WHERE id=?",
            &[run["project_id"].clone()],
        )?;
        if !project.first().is_some_and(|p| p["hidden_at"].is_null()) {
            return Err(invalid("This project is hidden or no longer available"));
        }
        if host == "local" {
            if steering {
                return takeover::steer(&self.ctx, request);
            }
            if taking_over {
                return takeover::apply(&self.ctx, run_id);
            }
            let mut result = conversation::page(&self.ctx, run_id, &cursor)?;
            crate::issues::provenance::enrich_conversation(&self.ctx.db()?, run_id, &mut result)?;
            return Ok(result);
        }
        let identifier = id()?;
        let (tx, rx) = mpsc::sync_channel(1);
        {
            let mut state = self.state.lock().unwrap();
            if state.waiters.len() >= 32 {
                return Err(invalid("Conversations are busy. Try again in a moment."));
            }
            let (_, outgoing) = state.connections.get(host).ok_or_else(|| {
                invalid("This device is disconnected. Reconnect to load its conversation.")
            })?;
            if machine["state"] != "connected" {
                return Err(invalid(
                    "This device is disconnected. Reconnect to load its conversation.",
                ));
            }
            outgoing
                .try_send(
                    json!({"kind":request["kind"],"id":identifier,"run":run_id,"cursor":cursor,"scope":request["scope"],"text":request["text"],"request_id":request["request_id"]}),
                )
                .map_err(|_| invalid("This device is busy. Try again in a moment."))?;
            state.waiters.insert(identifier.clone(), tx);
        }
        let result = rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| invalid("This device did not respond. Try again when it reconnects."));
        self.state.lock().unwrap().waiters.remove(&identifier);
        let mut result = result?;
        if !mutation && result["ok"] == true {
            crate::issues::provenance::enrich_conversation(&self.ctx.db()?, run_id, &mut result)?;
        }
        if taking_over && result["ok"] != false && result["stopped"] == true {
            result["resume_command"] = takeover::command(
                host,
                result["directory"].as_str().unwrap_or(""),
                result["session_id"].as_str().unwrap_or(""),
            )
            .map(|c| json!(c))
            .unwrap_or(Value::Null);
        }
        Ok(result)
    }
    fn signal(&self, request: &Value) -> Result<Value> {
        let host = request["host"]
            .as_str()
            .ok_or_else(|| invalid("Missing machine"))?;
        let worker = request["worker"]
            .as_str()
            .ok_or_else(|| invalid("Missing worker"))?;
        let action = request["signal"]
            .as_str()
            .ok_or_else(|| invalid("Missing signal"))?;
        if host != "local" && !self.ctx.inventory()?.iter().any(|h| h["host"] == host) {
            return Err(invalid("Machine is not in the configured inventory"));
        }
        if !matches!(action, "pause" | "resume" | "stop" | "restart") {
            return Err(invalid("Unknown signal"));
        }
        // Keep controls ordered with config-file changes, independently of liveness.
        let _configuration = self.configuration.lock().unwrap();
        let identifier = request["id"].as_str().map(str::to_owned).unwrap_or(id()?);
        let db = self.ctx.db()?;
        let old = replica::rows(
            &db,
            "SELECT * FROM fleet_signals WHERE id=?",
            &[json!(identifier)],
        )?;
        if let Some(old) = old.first() {
            if old["host"] != host || old["worker"] != worker || old["signal"] != action {
                return Err(invalid("Signal ID already has a different payload"));
            }
            return Ok(json!({"ok":true,"id":identifier,"state":old["state"]}));
        }
        replica::execute(
            &db,
            "INSERT INTO fleet_signals VALUES(?,?,?,?,'pending',NULL,?)",
            &[
                json!(identifier),
                json!(host),
                json!(worker),
                json!(action),
                json!(now()),
            ],
        )?;
        let mut saved = self.ctx.read_json(&self.ctx.desired, json!({}))?;
        let default = if host == "local" {
            self.ctx
                .read_json(&self.ctx.state.join("fleet-main.json"), json!({}))?["workers"]
                .clone()
        } else {
            self.state
                .lock()
                .unwrap()
                .machines
                .get(host)
                .map(|m| m["desired_workers"].clone())
                .unwrap_or(json!([]))
        };
        if !saved["machines"][host]["workers"].is_array() {
            saved["machines"][host]["workers"] = default;
        }
        for w in saved["machines"][host]["workers"]
            .as_array_mut()
            .into_iter()
            .flatten()
        {
            if w["id"] == worker {
                w["intent"] = json!(if matches!(action, "resume" | "restart") {
                    "running"
                } else {
                    action
                });
            }
        }
        self.ctx.atomic_json(&self.ctx.desired, &saved)?;
        self.event(host, "signal", &format!("{action} queued for {worker}"));
        Ok(json!({"ok":true,"id":identifier,"state":"pending"}))
    }
    fn local_config(&self, host: &str, changes: &[Value], workers: &Value) -> Result<Value> {
        if changes.is_empty() {
            return Ok(workers.clone());
        }
        let _configuration = self.configuration.lock().unwrap();
        let previous = self.machine(host);
        let current = control::revision(&self.ctx.node, workers);
        let mut updated = workers.clone();
        let db = self.ctx.db()?;
        let tx = db.unchecked_transaction()?;
        for change in changes {
            let key = format!("config:{host}:{}", change["id"].as_str().unwrap_or(""));
            if replica::state_get(&db, &key, json!(0))?
                .as_i64()
                .unwrap_or(0)
                >= change["local_revision"].as_i64().unwrap_or(0)
            {
                continue;
            }
            if !configuration_base_matches(&change["base_revision"], &current, &previous, workers) {
                let identifier = format!("{key}:{}", change["local_revision"]);
                replica::execute(
                    &db,
                    "INSERT OR IGNORE INTO fleet_conflicts(id,node,seq,table_name,data,reason,created_at) VALUES(?,?,?,'worker_configuration',?,?,?)",
                    &[
                        json!(identifier),
                        json!(host),
                        change["local_revision"].clone(),
                        json!(change.to_string()),
                        json!("Supervisor configuration changed while local settings were edited"),
                        json!(crate::issues::worker::now()),
                    ],
                )?;
            } else {
                let definition =
                    json!({"id":change["id"],"config":change["config"],"intent":change["intent"]});
                let list = updated
                    .as_array_mut()
                    .ok_or_else(|| invalid("Invalid worker definitions"))?;
                if let Some(old) = list.iter_mut().find(|w| w["id"] == change["id"]) {
                    *old = definition;
                } else {
                    list.push(definition);
                }
            }
            replica::state_set(&db, &key, &change["local_revision"])?;
        }
        tx.commit()?;
        let mut saved = self.ctx.read_json(&self.ctx.desired, json!({}))?;
        saved["machines"][host]["workers"] = updated.clone();
        self.ctx.atomic_json(&self.ctx.desired, &saved)?;
        self.event(host, "configuration", "Local worker settings synchronized");
        Ok(updated)
    }
    fn configured(&self, host: &str, fallback: &Value) -> Result<Value> {
        Ok(self
            .ctx
            .inventory()?
            .iter()
            .find(|h| h["host"] == host)
            .and_then(|h| h.get("workers"))
            .cloned()
            .unwrap_or_else(|| fallback.clone()))
    }
    fn connection(self: Arc<Self>, host: String) {
        let mut failures = 0u32;
        while !self.ctx.stopped()
            && self
                .ctx
                .inventory()
                .unwrap_or_default()
                .iter()
                .any(|h| h["host"] == host)
        {
            let result = self.channel(&host);
            if let Err(e) = result {
                failures += 1;
                let _=self.update(&host,json!({"state":"disconnected","error":e.to_string(),"retry_at":now()+2u64.pow(failures.min(6)).min(60) as f64}));
                self.event(&host, "disconnected", &e.to_string());
            } else {
                failures = 0;
            }
            self.state.lock().unwrap().connections.remove(&host);
            self.ctx
                .wait(Duration::from_secs(2u64.pow(failures.min(6)).min(60)));
        }
    }
    fn channel(&self, host: &str) -> Result<()> {
        self.update(host, json!({"state":"connecting"}))?;
        let script = "export PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH\"; hey-boss fleet agent --install >&2 && exec hey-boss fleet agent --stdio";
        let mut child = Command::new("ssh")
            .args([
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=8",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ServerAliveInterval=5",
                "-o",
                "ServerAliveCountMax=2",
                host,
                script,
            ])
            .env("SFT_NO_BROWSER", "1")
            .env("SSH_ASKPASS_REQUIRE", "never")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let result = self.channel_inner(host, &mut child);
        // Only this SSH transport is owned by the supervisor. Never signal its
        // independently detached remote workers or their Codex processes.
        let _ = child.kill();
        let _ = child.wait();
        result
    }
    fn channel_inner(&self, host: &str, child: &mut Child) -> Result<()> {
        let mut input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let errors = child.stderr.take().unwrap();
        let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let error_tail = tail.clone();
        let (errors_finished, errors_done) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            for line in BufReader::new(errors)
                .lines()
                .map_while(std::result::Result::ok)
            {
                let mut tail = error_tail.lock().unwrap();
                tail.push_back(line.chars().take(2000).collect());
                while tail.len() > 10 {
                    tail.pop_front();
                }
            }
            let _ = errors_finished.send(());
        });
        let (incoming, rx) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let result = read_frame(&mut reader);
                let done = !matches!(result, Ok(Some(_)));
                if incoming.send(result).is_err() || done {
                    break;
                }
            }
        });
        let (outgoing, requests) = mpsc::sync_channel(32);
        self.state
            .lock()
            .unwrap()
            .connections
            .insert(host.into(), (child.id(), outgoing));
        let hello = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(Some(hello))) => hello,
            result => {
                // stdout and stderr are read independently. Allow stderr to
                // catch up after EOF, without waiting for a descendant that
                // might retain the pipe after the SSH process exits.
                let _ = errors_done.recv_timeout(Duration::from_millis(100));
                let detail = tail
                    .lock()
                    .unwrap()
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                let reason = match result {
                    Ok(Ok(None)) => "Companion closed before hello".to_owned(),
                    Ok(Err(error)) => format!("Companion hello failed: {error}"),
                    Err(_) => {
                        "Companion is missing or has an incompatible fleet protocol".to_owned()
                    }
                    Ok(Ok(Some(_))) => unreachable!(),
                };
                return Err(invalid(&if detail.is_empty() {
                    reason
                } else {
                    format!("{reason}: {detail}")
                }));
            }
        };
        if hello["kind"] != "hello" || hello["version"] != 1 {
            return Err(invalid("Companion has an incompatible fleet protocol"));
        }
        let node = hello["node"]
            .as_str()
            .ok_or_else(|| invalid("Companion has no machine identity"))?;
        {
            let db = self.ctx.db()?;
            let tx = db.unchecked_transaction()?;
            // Import compatibility rows so offline history keeps its foreign
            // keys. The name registry still exposes one destination per name.
            for project in hello["projects"].as_array().into_iter().flatten() {
                if replica::current_row(&db, "projects", project)?.is_null() {
                    let legacy: bool = db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM project_name_keys WHERE name=?1)",
                        [project["name"].as_str().unwrap_or("")],
                        |r| r.get(0),
                    )?;
                    if legacy {
                        db.execute("UPDATE fleet_meta SET syncing=1 WHERE id=1", [])?;
                    }
                    replica::put_row(&db, "projects", project)?;
                    if legacy {
                        db.execute("UPDATE fleet_meta SET syncing=0 WHERE id=1", [])?;
                        // Preserve propagation after bypassing the name guard
                        // for an imported compatibility row.
                        db.execute("INSERT INTO fleet_outbox(table_name,after_json,created_at) VALUES('projects',?1,?2)", rusqlite::params![replica::current_row(&db, "projects", project)?.to_string(),crate::issues::worker::now()])?;
                    }
                }
            }
            tx.commit()?;
        }
        let previous = self.machine(host);
        let fallback = if previous["desired_workers"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        {
            previous["desired_workers"].clone()
        } else {
            definitions(
                hello["workers"]
                    .as_array()
                    .ok_or_else(|| invalid("Missing companion workers"))?,
            )
        };
        let mut workers = self.local_config(
            host,
            hello["local_config"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            &self.configured(host, &fallback)?,
        )?;
        let mut revision = control::revision(&self.ctx.node, &workers);
        self.update(host,json!({"node":node,"hostname":hello["hostname"],"state":"connected","role":"agent","heartbeat":now(),"build":hello["build"],"workers":hello["workers"],"chief_ownership":hello["chief_ownership"],"desired_workers":workers,"desired_revision":revision,"applied_revision":hello["revision"],"pending":hello.get("pending").unwrap_or(&json!(0)),"error":null}))?;
        self.event(host, "connected", "Companion connected");
        send(
            &mut input,
            json!({"kind":"configure","capabilities":authority::capabilities(),"controller":self.ctx.node,"revision":revision,"workers":workers,"configuration_receipts":control::configuration_receipts(&hello["local_config"])}),
        )?;
        let mut last_message = Instant::now();
        let mut last_ping = Instant::now() - Duration::from_secs(5);
        while !self.ctx.stopped() {
            if last_ping.elapsed() >= Duration::from_secs(5) {
                send(&mut input, json!({"kind":"ping"}))?;
                last_ping = Instant::now();
            }
            while let Ok(request) = requests.try_recv() {
                send(&mut input, request)?;
            }
            if last_message.elapsed() > Duration::from_secs(15) {
                return Err("Companion heartbeat timed out".into());
            }
            let message = match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(result) => result?.ok_or_else(|| {
                    invalid(&format!(
                        "Companion connection closed: {}",
                        tail.lock()
                            .unwrap()
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n")
                    ))
                })?,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => return Err("Companion reader exited".into()),
            };
            if message["version"] != 1 {
                return Err(invalid("Protocol version mismatch"));
            }
            last_message = Instant::now();
            self.update(host, json!({"heartbeat":now()}))?;
            match message["kind"].as_str() {
                Some("authority_request") => {
                    let result = (if message["request"]["kind"] == "issue_numbers" {
                        self.issue_numbers(node, &message["request"])
                    } else {
                        self.authoritative(&message["request"])
                    })
                    .unwrap_or_else(authority::failure);
                    send(&mut input, authority::response(&message["id"], result)?)?;
                }
                Some("heartbeat") => {
                    workers = self.local_config(
                        host,
                        message["local_config"]
                            .as_array()
                            .map(Vec::as_slice)
                            .unwrap_or(&[]),
                        &self.configured(host, &workers)?,
                    )?;
                    for discovered in message["workers"].as_array().into_iter().flatten() {
                        if !workers
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|w| w["id"] == discovered["id"])
                        {
                            workers.as_array_mut().unwrap().push(definition(discovered));
                        }
                    }
                    let (payload, receipts, signals) = {
                        let db = self.ctx.db()?;
                        db.execute_batch("BEGIN IMMEDIATE")?;
                        let receipts = replica::accept_changes(
                            &db,
                            node,
                            message["changes"]
                                .as_array()
                                .ok_or_else(|| invalid("Missing companion journal"))?,
                        )?;
                        replica::refresh_allocation_deadlines(
                            &db,
                            node,
                            message["workers"]
                                .as_array()
                                .map(Vec::as_slice)
                                .unwrap_or(&[]),
                        )?;
                        replica::allocate(&db, node, workers.as_array().unwrap())?;
                        db.execute_batch("COMMIT; BEGIN")?;
                        let mut payload = match message["cursor"].as_i64() {
                            Some(cursor) => replica::incremental(&db, node, cursor)?,
                            None => replica::snapshot(&db, node)?,
                        };
                        for (change, receipt) in
                            message["changes"].as_array().unwrap().iter().zip(&receipts)
                        {
                            if receipt["state"] == "conflict"
                                && !matches!(
                                    change["table_name"].as_str(),
                                    Some("comments" | "events")
                                )
                            {
                                let table = change["table_name"].as_str().unwrap();
                                let row: Value = serde_json::from_str(
                                    change["after_json"]
                                        .as_str()
                                        .or(change["before_json"].as_str())
                                        .ok_or_else(|| invalid("Missing conflict row"))?,
                                )?;
                                let canonical = replica::current_row(&db, table, &row)?;
                                if !canonical.is_null() {
                                    if !payload["tables"][table].is_array() {
                                        payload["tables"][table] = json!([]);
                                    }
                                    payload["tables"][table]
                                        .as_array_mut()
                                        .unwrap()
                                        .push(canonical);
                                }
                            }
                        }
                        let signals = replica::rows(
                            &db,
                            "SELECT * FROM fleet_signals WHERE host=? AND state='pending' ORDER BY created_at",
                            &[json!(host)],
                        )?;
                        db.execute_batch("COMMIT")?;
                        (payload, receipts, signals)
                    };
                    pull::send_pull(
                        &mut input,
                        payload,
                        receipts,
                        hello["capabilities"]["pull_gzip_chunks"] == true,
                    )?;
                    // Our own encoding/writing time is not companion silence.
                    last_message = Instant::now();
                    self.update(host,json!({"workers":message["workers"],"chief_ownership":message["chief_ownership"],"pending":message["pending"],"conflicts":message["conflicts"],"applied_revision":message["revision"],"last_sync":now()}))?;
                    self.reconcile_chief_ownership()?;
                    let active = message["workers"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|w| w["active"].as_u64())
                        .sum::<u64>();
                    self.event(
                        host,
                        "heartbeat",
                        &format!(
                            "{active} active sessions; {} outgoing changes",
                            message["pending"]
                        ),
                    );
                    for pending in signals {
                        send(
                            &mut input,
                            json!({"kind":"signal","id":pending["id"],"worker":pending["worker"],"signal":pending["signal"]}),
                        )?;
                    }
                    let current = self.configured(host, &workers)?;
                    let updated = control::revision(&self.ctx.node, &current);
                    if updated != revision
                        || !self.machine(host)["configuration_error"].is_null()
                        || message["local_config"]
                            .as_array()
                            .is_some_and(|changes| !changes.is_empty())
                    {
                        workers = current;
                        revision = updated;
                        send(
                            &mut input,
                            json!({"kind":"configure","capabilities":authority::capabilities(),"controller":self.ctx.node,"revision":revision,"workers":workers,"configuration_receipts":control::configuration_receipts(&message["local_config"])}),
                        )?;
                        self.update(
                            host,
                            json!({"desired_workers":workers,"desired_revision":revision}),
                        )?;
                    }
                }
                Some("conversation" | "takeover" | "steer") => {
                    if let Some(waiter) = self
                        .state
                        .lock()
                        .unwrap()
                        .waiters
                        .get(message["id"].as_str().unwrap_or(""))
                    {
                        let _ = waiter.try_send(message["result"].clone());
                    }
                }
                Some("ack") => {
                    if let Some(ack) = message.get("signal") {
                        replica::execute(
                            &self.ctx.db()?,
                            "UPDATE fleet_signals SET state=?,result=? WHERE id=?",
                            &[
                                ack["state"].clone(),
                                json!(ack.to_string()),
                                ack["id"].clone(),
                            ],
                        )?;
                        self.event(host, "signal", &ack.to_string());
                    }
                    if let Some(revision) = message.get("revision") {
                        self.update(
                            host,
                            json!({"applied_revision":revision,"configuration_error":null}),
                        )?;
                    }
                    if let Some(error) = message.get("configuration_error") {
                        self.update(host, json!({"configuration_error":error}))?;
                        self.event(
                            host,
                            "configuration",
                            error.as_str().unwrap_or("Configuration error"),
                        );
                    }
                }
                _ => return Err(invalid("Unknown companion message")),
            }
        }
        Ok(())
    }
    fn schedule_deploy(self: &Arc<Self>, host: &str) {
        {
            let mut state = self.state.lock().unwrap();
            if state.deploying {
                return;
            }
            state.deploying = true;
        }
        let app = self.clone();
        let host = host.to_owned();
        std::thread::spawn(move || {
            let result = app.deploy(&host);
            if let Err(e) = result {
                let _=app.update(&host,json!({"deployment":"failed","deployment_error":e.to_string(),"retry_deploy_at":now()+60.0}));
                app.event(&host, "deployment", &e.to_string());
            }
            app.state.lock().unwrap().deploying = false;
        });
    }
    fn deploy(&self, host: &str) -> Result<()> {
        let desired = self.state.lock().unwrap().desired_build.clone();
        let desired = desired
            .as_str()
            .filter(|build| !build.is_empty())
            .ok_or_else(|| invalid("No desired published build for deployment"))?;
        self.update(host, json!({"deployment":"updating"}))?;
        self.event(host, "deployment", "Installing desired software");
        let mut command = Command::new(&self.ctx.binary);
        command.args(["upgrade", "--json"]);
        if host == "local" {
            command.arg("--local-only");
        } else {
            command.args(["--host", host]);
        }
        // Remembered checkouts are locations, never implicit development opt-ins.
        let output = output_timeout(command, Duration::from_secs(1200))?;
        let report: Value = serde_json::from_slice(&output.stdout).unwrap_or(json!({}));
        let target = report["machines"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["host"] == host);
        if !target.is_some_and(|m| matches!(m["status"].as_str(), Some("current" | "updated"))) {
            return Err(target
                .and_then(|m| m["error"].as_str())
                .unwrap_or("Software deployment failed")
                .to_owned()
                .into());
        }
        let target = target.unwrap();
        let source = &report["source"];
        let verified = &target["verified"];
        let final_installation = &target["final_installation"];
        let receipt_source = &verified["receipt"]["source"];
        // A successful installer report may describe a different fetched release,
        // or one superseded by a concurrent rollout. Neither is convergence.
        if report["build"] != desired
            || source["build"] != desired
            || source["kind"] != "main"
            || source["commit"]
                .as_str()
                .is_none_or(|commit| commit.is_empty())
            || target["build"] != desired
            || verified["build"] != desired
            || verified != final_installation
            || ["kind", "repository", "commit", "build"]
                .iter()
                .any(|key| source[*key] != receipt_source[*key])
            || !target["error"].is_null()
        {
            return Err(invalid(&format!(
                "Software deployment did not verify desired published build {desired}; reported {}",
                report["build"]
            )));
        }
        self.update(
            host,
            json!({"deployment":"current","deployment_error":null,"retry_deploy_at":null}),
        )?;
        self.event(host, "deployment", "Software deployment complete");
        let state = self.state.lock().unwrap();
        let matching_transport = state
            .machines
            .get(host)
            .and_then(|machine| machine["build"].as_str())
            .is_some_and(|build| build.contains(&format!("build {desired})")));
        if !matching_transport && let Some((pid, _)) = state.connections.get(host) {
            unsafe { libc::kill(*pid as i32, libc::SIGTERM) };
        }
        Ok(())
    }
    fn tick(self: &Arc<Self>) -> Result<()> {
        // A newer installer can migrate the store while this process still has
        // the old schema code loaded. Check before any reconciliation so that
        // incompatible database work cannot prevent the service manager reload.
        if std::env::var("HEY_BOSS_FLEET_SUPERVISED").as_deref() == Ok("1")
            && self.ctx.build()? != self.state.lock().unwrap().build
        {
            self.event(
                "local",
                "deployment",
                "Supervisor reloading updated software",
            );
            self.ctx
                .stop
                .store(true, std::sync::atomic::Ordering::Release);
            return Ok(());
        }
        let hosts = self.ctx.inventory()?;
        let Some(worker_configuration) = self.ctx.lock("fleet-worker-control.lock", false)? else {
            return Ok(());
        };
        let observed: Vec<_> = self
            .state
            .lock()
            .unwrap()
            .local
            .iter()
            .map(definition)
            .collect();
        let main = self
            .ctx
            .read_json(&self.ctx.state.join("fleet-main.json"), json!({}))?;
        let mut desired = {
            let _configuration = self.configuration.lock().unwrap();
            let mut saved = self.ctx.read_json(&self.ctx.desired, json!({}))?;
            if main["workers"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|w| w.get("local_revision").is_some())
            {
                saved["machines"]["local"]["workers"] = json!(
                    main["workers"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|w| json!({"id":w["id"],"config":w["config"],"intent":w["intent"]}))
                        .collect::<Vec<_>>()
                );
                self.ctx.atomic_json(&self.ctx.desired, &saved)?;
            }
            saved["machines"]["local"]
                .get("workers")
                .cloned()
                .unwrap_or_else(|| main["workers"].clone())
        };
        if !desired.is_array() {
            desired = json!([]);
        }
        for discovered in &observed {
            if !desired
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["id"] == discovered["id"])
            {
                desired.as_array_mut().unwrap().push(discovered.clone());
            }
        }
        let main = json!({"role":"controller","workers":desired,"revision":hash(&desired)});
        self.ctx
            .atomic_json(&self.ctx.state.join("fleet-main.json"), &main)?;
        let failures = control::configure_workers(&self.ctx, desired.as_array().unwrap())?;
        if !failures.is_empty() {
            self.event("local", "configuration", &failures.join("; "));
        }
        drop(worker_configuration);
        control::reconcile(&self.ctx, &main)?;
        replica::prune_journal(&self.ctx.db()?)?;
        for pending in replica::rows(
            &self.ctx.db()?,
            "SELECT * FROM fleet_signals WHERE host='local' AND state IN ('pending','stopping','starting') ORDER BY created_at",
            &[],
        )? {
            match control::apply_signal(&self.ctx, &pending) {
                Ok(receipt) => self.event("local", "signal", &receipt.to_string()),
                Err(e) => self.event("local", "signal", &format!("{}: {e}", pending["id"])),
            }
        }
        let source_path = self.ctx.state.join("upgrade-source");
        self.ctx.protect_file(&source_path)?;
        self.ctx
            .protect_file(&self.ctx.state.join("upgrade-receipt.json"))?;
        if let Ok(source) = std::fs::read_to_string(source_path)
            && !super::context::development_install_active(
                &self.ctx.state,
                std::path::Path::new(source.trim()),
                &self.ctx.build()?,
            )?
        {
            {
                let fingerprint =
                    super::context::published_source_build(std::path::Path::new(source.trim()))?;
                let build = &json!(fingerprint);
                self.state.lock().unwrap().desired_build = build.clone();
                let db = self.ctx.db()?;
                let previous = replica::state_get(&db, "desired_build", Value::Null)?;
                replica::state_set(&db, "desired_build", build)?;
                if !previous.is_null() && previous != *build {
                    self.event(
                        "local",
                        "deployment",
                        "Published main changed; automatic deployment scheduled",
                    );
                }
                let needle = format!(
                    "build {})",
                    build
                        .as_str()
                        .ok_or_else(|| invalid("Invalid desired build"))?
                );
                let startup = self.state.lock().unwrap().build.clone();
                if !startup.contains(&needle)
                    && self.machine("local")["retry_deploy_at"]
                        .as_f64()
                        .unwrap_or(0.0)
                        <= now()
                {
                    self.schedule_deploy("local");
                }
                for entry in &hosts {
                    let host = entry["host"].as_str().unwrap();
                    let m = self.machine(host);
                    if let Some(installed) = m["build"].as_str() {
                        if installed.contains(&needle) {
                            if m["deployment"] != "current" || !m["deployment_error"].is_null() {
                                self.update(
                                    host,
                                    json!({"deployment":"current","deployment_error":null}),
                                )?;
                            }
                        } else if m["deployment"] != "updating"
                            && m["retry_deploy_at"].as_f64().unwrap_or(0.0) <= now()
                        {
                            self.update(host, json!({"deployment":"outdated"}))?;
                        }
                    }
                }
            }
            for entry in &hosts {
                let host = entry["host"].as_str().unwrap();
                if self.machine(host)["deployment"] == "outdated" {
                    self.schedule_deploy(host);
                }
            }
        }
        Ok(())
    }
    fn scheduler(self: Arc<Self>) {
        let mut threads = BTreeMap::new();
        while !self.ctx.stopped() {
            match self.ctx.inventory() {
                Ok(hosts) => self.start_connections(&hosts, &mut threads),
                Err(e) => self.event("local", "error", &e.to_string()),
            }
            self.ctx.wait(Duration::from_secs(5));
        }
    }
    fn start_connections(
        self: &Arc<Self>,
        hosts: &[Value],
        threads: &mut BTreeMap<String, std::thread::JoinHandle<()>>,
    ) {
        for entry in hosts {
            let host = entry["host"].as_str().unwrap();
            if threads.get(host).is_none_or(|t| t.is_finished()) {
                let app = self.clone();
                let host = host.to_owned();
                threads.insert(
                    host.clone(),
                    std::thread::spawn(move || app.connection(host)),
                );
            }
        }
    }
    fn observe_local(&self) -> Result<()> {
        // Read acknowledgment before worker state: a revoked worker cannot
        // start between the status observation and this acknowledgment.
        let chief_ownership = crate::chief_ownership::read(&self.ctx.db()?)?;
        let observed = self.ctx.workers()?;
        {
            let mut state = self.state.lock().unwrap();
            state.local = observed;
            state.chief_ownership = chief_ownership;
            // Advance only after a successful fresh collection, never for a
            // timer tick or a failed observation.
            state.local_updated = now();
        }
        self.reconcile_chief_ownership()?;
        self.event("local", "heartbeat", "Worker state refreshed");
        Ok(())
    }
    fn reconcile_chief_ownership(&self) -> Result<()> {
        let machines = {
            let state = self.state.lock().unwrap();
            let mut machines = state.machines.values().cloned().collect::<Vec<_>>();
            machines.push(json!({"node":self.ctx.node,"state":"connected","heartbeat":state.local_updated,"workers":state.local,"chief_ownership":state.chief_ownership}));
            for m in &mut machines {
                if now() - m["heartbeat"].as_f64().unwrap_or(0.0) > 15.0 {
                    m["state"] = json!("disconnected");
                }
            }
            machines
        };
        let db = self.ctx.db()?;
        crate::chief_ownership::reconcile(&db, &machines)?;
        crate::chief_ownership::stop_unassigned(&db)?;
        Ok(())
    }
    fn observer(self: Arc<Self>) {
        while !self.ctx.stopped() {
            if let Err(e) = self.observe_local() {
                self.event("local", "error", &e.to_string());
            }
            self.ctx.wait(Duration::from_secs(5));
        }
    }
    fn maintenance(self: Arc<Self>) {
        while !self.ctx.stopped() {
            if let Err(e) = self.tick() {
                self.event("local", "error", &e.to_string());
            }
            if self.ctx.stopped() {
                break;
            }
            if let Err(e) = self.save_machines() {
                self.event("local", "error", &e.to_string());
            }
            self.ctx.wait(Duration::from_secs(5));
        }
    }
    fn issue_numbers(&self, node: &str, value: &Value) -> crate::issues::Result<Value> {
        let project: crate::issues::Project = serde_json::from_value(value["project"].clone())?;
        let next = value["next"]
            .as_i64()
            .filter(|n| *n > 0)
            .ok_or_else(|| crate::issues::Error::invalid("Invalid next issue number"))?;
        let project =
            crate::issues::Store::open(&self.ctx.path)?.notification_project(&project, None)?;
        let result = (|| -> Result<Value> {
            let mut db = self.ctx.db()?;
            let range = replica::reserve_numbers(&mut db, node, &project.id, next)?;
            Ok(json!({"ok":true,"project":project,"range":range}))
        })();
        result.map_err(|e| crate::issues::Error::new("fleet_error", e.to_string()))
    }

    fn authoritative(&self, value: &Value) -> crate::issues::Result<Value> {
        match value["kind"].as_str() {
            Some("issue_metadata") => {
                let request: crate::issues::Request =
                    serde_json::from_value(value["request"].clone())?;
                crate::issues::Store::open(&self.ctx.path)?.execute_supervisor(&request)
            }
            Some("resource") => {
                let request: crate::issues::Request =
                    serde_json::from_value(value["request"].clone())?;
                if !matches!(
                    request.operation,
                    crate::issues::Operation::Mindmap { .. }
                        | crate::issues::Operation::Artifact { .. }
                        | crate::issues::Operation::Attachment { .. }
                ) {
                    return Err(crate::issues::Error::invalid(
                        "Only maps, artifacts and attachments use the authority relay",
                    ));
                }
                crate::issues::Store::open(&self.ctx.path)?.execute(&request)
            }
            Some("status") => self
                .status()
                .map_err(|e| crate::issues::Error::new("fleet_error", e.to_string())),
            Some("overview") => self
                .overview()
                .map_err(|e| crate::issues::Error::new("fleet_error", e.to_string())),
            _ => Err(crate::issues::Error::invalid(
                "Unsupported authority request",
            )),
        }
    }

    fn handle(&self, mut stream: UnixStream) -> Result<()> {
        // macOS accepted sockets inherit the listener's nonblocking mode.
        // Blocking writes with the existing timeout must send the whole frame.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        stream.set_write_timeout(Some(Duration::from_secs(15)))?;
        let request = read_frame(&mut BufReader::new(stream.try_clone()?))?
            .ok_or_else(|| invalid("Missing supervisor request"))?;
        if request["kind"] == "subscribe" {
            let mut cursor = request["after"].as_u64().unwrap_or(0);
            stream.write_all(b"retry: 2000\nevent: connected\ndata: {}\n\n")?;
            stream.flush()?;
            while !self.ctx.stopped() {
                let events = self
                    .state
                    .lock()
                    .unwrap()
                    .events
                    .iter()
                    .filter(|e| e["id"].as_u64().unwrap() > cursor)
                    .cloned()
                    .collect::<Vec<_>>();
                if events.is_empty() {
                    stream.write_all(b": heartbeat\n\n")?;
                }
                for event in events {
                    writeln!(stream, "id: {}\ndata: {}\n", event["id"], event)?;
                    cursor = event["id"].as_u64().unwrap();
                }
                stream.flush()?;
                self.ctx.wait(Duration::from_secs(1));
            }
            return Ok(());
        }
        let result = match request["kind"].as_str() {
            Some("status") => self.status(),
            Some("overview") => self.overview(),
            Some("conversation" | "takeover" | "steer") => self.conversation(&request),
            Some("signal") => self.signal(&request),
            _ => Err(invalid("Unknown supervisor request")),
        };
        let result = result.unwrap_or_else(|e| json!({"ok":false,"error":e.to_string()}));
        let bytes = match encode_frame(&result, crate::issues::WIRE_LIMIT)? {
            Some(bytes) => bytes,
            None => serde_json::to_vec(
                &json!({"ok":false,"error":"Supervisor response exceeds 16 MiB"}),
            )?,
        };
        stream.write_all(&bytes)?;
        Ok(())
    }
}
fn overview_worker(worker: &Value, visible: &BTreeSet<String>) -> Value {
    let mut result = Value::Object(
        worker
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| !matches!(key.as_str(), "runs" | "chiefs"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    result["runs"] = Value::Array(
        worker["runs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| visible.contains(r["project_id"].as_str().unwrap_or("")))
            .map(crate::agent_conversations::compact_run)
            .collect(),
    );
    result["chiefs"] = Value::Array(
        worker["chiefs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| visible.contains(r["project_id"].as_str().unwrap_or("")))
            .map(|r| {
                let mut value = r.clone();
                for key in ["summary", "last_event"] {
                    value[key] = json!(
                        r[key]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(1000)
                            .collect::<String>()
                    );
                }
                value
            })
            .collect(),
    );
    result
}
fn definition(w: &Value) -> Value {
    json!({"id":w["id"],"config":w["config"],"intent":if w["config"]["enabled"]==true{"running"}else{"pause"}})
}
fn definitions(workers: &[Value]) -> Value {
    json!(workers.iter().map(definition).collect::<Vec<_>>())
}
pub(super) fn run(ctx: Context) -> Result<()> {
    let Some(_lock) = ctx.lock("fleet-controller.lock", false)? else {
        return Err("Fleet supervisor is already running".into());
    };
    let app = Supervisor::new(ctx.clone())?;
    let socket = ctx.state.join("fleet.sock");
    if socket.exists() {
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let scheduler = app.clone();
    std::thread::spawn(move || scheduler.scheduler());
    let observer = app.clone();
    std::thread::spawn(move || observer.observer());
    let maintenance = app.clone();
    std::thread::spawn(move || maintenance.maintenance());
    let mobile = ctx.clone();
    std::thread::spawn(move || super::mobile::run(mobile));
    let monitor = ctx.clone();
    std::thread::spawn(move || super::pr_monitor::run(monitor));
    while !ctx.stopped() {
        match listener.accept() {
            Ok((stream, _)) => {
                let app = app.clone();
                std::thread::spawn(move || {
                    let _ = app.handle(stream);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                ctx.wait(Duration::from_millis(250))
            }
            Err(e) => return Err(e.into()),
        }
    }
    std::fs::remove_file(socket)?;
    Ok(())
}
pub(super) fn output_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<std::process::Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut bytes = vec![];
        stdout
            .take(crate::issues::WIRE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let err = std::thread::spawn(move || {
        let mut bytes = vec![];
        stderr
            .take(crate::issues::WIRE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Fleet subprocess timed out".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stdout = out
        .join()
        .map_err(|_| invalid("Subprocess output reader failed"))??;
    let stderr = err
        .join()
        .map_err(|_| invalid("Subprocess error reader failed"))??;
    if stdout.len() > crate::issues::WIRE_LIMIT || stderr.len() > crate::issues::WIRE_LIMIT {
        return Err(invalid("Fleet subprocess output exceeds limit"));
    }
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn configuration_base_matches(
    base: &Value,
    current: &str,
    previous: &Value,
    workers: &Value,
) -> bool {
    base.as_str() == Some(current)
        || (base.is_string()
            && previous["desired_workers"] == *workers
            && previous["desired_revision"] == *base)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(std::path::PathBuf);
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn test_supervisor() -> (TestDirectory, Supervisor) {
        let directory =
            TestDirectory(std::env::temp_dir().join(format!("hb-supervisor-{}", id().unwrap())));
        std::fs::create_dir(&directory.0).unwrap();
        let ctx = Context {
            home: directory.0.clone(),
            state: directory.0.clone(),
            desired: directory.0.join("fleet.json"),
            binary: std::env::current_exe().unwrap(),
            path: directory.0.join("issues.db"),
            node: "test-supervisor".into(),
            stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        drop(crate::issues::Store::open(&ctx.path).unwrap());
        replica::ensure_metadata(&ctx.db().unwrap()).unwrap();
        let app = Supervisor {
            ctx,
            persistence: Mutex::new(()),
            configuration: Mutex::new(()),
            state: Mutex::new(State {
                machines: BTreeMap::new(),
                local: vec![],
                local_updated: 0.0,
                chief_ownership: vec![],
                events: VecDeque::new(),
                sequence: 0,
                epoch: "test".into(),
                build: "test".into(),
                desired_build: Value::Null,
                connections: BTreeMap::new(),
                waiters: BTreeMap::new(),
                deploying: false,
                machines_dirty: false,
            }),
        };
        (directory, app)
    }

    #[test]
    fn authority_relay_cannot_execute_arbitrary_issue_or_worker_operations() {
        let (_directory, app) = test_supervisor();
        for request in [
            json!({"kind":"signal","worker":"other","signal":"stop"}),
            json!({"kind":"resource","request":{"version":1,"project":{"id":"named:Test","name":"Test"},"operation":{"action":"list","state":"all"}}}),
        ] {
            assert_eq!(
                app.authoritative(&request).unwrap_err().code,
                "invalid_input"
            );
        }
        assert_eq!(
            app.authoritative(&json!({"kind":"status"})).unwrap()["ok"],
            true
        );
    }

    #[test]
    fn authority_metadata_preserves_ownership_guards_and_receipts() {
        let (_directory, app) = test_supervisor();
        let actor = json!({"id":"codex:chief","kind":"codex","session_id":"chief","machine":"peer","host":"peer","pid":null,"process_start":null,"cwd":"/tmp","source":"test"});
        let request = |mut operation: Value, key: Option<&str>| -> crate::issues::Request {
            if operation["action"] == "edit" {
                let fields = operation.as_object_mut().unwrap();
                fields.entry("add_labels").or_insert_with(|| json!([]));
                fields.entry("remove_labels").or_insert_with(|| json!([]));
            }
            serde_json::from_value(json!({"version":1,"project":{"id":"named:Test","name":"Test"},"actor":actor,"operation":operation,"request_id":key})).unwrap()
        };
        let mut store = crate::issues::Store::open(&app.ctx.path).unwrap();
        for title in ["Closed cleanup", "Live assignment"] {
            store
                .execute(&request(
                    json!({"action":"create","title":title,"body":"","labels":["rework needed"]}),
                    None,
                ))
                .unwrap();
        }
        store
            .execute(&request(
                json!({"action":"close","number":1,"force":false}),
                None,
            ))
            .unwrap();
        let mut claim = request(json!({"action":"claim","number":2,"force":false}), None);
        claim.actor.as_mut().unwrap().id = "codex:worker".into();
        claim.actor.as_mut().unwrap().session_id = Some("worker".into());
        store.execute(&claim).unwrap();
        app.ctx
            .db()
            .unwrap()
            .execute_batch("INSERT INTO fleet_allocations VALUES('named:Test',2,'worker-machine');")
            .unwrap();
        let ownership = || {
            app.ctx
                .db()
                .unwrap()
                .prepare("SELECT number,state,assignee FROM issues ORDER BY number")
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap()
        };
        let before = ownership();
        let route = |operation: Value, key: Option<&str>| {
            app.authoritative(&json!({"kind":"issue_metadata","request":request(operation, key)}))
        };
        for number in [1, 2] {
            let edit = json!({"action":"edit","number":number,"if_version":2,"add_labels":["reviewed"],"remove_labels":["rework needed"]});
            let key = format!("edit-{number}");
            let saved = route(edit.clone(), Some(key.as_str())).unwrap();
            assert_eq!(saved["issue"]["labels"], json!(["reviewed"]));
            assert_eq!(route(edit.clone(), Some(key.as_str())).unwrap(), saved);
            assert_eq!(route(edit, Some("stale")).unwrap_err().code, "conflict");
        }
        assert_eq!(ownership(), before);
        assert_eq!(
            app.ctx
                .db()
                .unwrap()
                .query_row(
                    "SELECT node FROM fleet_allocations WHERE issue_number=2",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "worker-machine"
        );
        let batch = json!({"action":"batch","edits":[{"number":1,"if_version":3,"expected_assignee":null,"add_labels":["batch"]},{"number":2,"if_version":3,"expected_assignee":"wrong","add_labels":["batch"]}]});
        assert_eq!(
            route(batch, Some("batch-stale")).unwrap()["accepted"],
            false
        );
        for operation in [
            json!({"action":"claim","number":1,"force":true}),
            json!({"action":"reopen","number":1,"if_version":3}),
            json!({"action":"edit","number":2,"draft":true,"if_version":3}),
            json!({"action":"edit","number":2,"add_labels":["unguarded"]}),
            json!({"action":"batch","edits":[{"number":2,"if_version":3,"expected_assignee":"codex:worker","assignment":"unassign"}]}),
        ] {
            assert_eq!(
                route(operation, Some("unsupported")).unwrap_err().code,
                "invalid_input"
            );
        }
        let edit = json!({"action":"edit","number":2,"if_version":3,"add_labels":["yolo"]});
        assert_eq!(route(edit, Some("unsafe")).unwrap_err().code, "forbidden");
        let edit = json!({"action":"edit","number":2,"if_version":3,"body":"Correct guidance"});
        assert_eq!(route(edit.clone(), None).unwrap_err().code, "invalid_input");
        assert_eq!(
            route(edit, Some("body")).unwrap()["issue"]["body"],
            "Correct guidance"
        );
        assert_eq!(ownership(), before);
    }

    fn large_report_supervisor() -> (TestDirectory, Supervisor) {
        let (directory, app) = test_supervisor();
        app.ctx.db().unwrap().execute_batch(
            "INSERT INTO projects(id,name,next_number) VALUES('Atlas','Atlas',1),('Hidden','Hidden',1); UPDATE projects SET hidden_at=1 WHERE id='Hidden';"
        ).unwrap();
        let run = json!({
            "id":"run", "project_id":"Atlas", "project_name":"Atlas", "number":4,
            "actor_id":"worker:run", "session_id":"session", "kind":"issue",
            "next_at":12, "enabled":true, "state":"running",
            "summary":"é".repeat(1100), "last_event":"event".repeat(300),
            "events":vec![json!({"text":"x".repeat(4000)});12],
            "expanded_prompt":"private"
        });
        let mut runs = vec![run; 125];
        runs.push(json!({"id":"hidden","project_id":"Hidden"}));
        let workers = json!([{"id":"worker","pid":1,"config":{"concurrency":128},"runs":runs,"chiefs":[{"id":"chief","project_id":"Atlas","actor_id":"chief:Atlas","session_id":"chief-session"}]}]);
        for host in ["one", "two", "three"] {
            app.state.lock().unwrap().machines.insert(
                host.into(),
                json!({"host":host,"state":"connected","workers":workers}),
            );
        }
        (directory, app)
    }

    struct TestTransport(Child);
    impl Drop for TestTransport {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    impl TestTransport {
        fn assert_alive(&mut self) {
            let deadline = Instant::now() + Duration::from_millis(100);
            while Instant::now() < deadline {
                assert!(
                    self.0.try_wait().unwrap().is_none(),
                    "Deployment killed the transport"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    fn deployment_fixture(status: &str) -> (TestDirectory, Supervisor, TestTransport, Value) {
        let (directory, mut app) = test_supervisor();
        app.ctx.binary = directory.0.join("upgrade-cli");
        // Keep the transport alive until the fixture closes it. A wall-clock
        // sleep can expire during slow Git/source checks and mimic a kill.
        let transport = TestTransport(
            Command::new("cat")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let (tx, _rx) = mpsc::sync_channel(1);
        app.state
            .lock()
            .unwrap()
            .connections
            .insert("peer".into(), (transport.0.id(), tx));
        app.state.lock().unwrap().desired_build = json!("desired");
        app.update("peer", json!({"build":"hey-boss (build desired)","state":"connected","workers":[{"id":"worker","active":1}],"last_sync":123})).unwrap();
        let installation = json!({"build":"desired","receipt":{"source":{"kind":"main","repository":"github.com/kamilio/hey-boss","commit":"published","build":"desired"},"generation":2}});
        let report = json!({"build":"desired","source":installation["receipt"]["source"],"machines":[{"host":"peer","build":"desired","status":status,"error":null,"before":installation,"verified":installation,"final_installation":installation}]});
        (directory, app, transport, report)
    }

    fn write_deployment_report(app: &Supervisor, report: &Value) {
        // JSON is synthetic; single quotes cannot occur in these fixture values.
        std::fs::write(
            &app.ctx.binary,
            format!("#!/bin/sh\nif [ \"$1\" = --version ]; then\n  printf '%s\\n' 'hey-boss (build {})'\nelse\n  printf '%s\\n' '{}'\nfi\n", report["build"].as_str().unwrap(), report),
        )
        .unwrap();
        std::fs::set_permissions(&app.ctx.binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn deployment_refuses_an_older_published_report_without_cycling_transport() {
        let (_directory, app, mut transport, mut report) = deployment_fixture("current");
        report["build"] = json!("older-published");
        report["source"]["build"] = json!("older-published");
        let target = &mut report["machines"][0];
        target["build"] = json!("older-published");
        for key in ["before", "verified", "final_installation"] {
            target[key]["build"] = json!("older-published");
            target[key]["receipt"]["source"]["build"] = json!("older-published");
        }
        write_deployment_report(&app, &report);
        let error = app.deploy("peer").unwrap_err();
        assert!(error.to_string().contains("desired"), "{error}");
        assert_ne!(app.machine("peer")["deployment"], "current");
        transport.assert_alive();
        assert_eq!(app.machine("peer")["workers"][0]["active"], 1);
        assert_eq!(app.machine("peer")["last_sync"], 123);
    }

    #[test]
    fn deployment_current_report_keeps_the_matching_transport() {
        let (_directory, app, mut transport, report) = deployment_fixture("current");
        write_deployment_report(&app, &report);
        app.deploy("peer").unwrap();
        assert_eq!(app.machine("peer")["deployment"], "current");
        transport.assert_alive();
    }

    #[test]
    fn deployment_refuses_inconsistent_final_build_or_source_receipt() {
        for field in ["build", "source", "receipt", "missing-final", "development"] {
            let (_directory, app, mut transport, mut report) = deployment_fixture("updated");
            let final_installation = &mut report["machines"][0]["final_installation"];
            match field {
                "build" => final_installation["build"] = json!("superseding"),
                "source" => {
                    final_installation["receipt"]["source"]["commit"] = json!("different-source")
                }
                "missing-final" => *final_installation = Value::Null,
                "receipt" => {
                    final_installation["receipt"]["source"]["commit"] = json!("different-source");
                    report["machines"][0]["verified"] =
                        report["machines"][0]["final_installation"].clone();
                }
                "development" => {
                    report["source"]["kind"] = json!("development");
                    for key in ["verified", "final_installation"] {
                        report["machines"][0][key]["receipt"]["source"]["kind"] =
                            json!("development");
                    }
                }
                _ => unreachable!(),
            }
            write_deployment_report(&app, &report);
            assert!(app.deploy("peer").is_err());
            transport.assert_alive();
        }
    }

    #[test]
    fn deployment_updated_report_reconnects_an_old_transport() {
        let (_directory, app, mut transport, report) = deployment_fixture("updated");
        app.update("peer", json!({"build":"hey-boss (build old)"}))
            .unwrap();
        write_deployment_report(&app, &report);
        app.deploy("peer").unwrap();
        assert_eq!(app.machine("peer")["deployment"], "current");
        assert!(!transport.0.wait().unwrap().success());
    }

    #[test]
    fn owning_supervisor_converges_on_published_source_while_local_main_is_unpushed() {
        let (directory, app, mut transport, report) = deployment_fixture("current");
        let source = directory.0.join("source");
        for name in [
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            "src/file",
            "skills/hey-boss/file",
            "packages/hey-gh/file",
            "packages/hey-harvester/file",
            "tools/upgrade_hey_boss.py",
            "tools/drain_github_issues.py",
            "hey_boss_daemon.swift",
            "package_hey_boss.swift",
            "setup_hey_boss.swift",
            "assets/file",
        ] {
            let file = source.join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, name).unwrap();
        }
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&source)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "published",
        ]);
        git(&[
            "remote",
            "add",
            "origin",
            "https://example.invalid/hey-boss.git",
        ]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let published = super::super::context::published_source_build(&source).unwrap();
        std::fs::write(source.join("src/file"), "unpublished changes").unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "unpushed",
        ]);
        std::fs::write(
            app.ctx.state.join("upgrade-source"),
            source.to_str().unwrap(),
        )
        .unwrap();
        std::fs::write(app.ctx.state.join("companion-hosts"), "peer\n").unwrap();
        let report: Value = serde_json::from_str(
            &report
                .to_string()
                .replace("\"desired\"", &format!("\"{published}\"")),
        )
        .unwrap();
        write_deployment_report(&app, &report);
        let installed = format!("hey-boss (build {published})");
        {
            let mut state = app.state.lock().unwrap();
            state.build = installed.clone();
            // Do not spawn real installs if the regression requests an unpublished build.
            state.deploying = true;
        }
        app.update("peer", json!({"build":installed})).unwrap();
        let app = Arc::new(app);
        app.tick().unwrap();
        assert_eq!(app.state.lock().unwrap().desired_build, published);
        app.deploy("peer").unwrap();
        for _ in 0..3 {
            app.tick().unwrap();
            assert_eq!(app.machine("peer")["deployment"], "current");
        }
        transport.assert_alive();
        assert_eq!(app.machine("peer")["workers"][0]["active"], 1);
        assert_eq!(app.machine("peer")["last_sync"], 123);
        assert_eq!(
            replica::state_get(&app.ctx.db().unwrap(), "desired_build", Value::Null).unwrap(),
            published
        );
    }

    #[test]
    fn deployment_mismatch_uses_the_normal_retry_budget() {
        let (_directory, app, mut transport, mut report) = deployment_fixture("current");
        report["build"] = json!("older-published");
        write_deployment_report(&app, &report);
        let app = Arc::new(app);
        let started = now();
        app.schedule_deploy("peer");
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.state.lock().unwrap().deploying {
            assert!(Instant::now() < deadline, "Deployment did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        let machine = app.machine("peer");
        assert_eq!(machine["deployment"], "failed");
        assert!(
            machine["deployment_error"]
                .as_str()
                .unwrap()
                .contains("desired")
        );
        assert!(machine["retry_deploy_at"].as_f64().unwrap() >= started + 60.0);
        transport.assert_alive();
    }

    #[test]
    fn overview_preserves_local_and_remote_metadata_and_chief_fields() {
        let (_directory, app) = test_supervisor();
        app.ctx.db().unwrap().execute_batch(
            "INSERT INTO projects(id,name,next_number) VALUES('Atlas','Atlas',1);
             INSERT INTO fleet_conflicts(id,node,seq,table_name,data,reason,created_at) VALUES('conflict','peer',1,'issues','private body','Conflict',1);
             INSERT INTO fleet_signals VALUES('signal','peer','worker','pause','pending',NULL,1);"
        ).unwrap();
        let worker = json!({"id":"worker","kind":"service","pid":1,"build":"worker-build","config":{"concurrency":2},"active":1,"free":1,"eligible":3,"upgrading":false,"updated_at":2,"version":4,"future_metadata":{"keep":true},"runs":[{"id":"run","project_id":"Atlas","actor_id":"actor","session_id":"session","events":["private event"]}],"chiefs":[{"id":"chief","kind":"chief","project_id":"Atlas","worker_id":"worker","machine":"peer","enabled":true,"next_at":10,"session_id":"chief-session","summary":"Done","last_event":"Done","future_metadata":"keep"}]});
        {
            let mut state = app.state.lock().unwrap();
            state.local = vec![worker.clone()];
            state.machines.insert("peer".into(), json!({"host":"peer","role":"agent","state":"connected","heartbeat":1,"build":"companion-build","configuration_error":null,"desired_revision":"revision","desired_workers":[{"id":"worker","config":{"concurrency":2}}],"future_metadata":{"keep":true},"workers":[worker]}));
            state.machines.insert(
                "local".into(),
                json!({"host":"local","workers":[{"id":"stale-local"}]}),
            );
            state.events.push_back(json!({"id":1,"detail":"event"}));
        }
        let full = app.status().unwrap();
        let overview = app.overview().unwrap();
        assert_eq!(overview["machines"].as_array().unwrap().len(), 2);
        for (index, machine) in overview["machines"].as_array().unwrap().iter().enumerate() {
            for (key, value) in full["machines"][index].as_object().unwrap() {
                if key != "workers" {
                    assert_eq!(machine.get(key), Some(value), "{key}");
                }
            }
            let worker = &machine["workers"][0];
            for (key, value) in full["machines"][index]["workers"][0].as_object().unwrap() {
                if !matches!(key.as_str(), "runs" | "chiefs") {
                    assert_eq!(worker.get(key), Some(value), "{key}");
                }
            }
            assert_eq!(
                worker["chiefs"],
                full["machines"][index]["workers"][0]["chiefs"]
            );
            assert_eq!(worker["runs"][0]["actor_id"], "actor");
            assert_eq!(worker["runs"][0]["session_id"], "session");
            assert!(worker["runs"][0].get("events").is_none());
        }
        assert_eq!(overview["machines"][1]["role"], "companion");
        assert_eq!(overview["signals"], full["signals"]);
        assert_eq!(full["events"].as_array().unwrap().len(), 1);
        assert_eq!(overview["events"], json!([]));
        let mut conflict = full["conflicts"][0].clone();
        assert_eq!(
            conflict.as_object_mut().unwrap().remove("saved_change"),
            Some(json!("private body"))
        );
        assert_eq!(overview["conflicts"], json!([conflict]));
    }

    #[test]
    fn overview_does_not_allocate_copies_of_discarded_run_events() {
        let (_directory, app) = large_report_supervisor();
        let (overview, allocated) = crate::test_allocations::measure(|| app.overview().unwrap());
        let encoded = serde_json::to_vec(&overview).unwrap().len();
        eprintln!("overview: {allocated} requested Rust allocation bytes; {encoded} encoded bytes");
        assert!(allocated < 8 * 1024 * 1024, "{allocated} allocation bytes");
    }

    #[test]
    fn oversized_status_returns_a_bounded_error_over_the_control_socket() {
        let (_directory, app) = large_report_supervisor();
        let (mut client, server) = UnixStream::pair().unwrap();
        client.write_all(b"{\"kind\":\"status\"}\n").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        app.handle(server).unwrap();
        let mut bytes = vec![];
        client.read_to_end(&mut bytes).unwrap();
        assert!(bytes.len() < 1024, "{}", bytes.len());
        let error: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(error["ok"], false);
        assert!(error["error"].as_str().unwrap().contains("exceeds 16 MiB"));
    }

    #[test]
    fn overview_preserves_agent_identity_without_transporting_event_payloads() {
        let (_directory, app) = large_report_supervisor();
        let full = app.status().unwrap();
        let overview = app.overview().unwrap();
        let full_bytes = serde_json::to_vec(&full).unwrap().len();
        let overview_bytes = serde_json::to_vec(&overview).unwrap().len();
        for machine in full["machines"].as_array().unwrap() {
            assert!(serde_json::to_vec(machine).unwrap().len() < crate::issues::WIRE_LIMIT);
        }
        assert!(full_bytes > 16 * 1024 * 1024, "{full_bytes}");
        assert!(overview_bytes < 2 * 1024 * 1024, "{overview_bytes}");
        for machine in overview["machines"].as_array().unwrap().iter().skip(1) {
            let worker = &machine["workers"][0];
            assert_eq!(worker["config"]["concurrency"], 128);
            assert_eq!(worker["runs"].as_array().unwrap().len(), 125);
            let run = &worker["runs"][0];
            assert_eq!(run["actor_id"], "worker:run");
            assert_eq!(run["session_id"], "session");
            assert_eq!(run["kind"], "issue");
            assert_eq!(run["next_at"], 12);
            assert_eq!(run["enabled"], true);
            assert_eq!(run["summary"].as_str().unwrap().chars().count(), 1000);
            assert_eq!(run["last_event"].as_str().unwrap().chars().count(), 1000);
            assert!(run.get("events").is_none());
            assert!(run.get("expanded_prompt").is_none());
            assert_eq!(worker["chiefs"][0]["actor_id"], "chief:Atlas");
        }
        assert_eq!(
            full["machines"][1]["workers"][0]["runs"][0]["summary"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            1100
        );
    }

    #[test]
    fn installed_replacement_reload_precedes_incompatible_schema_work() {
        const PROBE: &str = "HEY_BOSS_SUPERVISOR_RELOAD_PROBE";
        if std::env::var_os(PROBE).is_none() {
            // Isolate the service environment from other tests without mutating
            // process-global environment variables in a multithreaded runner.
            for supervised in ["1", "0"] {
                let output = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "fleet::native::supervisor::tests::installed_replacement_reload_precedes_incompatible_schema_work",
                        "--nocapture",
                    ])
                    .env(PROBE, "1")
                    .env("HEY_BOSS_FLEET_SUPERVISED", supervised)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "supervised={supervised}: {}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            return;
        }
        let supervised = std::env::var("HEY_BOSS_FLEET_SUPERVISED").unwrap() == "1";
        for installed in ["test", "replacement", "lookup-failure"] {
            let (_directory, mut fixture) = test_supervisor();
            fixture.ctx.binary = fixture.ctx.state.join("replacement-cli");
            let script = if installed == "lookup-failure" {
                "#!/bin/sh\nexit 1\n".to_owned()
            } else {
                format!("#!/bin/sh\nprintf '%s\\n' '{installed}'\n")
            };
            std::fs::write(&fixture.ctx.binary, script).unwrap();
            std::fs::set_permissions(&fixture.ctx.binary, std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let mut worker = TestTransport(Command::new("sleep").arg("60").spawn().unwrap());
            let local = json!({"id":"worker","pid":worker.0.id(),"active":1,
                "config":{"enabled":true},"runs":[{"id":"run","session_id":"session","number":132}]});
            fixture.state.lock().unwrap().local = vec![local.clone()];
            // Simulate a newer installer migrating the store while this loaded
            // supervisor remains alive. Never change a production database.
            let db = fixture.ctx.db().unwrap();
            let schema: i64 = db
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .unwrap();
            db.pragma_update(None, "user_version", schema + 1).unwrap();
            drop(db);
            assert!(
                fixture
                    .ctx
                    .workers()
                    .unwrap_err()
                    .to_string()
                    .contains("Incompatible issue database")
            );
            let app = Arc::new(fixture);
            let result = app.tick();
            let reload = supervised && installed == "replacement";
            if reload {
                result.unwrap();
            } else {
                let error = result.unwrap_err().to_string();
                assert!(
                    error.contains(if supervised && installed == "lookup-failure" {
                        "CLI build lookup failed"
                    } else {
                        "Incompatible issue database"
                    }),
                    "{error}"
                );
            }
            assert_eq!(app.ctx.stopped(), reload);
            let state = app.state.lock().unwrap();
            assert_eq!(state.local, vec![local]);
            assert_eq!(
                state.local_updated, 0.0,
                "Reload is not a fresh worker observation"
            );
            assert_eq!(
                state
                    .events
                    .iter()
                    .filter(|e| e["detail"] == "Supervisor reloading updated software")
                    .count(),
                usize::from(reload)
            );
            drop(state);
            worker.assert_alive();
            let db = app.ctx.db().unwrap();
            assert_eq!(
                db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                    .unwrap(),
                schema + 1
            );
            if reload {
                assert!(
                    !app.ctx.state.join("fleet-main.json").exists(),
                    "Old supervisor rewrote worker configuration before reload"
                );
            }
        }
    }

    #[test]
    fn supervisor_reports_its_loaded_build_after_the_installed_binary_changes() {
        let (_directory, mut fixture) = test_supervisor();
        fixture.ctx.binary = fixture.ctx.state.join("replacement-cli");
        std::fs::write(
            &fixture.ctx.binary,
            "#!/bin/sh\nprintf '%s\\n' 'hey-boss 0.1.0 (build replacement-on-disk)'\n",
        )
        .unwrap();
        std::fs::set_permissions(&fixture.ctx.binary, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        assert!(fixture.ctx.build().unwrap().contains("replacement-on-disk"));
        let app = Supervisor::new(fixture.ctx.clone()).unwrap();
        let loaded = app.state.lock().unwrap().build.clone();
        assert!(loaded.contains(env!("HEY_BOSS_BUILD_ID")), "{loaded}");
        assert!(!loaded.contains("replacement-on-disk"));
    }

    #[test]
    fn startup_eof_reports_the_upstream_transport_error() {
        let (_directory, app) = test_supervisor();
        let mut child = Command::new("sh")
            .args([
                "-c",
                "printf '%s\\n' 'synthetic SSH route failure' >&2; exit 1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let error = app.channel_inner("test-host", &mut child).unwrap_err();
        child.wait().unwrap();
        assert!(error.to_string().contains("Companion closed before hello"));
        assert!(error.to_string().contains("synthetic SSH route failure"));
    }

    #[test]
    fn startup_eof_does_not_wait_indefinitely_for_open_stderr() {
        let (_directory, app) = test_supervisor();
        let mut child = Command::new("sh")
            .args(["-c", "exec 1>&-; exec sleep 10"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let error = app.channel_inner("test-host", &mut child).unwrap_err();
        let elapsed = started.elapsed();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(error.to_string().contains("Companion closed before hello"));
        assert!(elapsed < Duration::from_secs(2), "waited {elapsed:?}");
    }

    #[test]
    fn queued_signal_contention_does_not_block_local_observation_or_peers() {
        let (_directory, fixture) = test_supervisor();
        let app = Arc::new(fixture);
        let writer = app.ctx.db().unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        let slow = app.clone();
        let signaling = std::thread::spawn(move || {
            slow.signal(
                &json!({"id":"test-signal","host":"local","worker":"test","signal":"pause"}),
            )
        });
        // Observe the actual SQLite wait rather than relying on a sleep.
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.configuration.try_lock().is_ok() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            app.configuration.try_lock().is_err(),
            "signal never reached its serialized write"
        );
        let healthy = app.clone();
        let (tx, rx) = mpsc::channel();
        let progressing = std::thread::spawn(move || {
            healthy.observe_local().unwrap();
            tx.send(healthy.update("healthy", json!({"heartbeat":2})))
                .unwrap();
        });
        let progress = rx.recv_timeout(Duration::from_secs(1));
        writer.execute_batch("ROLLBACK").unwrap();
        signaling.join().unwrap().unwrap();
        progressing.join().unwrap();
        progress
            .expect("signal writer stalled local observation and healthy peer")
            .unwrap();
        assert!(app.state.lock().unwrap().local_updated > 0.0);
        let db = app.ctx.db().unwrap();
        assert_eq!(
            replica::rows(
                &db,
                "SELECT state FROM fleet_signals WHERE id='test-signal'",
                &[]
            )
            .unwrap()[0]["state"],
            "pending"
        );
    }

    #[test]
    fn sqlite_snapshot_contention_does_not_block_other_machine_updates() {
        let (_directory, fixture) = test_supervisor();
        let app = Arc::new(fixture);
        app.update("healthy", json!({"state":"connected","heartbeat":1}))
            .unwrap();
        let writer = app.ctx.db().unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        let slow = app.clone();
        let saving = std::thread::spawn(move || slow.update("slow", json!({"state":"connected"})));
        // Do not probe persistence.try_lock(): save_machines uses try_lock too,
        // so the observer could steal its one chance to begin the write.
        let snapshot_started = || {
            let state = app.state.lock().unwrap();
            state.machines.contains_key("slow") && !state.machines_dirty
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !snapshot_started() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            snapshot_started(),
            "snapshot never reached its serialized write"
        );
        let healthy = app.clone();
        let (tx, rx) = mpsc::channel();
        let progressing = std::thread::spawn(move || {
            let result = healthy.update("healthy", json!({"heartbeat":2}));
            let startup = healthy.update("new-peer", json!({"state":"connecting"}));
            tx.send((result, startup)).unwrap();
        });
        let progress = rx.recv_timeout(Duration::from_secs(1));
        writer.execute_batch("ROLLBACK").unwrap();
        saving.join().unwrap().unwrap();
        progressing.join().unwrap();
        let (heartbeat, startup) =
            progress.expect("unrelated heartbeat and startup stalled behind snapshot write");
        heartbeat.unwrap();
        startup.unwrap();
        assert!(app.state.lock().unwrap().machines_dirty);
        // The maintenance retry must save updates that arrived during the
        // blocked write even without another substantive peer update.
        app.save_machines().unwrap();
        let saved = replica::state_get(&app.ctx.db().unwrap(), "machines", Value::Null).unwrap();
        assert_eq!(saved["healthy"]["heartbeat"], 2);
        assert_eq!(saved["new-peer"]["state"], "connecting");
    }

    #[test]
    fn connection_startup_does_not_wait_for_collection_or_maintenance_locks() {
        let (_directory, fixture) = test_supervisor();
        let app = Arc::new(fixture);
        // A stopped context prevents any real transport from being launched.
        app.ctx
            .stop
            .store(true, std::sync::atomic::Ordering::Release);
        let _state = app.state.lock().unwrap();
        let _configuration = app.configuration.lock().unwrap();
        let _persistence = app.persistence.lock().unwrap();
        let mut threads = BTreeMap::new();
        app.start_connections(&[json!({"host":"test-peer"})], &mut threads);
        assert_eq!(threads.len(), 1);
        threads.remove("test-peer").unwrap().join().unwrap();
    }

    #[test]
    fn failed_observation_does_not_advance_the_local_heartbeat() {
        let (_directory, app) = test_supervisor();
        app.ctx
            .db()
            .unwrap()
            .execute_batch("DROP TABLE issue_workers")
            .unwrap();
        assert!(app.observe_local().is_err());
        assert_eq!(app.state.lock().unwrap().local_updated, 0.0);
    }

    #[test]
    fn heartbeat_progress_does_not_rewrite_durable_machine_snapshots() {
        let (_directory, app) = test_supervisor();
        let workers = json!([{"id":"worker","runs":[{"summary":"x".repeat(800_000)}]}]);
        app.update(
            "remote",
            json!({"state":"connected","heartbeat":1,"last_sync":1,"workers":workers,"pending":0,"conflicts":0,"applied_revision":"revision"}),
        )
        .unwrap();
        let db = app.ctx.db().unwrap();
        let saved = replica::state_get(&db, "machines", Value::Null).unwrap();
        for heartbeat in 2..100 {
            app.update("remote", json!({"heartbeat":heartbeat}))
                .unwrap();
            // The real heartbeat path also reports a successful sync time.
            app.update("remote", json!({"workers":workers,"pending":0,"conflicts":0,"applied_revision":"revision","last_sync":heartbeat})).unwrap();
        }
        assert_eq!(app.machine("remote")["heartbeat"], 99);
        assert_eq!(app.machine("remote")["last_sync"], 99);
        assert!(saved == replica::state_get(&db, "machines", Value::Null).unwrap());
        app.update("remote", json!({"state":"disconnected","error":"offline"}))
            .unwrap();
        let saved = replica::state_get(&db, "machines", Value::Null).unwrap();
        assert_eq!(saved["remote"]["state"], "disconnected");
        assert_eq!(saved["remote"]["heartbeat"], 99);
        assert_eq!(saved["remote"]["last_sync"], 99);
        assert!(saved["remote"]["workers"] == workers);
    }

    #[test]
    fn a_failed_machine_save_is_retried_even_when_the_fields_are_identical() {
        let (_directory, app) = test_supervisor();
        let db = app.ctx.db().unwrap();
        db.execute_batch("CREATE TRIGGER reject_machine_save BEFORE INSERT ON fleet_state WHEN new.key='machines' BEGIN SELECT RAISE(ABORT,'synthetic persistence failure'); END;").unwrap();
        let fields = json!({"state":"connected","applied_revision":"revision"});
        assert!(app.update("remote", fields.clone()).is_err());
        db.execute_batch("DROP TRIGGER reject_machine_save")
            .unwrap();
        app.update("remote", fields).unwrap();
        let saved = replica::state_get(&db, "machines", Value::Null).unwrap();
        assert_eq!(saved["remote"]["applied_revision"], "revision");
    }

    #[test]
    fn identical_machine_updates_do_not_acquire_the_database_writer_lock() {
        let (_directory, app) = test_supervisor();
        let fields = json!({"state":"connected","applied_revision":"revision"});
        app.update("remote", fields.clone()).unwrap();
        let writer = app.ctx.db().unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        // A write here would wait for the ten-second SQLite timeout and fail.
        app.update("remote", fields).unwrap();
        app.update("remote", json!({"heartbeat":42})).unwrap();
        app.update("remote", json!({"last_sync":42})).unwrap();
        assert_eq!(app.machine("remote")["heartbeat"], 42);
        assert_eq!(app.machine("remote")["last_sync"], 42);
        writer.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn rolling_upgrade_accepts_saved_revision_only_for_identical_definitions() {
        let workers = json!([{"id":"worker","config":{"enabled":true},"intent":"running"}]);
        let previous =
            json!({"desired_workers":workers,"desired_revision":"python-order-dependent-hash"});
        assert!(configuration_base_matches(
            &json!("python-order-dependent-hash"),
            "native-hash",
            &previous,
            &workers
        ));
        assert!(!configuration_base_matches(
            &json!("python-order-dependent-hash"),
            "native-hash",
            &previous,
            &json!([])
        ));
        assert!(configuration_base_matches(
            &json!("native-hash"),
            "native-hash",
            &previous,
            &json!([])
        ));
        assert!(!configuration_base_matches(
            &Value::Null,
            "native-hash",
            &json!({}),
            &workers
        ));
    }
}
