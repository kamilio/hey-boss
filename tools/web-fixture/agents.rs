use super::*;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub(super) fn supervisor(f: &Fixture) -> Result<Service> {
    let mut child = Service(
        f.command(&["fleet", "supervisor"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while !f.root.path().join("fleet.sock").exists() {
        if let Some(status) = child.0.try_wait()? {
            return Err(format!("fixture supervisor exited: {status}").into());
        }
        if Instant::now() >= deadline {
            return Err("fixture supervisor did not start".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(child)
}

pub(super) fn conversations(f: &Fixture) -> Result<()> {
    for p in ["Atlas", "Hey Boss", "Studio"] {
        if p != "Atlas" {
            f.init(p)?;
        }
        run(f.issue(p, &["create", "--title", "Fixture issue"]))?;
    }
    let session = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    fs::write(
        f.root
            .path()
            .join(format!("codex/sessions/rollout-fixture-{session}.jsonl")),
        include_bytes!("agent-session.jsonl"),
    )?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
    let db = rusqlite::Connection::open(&f.db)?;
    let mut machines = Vec::new();
    for (host, hostname, worker, project, tasks) in [
        (
            "local",
            "This MacBook",
            "local-service",
            "Atlas",
            &[
                ("atlas-1", "Atlas", "Make reconnecting feel effortless"),
                ("boss-1", "Hey Boss", "A calmer home for your agents"),
                ("done-1", "Atlas", "Keep every message in order"),
            ][..],
        ),
        (
            "devbox",
            "Devbox",
            "remote-service",
            "Atlas",
            &[("atlas-2", "Atlas", "Give search a little more polish")][..],
        ),
        (
            "studio",
            "Studio Mac",
            "studio-service",
            "Studio",
            &[("studio-1", "Studio", "Finish the new listening room")][..],
        ),
    ] {
        let mut runs = Vec::new();
        for &(id, p, title) in tasks {
            let project_id = format!("named:{p}");
            let finished = (id == "done-1").then_some(now - 30_000);
            db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,session_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at,expanded_prompt) SELECT ?1,project_id,number,?2,created_by,?3,'completed',1,'fixture',?4,?5,?5,?5,'Original request' FROM issues WHERE project_id=?6 AND number=1", rusqlite::params![id,json!({"issue":{"title":title}}).to_string(),session,host,now-180_000,project_id])?;
            runs.push(json!({"id":id,"project_id":project_id,"project_name":p,"number":1,"title":title,"state":if finished.is_some() {"completed"} else {"running"},"started_at":now-180_000,"finished_at":finished,"last_event":"Checking the reconnect flow and preserving your conversation."}));
        }
        machines.push(json!({"host":host,"hostname":hostname,"state":if host=="studio" {"disconnected"} else {"connected"},"heartbeat":now/1000-90,"workers":[{"id":worker,"pid":123,"managed":true,"intent":"running","config":{"enabled":true,"name":format!("{project} worker")},"runs":runs}]}));
    }
    fleet::serve(f, json!(machines))
}

pub(super) fn workers(f: &Fixture, p: &str) -> Result<()> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
    let task = |id, number, title, activity| json!({"id":id,"project_id":p,"project_name":"Poe Code","number":number,"title":title,"state":"running","started_at":now-240_000,"finished_at":null,"last_event":activity});
    let config = |name, slots, enabled| json!({"name":name,"concurrency":slots,"enabled":enabled,"projects":[p],"directory":"/Users/kamil/Workspace/poe-code"});
    let mut workers = vec![
        json!({"id":"poe-code","managed":true,"intent":"running","pid":12,"config":config("Poe Code",2,true),"active":2,"free":0,"eligible":2,"runs":[task("run-1",688,"Keep workers synchronized","Checking recovery after a disconnected machine returns."),task("run-2",689,"Simplify the worker dashboard","Checking the phone layout and keyboard navigation.")]}),
        json!({"id":"tools","managed":true,"intent":"running","pid":13,"config":config("Tools",1,true),"active":0,"free":1,"eligible":0}),
        json!({"id":"independent","managed":false,"pid":14,"config":config("Poe Code",2,true)}),
    ];
    for n in 0..36 {
        let mut cfg = config("Stopped", 2, false);
        cfg["name"] = json!(format!("Saved worker {}", n + 1));
        workers.push(json!({"id":format!("stopped-{n}"),"managed":true,"intent":"stop","pid":null,"config":cfg}));
    }
    for n in 0..3 {
        let mut cfg = config("Worker", 3, true);
        cfg["name"] = json!(format!("Worker {}", n + 1));
        cfg["projects"] = json!([
            "github.com/kamilio/ashby-mcp",
            "github.com/kamilio/hey-gh",
            "github.com/kamilio/hey-proxy"
        ]);
        workers.push(json!({"id":format!("shared-{n}"),"managed":true,"intent":"running","pid":100+n,"config":cfg,"active":0}));
    }
    let mut remote = config("Remote worker", 2, true);
    remote["directory"] = json!("/home/kamil/Workspace/poe-code");
    fleet::serve(
        f,
        json!([
            {"host":"local","hostname":"MacBook Pro","state":"connected","workers":workers,"desired_revision":"1","applied_revision":"1"},
            {"host":"devbox","hostname":"Devbox","state":"disconnected","heartbeat":now/1000-180,"desired_revision":"1","applied_revision":"old","workers":[{"id":"remote","managed":true,"intent":"running","pid":42,"config":remote,"runs":[task("offline-run",688,"Verify reconnect behavior","Last seen checking machine connectivity.")]}]},
            {"host":"studio","hostname":"Studio Mac","state":"connected","desired_revision":"1","applied_revision":"1","workers":[{"id":"paused","managed":true,"intent":"pause","pid":43,"config":config("Review queue",2,false),"active":1,"runs":[task("paused-run",688,"Finish the current review","Wrapping up the review before pickup pauses.")]}]}
        ]),
    )
}
