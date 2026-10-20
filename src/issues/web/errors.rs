use super::*;
use crate::error_inbox::{Event, Inbox, now};
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    List { project: String },
    Enable { project: String, enabled: bool },
    Ingest { project: String, events: Vec<Event> },
    Detail { project: String, id: String },
    Promote { project: String, id: String },
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
        Action::Detail { project, id } => inbox.detail(&project, &id, now()),
        Action::Promote { project, id } => {
            let evidence = inbox.snapshot(&project, &id)?;
            if evidence["issue"].is_number() {
                return Ok(evidence);
            }
            let operation = Operation::Create {
                title: format!(
                    "Investigate: {}",
                    evidence["title"]
                        .as_str()
                        .unwrap_or("Error")
                        .chars()
                        .take(160)
                        .collect::<String>()
                ),
                body: format!(
                    "Observed diagnostic samples at task creation. Counts describe collected logs, not unique failed requests.\n\n```json\n{}\n```",
                    serde_json::to_string_pretty(&evidence)?
                ),
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
