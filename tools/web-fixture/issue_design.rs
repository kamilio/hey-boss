use super::*;
use rusqlite::{Connection, params};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn architecture(f: &Fixture, p: &str) -> Result<()> {
    let issue = |args: &[&str]| run(f.issue(p, args));
    issue(&["settings", "set", "--prs-enabled"])?;
    for (title, body) in [
        (
            "Reconnect reliably after waking from sleep",
            "## Expected behavior\n\nReconnect without losing the conversation.\n\n- [ ] Restore the connection\n- [x] Preserve the draft\n\nKeep findings in the discussion below.",
        ),
        ("An issue ready for the next agent", ""),
        (
            "Explore the next release",
            "Refine the scope before starting work.",
        ),
        (
            "Waiting for the reconnect fix",
            "Resume when the blocking work is complete.",
        ),
        (
            "Completed design review",
            "The design and keyboard controls have been reviewed.",
        ),
        (
            "A long issue title that remains understandable and readable on even the narrowest supported phone screen",
            &format!(
                "A long description with an identifier: {}",
                "reconnect-".repeat(60)
            ),
        ),
    ] {
        issue(&[
            "create",
            "--title",
            title,
            "--body",
            body,
            "--label",
            "enhancement",
        ])?;
    }
    issue(&["claim", "1"])?;
    for step in 1..=22 {
        issue(&[
            "status",
            "1",
            "green",
            "--comment",
            &format!("Checking reconnect behavior, step {step}."),
        ])?;
    }
    for args in [
        &[
            "status",
            "1",
            "green",
            "--comment",
            "The fix passes tests. Checking the phone layout next.",
        ][..],
        &[
            "comment",
            "1",
            "--body",
            "The conversation and draft now survive reconnecting.",
        ],
        &[
            "pr",
            "add",
            "1",
            "https://github.com/example/hey-boss/pull/42",
            "--purpose",
            "fix",
        ],
        &[
            "create",
            "--title",
            "Preserve the unsent draft",
            "--body",
            "Keep text across reconnects.",
        ],
        &["unassign", "1"],
        &["subtask", "add", "1", "7"],
        &["edit", "3", "--draft"],
        &[
            "block",
            "4",
            "--by",
            "2",
            "--comment",
            "Waiting for the reconnect fix.",
        ],
        &["close", "5", "--comment", "Reviewed and complete."],
        &["claim", "6"],
        &["create", "--title", "Archived issue"],
        &["delete", "8"],
    ] {
        issue(args)?;
    }
    run(f.command(&[
        "artifact",
        "--project",
        p,
        "--json",
        "create",
        "--title",
        "Implementation notes",
        "--body",
        "## Findings\n\nThe reconnect behavior is now reliable.",
        "--issue",
        "1",
    ]))?;
    Ok(())
}

pub(super) fn timeline(f: &Fixture, p: &str) -> Result<()> {
    let issue = |args: &[&str]| {
        let mut c = f.issue(p, &["--agent", "human:boss"]);
        c.args(args);
        run(c)
    };
    let title = "Preserve the conversation when reconnecting";
    issue(&[
        "create",
        "--title",
        title,
        "--body",
        "Keep the conversation and unsent draft when a connection drops.\n\n- [x] Restore the session\n- [ ] Verify the phone layout",
        "--label",
        "enhancement",
    ])?;
    issue(&[
        "create",
        "--title",
        "A quiet issue",
        "--body",
        "No discussion yet.",
    ])?;
    issue(&["create", "--title", "Archived conversation"])?;
    issue(&["delete", "3"])?;
    let mut db = Connection::open(&f.db)?;
    let tx = db.transaction()?;
    let project = format!("named:{p}");
    let at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64 - 3_600_000;
    tx.execute(
        "UPDATE events SET created_at=? WHERE issue_number=1 AND action='created'",
        [at - 1000],
    )?;
    tx.execute("UPDATE issues SET created_at=? WHERE number=1", [at - 1000])?;
    let agent = "codex:timeline";
    let boss = "human:boss";
    let watcher = "watcher:github";
    for (id, kind) in [(agent, "codex"), (watcher, "system")] {
        tx.execute("INSERT INTO agents(id,metadata,last_seen) VALUES(?,?,?)",params![id,json!({"id":id,"kind":kind,"model":"gpt-6-astra","machine":"test","host":"MacBook Pro","cwd":f.root.path(),"session_id":"timeline","source":"fixture"}).to_string(),at])?;
    }
    let event = |seconds: i64, who: &str, action: &str, mut data: Value| {
        if who == agent {
            data["actor_model"] = json!("gpt-6-astra");
        }
        tx.execute("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES(?,1,?,?,?,?)",params![project,who,action,at+seconds*1000,data.to_string()])
    };
    let comment = |seconds: i64, who: &str, body: &str| {
        tx.execute("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES(?,1,?,?,?)",params![project,who,body,at+seconds*1000])?;
        event(
            seconds,
            who,
            "commented",
            json!({"comment_id":tx.last_insert_rowid(),"body":body}),
        )
    };
    for n in 0..36 {
        event(
            n,
            boss,
            "edited",
            json!({"before":{"labels":[]},"after":{"labels":[format!("triage-{n}")]}}),
        )?;
    }
    event(
        60,
        boss,
        "assigned",
        json!({"target":"machine:devbox","previous_assignee":null,"assignee":null}),
    )?;
    event(
        120,
        agent,
        "claimed",
        json!({"previous_assignee":null,"assignee":agent}),
    )?;
    comment(
        180,
        agent,
        "The draft now survives reconnecting. **Tests pass.**\n\nChecking the narrow layout next.",
    )?;
    event(
        240,
        agent,
        "edited",
        json!({"before":{"labels":["enhancement"]},"after":{"labels":["bug","needs-review"]}}),
    )?;
    event(
        300,
        agent,
        "assigned",
        json!({"target":"github","previous_assignee":agent,"assignee":watcher}),
    )?;
    event(
        360,
        watcher,
        "unassigned",
        json!({"previous_assignee":watcher}),
    )?;
    comment(
        420,
        boss,
        "Looks good on the phone. Keep the activity readable when labels or issue titles are long.",
    )?;
    event(
        480,
        boss,
        "assigned",
        json!({"target":"boss","previous_assignee":null,"assignee":boss}),
    )?;
    event(
        540,
        boss,
        "edited",
        json!({"before":{"labels":[],"title":"Previous title"},"after":{"labels":["a-long-label-that-must-wrap-without-making-the-phone-scroll-sideways","<img src=x onerror=alert(1)>"],"title":title}}),
    )?;
    tx.commit()?;
    Ok(())
}

