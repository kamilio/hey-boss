//! Issue lifecycle integration for the shared Claude/Pi runtime.
use super::*;
use crate::agent_runtime::{AgentSession, Event, Launch, Provider, SessionRef, TurnStatus};
use std::collections::BTreeMap;

struct Owned {
    agent: AgentSession,
    path: PathBuf,
    job: Job,
}
impl Drop for Owned {
    fn drop(&mut self) {
        let path = self.path.clone();
        let job = self.job.clone();
        let protected = move || {
            Store::open(&path)
                .and_then(|s| s.worker_attempt_held(&job))
                .unwrap_or(true)
        };
        if protected() {
            self.agent.preserve_until(protected);
        } else {
            let _ = self.agent.stop();
        }
    }
}

pub(super) fn run(
    path: &Path,
    store: &mut Store,
    job: &mut Job,
    stop: &AtomicBool,
) -> Result<(String, String)> {
    validate_config(&job.config, &job.project)?;
    check_job(store, job, stop)?;
    let provider = job.config.provider;
    // Stable owned identity exists before the provider can execute a tool;
    // Claude's native session ID arrives only after its first user message.
    store.worker_prepare_provider(job)?;
    let agent = AgentSession::launch(Launch {
        codex_execution: Default::default(),
        model: None,
        provider,
        binary: None,
        cwd: job.config.cwd.clone().into(),
        resume: None,
        env: BTreeMap::from([
            ("HEY_BOSS_ISSUE_DB".into(), path.as_os_str().into()),
            (
                "HEY_BOSS_ISSUE_PROJECT".into(),
                job.project.id.clone().into(),
            ),
            (
                "HEY_BOSS_ISSUE_NUMBER".into(),
                job.number().to_string().into(),
            ),
            ("HEY_BOSS_WORKER_RUN".into(), job.id.clone().into()),
            ("HEY_BOSS_ISSUE_HOST".into(), "".into()),
        ]),
        output_schema: (provider == Provider::Claude)
            .then(|| turn_params("", "")["outputSchema"].clone()),
    })?;
    let mut owned = Owned {
        agent,
        path: path.into(),
        job: job.clone(),
    };
    store.worker_process(&job.id, owned.agent.pid())?;
    store.worker_event(&job.id, &format!("Launching {}", provider.name()), None)?;
    let (applied, _, _) = prompt(job);
    let text = if provider == Provider::Pi {
        format!(
            "{}\n\n{}",
            applied,
            include_str!("../agent_runtime/completion.md").trim()
        )
    } else {
        applied.clone()
    };
    let mut transcript = None;
    if let Some(reference) = owned.agent.state().session {
        attach(path, store, job, &reference, &applied, &mut transcript)?;
    }
    let mut turn = owned.agent.prompt(&text, None)?;
    // Claude announces its session in the first stream message. Keep the exact
    // initial input until then; never guess its most recent session on disk.
    let mut first_input = Some(text);
    let mut approvals = super::super::worker_approvals::Approvals::default();
    let mut input_requests = std::collections::BTreeSet::new();
    let mut last_message = String::new();
    let mut last_poll = Instant::now() - Duration::from_secs(2);
    let mut last_log = Instant::now();
    let mut activity = String::new();
    let mut model_started = false;
    let mut last_check = Instant::now() - Duration::from_secs(1);
    let outcome = (|| -> Result<(String, String)> {
        loop {
            if last_check.elapsed() >= Duration::from_millis(200) {
                check_job(store, job, stop)?;
                last_check = Instant::now();
            }
            for (response, cancelled) in approvals.poll()? {
                let id = response["id"]
                    .as_str()
                    .ok_or_else(|| Error::invalid("Missing approval identity"))?;
                if cancelled {
                    return Err(Error::new(
                        "blocked",
                        "Agent input was cancelled. Resume the saved session or explicitly retry.",
                    ));
                }
                if input_requests.remove(id) {
                    if provider == Provider::Claude {
                        owned
                            .agent
                            .respond_input(id, Some(&response["result"]["answers"].to_string()))?;
                    } else {
                        owned
                            .agent
                            .respond_input(id, response["result"]["input"].as_str())?;
                    }
                } else {
                    owned
                        .agent
                        .decide(id, response["result"]["allow"] == true)?;
                }
            }
            if last_poll.elapsed() >= Duration::from_secs(1) && !approvals.is_pending() {
                last_poll = Instant::now();
                let state = owned.agent.inspect()?;
                if state.turn.as_deref() == Some(&turn) && !state.awaiting_prompt_ack {
                    if let Some(instruction) = store.pending_broadcast(&job.id)? {
                        deliver(
                            &mut owned.agent,
                            store,
                            job,
                            &turn,
                            &instruction,
                            &mut transcript,
                        )?;
                    }
                }
            }
            let Some(event) = owned.agent.receive(Duration::from_millis(100))? else {
                continue;
            };
            if let Event::Session(reference) = &event {
                attach(path, store, job, reference, &applied, &mut transcript)?;
            }
            if transcript.is_some()
                && let Some(text) = first_input.take()
            {
                record(
                    &mut transcript,
                    &Event::Message {
                        role: "user".into(),
                        text,
                    },
                )?;
            }
            if !model_started
                && matches!(
                    &event,
                    Event::TextDelta { .. } | Event::Message { .. } | Event::ToolStarted { .. }
                )
            {
                store.worker_begin_claim(&job.id)?;
                model_started = true;
            }
            match &event {
                Event::TurnStarted { .. } => {
                    last_message.clear();
                    activity.clear();
                }
                Event::TextDelta { text } => {
                    activity.push_str(text);
                    if activity.len() > 4000 {
                        activity = activity
                            .chars()
                            .rev()
                            .take(2000)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                    }
                    if last_log.elapsed() > Duration::from_secs(1) {
                        store.worker_event(&job.id, &activity, None)?;
                        last_log = Instant::now();
                    }
                }
                Event::Message { text, .. } => {
                    last_message = text.clone();
                    activity.clear();
                    store.worker_event(&job.id, text, None)?;
                    record(&mut transcript, &event)?;
                }
                Event::ToolStarted { name, input, .. } => {
                    store.worker_event(&job.id, &format!("{name}: {input}"), None)?;
                    record(&mut transcript, &event)?;
                }
                Event::ToolCompleted { id, output, failed } => {
                    record(&mut transcript, &event)?;
                    if *failed {
                        store.worker_observation(job, "agent.tool_failed", id,
                            json!({"tool_call_id":id,"error_code":crate::event_log::producers::error_code(&output.to_string())}))?;
                    }
                }
                Event::Approval { id, payload, .. } => {
                    store.worker_observation(
                        job,
                        "agent.input_requested",
                        id,
                        json!({"request_id":id,"kind":"approval"}),
                    )?;
                    let request =
                        json!({"id":id,"method":"agent/tool","params":payload["request"]});
                    if !approvals.start(&request, job, None)? {
                        return Err(Error::new("blocked", "Unsupported agent approval"));
                    }
                    store.worker_event(&job.id, "Waiting for approval in Inbox", None)?;
                }
                Event::Input { id, payload } => {
                    store.worker_observation(
                        job,
                        "agent.input_requested",
                        id,
                        json!({"request_id":id,"kind":"input"}),
                    )?;
                    let request = if provider == Provider::Claude
                        && payload["request"]["tool_name"] == "AskUserQuestion"
                    {
                        json!({"id":id,"method":"agent/questions","params":payload["request"]["input"]})
                    } else if provider == Provider::Pi {
                        json!({"id":id,"method":"agent/input","params":payload})
                    } else {
                        return Err(Error::new(
                            "blocked",
                            "Unsupported agent input; resume the saved session",
                        ));
                    };
                    if !approvals.start(&request, job, None)? {
                        return Err(Error::new("blocked", "Unsupported agent input"));
                    }
                    input_requests.insert(id.clone());
                    store.worker_event(&job.id, "Waiting for your answer in Inbox", None)?;
                }
                Event::RequestCancelled { id } => {
                    approvals.observe(
                        &json!({"method":"serverRequest/resolved","params":{"requestId":id}}),
                    );
                    input_requests.remove(id);
                }
                Event::TurnCompleted {
                    id,
                    next_turn,
                    status,
                    output,
                    structured_output,
                    ..
                } if id == &turn => {
                    // Pi may publish the exact session file only after its first
                    // completion; persist the latest reference before returning.
                    if let Some(reference) = owned.agent.state().session {
                        attach(path, store, job, &reference, &applied, &mut transcript)?;
                    }
                    if output != &last_message && !output.is_empty() {
                        record(
                            &mut transcript,
                            &Event::Message {
                                role: "assistant".into(),
                                text: output.clone(),
                            },
                        )?;
                    }
                    if *status != TurnStatus::Completed {
                        return Err(Error::new(
                            "worker_error",
                            format!("Agent turn {status:?}: {output}"),
                        ));
                    }
                    if let Some(next) = next_turn {
                        turn = next.clone();
                        continue;
                    }
                    // Pending messages win over a completion report.
                    if let Some(instruction) = store.pending_broadcast(&job.id)? {
                        let text = instruction["text"].as_str().unwrap().to_owned();
                        let request = instruction["request_id"].as_str().unwrap();
                        if store.broadcast_result(request, "sending", None)? {
                            match owned.agent.prompt(&text, None) {
                                Ok(next) => {
                                    turn = next;
                                    store.broadcast_result(request, "delivered", None)?;
                                }
                                Err(error) => {
                                    store.broadcast_result(
                                        request,
                                        "uncertain",
                                        Some(&error.to_string()),
                                    )?;
                                    return Err(error.into());
                                }
                            }
                            record(
                                &mut transcript,
                                &Event::Message {
                                    role: "user".into(),
                                    text,
                                },
                            )?;

                            continue;
                        }
                    }
                    let report = structured_output
                        .clone()
                        .or_else(|| serde_json::from_str::<Value>(output.trim()).ok())
                        .filter(|v| {
                            matches!(v["status"].as_str(), Some("completed" | "blocked"))
                                && v["summary"].as_str().is_some_and(|s| !s.trim().is_empty())
                        });
                    if let Some(report) = report {
                        return Ok((
                            report["status"].as_str().unwrap().into(),
                            report["summary"].as_str().unwrap().into(),
                        ));
                    }
                    return Err(Error::new(
                        "worker_error",
                        format!(
                            "Agent ended without a completion report. Review the saved session.\n\n{output}"
                        ),
                    ));
                }
                _ => {}
            }
        }
    })();
    // Drop the process before dismissing unanswered requests.
    drop(owned);
    drop(approvals);
    outcome
}

