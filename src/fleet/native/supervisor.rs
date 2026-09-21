use super::{
    Result,
    context::{Context, atomic_json, hash, id, now, read_frame, read_json, send},
    control, conversation,
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
    events: VecDeque<Value>,
    sequence: u64,
    epoch: String,
    build: String,
    desired_build: Value,
    connections: BTreeMap<String, (u32, SyncSender<Value>)>,
    waiters: BTreeMap<String, SyncSender<Value>>,
    deploying: bool,
}
pub(super) struct Supervisor {
    pub ctx: Context,
    state: Mutex<State>,
    persistence: Mutex<()>,
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
        let local = ctx.workers()?;
        if !ctx.state.join("fleet-main.json").exists() {
            atomic_json(
                &ctx.state.join("fleet-main.json"),
                &json!({"role":"controller","workers":definitions(&local)}),
            )?;
        }
        let build = ctx.build()?;
        Ok(Arc::new(Self {
            ctx,
            persistence: Mutex::new(()),
            state: Mutex::new(State {
                machines,
                local,
                local_updated: now(),
                events: VecDeque::new(),
                sequence: 0,
                epoch: id()?,
                build,
                desired_build: Value::Null,
                connections: BTreeMap::new(),
                waiters: BTreeMap::new(),
                deploying: false,
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
        let _persistence = self.persistence.lock().unwrap();
        let machines = {
            let mut state = self.state.lock().unwrap();
            let m = state
                .machines
                .entry(host.into())
                .or_insert_with(|| json!({"host":host}));
            m.as_object_mut().unwrap().extend(
                fields
                    .as_object()
                    .ok_or_else(|| invalid("Invalid fleet state update"))?
                    .clone(),
            );
            json!(state.machines)
        };
        replica::state_set(&self.ctx.db()?, "machines", &machines)?;
        Ok(())
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
        let db = self.ctx.db()?;
        let signals = replica::rows(
            &db,
            "SELECT * FROM fleet_signals ORDER BY created_at DESC LIMIT 100",
            &[],
        )?;
        let conflicts = replica::rows(
            &db,
            "SELECT id,node,seq,table_name,reason,created_at,substr(data,1,8192) AS saved_change FROM fleet_conflicts WHERE resolved=0 ORDER BY created_at DESC LIMIT 100",
            &[],
        )?;
        let state = self.state.lock().unwrap();
        let mut machines = vec![
            json!({"host":"local","hostname":crate::issues::identity::host(),"node":self.ctx.node,"role":"supervisor","state":"connected","heartbeat":state.local_updated,"workers":state.local,"pending":0,"build":state.build}),
        ];
        for (host, m) in &state.machines {
            if host == "local" {
                continue;
            }
            let mut m = m.clone();
            if m["role"] == "agent" {
                m["role"] = json!("companion");
            }
            machines.push(m);
        }
        Ok(
            json!({"ok":true,"supervisor":self.ctx.node,"controller":self.ctx.node,"epoch":state.epoch,"sequence":state.sequence,"desired_build":state.desired_build,"machines":machines,"events":state.events,"signals":signals,"conflicts":conflicts}),
        )
    }
    pub fn overview(&self) -> Result<Value> {
        let mut status = self.status()?;
        let visible = replica::rows(
            &self.ctx.db()?,
            "SELECT id FROM projects WHERE hidden_at IS NULL",
            &[],
        )?
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect::<BTreeSet<_>>();
        for m in status["machines"].as_array_mut().unwrap() {
            for w in m["workers"].as_array_mut().into_iter().flatten() {
                let runs = w["runs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|r| visible.contains(r["project_id"].as_str().unwrap_or("")))
                    .map(|r| {
                        let mut v = json!({});
                        for k in [
                            "id",
                            "project_id",
                            "project_name",
                            "number",
                            "title",
                            "state",
                            "started_at",
                            "finished_at",
                        ] {
                            v[k] = r[k].clone();
                        }
                        for k in ["summary", "last_event"] {
                            v[k] = json!(
                                r[k].as_str()
                                    .unwrap_or("")
                                    .chars()
                                    .take(1000)
                                    .collect::<String>()
                            );
                        }
                        v
                    })
                    .collect::<Vec<_>>();
                w["runs"] = json!(runs);
            }
        }
        status["events"] = json!([]);
        for c in status["conflicts"].as_array_mut().unwrap() {
            c.as_object_mut().unwrap().remove("saved_change");
        }
        Ok(status)
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
            .flat_map(|w| w["runs"].as_array().into_iter().flatten())
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
        // Serialize controls and config-file changes under the same service mutex.
        let state = self.state.lock().unwrap();
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
        let mut saved = read_json(&self.ctx.desired, json!({}))?;
        let default = if host == "local" {
            read_json(&self.ctx.state.join("fleet-main.json"), json!({}))?["workers"].clone()
        } else {
            state
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
        atomic_json(&self.ctx.desired, &saved)?;
        drop(state);
        self.event(host, "signal", &format!("{action} queued for {worker}"));
        Ok(json!({"ok":true,"id":identifier,"state":"pending"}))
    }
    fn local_config(&self, host: &str, changes: &[Value], workers: &Value) -> Result<Value> {
        if changes.is_empty() {
            return Ok(workers.clone());
        }
        let _state = self.state.lock().unwrap();
        let previous = _state.machines.get(host).cloned().unwrap_or(Value::Null);
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
        let mut saved = read_json(&self.ctx.desired, json!({}))?;
        saved["machines"][host]["workers"] = updated.clone();
        atomic_json(&self.ctx.desired, &saved)?;
        drop(_state);
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
        let hello = rx
            .recv_timeout(Duration::from_secs(15))
            .map_err(|_| invalid("Companion is missing or has an incompatible fleet protocol"))??
            .ok_or_else(|| invalid("Companion closed before hello"))?;
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
        self.update(host,json!({"node":node,"hostname":hello["hostname"],"state":"connected","role":"agent","heartbeat":now(),"build":hello["build"],"workers":hello["workers"],"desired_workers":workers,"desired_revision":revision,"applied_revision":hello["revision"],"pending":hello.get("pending").unwrap_or(&json!(0)),"error":null}))?;
        self.event(host, "connected", "Companion connected");
        send(
            &mut input,
            json!({"kind":"configure","controller":self.ctx.node,"revision":revision,"workers":workers}),
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
                    send(
                        &mut input,
                        json!({"kind":"pull","payload":payload,"receipts":receipts}),
                    )?;
                    self.update(host,json!({"workers":message["workers"],"pending":message["pending"],"conflicts":message["conflicts"],"applied_revision":message["revision"],"last_sync":now()}))?;
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
                    if updated != revision || !self.machine(host)["configuration_error"].is_null() {
                        workers = current;
                        revision = updated;
                        send(
                            &mut input,
                            json!({"kind":"configure","controller":self.ctx.node,"revision":revision,"workers":workers}),
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
        self.update(host, json!({"deployment":"updating"}))?;
        self.event(host, "deployment", "Installing desired software");
        let mut command = Command::new(&self.ctx.binary);
        command.args(["upgrade", "--json"]);
        if host == "local" {
            command.arg("--local-only");
        } else {
            command.args(["--host", host]);
        }
        if let Ok(source) = std::fs::read_to_string(self.ctx.state.join("upgrade-source")) {
            command.args(["--source", source.trim()]);
        }
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
        self.update(
            host,
            json!({"deployment":"current","deployment_error":null}),
        )?;
        self.event(host, "deployment", "Software deployment complete");
        if let Some((pid, _)) = self.state.lock().unwrap().connections.get(host) {
            unsafe { libc::kill(*pid as i32, libc::SIGTERM) };
        }
        Ok(())
    }
    fn tick(
        self: &Arc<Self>,
        threads: &mut BTreeMap<String, std::thread::JoinHandle<()>>,
    ) -> Result<()> {
        let hosts = self.ctx.inventory()?;
        let observed = self.ctx.workers()?;
        let main = read_json(&self.ctx.state.join("fleet-main.json"), json!({}))?;
        let mut desired = {
            let _state = self.state.lock().unwrap();
            let mut saved = read_json(&self.ctx.desired, json!({}))?;
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
                atomic_json(&self.ctx.desired, &saved)?;
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
                desired.as_array_mut().unwrap().push(definition(discovered));
            }
        }
        let main = json!({"role":"controller","workers":desired,"revision":hash(&desired)});
        atomic_json(&self.ctx.state.join("fleet-main.json"), &main)?;
        {
            let Some(_lock) = self.ctx.lock("fleet-worker-control.lock", false)? else {
                return Ok(());
            };
            let failures = control::configure_workers(&self.ctx, desired.as_array().unwrap())?;
            if !failures.is_empty() {
                self.event("local", "configuration", &failures.join("; "));
            }
        }
        control::reconcile(&self.ctx, &main)?;
        {
            let mut state = self.state.lock().unwrap();
            state.local = observed;
            state.local_updated = now();
        }
        self.event("local", "heartbeat", "Worker state refreshed");
        for entry in &hosts {
            let host = entry["host"].as_str().unwrap();
            if !threads.get(host).is_some_and(|t| !t.is_finished()) {
                let app = self.clone();
                let host = host.to_owned();
                threads.insert(
                    host.clone(),
                    std::thread::spawn(move || app.connection(host)),
                );
            }
        }
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
        if let Ok(source) = std::fs::read_to_string(self.ctx.state.join("upgrade-source")) {
            {
                let fingerprint =
                    super::context::source_build(std::path::Path::new(source.trim()))?;
                let build = &json!(fingerprint);
                self.state.lock().unwrap().desired_build = build.clone();
                let db = self.ctx.db()?;
                let previous = replica::state_get(&db, "desired_build", Value::Null)?;
                replica::state_set(&db, "desired_build", build)?;
                if !previous.is_null() && previous != *build {
                    self.event(
                        "local",
                        "deployment",
                        "Source changed; automatic deployment scheduled",
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
        }
        Ok(())
    }
    fn scheduler(self: Arc<Self>) {
        let mut threads = BTreeMap::new();
        while !self.ctx.stopped() {
            if let Err(e) = self.tick(&mut threads) {
                self.event("local", "error", &e.to_string());
            }
            self.ctx.wait(Duration::from_secs(5));
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
        let bytes = serde_json::to_vec(&result)?;
        if bytes.len() > crate::issues::WIRE_LIMIT {
            return Err(invalid("Supervisor response exceeds 16 MiB"));
        }
        stream.write_all(&bytes)?;
        Ok(())
    }
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
    let mobile = ctx.clone();
    std::thread::spawn(move || super::mobile::run(mobile));
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
