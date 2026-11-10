//! Selected operational evidence. Raw prompts, commands and responses stay in
//! their source stores; this log carries identities, outcomes and error codes.
use super::*;

pub(crate) fn error_code(text: &str) -> Option<&'static str> {
    let text = text.to_ascii_lowercase();
    if text.contains("no space left on device")
        || text.contains("enospc")
        || text.contains("disk full")
    {
        Some("disk_full")
    } else if text.contains("disk quota exceeded") || text.contains("edquot") {
        Some("disk_quota")
    } else if text.contains("rate limit")
        || text.contains("rate_limit")
        || text.contains("quota exhausted")
    {
        Some("rate_limited")
    } else if text.contains("timed out") || text.contains("timeout") {
        Some("timeout")
    } else if text.contains("connection refused")
        || text.contains("connection reset")
        || text.contains("network is unreachable")
    {
        Some("connection_failed")
    } else {
        None
    }
}

pub(crate) fn codex_tool_failure(item: &Value) -> Option<(&str, Value)> {
    let output = match item["type"].as_str()? {
        "commandExecution"
            if item["status"] == "failed" || item["exitCode"].as_i64().is_some_and(|c| c != 0) =>
        {
            &item["aggregatedOutput"]
        }
        "mcpToolCall"
            if item["status"] == "failed"
                || item["result"]["isError"] == true
                || !item["error"].is_null() =>
        {
            if item["error"].is_null() {
                &item["result"]
            } else {
                &item["error"]
            }
        }
        "dynamicToolCall" if item["success"] == false => &item["contentItems"],
        _ => return None,
    };
    let id = item["id"].as_str()?;
    Some((
        id,
        json!({"tool_call_id":id,"tool_kind":item["type"],"exit_code":item["exitCode"],"error_code":error_code(&output.to_string())}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_evidence_requires_failure_and_never_copies_raw_output() {
        let mut item = json!({"id":"call","type":"commandExecution","exitCode":0,"aggregatedOutput":"No space left on device: secret"});
        assert!(codex_tool_failure(&item).is_none());
        item["exitCode"] = json!(1);
        let (_, data) = codex_tool_failure(&item).unwrap();
        assert_eq!(data["error_code"], "disk_full");
        assert!(!data.to_string().contains("secret"));
        for item in [
            json!({"id":"mcp","type":"mcpToolCall","result":{"isError":true,"text":"ENOSPC"}}),
            json!({"id":"dynamic","type":"dynamicToolCall","success":false,"contentItems":"ENOSPC"}),
        ] {
            assert_eq!(
                codex_tool_failure(&item).unwrap().1["error_code"],
                "disk_full"
            );
        }
    }
}

pub(crate) fn worker(
    db: &Connection,
    job: &crate::issues::worker::Job,
    kind: &str,
    key: &str,
    time: i64,
    mut data: Value,
) -> Result<()> {
    let id = format!(
        "agent:{}",
        hey_boss_sync::digest(serde_json::to_string(&(job.id.as_str(), kind, key))?.as_bytes())
    );
    // Provider redelivery may have a new observation time. The first durable
    // observation owns that time; do not publish a second copy on replay.
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM operational_events WHERE id=?1)",
        [&id],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let session: Option<String> = db.query_row(
        "SELECT session_id FROM worker_runs WHERE id=?1",
        [&job.id],
        |r| r.get(0),
    )?;
    data["session_id"] = json!(session);
    data["issue_number"] = json!(job.number());
    data["worker_id"] = json!(job.worker_id);
    data["provider"] = json!(job.config.provider.name());
    append(
        db,
        &Event {
            id,
            kind: kind.into(),
            version: 1,
            origin: job.machine.clone(),
            time,
            project: Some(job.project.id.clone()),
            subject: job.id.clone(),
            data,
        },
    )?;
    Ok(())
}

pub(crate) fn job(db: &Connection, run: &crate::jobs::Run) -> Result<()> {
    let (kind, time) = match run.state.as_str() {
        "pending" => ("job.queued", run.created_at),
        "running" => ("job.started", run.started_at.unwrap()),
        "skipped" => ("job.skipped", run.finished_at.unwrap()),
        _ => ("job.finished", run.finished_at.unwrap()),
    };
    append(
        db,
        &Event {
            id: format!("{}:{}", kind, run.id),
            kind: kind.into(),
            version: 1,
            origin: crate::issues::identity::machine()?,
            time,
            project: Some(run.snapshot.project_id.clone()),
            subject: run.id.clone(),
            data: json!({"job_id":run.snapshot.job_id,"revision":run.snapshot.revision,
            "state":run.state,"machine":run.machine,"session_id":run.session_id,
            "parent_run_id":run.parent_run_id,"target_machine":run.target_machine,
            "scheduled_at":run.scheduled_at,"trigger":run.trigger,
            "error_code":run.reason.as_deref().and_then(error_code),
            "skip_reason":(run.state=="skipped").then_some(if run.reason.as_deref()==Some("overlap") {"overlap"} else {"condition"})}),
        },
    )?;
    Ok(())
}
