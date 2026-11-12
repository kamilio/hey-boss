//! CLI and web editors must preserve existing claims when organizing subtasks.
use super::*;

const PROJECT: &str = "Subtasks";

#[test]
#[ignore = "requires a built hey-boss CLI"]
fn cli_preserves_claims_and_issue_numbers() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    for subcommand in [&[][..], &["add"], &["create"], &["remove"]] {
        let help = f
            .command(&[&["issue", "subtask"][..], subcommand, &["--help"]].concat())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&help.stdout);
        assert!(
            help.status.success() && text.contains("Blocked") && text.contains("claim"),
            "{text}"
        );
    }
    let command = |args: &[&str]| {
        let mut c = f.issue(PROJECT, args);
        c.env("HEY_BOSS_AGENT_ID", "codex:verification");
        c
    };
    let issue = |args: &[&str]| run(command(args)).unwrap();
    for title in ["Parent", "Follow-up"] {
        issue(&["create", "--title", title]);
    }
    for n in ["1", "2"] {
        issue(&["claim", n]);
    }
    let before = issue(&["view", "1"]);
    for args in [
        &["subtask", "add", "1", "2"][..],
        &["subtask", "create", "1", "--title", "Rejected child"],
    ] {
        let output = command(args).output().unwrap();
        assert_eq!(output.status.code(), Some(4));
        let failure: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(failure["error"]["code"], "subtask_claim_conflict");
        assert_eq!(issue(&["view", "1"]), before);
    }
    for n in ["1", "2"] {
        let value = issue(&["view", n]);
        assert_eq!(value["issue"]["assignee"], "codex:verification");
        assert_eq!(value["issue"]["state"], "open");
        assert!(value["issue"]["parent"].is_null());
    }
    issue(&["unassign", "1"]);
    issue(&["subtask", "add", "1", "2"]);
    assert_eq!(issue(&["view", "1"])["issue"]["state"], "blocked");
    assert_eq!(
        issue(&["view", "2"])["issue"]["assignee"],
        "codex:verification"
    );
    issue(&["subtask", "remove", "1", "2"]);
    assert_eq!(issue(&["view", "1"])["issue"]["state"], "open");
    assert_eq!(
        issue(&["create", "--title", "Next issue"])["issue"]["number"],
        3
    );
}

