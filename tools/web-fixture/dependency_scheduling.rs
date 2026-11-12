//! An independent sibling and a dependent share a parent, but only one needs rework.
use super::*;
use rusqlite::{Connection, params};

const PROJECT: &str = "Connectors";
fn seed(f: &Fixture) {
    f.init(PROJECT).unwrap();
    run(f.issue(PROJECT, &["create", "--title", "Connector integration"])).unwrap();
    for title in [
        "Define connector contract",
        "Build Settings independently",
        "Implement OAuth after the contract",
    ] {
        run(f.issue(PROJECT, &["subtask", "create", "1", "--title", title])).unwrap();
    }
    run(f.issue(PROJECT, &["blocked-by", "4", "2"])).unwrap();
    run(f.issue(PROJECT, &["block", "4"])).unwrap();
}

#[test]
#[ignore = "requires a built hey-boss CLI"]
fn cli_enforces_declared_dependencies_across_handoffs_and_old_writes() {
    let f = installed_fixture();
    seed(&f);
    let command = |args: &[&str]| {
        let mut c = f.issue(PROJECT, args);
        c.env("HEY_BOSS_AGENT_ID", "codex:verification");
        c
    };
    let issue = |args: &[&str]| run(command(args)).unwrap();
    let reject = |args: &[&str]| {
        let out = command(args).output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(4),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["error"].clone()
    };
    for (args, flag, shown) in [
        (
            &["settings", "set", "--help"][..],
            "--subtask-scheduling",
            false,
        ),
        (&["reopen", "--help"], "--clear-manual-hold", true),
    ] {
        let help = command(args).output().unwrap();
        assert!(help.status.success());
        assert_eq!(String::from_utf8_lossy(&help.stdout).contains(flag), shown);
    }
    assert_eq!(
        command(&["settings", "set", "--subtask-scheduling", "sequential"])
            .output()
            .unwrap()
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        issue(&["settings", "show"])["subtask_scheduling"],
        "explicit"
    );
    let sibling = issue(&["view", "3"])["issue"].clone();
    assert_eq!(sibling["state"], "open");
    assert_eq!(sibling["parent"]["number"], 1);
    assert_eq!(sibling["blocked_by"], json!([]));
    issue(&["claim", "3"]);
    let before = issue(&["view", "3"])["issue"].clone();
    let db = Connection::open(&f.db).unwrap();
    let notice = "Dependency rework: upstream tasks [2] need work. Read their latest changes and update/rebase the stacked PR before marking this task Ready. Running worker claims are preserved; new pickups wait for the dependencies.";
    db.execute("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Connectors',3,'codex:verification',?,123)",[notice]).unwrap();
    for (action, data) in [
        ("commented", json!({"comment_id":1,"body":notice})),
        ("dependency_rework", json!({"dependencies":[[2,1]]})),
    ] {
        db.execute("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Connectors',3,'codex:verification',?,123,?)",params![action,data.to_string()]).unwrap();
    }
    let count = |sql| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(
        count("SELECT count(*) FROM comments WHERE created_at=123"),
        0
    );
    assert_eq!(count("SELECT count(*) FROM events WHERE created_at=123"), 0);
    assert_eq!(issue(&["view", "3"])["issue"], before);
    let version = issue(&["view", "4"])["issue"]["version"].to_string();
    let rejected = reject(&["reopen", "4"]);
    assert!(
        rejected["message"]
            .as_str()
            .unwrap()
            .contains("#2 (linked)")
    );
    assert_eq!(rejected["details"]["blocked_by"][0]["source"], "linked");
    issue(&[
        "reopen",
        "4",
        "--clear-manual-hold",
        "--if-version",
        &version,
    ]);
    let held = issue(&["view", "4"]);
    assert_eq!(held["issue"]["manual_blocked"], false);
    assert_eq!(held["issue"]["state"], "blocked");
    assert_eq!(held["issue"]["blocker_numbers"], json!([2]));
    issue(&["settings", "set", "--prs-enabled"]);
    issue(&[
        "pr",
        "add",
        "2",
        "https://github.com/example/connectors/pull/2",
        "--purpose",
        "fix",
    ]);
    issue(&["ready", "2"]);
    assert_eq!(issue(&["view", "4"])["issue"]["state"], "open");
    issue(&["claim", "4"]);
    assert_eq!(
        reject(&["subtask", "create", "4", "--title", "Rejected child"])["code"],
        "subtask_claim_conflict"
    );
    issue(&["reopen", "2"]);
    let rework = issue(&["view", "4"]);
    assert_eq!(rework["issue"]["assignee"], "codex:verification");
    assert_eq!(
        rework["comments"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["body"]
                .as_str()
                .unwrap()
                .starts_with("Dependency rework: upstream tasks [2]"))
            .count(),
        1
    );
    assert_eq!(issue(&["view", "3"])["comments"], json!([]));
    issue(&[
        "pr",
        "add",
        "3",
        "https://github.com/example/connectors/pull/3",
        "--purpose",
        "fix",
    ]);
    issue(&["ready", "3"]);
    let handed_off = issue(&["view", "3"])["issue"].clone();
    db.execute_batch(r#"UPDATE issues SET state='blocked',assignee=NULL,version=version+1,updated_at=123 WHERE project_id='named:Connectors' AND number=3;
        INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Connectors',3,'codex:verification','blocked',123,'{"blocked_by":[2]}');"#).unwrap();
    assert_eq!(issue(&["view", "3"])["issue"], handed_off);
    assert_eq!(
        count("SELECT count(*) FROM events WHERE action='blocked' AND created_at=123"),
        0
    );
    db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,reservation_expires) VALUES('verification-reservation','named:Connectors',3,'{}','codex:other-worker','awaiting_claim',1,'synthetic','test',0,0,9999999999999)",[]).unwrap();
    let allocation = issue(&["allocation", "3"])["allocation"].clone();
    assert_eq!(
        allocation["worker_reservation"]["actor_id"],
        "codex:other-worker"
    );
    assert_eq!(allocation["worker_reservation"]["state"], "awaiting_claim");
    assert!(
        allocation["summary"]
            .as_str()
            .unwrap()
            .contains("codex:other-worker")
    );
    let reserved = reject(&["ready", "3"]);
    assert_eq!(reserved["code"], "conflict");
    assert!(
        reserved["message"]
            .as_str()
            .unwrap()
            .contains("verification-reservation")
    );
    assert_eq!(
        count(
            "SELECT count(*) FROM worker_runs WHERE id='verification-reservation' AND finished_at IS NULL"
        ),
        1
    );
}

