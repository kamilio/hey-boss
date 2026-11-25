use crate::{emit, field, plugins::Plugins};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::mpsc,
    task::AbortHandle,
};
use uuid::Uuid;

/// The session owns children, rather than the inference that launched them.
/// Completed deliveries stay on disk until the parent notebook accepts them.
pub struct Delegates {
    directory: PathBuf,
    ready: mpsc::UnboundedSender<Value>,
    tasks: Mutex<HashMap<String, Option<AbortHandle>>>,
}
impl Delegates {
    pub fn open(state: &Path) -> Result<(Self, mpsc::UnboundedReceiver<Value>)> {
        let directory = state.with_extension("delegates");
        let (ready, receiver) = mpsc::unbounded_channel();
        let mut tasks = HashMap::new();
        if directory.exists() {
            for entry in fs::read_dir(&directory)? {
                let path = entry?.path();
                if path.extension().and_then(|v| v.to_str()) != Some("json")
                    || path
                        .file_stem()
                        .and_then(|v| v.to_str())
                        .and_then(|v| Uuid::parse_str(v).ok())
                        .is_none()
                {
                    continue;
                }
                let mut item: Value = serde_json::from_slice(&fs::read(&path)?)?;
                let id = field(&item, "id")?.to_owned();
                ensure!(
                    path.file_stem().and_then(|v| v.to_str()) == Some(&id),
                    "Delegate receipt has a different ID"
                );
                if item["status"] == "running" {
                    item["status"] = json!("completed");
                    item["failed"] = json!(true);
                    item["aggregatedOutput"] = json!(
                        "The owning session stopped before the delegate result was saved. The task may have run."
                    );
                    save(&path, &item)?;
                }
                tasks.insert(id, None);
                ready.send(item)?;
            }
        }
        Ok((
            Self {
                directory,
                ready,
                tasks: Mutex::new(tasks),
            },
            receiver,
        ))
    }
    pub fn pending(&self) -> bool {
        !self.tasks.lock().unwrap().is_empty()
    }
    pub fn cancel(&self) -> Result<()> {
        let tasks: Vec<_> = self.tasks.lock().unwrap().drain().collect();
        for (id, task) in tasks {
            if let Some(task) = task {
                task.abort();
            }
            self.accept(&id)?;
        }
        Ok(())
    }
    pub fn accept(&self, id: &str) -> Result<()> {
        ensure!(Uuid::parse_str(id).is_ok(), "Invalid delegate ID");
        let path = self.directory.join(format!("{id}.json"));
        match fs::remove_file(path) {
            Ok(()) => fs::File::open(&self.directory)?.sync_all()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let child = self.directory.join(format!("{id}.agent.json"));
        for path in [&child, &child.with_extension("jsonl")] {
            match fs::remove_file(path) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        let descendants = child.with_extension("delegates");
        if descendants.exists() {
            fs::remove_dir_all(descendants)?;
        }
        self.tasks.lock().unwrap().remove(id);
        Ok(())
    }
    pub fn start(
        &self,
        tools: &Plugins<'_>,
        url: &str,
        args: &Value,
        history: &[Value],
    ) -> Result<Value> {
        let provider = args
            .get("provider")
            .map(|_| field(args, "provider"))
            .transpose()?
            .unwrap_or("poe");
        ensure!(
            matches!(provider, "t3" | "codex" | "claude" | "pi" | "poe"),
            "Unknown delegate provider"
        );
        ensure!(
            args.get("fork").is_none_or(Value::is_boolean),
            "fork must be a boolean"
        );
        ensure!(
            args["fork"] != true || provider == "poe",
            "Only Poe delegates can fork this conversation"
        );
        ensure!(
            args.get("description").is_none_or(Value::is_string),
            "description must be a string"
        );
        let id = Uuid::new_v4().to_string();
        let mut item = json!({"id":id,"provider":provider,"request":field(args,"request")?,"description":args["description"].as_str().unwrap_or(""),"status":"running"});
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.directory)?;
        let record = self.directory.join(format!("{id}.json"));
        save(&record, &item)?;
        let mut request = json!({"turn":item["request"]});
        if args["fork"] == true {
            let mut messages = history.to_vec();
            // A fork can see pending parent calls, but must never execute them again.
            for (index, message) in history.iter().enumerate() {
                for call in message["tool_calls"].as_array().into_iter().flatten() {
                    if !history[index + 1..]
                        .iter()
                        .any(|m| m["toolCallId"] == call["id"])
                    {
                        messages.push(json!({"role":"tool","toolCallId":call["id"],"name":call["function"]["name"],"content":json!({"status":"running_in_parent"}).to_string()}));
                    }
                }
            }
            request["history"] = json!(messages);
        }
        let child = if provider == "poe" {
            None
        } else {
            let mut cmd = if provider == "t3" {
                let mut cmd = Command::new("node");
                cmd.arg(tools.resources.join("hey-t3.mjs"))
                    .arg("--conversation");
                cmd
            } else {
                let mut cmd = Command::new(
                    std::env::var_os("HEY_BOSS_CLI_PATH").unwrap_or_else(|| "hey-boss".into()),
                );
                cmd.args(["workspace-chat", "--provider", provider]);
                cmd
            };
            match cmd
                .current_dir(tools.cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(child) => Some(child),
                Err(error) => {
                    fs::remove_file(record)?;
                    return Err(error).context("Could not launch delegate");
                }
            }
        };
        let state = self.directory.join(format!("{id}.agent.json"));
        let (home, cwd, login, resources, http, url) = (
            tools.home.to_owned(),
            tools.cwd.to_owned(),
            tools.login.to_owned(),
            tools.resources.to_owned(),
            tools.http.clone(),
            url.to_owned(),
        );
        let ready = self.ready.clone();
        let parent_events = crate::EVENTS.try_with(Clone::clone).ok();
        let task_id = id.clone();
        let task = tokio::spawn(async move {
            let mut child = child;
            let result: Result<String> = async {
                let Some(child) = child.as_mut() else {
                    let (events, mut output) = mpsc::unbounded_channel();
                    let (_ack, mut acks) = mpsc::unbounded_channel();
                    let (delegates, mut completions) = Delegates::open(&state)?;
                    let mut tools = Plugins {
                        delegates: Some(&delegates),
                        home: &home,
                        cwd: &cwd,
                        login: &login,
                        resources: &resources,
                        http: &http,
                        focus: None,
                        acks: &mut acks,
                    };
                    crate::EVENTS
                        .scope(
                            events,
                            crate::agent::run_job(
                                &state,
                                &url,
                                request,
                                &mut tools,
                                &mut completions,
                            ),
                        )
                        .await?;
                    let mut final_output = None;
                    while let Ok(event) = output.try_recv() {
                        if let Some(turn) = event.get("TurnCompleted") {
                            final_output = turn["output"].as_str().map(str::to_owned);
                        }
                    }
                    return final_output.context("Delegate finished without a final response");
                };
                let mut input = child.stdin.take().context("Missing delegate input")?;
                input.write_all(format!("{request}\n").as_bytes()).await?;
                let mut lines =
                    BufReader::new(child.stdout.take().context("Missing delegate output")?).lines();
                while let Some(line) = lines.next_line().await? {
                    let event: Value =
                        serde_json::from_str(&line).context("Invalid delegate event")?;
                    if let Some(error) = event["error"].as_str() {
                        bail!("{error}");
                    }
                    if let Some(link) = event["Session"]["link"].as_str() {
                        item["link"] = json!(link);
                        save(&record, &item)?;
                        let event = json!({"ToolStarted":{"id":task_id,"input":item}});
                        if let Some(events) = &parent_events {
                            events.send(event)?;
                        } else {
                            emit(event)?;
                        }
                    }
                    if event.get("Approval").is_some() {
                        bail!("The child requested approval; no permission was granted.");
                    }
                    if let Some(turn) = event.get("TurnCompleted") {
                        ensure!(
                            turn["status"] == "Completed",
                            "The child did not finish: {}",
                            turn["output"]
                        );
                        return Ok(field(turn, "output")?.to_owned());
                    }
                }
                bail!(
                    "Delegate exited without a final response ({})",
                    child.wait().await?
                )
            }
            .await;
            // Closing stdin releases only this owned child. T3 also settles its thread.
            item["status"] = json!("completed");
            item["failed"] = json!(result.is_err());
            item["aggregatedOutput"] = json!(result.unwrap_or_else(|error| format!("{error:#}")));
            if let Err(error) = save(&record, &item) {
                item["failed"] = json!(true);
                item["aggregatedOutput"] =
                    json!(format!("Could not save delegate result: {error:#}"));
            }
            let _ = ready.send(item);
            if let Some(child) = child.as_mut() {
                let _ = child.wait().await;
            }
        });
        self.tasks
            .lock()
            .unwrap()
            .insert(id.clone(), Some(task.abort_handle()));
        emit(
            json!({"ToolStarted":{"id":id,"input":{"provider":provider,"request":args["request"],"description":args["description"].as_str().unwrap_or("")}}}),
        )?;
        Ok(json!({"id":id,"status":"running"}))
    }
}
impl Drop for Delegates {
    fn drop(&mut self) {
        for task in self.tasks.get_mut().unwrap().values().flatten() {
            task.abort();
        }
    }
}
fn save(path: &Path, item: &Value) -> Result<()> {
    let directory = path.parent().context("Missing delegate directory")?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut file, item)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

pub fn follow_up(item: Value) -> Value {
    json!({"turn":item.to_string(),"delegate_result":item})
}
