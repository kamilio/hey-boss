use super::*;
use crate::error_inbox::{Event, Inbox, now};
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    List {
        project: String,
    },
    Enable {
        project: String,
        enabled: bool,
    },
    Ingest {
        project: String,
        events: Vec<Event>,
    },
    Detail {
        project: String,
        id: String,
    },
    Promote {
        project: String,
        id: String,
        prompt: Option<String>,
    },
}
fn task_prompt(app: &App, project: &str, id: &str) -> Result<String> {
    let settings = app.execute(Some(project.into()), Operation::ProjectSettings, None)?;
    let template = settings["prompt_overrides"]["error_task"]
        .as_str()
        .unwrap_or(super::super::worker::DEFAULT_ERROR_TASK_PROMPT);
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
    let command = format!(
        "hey-boss errors --project {} show {}",
        quote(project),
        quote(id)
    );
    Ok(super::super::worker::render(template, |key| {
        (key == "command").then(|| command.clone())
    }))
}
pub(super) fn act(bytes: &[u8], app: &App) -> Result<Value> {
    let Backend::Local(path) = &app.backend else {
        return Err(Error::new(
            "local_only",
            "Open Errors on the collector's machine; remote inbox routing is not supported",
        ));
    };
    let mut inbox = Inbox::open(path)?;
    match serde_json::from_slice(bytes)? {
        Action::List { project } => inbox.list(&project, now()),
        Action::Enable { project, enabled } => inbox.enable(&project, enabled),
        Action::Ingest { project, events } => inbox.ingest(&project, &events, now()),
        Action::Detail { project, id } => {
            let mut detail = inbox.detail(&project, &id, now())?;
            detail["task_prompt"] = json!(task_prompt(app, &project, &id)?);
            Ok(detail)
        }
        Action::Promote {
            project,
            id,
            prompt,
        } => {
            let detail = inbox.detail(&project, &id, now())?;
            if detail["issue"].is_number() {
                return Ok(detail);
            }
            let prompt = prompt
                .map(Ok)
                .unwrap_or_else(|| task_prompt(app, &project, &id))?;
            if prompt.trim().is_empty() || prompt.len() > 32000 {
                return Err(Error::invalid("Task prompt must contain 1–32000 bytes"));
            }
            let evidence = inbox.snapshot(&project, &id, &prompt)?;
            let operation = Operation::Create {
                title: format!(
                    "Fix: {}",
                    evidence["title"]
                        .as_str()
                        .unwrap_or("Error")
                        .chars()
                        .take(160)
                        .collect::<String>()
                ),
                body: evidence["task_prompt"].as_str().unwrap_or(&prompt).into(),
                labels: vec!["error".into()],
                draft: false,
                attachments: vec![],
                at_top: false,
                blockers: vec![],
                then_titles: vec![],
            };
            let result = app.execute(
                Some(project.clone()),
                operation,
                Some(format!("error-inbox-{id}")),
            )?;
            let number = result["issue"]["number"]
                .as_i64()
                .or_else(|| result["number"].as_i64())
                .ok_or_else(|| {
                    Error::new(
                        "invalid_response",
                        "Task was created but its number was missing; retry to recover",
                    )
                })?;
            inbox.link(&project, &id, number)
        }
    }
}