fn attach(
    path: &Path,
    store: &mut Store,
    job: &mut Job,
    reference: &SessionRef,
    prompt: &str,
    transcript: &mut Option<crate::agent_transcript::Transcript>,
) -> Result<()> {
    if job.actor.session_id.as_deref() != Some(&reference.id) {
        store.worker_attach(job, &reference.id)?;
    }
    if job.session_ref.as_ref() != Some(reference) {
        job.session_ref = Some(reference.clone());
        store.worker_prompt(job, prompt)?;
    }
    if transcript.is_none() {
        *transcript = Some(crate::agent_transcript::Transcript::open(path, reference)?);
    }
    Ok(())
}
fn record(
    transcript: &mut Option<crate::agent_transcript::Transcript>,
    event: &Event,
) -> Result<()> {
    if let Some(transcript) = transcript {
        transcript.append(event)?;
    }
    Ok(())
}
fn deliver(
    agent: &mut AgentSession,
    store: &mut Store,
    job: &mut Job,
    turn: &str,
    instruction: &Value,
    transcript: &mut Option<crate::agent_transcript::Transcript>,
) -> Result<()> {
    let request = instruction["request_id"].as_str().unwrap();
    if !store.broadcast_result(request, "sending", None)? {
        return Ok(());
    }

    let text = instruction["text"].as_str().unwrap().to_owned();
    match agent.steer(turn, &text) {
        Ok(delivery) => {
            store.broadcast_result(request, "delivered", None)?;

            record(
                transcript,
                &Event::Message {
                    role: "user".into(),
                    text,
                },
            )?;
            store.worker_event(&job.id, &format!("Message accepted: {delivery:?}"), None)?;
        }
        Err(error) => {
            let state = agent.state();
            let result = if state.outcome_uncertain {
                "uncertain"
            } else if state.turn.as_deref() != Some(turn) {
                "queued"
            } else {
                "rejected"
            };
            store.broadcast_result(request, result, Some(&error.to_string()))?;
            if state.outcome_uncertain {
                return Err(error.into());
            }
        }
    }
    Ok(())
}
