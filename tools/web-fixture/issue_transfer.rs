use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn transfer_controls_survive_optional_section_failures() {
    let f = fixture();
    for project in ["Source", "Destination"] {
        f.init(project).unwrap();
    }
    run(f.issue("Source", &["create", "--title", "Transfer this issue"])).unwrap();
    let (_web, base) = web(&f, "Source");
    let mut p = Page::open(&format!("{base}/#project=named%3ASource&issue=1"));
    p.wait("!!model.detail?.issue");
    // close() queues its event. Reopening before it arrives must keep the new
    // dialog's request and focus, even if the first response is still pending.
    p.eval(r#"new Promise(done=>{
        const trigger=document.querySelector('[data-transfer]'),dialog=document.querySelector('#transfer-dialog');
        trigger.click(); dialog.addEventListener('close',()=>done(true),{once:true});
        document.querySelector('[data-transfer-cancel]').click(); trigger.click();
    })"#).unwrap();
    p.wait("document.querySelector('#transfer-dialog').open && !document.querySelector('#transfer-project').disabled && document.activeElement.id==='transfer-project'");
    p.eval("document.querySelector('[data-transfer-cancel]').click()")
        .unwrap();
    p.eval("window.originalDetail=structuredClone(model.detail)")
        .unwrap();
    for state in ["open", "blocked", "ready", "closed", "draft", "deleted"] {
        for section in ["Progress", "Attachments", "Linked documents", "Subtasks"] {
            // Fail one component at the mount boundary; render all others normally.
            let report=p.eval(&format!(r#"(()=>{{
              const value=structuredClone(originalDetail), state={state}, section={section};
              Object.assign(value.issue,{{state:['draft','deleted'].includes(state)?'open':state,draft:state==='draft',deleted_at:state==='deleted'?1:null}});
              const mount=mountIssueSection, log=console.error, mounted=[], errors=[];
              mountIssueSection=(selector,name,component)=>mount(selector,name,()=>{{mounted.push(name);if(name===section)throw Error(name);component()}});
              console.error=(message)=>errors.push(message);
              try{{renderDetail(value)}}finally{{mountIssueSection=mount;console.error=log}}
              return {{mounted,errors}};
            }})()"#,state=json!(state),section=json!(section))).unwrap();
            assert_eq!(
                report["mounted"],
                json!(["Progress", "Attachments", "Linked documents", "Subtasks"])
            );
            assert_eq!(
                report["errors"],
                json!([format!("{section} could not load")])
            );
            if state == "deleted" {
                assert_eq!(
                    p.eval("!!document.querySelector('[data-transfer]')")
                        .unwrap(),
                    false
                );
                continue;
            }
            p.eval("document.querySelector('.issue-overflow summary[aria-label=\"More issue actions\"]').click()").unwrap();
            p.wait("document.querySelector('[data-action=delete]').checkVisibility() && document.querySelector('[data-transfer]').checkVisibility()");
            p.eval("document.querySelector('[data-transfer]').click()")
                .unwrap();
            p.wait("document.querySelector('#transfer-dialog').open && Array.from(document.querySelector('#transfer-project').options).some(o=>o.value==='named:Destination')");
            p.eval("document.querySelector('[data-transfer-cancel]').click()")
                .unwrap();
            if state == "open" && matches!(section, "Attachments" | "Linked documents") {
                p.wait("document.querySelector('#detail-view [data-reload]')?.checkVisibility()");
                screenshot(&mut p, "out/issue-transfer", &format!("{section}-retry"));
                p.eval("document.querySelector('#detail-view [data-reload]').click()")
                    .unwrap();
                p.wait("!document.querySelector('#detail-view [data-reload]') && document.querySelector('.issue-overflow summary')?.checkVisibility()");
            }
        }
    }
}

