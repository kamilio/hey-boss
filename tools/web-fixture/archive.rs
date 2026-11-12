use super::*;
use rusqlite::{Connection, params};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub(super) fn seed(f: &Fixture, p: &str) -> Result<()> {
    let issue = |args: &[&str]| {
        let mut c = f.issue(p, &["--agent", "human:boss"]);
        c.args(args);
        run(c)
    };
    for (n, title) in [
        "Closed archive conversation 1",
        "Closed archive conversation 2",
        "Deleted archive conversation",
        "Recent closed issue",
    ]
    .into_iter()
    .enumerate()
    {
        let number = (n + 1).to_string();
        issue(&[
            "create",
            "--title",
            title,
            "--body",
            "Historical body: cold-search-needle 🦀. **Formatting survives.**",
        ])?;
        issue(&[
            "comment",
            &number,
            "--body",
            "Saved discussion **survives** archival.",
        ])?;
        issue(&[if n == 2 { "delete" } else { "close" }, &number])?;
    }
    let project = format!("named:{p}");
    let old = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64 - 6 * 86_400_000;
    let mut db = Connection::open(&f.db)?;
    let tx = db.transaction()?;
    tx.execute("UPDATE issues SET created_at=?1, updated_at=?1, closed_at=CASE WHEN closed_at IS NOT NULL THEN ?1 END, deleted_at=CASE WHEN deleted_at IS NOT NULL THEN ?1 END WHERE number<=3",[old])?;
    for table in ["comments", "events", "issue_status_updates"] {
        tx.execute(
            &format!("UPDATE {table} SET created_at=? WHERE issue_number<=3"),
            [old],
        )?;
    }
    let mut edit = tx.prepare("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES(?1,1,'human:boss','edited',?2,?3)")?;
    edit.execute(params![project, old, "{}"])?;
    for n in 0..48 {
        let data = json!({"before":{"title":format!("Earlier title {n}")},"after":{"title":format!("Later title {n}")}});
        edit.execute(params![project, old + n + 1, data.to_string()])?;
    }
    drop(edit);
    let root = f.root.path();
    let job = json!({"id":"archive-ui-run","project":{"id":project,"name":p},"issue":{"number":2,"title":"Closed archive conversation 2","body":"Saved job context"},
        "comments":[],"config":{"cwd":root,"prompt":"Saved prompt","enabled":false},
        "actor":{"id":"human:boss","kind":"human","session_id":null,"machine":"fixture","host":"fixture","pid":null,"process_start":null,"cwd":root,"source":"fixture","model":null},
        "owner_pid":1,"owner_start":"finished","machine":"fixture"});
    tx.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES('archive-ui-worker','managed',?1,1,?2)",
        params![json!({"name":"Archive fixture","directory":root,"projects":[project],"enabled":false}).to_string(),old])?;
    tx.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at,expanded_prompt,last_event,worker_id) VALUES('archive-ui-run',?1,2,?2,'human:boss','completed',1,'finished','fixture',?3,?3,?3,'Saved expanded prompt','Archived worker event 28','archive-ui-worker')",
        params![project,job.to_string(),old])?;
    for n in 1..=28 {
        tx.execute(
            "INSERT INTO worker_events(run_id,created_at,text) VALUES('archive-ui-run',?1,?2)",
            params![old, format!("Archived worker event {n}")],
        )?;
    }
    tx.commit()?;
    Ok(())
}

// The owner runs real archive maintenance. Publish the URL only after hot copies are gone.
pub(super) fn wait(f: &Fixture, signal: &mpsc::Receiver<()>) -> Result<bool> {
    let db = Connection::open_with_flags(&f.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let started = Instant::now();
    loop {
        let cold: bool = db.query_row("SELECT
            (SELECT count(*) FROM issues WHERE archive_key IS NOT NULL AND archive_cleanup=0)=3 AND
            EXISTS(SELECT 1 FROM worker_runs WHERE id='archive-ui-run' AND archive_key IS NOT NULL AND archive_cleanup=0) AND
            NOT EXISTS(SELECT 1 FROM comments WHERE issue_number<=3) AND
            (SELECT count(*) FROM worker_events)=1", [], |r| r.get(0))?;
        if cold {
            return Ok(true);
        }
        if started.elapsed() >= Duration::from_secs(180) {
            return Err("archive maintenance did not finish".into());
        }
        if signal.recv_timeout(Duration::from_millis(250)).is_ok() {
            return Ok(false);
        }
    }
}
