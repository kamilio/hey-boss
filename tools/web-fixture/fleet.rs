//! A private, read-only fleet socket for browser fixtures without real workers.
use super::*;
use std::{
    io::Write,
    os::unix::net::{UnixListener, UnixStream},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn serve(f: &Fixture, machines: Value) -> Result<()> {
    let listener = UnixListener::bind(f.root.path().join("fleet.sock"))?;
    let machines = Arc::new(machines);
    thread::spawn(move || {
        for stream in listener.incoming().map_while(std::result::Result::ok) {
            let machines = Arc::clone(&machines);
            thread::spawn(move || {
                let _ = respond(stream, &machines);
            });
        }
    });
    Ok(())
}
fn respond(mut stream: UnixStream, machines: &Value) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let request: Value = serde_json::from_str(&line)?;
    let reply = match request["kind"].as_str() {
        Some("overview" | "status") => {
            let mut reply = json!({"ok":true,"machines":machines,"signals":[],"conflicts":[]});
            for m in reply["machines"].as_array_mut().unwrap() {
                if m["state"] == "connected" {
                    m["heartbeat"] = json!(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs());
                }
            }
            reply
        }
        Some("subscribe") => {
            stream.write_all(b"event: connected\ndata: {}\n\n")?;
            loop {
                thread::sleep(Duration::from_secs(10));
                stream.write_all(b": heartbeat\n\n")?;
            }
        }
        _ => json!({"ok":false,"error":"Read-only fixture"}),
    };
    serde_json::to_writer(stream, &reply)?;
    Ok(())
}
pub(super) fn assigned_agent(f: &Fixture, p: &str) -> Result<()> {
    let session = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let title = "Keep conversations intact across device reconnects";
    for title in [
        title,
        "Review the finished design",
        "Ready for the next agent",
        "Manual assignment without recorded history",
    ] {
        run(f.issue(p, &["create", "--title", title]))?;
    }
    run(f.issue(p, &["claim", "1", "--agent", &format!("codex:{session}")]))?;
    run(f.issue(p, &["assign-to-boss", "2"]))?;
    run(f.issue(p, &["claim", "4", "--agent", "codex:missing"]))?;
    let mut history = fs::File::create(
        f.root
            .path()
            .join(format!("codex/sessions/rollout-fixture-{session}.jsonl")),
    )?;
    for (role, kind, text) in [
        ("user", "input_text", "Please preserve my conversation."),
        (
            "assistant",
            "output_text",
            "The correct assigned session is open.",
        ),
    ] {
        writeln!(
            history,
            "{}",
            json!({"type":"response_item","payload":{"type":"message","role":role,"content":[{"type":kind,"text":text}]}})
        )?;
    }
    let project = format!("named:{p}");
    rusqlite::Connection::open(&f.db)?.execute(
        "INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,session_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at) VALUES('assigned-run',?1,1,?2,'worker:assigned',?3,'completed',1,'fixture','local',2,2,3)",
        rusqlite::params![project,json!({"issue":{"title":title}}).to_string(),session])?;
    let run = |id, number, owner, started| json!({"id":id,"project_id":project,"project_name":p,"number":number,"title":title,"session_id":owner,"actor_id":"worker:assigned","state":"completed","started_at":started,"finished_at":3});
    serve(
        f,
        json!([
            {"host":"local","hostname":"This MacBook","state":"connected","workers":[{"id":"fixture","pid":1,"runs":[run("assigned-run",1,session,2),run("wrong-owner",1,"other",100)]}]},
            {"host":"devbox","hostname":"Devbox","state":"disconnected","heartbeat":0,"workers":[{"id":"remote","runs":[run("remote-run",5,session,2)]}]}
        ]),
    )
}