fn page(base: &str) -> Page {
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error',e=>window.name=e.message);addEventListener('unhandledrejection',e=>window.name=String(e.reason))"})).unwrap();
    p.command(
        "Page.navigate",
        json!({"url":format!("{base}/#project=named%3ASubtasks&issue=1")}),
    )
    .unwrap();
    p.wait("model.detail?.issue.number===1");
    p
}
use super::pointer_click as click;
fn fill(p: &mut Page, selector: &str, text: &str) {
    p.eval(&format!("(()=>{{const el=document.querySelector({});el.value={};el.dispatchEvent(new Event('input',{{bubbles:true}}))}})()",json!(selector),json!(text))).unwrap();
}
fn check_widths(p: &mut Page, condition: &str) {
    for width in [320, 390, 768, 1440] {
        p.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":false}),
        )
        .unwrap();
        assert_eq!(p.eval(condition).unwrap(), true, "width {width}");
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn claim_conflicts_preserve_editor_picker_and_parent() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    let issue = |args: &[&str]| run(f.issue(PROJECT, args)).unwrap();
    for title in ["Active parent", "Completed child", "Open follow-up"] {
        issue(&["create", "--title", title]);
    }
    issue(&["close", "2"]);
    issue(&["subtask", "add", "1", "2"]);
    issue(&["assign", "1", "boss"]);
    let before = issue(&["view", "1"]);
    let (_web, base) = web(&f, PROJECT);
    let mut p = page(&base);
    click(&mut p, "[data-create-subtask]");
    p.wait("document.querySelector('#editor-subtask-help').checkVisibility() && document.querySelector('#editor-subtask-help').innerText.includes('claim')");
    fill(&mut p, "#editor-subject", "Keep this follow-up draft");
    fill(
        &mut p,
        "#editor-body",
        "## Unfinished work\n\nPreserve this text.",
    );
    click(&mut p, "#editor-submit");
    p.wait("document.querySelector('#editor-error').checkVisibility() && document.querySelector('#editor-error').innerText.includes('claim')");
    assert_eq!(
        p.eval("document.querySelector('#editor-conflict').hidden && pendingMutation.size===0")
            .unwrap(),
        true
    );
    assert_eq!(
        p.eval("document.querySelector('#editor-subject').value")
            .unwrap(),
        "Keep this follow-up draft"
    );
    assert_eq!(
        p.eval("document.querySelector('#editor-body').value")
            .unwrap(),
        "## Unfinished work\n\nPreserve this text."
    );
    check_widths(
        &mut p,
        "document.documentElement.scrollWidth<=innerWidth && document.querySelector('#editor-error').scrollWidth<=document.querySelector('#editor-error').clientWidth",
    );
    click(&mut p, "#editor-cancel");
    p.wait("document.activeElement.matches('[data-create-subtask]')");
    click(&mut p, "[data-add-existing-subtask]");
    p.wait("!!document.querySelector('[data-existing-subtask=\"3\"]')");
    assert!(
        p.eval("document.querySelector('#subtask-picker-help').innerText")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("Blocked")
    );
    click(&mut p, "[data-existing-subtask=\"3\"]");
    p.wait("document.querySelector('#subtask-picker-error').checkVisibility() && document.querySelector('#subtask-picker-error').innerText.includes('claim')");
    assert_eq!(
        p.eval("document.querySelector('#subtask-picker-dialog').open && pendingMutation.size===0")
            .unwrap(),
        true
    );
    check_widths(
        &mut p,
        "(()=>{const r=document.querySelector('#subtask-picker-dialog').getBoundingClientRect();return r.left>=0 && r.right<=innerWidth && r.top>=0 && r.bottom<=innerHeight})()",
    );
    for kind in ["keyDown", "keyUp"] {
        p.command(
            "Input.dispatchKeyEvent",
            json!({"type":kind,"key":"Escape","windowsVirtualKeyCode":27}),
        )
        .unwrap();
    }
    p.wait("document.activeElement.matches('[data-add-existing-subtask]')");
    assert_eq!(issue(&["view", "1"]), before);
    assert!(issue(&["view", "3"])["issue"]["parent"].is_null());
    assert_eq!(
        issue(&["list", "--all", "--state", "all"])["issues"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    p.eval("navigate({issue:null})").unwrap();
    p.wait("document.querySelector('#new-issue').checkVisibility()");
    click(&mut p, "#new-issue");
    assert_eq!(p.eval("document.querySelector('#editor-dialog').open && document.querySelector('#editor-subtask-help').hidden").unwrap(),true);
    assert_eq!(p.eval("window.name").unwrap(), "");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn child_creation_and_link_recover_from_conflicts_and_lost_replies() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    let issue = |args: &[&str]| run(f.issue(PROJECT, args)).unwrap();
    issue(&[
        "create",
        "--title",
        "Retry parent",
        "--body",
        "Original parent",
    ]);
    let (_web, base) = web(&f, PROJECT);
    let mut p = page(&base);
    click(&mut p, "[data-create-subtask]");
    fill(&mut p, "#editor-subject", "Retained child");
    fill(&mut p, "#editor-body", "## Child draft");
    issue(&["edit", "1", "--title", "Changed parent"]);
    click(&mut p, "#editor-submit");
    p.wait("document.querySelector('#editor-conflict').checkVisibility()");
    assert_eq!(
        p.eval("document.querySelector('#editor-body').value")
            .unwrap(),
        "## Child draft"
    );
    assert_eq!(
        p.eval("document.querySelector('#conflict-replace').innerText")
            .unwrap(),
        "Create using the latest parent revision"
    );
    click(&mut p, "#conflict-replace");
    p.wait("!document.querySelector('#editor-dialog').open && model.detail?.subtasks?.length===1");
    assert_eq!(
        p.eval("model.detail.issue.title").unwrap(),
        "Changed parent"
    );
    // Track the request receipt across a real reload. Fail only after the first
    // server response confirms creation; the second response reaches the page.
    let script = r#"const normalFetch=window.fetch;window.fetch=async(...args)=>{
        if(args[0]!=='/api/action' || !args[1]?.body) return normalFetch(...args);
        const data=JSON.parse(args[1].body);
        if(data.operation.action!=='create_subtask' || data.operation.title!=='One durable child')return normalFetch(...args);
        const ids=JSON.parse(sessionStorage.testReceipts||'[]');ids.push(data.request_id);
        sessionStorage.testReceipts=JSON.stringify(ids);
        const response=await normalFetch(...args);
        if(ids.length===1 && (await response.clone().json()).ok)throw TypeError('reply lost after commit');
        return response;
    }"#;
    p.command(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":script}),
    )
    .unwrap();
    p.eval(script).unwrap();
    click(&mut p, "[data-create-subtask]");
    fill(&mut p, "#editor-subject", "One durable child");
    fill(&mut p, "#editor-body", "## Durable retry Markdown");
    click(&mut p, "#editor-submit");
    p.wait("document.querySelector('#editor-error').checkVisibility() && sessionStorage.testReceipts!==undefined");
    assert_eq!(
        p.eval("document.querySelector('#editor-subject').value")
            .unwrap(),
        "One durable child"
    );
    p.command("Page.reload", json!({})).unwrap();
    p.wait("model.detail?.subtasks?.length===2 && !document.querySelector('#editor-dialog').open");
    click(&mut p, "[data-create-subtask]");
    assert_eq!(
        p.eval("document.querySelector('#editor-body').value")
            .unwrap(),
        "## Durable retry Markdown"
    );
    click(&mut p, "#editor-submit");
    p.wait("!document.querySelector('#editor-dialog').open && model.detail?.subtasks?.length===2");
    let ids = p.eval("JSON.parse(sessionStorage.testReceipts)").unwrap();
    assert_eq!(ids.as_array().unwrap().len(), 2);
    assert!(!ids[0].as_str().unwrap().is_empty());
    assert_eq!(ids[0], ids[1]);
    let children = issue(&["subtask", "list", "1"]);
    let matches: Vec<_> = children["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["title"] == "One durable child")
        .collect();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["body"], "## Durable retry Markdown");
    assert_eq!(p.eval("model.route.issue").unwrap(), 1);
    issue(&["create", "--title", "Picker revision", "--body", "Before"]);
    click(&mut p, "[data-add-existing-subtask]");
    p.wait("!!document.querySelector('[data-existing-subtask=\"4\"]')");
    issue(&["edit", "4", "--title", "Updated picker revision"]);
    click(&mut p, "[data-existing-subtask=\"4\"]");
    p.wait("document.querySelector('#subtask-picker-error').checkVisibility() && document.querySelector('[data-existing-subtask=\"4\"]')?.textContent.includes('Updated picker revision')");
    assert_eq!(
        p.eval("document.querySelector('#subtask-picker-search').disabled")
            .unwrap(),
        false
    );
    click(&mut p, "[data-existing-subtask=\"4\"]");
    p.wait("!document.querySelector('#subtask-picker-dialog').open && model.detail?.subtasks?.length===3 && document.querySelector('#subtask-list').innerText.includes('Updated picker revision')");
    assert_eq!(p.eval("window.name").unwrap(), "");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn assigned_subtask_opens_with_its_own_assignment_card() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    let issue = |args: &[&str]| run(f.issue(PROJECT, args)).unwrap();
    for title in ["Parent", "Owned child", "Unassigned child"] {
        issue(&["create", "--title", title]);
    }
    for child in ["2", "3"] {
        issue(&["subtask", "add", "1", child]);
    }
    issue(&["assign", "2", "boss"]);
    let (_web, base) = web(&f, PROJECT);
    let mut p = Page::open("about:blank");
    p.command(
        "Page.navigate",
        json!({"url":format!("{base}/#project=named%3ASubtasks&state=all")}),
    )
    .unwrap();
    p.wait("!!document.querySelector('.issue-title[data-issue=\"1\"]')");
    click(&mut p, ".issue-title[data-issue=\"1\"]");
    p.wait("document.querySelectorAll('#subtask-list .subtask-title').length===2");
    p.eval("document.querySelector('#subtask-list [data-assignment-kind=boss]').focus()")
        .unwrap();
    p.wait("document.querySelector('#subtask-list .assignment-card')?.matches(':popover-open')");
    assert_eq!(
        p.eval("document.querySelectorAll('#subtask-list .assignment-card').length")
            .unwrap(),
        1
    );
    click(&mut p, ".subtask-title[data-issue=\"3\"]");
    p.wait("document.querySelector('.detail-top h1')?.textContent.includes('Unassigned child')");
}
