use super::*;
use std::io::Write;

pub(super) fn seed(f: &Fixture, p: &str) -> Result<Value> {
    let session = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let standalone = "11111111-2222-3333-4444-555555555555";
    let actor_run = |mut c: Command, id: &str| {
        c.env_remove("HEY_BOSS_AGENT_ID").env("CODEX_THREAD_ID", id);
        run(c)
    };
    let title = "Improve reconnect diagnostics";
    run(f.issue(
        p,
        &[
            "--agent",
            "human:boss",
            "create",
            "--title",
            title,
            "--body",
            "Investigate why connections drop after sleep.",
        ],
    ))?;
    let agent = actor_run(f.issue(p, &["whoami"]), session)?;
    let db = rusqlite::Connection::open(&f.db)?;
    db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,session_id,state,owner_pid,owner_start,machine,started_at,updated_at) VALUES('historical-origin',?1,1,?2,'worker:source',?3,'running',1,'fixture',?4,1,1)",
        rusqlite::params![format!("named:{p}"),json!({"issue":{"title":title}}).to_string(),session,agent["agent"]["machine"].as_str().ok_or("missing actor machine")?])?;
    let path = |id: &str| {
        f.root
            .path()
            .join(format!("codex/sessions/rollout-fixture-{id}.jsonl"))
    };
    let first=json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Investigate reconnect failures and record any follow-up work."}]}}).to_string();
    let call = json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"create-follow-up","arguments":"hey-boss issue create --title \"Retain the creating session\""}});
    for id in [session, standalone] {
        fs::write(path(id), format!("{first}\n{call}\n"))?;
    }
    actor_run(
        f.issue(
            p,
            &[
                "create",
                "--title",
                "Retain the creating session",
                "--body",
                "A follow-up discovered during reconnect investigation.",
            ],
        ),
        session,
    )?;
    let artifact = actor_run(
        f.command(&[
            "artifact",
            "--project",
            p,
            "--json",
            "create",
            "--title",
            "Reconnect investigation",
            "--body",
            "# Findings\n\nKeep the source conversation connected to this investigation.",
        ]),
        session,
    )?;
    actor_run(
        f.issue(
            p,
            &[
                "create",
                "--title",
                "Created outside a worker",
                "--body",
                "This session still has saved creation context.",
            ],
        ),
        standalone,
    )?;
    run(f.issue(
        p,
        &[
            "--agent",
            "human:boss",
            "create",
            "--title",
            "Older issue with no recorded origin",
        ],
    ))?;
    actor_run(
        f.issue(
            p,
            &["create", "--title", "Follow-up from a disconnected device"],
        ),
        session,
    )?;
    db.execute_batch("UPDATE issues SET origin=NULL WHERE number=4;
        UPDATE issues SET origin=json_set(origin,'$.host','devbox-host','$.run.id','remote-origin') WHERE number=5;
        UPDATE worker_runs SET finished_at=2,state='completed' WHERE id='historical-origin';")?;
    writeln!(
        fs::OpenOptions::new().append(true).open(path(session))?,
        "{}",
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"The follow-up issue and investigation artifact are saved."}]}})
    )?;
    fleet::serve(
        f,
        json!([
            {"host":"local","hostname":agent["agent"]["host"],"state":"connected","workers":[]},
            {"host":"devbox","hostname":"devbox-host","state":"disconnected","heartbeat":0,"workers":[]}
        ]),
    )?;
    Ok(json!({"artifact":artifact["artifact"]["id"],"invocation": first.len()+1}))
}