fn key(p: &mut Page, key: &str, code: u32) {
    for event in ["keyDown", "keyUp"] {
        p.command("Input.dispatchKeyEvent",json!({"type":event,"key":key,"code":key,"windowsVirtualKeyCode":code,"text":if key=="Enter" && event=="keyDown" {"\r"} else {""}})).unwrap();
    }
}
fn click(p: &mut Page, selector: &str) {
    p.eval(&format!(
        "document.querySelector({}).click()",
        json!(selector)
    ))
    .unwrap();
}
fn dialog(p: &mut Page) {
    click(p, ".issue-overflow summary");
    p.wait("document.querySelector('[data-transfer]').checkVisibility()");
    click(p, "[data-transfer]");
    p.wait("document.querySelector('#transfer-dialog').open && !document.querySelector('#transfer-project').disabled");
}
fn select(p: &mut Page, target: &str) {
    p.eval(&format!("(()=>{{const el=document.querySelector('#transfer-project');el.value={};el.dispatchEvent(new Event('change'))}})()",json!(target))).unwrap();
    assert_eq!(
        p.eval("document.querySelector('#transfer-submit').disabled")
            .unwrap(),
        false
    );
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn moving_an_issue_preserves_drafts_across_conflicts_retries_and_old_links() {
    let f = installed_fixture();
    for project in ["Source", "Destination"] {
        f.init(project).unwrap();
    }
    const BODY: &str = "## Release notes\nPreserve **Markdown**, comments and labels.";
    run(f.issue(
        "Source",
        &[
            "create",
            "--title",
            "Move issue",
            "--body",
            BODY,
            "--label",
            "ready",
        ],
    ))
    .unwrap();
    run(f.issue(
        "Source",
        &[
            "comment",
            "1",
            "--body",
            "A comment that follows the issue.",
        ],
    ))
    .unwrap();
    run(f.issue("Destination", &["create", "--title", "Existing issue"])).unwrap();
    let (_web, base) = web(&f, "Source");
    let old_url = format!("{base}/issues#project=named%3ASource&issue=1");
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error',e=>window.name=e.message);addEventListener('unhandledrejection',e=>window.name=String(e.reason))"})).unwrap();
    p.command("Page.navigate", json!({"url":old_url})).unwrap();
    p.wait("model.detail?.issue.title==='Move issue'");
    click(&mut p, ".issue-overflow summary");
    key(&mut p, "Escape", 27);
    p.wait("!document.querySelector('.issue-overflow').open");
    p.eval("document.querySelector('.issue-overflow summary').focus()")
        .unwrap();
    key(&mut p, "Enter", 13);
    key(&mut p, "Tab", 9);
    key(&mut p, "Enter", 13);
    p.wait("document.activeElement.id==='transfer-project' && !document.querySelector('#transfer-project').disabled");
    assert_eq!(p.eval("document.querySelector('#transfer-submit').disabled && !document.querySelector('#transfer-project option[value=\"named:Source\"]')").unwrap(),true);
    key(&mut p, "Escape", 27);
    p.wait("document.activeElement.matches('.issue-overflow summary')");
    p.eval("const field=document.querySelector('#comment-body');field.value='Unsent transfer draft';field.dispatchEvent(new Event('input'))").unwrap();
    dialog(&mut p);
    run(f.issue("Source", &["edit", "1", "--title", "Revised title"])).unwrap();
    select(&mut p, "named:Destination");
    click(&mut p, "#transfer-submit");
    p.wait("document.querySelector('#transfer-error').checkVisibility() && document.querySelector('#transfer-error').innerText.includes('Issue changed')");
    screenshot(&mut p, "out/issue-transfer", "stale-revision");
    key(&mut p, "Escape", 27);
    p.eval("refresh(false)").unwrap();
    p.eval("document.querySelector('[data-reload]')?.click()")
        .unwrap();
    p.wait("model.detail?.issue.title==='Revised title'");
    dialog(&mut p);
    select(&mut p, "named:Destination");
    p.eval(r#"window.receipts=[];const normalFetch=window.fetch;window.fetch=async(...args)=>{
        if(args[0]!=='/api/action'||!args[1]?.body)return normalFetch(...args);
        const data=JSON.parse(args[1].body);
        if(data.operation.action!=='transfer')return normalFetch(...args);
        receipts.push(data.request_id);const response=await normalFetch(...args);
        if(receipts.length===1 && (await response.clone().json()).ok)throw TypeError('Lost move reply');
        return response;
    }"#).unwrap();
    click(&mut p, "#transfer-submit");
    p.wait("document.querySelector('#transfer-error').innerText.includes('Connection lost') && !document.querySelector('#transfer-submit').disabled");
    click(&mut p, "#transfer-submit");
    p.wait("model.project?.id==='named:Destination' && model.detail?.issue.number===2 && !document.querySelector('#transfer-dialog').open");
    assert_eq!(
        p.eval("receipts.length===2 && !!receipts[0] && receipts[0]===receipts[1]")
            .unwrap(),
        true
    );
    assert_eq!(
        p.eval("document.querySelector('#comment-body').value")
            .unwrap(),
        "Unsent transfer draft"
    );
    p.wait("document.querySelector('#comments').innerText.includes('A comment that follows the issue.')");
    let record = run(f.issue("Destination", &["view", "2"])).unwrap();
    assert_eq!(record["issue"]["body"], BODY);
    assert_eq!(record["issue"]["title"], "Revised title");
    assert_eq!(record["issue"]["labels"], json!(["ready"]));
    assert_eq!(
        run(f.issue("Destination", &["list", "--state", "all", "--all"])).unwrap()["issues"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    p.command("Page.navigate", json!({"url":"about:blank"}))
        .unwrap();
    p.wait("location.href==='about:blank'");
    p.command("Page.navigate", json!({"url":old_url})).unwrap();
    p.wait("model.project?.id==='named:Destination' && model.detail?.issue.number===2 && document.querySelector('#comment-body').value==='Unsent transfer draft'");
    for scheme in ["light", "dark"] {
        p.command(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-color-scheme","value":scheme}]}),
        )
        .unwrap();
        for width in [1440, 768, 390, 320] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            click(&mut p, ".issue-overflow summary");
            p.wait("(()=>{const r=document.querySelector('.issue-overflow-menu').getBoundingClientRect();return r.width>0 && r.left>=0 && r.right<=innerWidth})()");
            screenshot(
                &mut p,
                "out/issue-transfer",
                &format!("menu-{scheme}-{width}"),
            );
            click(&mut p, "[data-transfer]");
            p.wait("document.activeElement.id==='transfer-project' && !document.querySelector('#transfer-project').disabled");
            select(&mut p, "named:Source");
            p.wait("(()=>{const r=document.querySelector('#transfer-dialog').getBoundingClientRect();return r.left>=0 && r.right<=innerWidth && r.top>=0 && r.bottom<=innerHeight && document.documentElement.scrollWidth<=innerWidth})()");
            screenshot(
                &mut p,
                "out/issue-transfer",
                &format!("dialog-{scheme}-{width}"),
            );
            key(&mut p, "Escape", 27);
            p.wait("document.activeElement.matches('.issue-overflow summary')");
        }
    }
    assert_eq!(p.eval("window.name").unwrap(), "");
}