pub(super) fn watch(f: &Fixture, p: &str) -> Result<()> {
    let issue = |args: &[&str]| {
        let mut c = f.issue(p, &["--agent", "human:boss"]);
        c.args(args);
        run(c)
    };
    let project = format!("named:{p}");
    let mut db = Connection::open(&f.db)?;
    let at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
    let machines = json!([
        {"node":"devbox","host":"devbox","hostname":"Devbox","state":"connected","workers":[]},
        {"node":"laptop","host":"local","hostname":"MacBook Pro","state":"connected","workers":[]},
        {"node":"offline","host":"remote","hostname":"Offline device with a long but readable name","state":"disconnected","workers":[]}
    ]);
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS fleet_state(key TEXT PRIMARY KEY,value TEXT NOT NULL)",
    )?;
    db.execute(
        "INSERT OR REPLACE INTO fleet_state VALUES('machines',?)",
        [machines.to_string()],
    )?;
    for (i, title) in [
        "Waiting for GitHub checks",
        "Required check failed while optional review is running",
        "An agent is fixing a failed required check",
        "Queued on the development machine",
        "Attach a pull request before enabling monitoring",
        "Review feedback after all checks have finished",
        "A failed refresh retains the last known evidence",
    ]
    .iter()
    .enumerate()
    {
        let n = i + 1;
        issue(&[
            "create",
            "--title",
            title,
            "--body",
            "Check the assignment and current GitHub status.",
        ])?;
        let number = n.to_string();
        if n == 4 {
            issue(&["assign", &number, "machine:devbox"])?;
            continue;
        }
        if n == 5 {
            continue;
        }
        let url = format!("https://github.com/example/project/pull/{n}");
        issue(&["pr", "add", &number, &url, "--purpose", "fix"])?;
        issue(&["assign", &number, "github"])?;
        let failed = n == 2 || n == 3;
        let evidence = json!({"repository":"example/project","number":n,"head":"0123456789abcdef","complete":n>=6,"required":[{"context":"Unit tests","state":if failed {"failure"}else{"satisfied"},"url":"https://github.com/example/project/actions/runs/123"},{"context":"Build and type checks","state":"satisfied","url":"https://github.com/example/project/actions/runs/124"}],"reviews":if n==6 {json!([{"body":"Please handle a connection drop while saving. The draft should remain available after reconnecting.","html_url":"https://github.com/example/project/pull/6#review"}])}else{json!([])},"source_errors":[],"ci_errors":[],"policy_errors":[]});
        let mut pr = json!({"head":evidence["head"],"checked_at":at,"evidence":evidence});
        if n == 7 {
            pr["error"] =
                json!("GitHub rate limit reached. Monitoring will retry after the cooldown.");
        }
        let mut status = json!({"prs":{&url:pr}});
        if failed || n == 6 {
            status["event"] = json!(format!("fixture-event-{n}"));
        }
        let tx = db.transaction()?;
        tx.execute("INSERT OR REPLACE INTO github_fetch_status(url,requested_at,started_at,finished_at,next_at,error) VALUES(?,?,?,?,?,?)",params![url,(n==6).then_some(at),at-if n==2 {1000}else{65000},at-60000,at+if n==7 {300000}else{30000},(n==7).then_some("GitHub rate limit reached")])?;
        tx.execute("INSERT OR REPLACE INTO issue_github_watches(project_id,issue_number,status) VALUES(?,?,?)",params![project,n,status.to_string()])?;
        tx.commit()?;
    }
    let tx = db.transaction()?;
    tx.execute("INSERT INTO agents(id,metadata,last_seen) VALUES('codex:fixture',?,?)",params![json!({"id":"codex:fixture","kind":"codex","machine":"devbox","host":"Devbox","cwd":f.root.path(),"session_id":"fixture","source":"visual fixture"}).to_string(),at])?;
    tx.execute_batch("UPDATE issues SET assignee=NULL WHERE number IN(2,6);UPDATE issues SET assignee='codex:fixture' WHERE number=3;")?;
    tx.commit()?;
    Ok(())
}
