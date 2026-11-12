use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn blocked_drafts_keep_dependency_links_and_unsent_comments() {
    let f = installed_fixture();
    let project = "Draft transitions";
    f.init(project).unwrap();
    let cli = |args: &[&str]| run(f.issue(project, args)).unwrap();
    cli(&["settings", "set", "--drafts-enabled"]);
    cli(&[
        "create",
        "--title",
        "Refine onboarding",
        "--body",
        "Review the first visit.",
    ]);
    cli(&["block", "1"]);
    let (_web, base) = web(&f, project);
    let url = format!("{base}/#project=named%3ADraft+transitions&issue=1&state=blocked&owner=all");
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"window.el=s=>document.querySelector(s);window.move=()=>el('[data-draft-action=draft]');window.ready=()=>el('[data-draft-action=ready]');window.note=t=>{el('#comment-body').value=t;el('#comment-body').dispatchEvent(new Event('input',{bubbles:true}))}"})).unwrap();
    p.command("Page.navigate", json!({"url":url})).unwrap();
    p.wait("move()?.disabled===false && !!el('.state-pill.blocked')");
    p.eval("note('Keep this review.');const send=window.fetch.bind(window);window.fetch=(u,o)=>{if(u==='/api/action' && JSON.parse(o.body).operation.draft===true){window.fetch=send;return Promise.resolve(Response.json({ok:false,error:{code:'conflict',message:'Issue changed. Refresh and retry.'}},{status:409}))}return send(u,o)};move().click()").unwrap();
    p.wait("!move().disabled && el('#draft-error')?.innerText.includes('Issue changed')");
    assert_eq!(
        p.eval("!!el('.state-pill.blocked') && el('#comment-body').value==='Keep this review.'")
            .unwrap(),
        true
    );
    p.eval("move().focus()").unwrap();
    key(&mut p, "Enter", "Enter", 13, "\r");
    p.wait("ready() && ready()===document.activeElement && !!el('.state-pill.draft')");
    assert_eq!(
        p.eval("el('#comment-body').value").unwrap(),
        "Keep this review."
    );
    let reload = |p: &mut Page| {
        p.eval("window.previousDocument=true").unwrap();
        p.command("Page.reload", json!({})).unwrap();
        p.wait("!window.previousDocument && !!el('.issue-readiness')");
    };
    reload(&mut p);
    p.wait("ready()?.disabled===false");
    p.eval("const send=window.fetch.bind(window);window.fetch=async(u,o)=>{if(u==='/api/action' && JSON.parse(o.body).operation.action==='undraft'){await new Promise(r=>window.deliver=r);window.fetch=send}return send(u,o)};ready().click()").unwrap();
    p.wait("typeof deliver==='function'");
    p.eval("note('Typing during the save.');deliver()").unwrap();
    p.wait("move() && move()===document.activeElement && !!el('.state-pill.open')");
    assert_eq!(
        p.eval("el('#comment-body').value").unwrap(),
        "Typing during the save."
    );
    cli(&["create", "--title", "Review the empty state"]);
    cli(&["subtask", "add", "1", "2"]);
    cli(&["settings", "set", "--no-drafts"]);
    reload(&mut p);
    assert_eq!(
        p.eval("!!el('.state-pill.blocked') && !move()").unwrap(),
        true
    );
    cli(&["settings", "set", "--drafts-enabled"]);
    reload(&mut p);
    let layouts = |p: &mut Page, selector: &str, name: &str| {
        for theme in ["light", "dark"] {
            p.command(
                "Emulation.setEmulatedMedia",
                json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
            )
            .unwrap();
            for width in [1440, 768, 390, 320] {
                p.command(
                    "Emulation.setDeviceMetricsOverride",
                    json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":false}),
                )
                .unwrap();
                p.eval(&format!(
                    "el({}).scrollIntoView({{block:'center'}})",
                    json!(selector)
                ))
                .unwrap();
                assert_eq!(
                    p.eval("document.documentElement.scrollWidth<=innerWidth+1")
                        .unwrap(),
                    true,
                    "{name}/{theme}/{width}"
                );
                screenshot(
                    p,
                    "out/draft-transitions",
                    &format!("{name}-{theme}-{width}"),
                );
            }
        }
    };
    layouts(&mut p, "[data-draft-action=draft]", "blocked");
    p.eval("el('[data-edit]').click()").unwrap();
    p.wait("el('#editor-dialog').open && !el('#editor-draft').disabled");
    p.eval("el('#editor-draft').click()").unwrap();
    assert_eq!(
        p.eval("el('#editor-submit').innerText").unwrap(),
        "Save draft"
    );
    assert_eq!(
        p.eval("el('#editor-dialog').scrollWidth<=el('#editor-dialog').clientWidth+1")
            .unwrap(),
        true
    );
    screenshot(&mut p, "out/draft-transitions", "blocked-editor-320");
    p.eval("el('#editor-submit').click()").unwrap();
    p.wait("el('.draft-notice')?.innerText.includes('Unfinished blockers are kept')");
    layouts(&mut p, ".draft-notice", "draft");
    p.eval("ready().click()").unwrap();
    p.wait("el('#toast').innerText.includes('unfinished blockers') && !!el('.state-pill.blocked')");
    p.eval("move().click()").unwrap();
    p.wait("!!el('.draft-notice')");
    assert_eq!(
        cli(&["view", "1"])["issue"]["blocked_by"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    cli(&["close", "2"]);
    reload(&mut p);
    assert_eq!(p.eval("!!el('.state-pill.draft')").unwrap(), true);
    cli(&["block", "1"]);
    reload(&mut p);
    p.eval("el('[data-edit]').click()").unwrap();
    p.wait("el('#editor-draft').checked && !el('#editor-draft').disabled");
    p.eval("el('#editor-submit').click()").unwrap();
    p.wait("!!el('.state-pill.draft') && !!el('.draft-notice') && !el('#editor-dialog').open");
    let saved = cli(&["view", "1"]);
    assert_eq!(saved["issue"]["draft"], true);
    assert_eq!(saved["issue"]["manual_blocked"], false);
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn draft_creation_and_readiness_follow_project_policy() {
    let f = installed_fixture();
    let project = "Draft policy";
    f.init(project).unwrap();
    let cli = |args: &[&str]| run(f.issue(project, args)).unwrap();
    cli(&["settings", "set", "--drafts-enabled"]);
    let (_web, base) = web(&f, project);
    let url = format!("{base}/#project=named%3ADraft+policy");
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"window.el=s=>document.querySelector(s);window.click=s=>el(s).click();window.setText=(s,t)=>{el(s).value=t;el(s).dispatchEvent(new Event('input',{bubbles:true}))}"})).unwrap();
    p.command("Page.navigate", json!({"url":url})).unwrap();
    p.wait("model.project?.id==='named:Draft policy' && !!model.signature");
    p.eval("click('#new-issue')").unwrap();
    p.wait("el('#editor-dialog').open && !el('#editor-draft').disabled");
    assert_eq!(p.eval("el('#editor-draft').checked").unwrap(), false);
    p.eval("setText('#editor-subject','Review onboarding');el('#editor-draft').focus()")
        .unwrap();
    key(&mut p, " ", "Space", 32, " ");
    p.wait("el('#editor-submit').innerText==='Create draft'");
    key(&mut p, "Escape", "Escape", 27, "");
    p.eval("click('#new-issue')").unwrap();
    p.wait("el('#editor-dialog').open && el('#editor-draft').checked && !el('#editor-submit').disabled");
    p.eval("click('#editor-submit')").unwrap();
    p.wait("!!el('[data-issue-number=\"1\"] .draft-badge')");
    assert_eq!(cli(&["view", "1"])["issue"]["draft"], true);
    p.eval("click('#new-issue')").unwrap();
    p.wait("el('#editor-dialog').open && !el('#editor-draft').disabled && !el('#editor-draft').checked");
    p.eval("click('#editor-cancel');click('.issue-title')")
        .unwrap();
    p.wait("el('.draft-notice')?.innerText.includes('Agents won’t pick up')");
    p.eval("setText('#comment-body','Keep this review.');const send=window.fetch.bind(window);window.fetch=(u,o)=>{if(u==='/api/action' && JSON.parse(o.body).operation.action==='undraft'){window.fetch=send;return Promise.resolve(Response.json({ok:false,error:{message:'Unable to save readiness'}},{status:500}))}return send(u,o)};click('[data-draft-action=ready]')").unwrap();
    p.wait("el('#draft-error')?.innerText.includes('Unable to save readiness') && !el('[data-draft-action=ready]').disabled && !!el('.state-pill.draft')");
    assert_eq!(
        p.eval("el('#comment-body').value").unwrap(),
        "Keep this review."
    );
    cli(&["settings", "set", "--no-drafts"]);
    let reload = |p: &mut Page| {
        p.eval("window.previousPage=true").unwrap();
        p.command("Page.reload", json!({})).unwrap();
        p.wait("!window.previousPage && !!el('[data-edit]')");
    };
    reload(&mut p);
    p.eval("click('[data-draft-action=ready]')").unwrap();
    p.wait("model.detail?.issue.draft===false && !el('[data-draft-action=draft]') && document.activeElement===el('[data-edit]')");
    p.eval("click('[data-edit]')").unwrap();
    p.wait("el('#editor-draft-help').textContent.includes('disabled')");
    assert_eq!(p.eval("editorBusy(true);editorBusy(false); el('#editor-draft').disabled && el('#editor-draft-control').hidden").unwrap(),true);
    p.eval("click('#editor-cancel')").unwrap();
    cli(&["settings", "set", "--drafts-enabled"]);
    reload(&mut p);
    p.eval("click('[data-draft-action=draft]')").unwrap();
    p.wait("!!el('.draft-notice')");
    for theme in ["light", "dark"] {
        p.command(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
        )
        .unwrap();
        p.eval("click('[data-edit]')").unwrap();
        p.wait("el('#editor-draft').checked && !el('#editor-draft').disabled && el('#editor-submit').innerText==='Save draft'");
        p.eval("click('#editor-draft')").unwrap();
        p.wait("el('#editor-submit').innerText==='Save & mark ready'");
        for width in [1440, 390, 320] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            p.eval("el('#editor-draft-control').scrollIntoView({block:'center'})")
                .unwrap();
            assert_eq!(p.eval("el('#editor-dialog').scrollWidth<=el('#editor-dialog').clientWidth+1 && el('#editor-submit').getBoundingClientRect().right<=innerWidth").unwrap(),true);
            screenshot(
                &mut p,
                "out/draft-transitions",
                &format!("editor-{theme}-{width}"),
            );
        }
        // Reverting to the saved value must not leave a stale edit behind.
        p.eval("window.editKey=model.editor.key;click('#editor-draft');click('#editor-cancel');click('[data-back]')")
            .unwrap();
        p.wait("!!el('.draft-badge')");
        assert_eq!(p.eval("storage.get(editKey)").unwrap(), Value::Null);
        screenshot(&mut p, "out/draft-transitions", &format!("list-{theme}"));
        p.eval("click('#project-more');click('#project-settings-trigger')")
            .unwrap();
        p.wait("!el('#project-drafts').disabled && el('#project-settings-dialog').open");
        p.eval("el('#project-drafts').scrollIntoView({block:'center'})")
            .unwrap();
        assert_eq!(
            p.eval("el('#project-drafts-help').textContent.includes('Existing drafts')")
                .unwrap(),
            true
        );
        screenshot(
            &mut p,
            "out/draft-transitions",
            &format!("settings-{theme}-320"),
        );
        p.eval("click('#project-settings-close');click('.issue-title')")
            .unwrap();
        p.wait("!!el('.draft-notice')");
    }
    p.eval("click('[data-draft-action=ready]')").unwrap();
    p.wait("model.detail?.issue.draft===false");
    cli(&["assign", "1", "boss"]);
    reload(&mut p);
    assert_eq!(p.eval("!el('[data-draft-action=draft]')").unwrap(), true);
    p.eval("click('[data-edit]')").unwrap();
    p.wait(
        "el('#editor-draft').disabled && el('#editor-draft-help').textContent.includes('Unassign')",
    );
    p.eval("click('#editor-cancel')").unwrap();
    cli(&["unassign", "1"]);
    cli(&["close", "1"]);
    reload(&mut p);
    assert_eq!(p.eval("!el('[data-draft-action=draft]') && el('.readiness-status')?.textContent==='Completed'").unwrap(),true);
}