fn layouts(p: &mut Page, name: &str, selector: &str, widths: &[u32]) {
    for theme in ["light", "dark"] {
        p.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        for width in widths {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            p.eval(&format!(
                "new Promise(done=>requestAnimationFrame(()=>requestAnimationFrame(()=>{{document.querySelector({}).scrollIntoView({{block:'center',behavior:'instant'}});requestAnimationFrame(()=>done(true))}})))",
                json!(selector)
            ))
            .unwrap();
            let bounds=p.eval(&format!("(()=>{{const el=document.querySelector({}),r=el.getBoundingClientRect();return {{left:r.left,right:r.right,top:r.top,bottom:r.bottom,scroll:el.scrollWidth,client:el.clientWidth,page:document.documentElement.scrollWidth,viewport:innerWidth}}}})()",json!(selector))).unwrap();
            let n = |key: &str| bounds[key].as_f64().unwrap();
            assert!(
                n("right") > n("left")
                    && n("bottom") > 0.
                    && n("top") < 900.
                    && n("left") >= 0.
                    && n("right") <= n("viewport")
                    && n("scroll") <= n("client") + 1.
                    && n("page") <= n("viewport"),
                "{name} at {width} {theme}: {bounds}"
            );

            if name == "settings" {
                p.wait("(()=>{const r=document.querySelector('#project-settings-dialog').getBoundingClientRect();return r.left>=0 && r.right<=innerWidth && r.top>=0 && r.bottom<=innerHeight})()");
            }
            screenshot(
                p,
                "out/dependency-scheduling",
                &format!("{name}-{width}-{theme}"),
            );
        }
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn web_distinguishes_manual_hold_sibling_and_rework() {
    let f = installed_fixture();
    seed(&f);
    let issue = |args: &[&str]| run(f.issue(PROJECT, args)).unwrap();
    let (_web, base) = web(&f, PROJECT);
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error',e=>window.name=e.message);addEventListener('unhandledrejection',e=>window.name=String(e.reason))"})).unwrap();
    let view = |p: &mut Page, n| {
        p.command(
            "Page.navigate",
            json!({"url":format!("{base}/#project=named%3AConnectors&issue={n}")}),
        )
        .unwrap();
        p.wait(&format!("model.detail?.issue.number==={n}"));
    };
    view(&mut p, 4);
    p.wait("document.querySelector('.blocked-notice h2').innerText==='On hold'");
    assert_eq!(p.eval("document.querySelector('.issue-blockers').innerText.includes('#2') && !document.querySelector('.issue-blockers').innerText.includes('#3')").unwrap(),true);
    layouts(&mut p, "hold", ".blocked-notice", &[1440, 320]);
    p.eval("document.querySelector('.blocked-notice [data-action=clear_manual_hold]').click()")
        .unwrap();
    p.wait("document.querySelector('.blocked-notice h2')?.innerText==='Waiting for dependencies' && !document.querySelector('[data-action=clear_manual_hold]')");
    assert_eq!(issue(&["view", "4"])["issue"]["manual_blocked"], false);
    let settings = |p: &mut Page| {
        p.eval("document.querySelector('#project-more').click();document.querySelector('#project-settings-trigger').click()").unwrap();
        p.wait("document.querySelector('#project-settings-dialog').open && !!projectSettingsOriginal && !document.querySelector('#project-prs').disabled");
    };
    settings(&mut p);
    assert_eq!(
        p.eval("document.querySelector('#project-subtask-scheduling')===null")
            .unwrap(),
        true
    );
    layouts(
        &mut p,
        "settings",
        "#project-prs-help",
        &[1440, 768, 390, 320],
    );
    for event in ["keyDown", "keyUp"] {
        p.command(
            "Input.dispatchKeyEvent",
            json!({"type":event,"key":"Escape","windowsVirtualKeyCode":27}),
        )
        .unwrap();
    }
    p.wait("document.activeElement.id==='project-more'");
    settings(&mut p);
    p.eval("document.querySelector('#project-worker-reasoning').value='none'")
        .unwrap();
    p.eval("document.querySelector('#project-prs').click();document.querySelector('#project-settings-form button[type=submit]').click()").unwrap();
    p.wait("!document.querySelector('#project-settings-dialog').open");
    let saved = issue(&["settings", "show"]);
    assert_eq!(saved["prs_enabled"], true);
    assert_eq!(saved["worker_reasoning"], "none");
    view(&mut p, 3);
    p.wait("model.detail.issue.state==='open' && model.detail.issue.parent.number===1");
    view(&mut p, 1);
    p.wait("model.detail.issue.state==='blocked' && document.querySelector('.subtasks-card').innerText.includes('Dependencies use blocked-by')");
    p.eval("document.querySelector('.subtasks-card').scrollIntoView({block:'center',behavior:'instant'})").unwrap();
    screenshot(&mut p, "out/dependency-scheduling", "parent-mobile");
    issue(&[
        "pr",
        "add",
        "2",
        "https://github.com/example/connectors/pull/2",
        "--purpose",
        "fix",
    ]);
    issue(&["ready", "2"]);
    view(&mut p, 4);
    p.wait("model.detail.issue.state==='open' && model.detail.issue.dependency_context[0].state==='ready'");
    for n in ["4", "3"] {
        issue(&["assign", n, "boss"]);
    }
    issue(&["reopen", "2"]);
    view(&mut p, 3);
    p.wait("model.detail.issue.assignee==='human:boss' && !model.detail.issue.blocked_by.length && !model.detail.comments.some(c=>c.body.startsWith('Dependency rework:'))");
    view(&mut p, 4);
    p.wait("model.detail.issue.assignee==='human:boss' && model.detail.comments.filter(c=>c.body.startsWith('Dependency rework: upstream tasks [2]')).length===1");
    assert_eq!(
        p.eval("document.querySelector('.detail-meta span:last-child').innerText")
            .unwrap(),
        "1 comment"
    );
    assert!(
        p.eval("document.querySelector('[data-comment-id] .comment-body').innerText")
            .unwrap()
            .as_str()
            .unwrap()
            .starts_with("Dependency rework: upstream tasks [2]")
    );
    layouts(
        &mut p,
        "rework",
        "[data-comment-id] .comment-body",
        &[1440, 390, 320],
    );
    assert_eq!(p.eval("window.name").unwrap(), "");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn blocker_picker_rejects_cycles_and_releases_only_the_last_active_link() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    let issue = |args: &[&str]| run(f.issue(PROJECT, args)).unwrap();
    for title in [
        "Restore GitHub SSO access",
        "Finish native CI",
        "Audit linked blockers",
        "Another task",
    ] {
        issue(&["create", "--title", title]);
    }
    issue(&["blocked-by", "3", "2"]);
    let (_web, base) = web(&f, PROJECT);
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    let go = |p: &mut Page, n| {
        // A new document also clears any cached detail after a CLI mutation.
        p.command("Page.navigate", json!({"url":"about:blank"}))
            .unwrap();
        p.wait("location.href==='about:blank'");
        p.command(
            "Page.navigate",
            json!({"url":format!("{base}/#project=named%3AConnectors&state=blocked&issue={n}")}),
        )
        .unwrap();
        p.wait(&format!(
            "model.detail?.issue.number==={n} && !!document.querySelector('.state-pill')"
        ));
    };
    let click = |p: &mut Page, selector: &str| {
        p.eval(&format!(
            "document.querySelector({}).click()",
            json!(selector)
        ))
        .unwrap();
    };
    let picker = |p: &mut Page| {
        click(p, "[data-add-blocker]");
        p.wait("document.querySelector('#blocker-picker-dialog').open && !!document.querySelector('[data-select-blocker]')");
    };
    let select = |p: &mut Page, n: i64, text: &str| {
        picker(p);
        p.eval(&format!("const input=document.querySelector('#blocker-picker-search');input.value={};input.dispatchEvent(new Event('input'))",json!(text))).unwrap();
        click(p, &format!("[data-select-blocker='{n}']"));
    };
    go(&mut p, 3);
    p.wait("!!document.querySelector('.state-pill.blocked') && document.querySelector('[data-action=reopen]')?.disabled");
    select(&mut p, 1, "SSO");
    p.wait("!document.querySelector('#blocker-picker-dialog').open && !!document.querySelector('[data-remove-blocker=\"1\"]')");
    issue(&["close", "1"]);
    go(&mut p, 3);
    p.wait("!!document.querySelector('.state-pill.blocked') && document.querySelector('.issue-blockers').innerText.includes('Closed')");
    issue(&["close", "2"]);
    go(&mut p, 3);
    p.wait("!!document.querySelector('.state-pill.open')");
    issue(&["reopen", "1"]);
    go(&mut p, 3);
    p.wait("!!document.querySelector('.state-pill.blocked')");
    go(&mut p, 1);
    select(&mut p, 3, "Audit");
    p.wait("document.querySelector('#blocker-picker-dialog').open && document.querySelector('#blocker-picker-error').innerText.includes('cycle')");
    assert_eq!(issue(&["view", "1"])["issue"]["blocker_numbers"], json!([]));
    go(&mut p, 3);
    click(&mut p, "[data-remove-blocker='1']");
    p.wait("!!document.querySelector('.state-pill.open') && !document.querySelector('[data-remove-blocker=\"1\"]')");
    issue(&["reopen", "2"]);
    go(&mut p, 3);
    p.wait("document.querySelector('[data-remove-blocker=\"2\"]').checkVisibility()");
    layouts(&mut p, "links", ".issue-blockers", &[1440, 768, 390, 320]);
    picker(&mut p);
    layouts(
        &mut p,
        "picker",
        "#blocker-picker-dialog",
        &[1440, 768, 390, 320],
    );
    click(&mut p, "#blocker-picker-close");
    click(&mut p, "[data-back]");
    p.wait("document.querySelector('[data-issue-number=\"3\"] .issue-blocked-by')?.innerText.includes('#2 Finish native CI')");
    layouts(
        &mut p,
        "list",
        "[data-issue-number='3']",
        &[1440, 768, 390, 320],
    );
}
