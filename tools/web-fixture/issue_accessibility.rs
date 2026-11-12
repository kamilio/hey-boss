//! Run the browser-side axe engine on the actual issue UI.
use super::*;

fn audit(p: &mut Page, state: &str) {
    let result=p.eval("axe.run(document,{runOnly:{type:'tag',values:['wcag2a','wcag2aa','wcag21aa','best-practice']}}).then(r=>({passed:r.passes.length,violations:r.violations.map(v=>({id:v.id,nodes:v.nodes.map(n=>[n.target,n.failureSummary])}))}))").unwrap();
    assert!(
        result["passed"].as_u64().unwrap() > 0,
        "No rules ran for {state}"
    );
    assert_eq!(result["violations"], json!([]), "{state}");
}

#[test]
#[ignore = "requires Chrome, a built CLI, and the axe-core browser asset (see docs/issues-web-verification.md)"]
fn draft_list_markdown_editor_and_settings_pass_accessibility_checks() {
    let engine = fs::read_to_string("out/issues-web-tools/node_modules/axe-core/axe.min.js")
        .expect("Install axe-core under out/issues-web-tools first");
    let f = installed_fixture();
    let project = "Accessibility";
    f.init(project).unwrap();
    run(f.issue(project, &["settings", "set", "--drafts-enabled"])).unwrap();
    run(f.issue(project,&["create","--draft","--title","Review the warning","--body","## Reading notes\n\n> [!WARNING]\n> Confirm the address before sending.\n\n- [x] Reviewed\n"])).unwrap();
    let (_web, base) = web(&f, project);
    let mut p = Page::open(&format!("{base}/#project=named%3AAccessibility"));
    p.wait("!!document.querySelector('.draft-badge')");
    p.eval(&engine).unwrap();
    p.eval("window.el=s=>document.querySelector(s);window.click=s=>el(s).click()")
        .unwrap();
    for theme in ["light", "dark"] {
        p.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        for width in [1440, 390, 320] {
            p.command("Emulation.setDeviceMetricsOverride",json!({"width":width,"height":if width==1440{1000}else{844},"deviceScaleFactor":1,"mobile":false})).unwrap();
            audit(&mut p, &format!("draft list: {theme}/{width}"));
            p.eval("click('.issue-title')").unwrap();
            p.wait("!!el('.draft-notice') && !!el('.markdown-alert-warning')");
            audit(
                &mut p,
                &format!("draft and Markdown warning: {theme}/{width}"),
            );
            p.eval("click('[data-edit]')").unwrap();
            p.wait("el('#editor-dialog').open && el('#editor-draft').checked && !el('#editor-draft').disabled");
            audit(&mut p, &format!("editor: {theme}/{width}"));
            p.eval(
                "click('#editor-cancel');click('#project-more');click('#project-settings-trigger')",
            )
            .unwrap();
            p.wait("el('#project-settings-dialog').open && !el('#project-drafts').disabled");
            p.eval("el('#project-drafts').scrollIntoView({block:'center'})")
                .unwrap();
            audit(&mut p, &format!("settings: {theme}/{width}"));
            p.eval("click('#project-settings-close');click('[data-back]')")
                .unwrap();
            p.wait("el('.draft-badge')?.checkVisibility()");
        }
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn system_appearance_updates_the_open_editor_and_markdown_without_a_reload() {
    let f = installed_fixture();
    let project = "System appearance";
    f.init(project).unwrap();
    run(f.issue(
        project,
        &[
            "create",
            "--title",
            "System appearance",
            "--body",
            "> [!WARNING]\n> Review changes.\n\n```rust\nlet n = 42;\n```",
        ],
    ))
    .unwrap();
    let (_web, base) = web(&f, project);
    let mut p = Page::open("about:blank");
    let theme = |p: &mut Page, theme: &str| {
        p.command(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
        )
        .unwrap()
    };
    theme(&mut p, "dark");
    p.command(
        "Emulation.setScriptExecutionDisabled",
        json!({"value":true}),
    )
    .unwrap();
    p.command("Page.navigate", json!({"url":base})).unwrap();
    p.wait("getComputedStyle(document.documentElement).colorScheme==='dark' && getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()==='#11151d'");
    p.command(
        "Emulation.setScriptExecutionDisabled",
        json!({"value":false}),
    )
    .unwrap();
    p.eval("localStorage.setItem('hey-boss-issues-theme',JSON.stringify('dark'))")
        .unwrap();
    theme(&mut p, "light");
    p.command("Page.reload", json!({})).unwrap();
    p.wait("document.querySelector('.issue-title')?.textContent==='System appearance'");
    assert_eq!(p.eval("getComputedStyle(document.documentElement).colorScheme==='light' && !document.querySelector('#theme,[aria-label*=\"Switch color theme\"]')").unwrap(),true);
    theme(&mut p, "dark");
    p.wait("getComputedStyle(document.documentElement).colorScheme==='dark'");
    screenshot(&mut p, "out/issue-appearance", "dark-list");
    p.eval("document.querySelector('#new-issue').click();window.titleField=document.querySelector('#editor-subject');titleField.focus()").unwrap();
    p.command("Input.insertText", json!({"text":"Keep this draft"}))
        .unwrap();
    p.eval("document.querySelector('#editor-body').focus()")
        .unwrap();
    p.command("Input.insertText", json!({"text":"Draft markdown"}))
        .unwrap();
    theme(&mut p, "light");
    p.wait("getComputedStyle(document.documentElement).colorScheme==='light' && titleField.isConnected && titleField.value==='Keep this draft' && document.querySelector('#editor-body').value==='Draft markdown'");
    p.eval("document.querySelector('#editor-cancel').click();document.querySelector('.issue-title').click()").unwrap();
    p.wait("!!document.querySelector('.markdown-alert-warning')");
    let color="getComputedStyle(document.querySelector('.markdown-alert-warning')).getPropertyValue('--alert').trim()";
    assert_eq!(p.eval(color).unwrap(), "#936415");
    theme(&mut p, "dark");
    assert_eq!(p.eval(color).unwrap(), "#e3b864");
    p.command(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":false}),
    )
    .unwrap();
    p.eval("window.beforeReload=true").unwrap();
    p.command("Page.reload", json!({})).unwrap();
    p.wait("!window.beforeReload && !!document.querySelector('.markdown-alert-warning') && getComputedStyle(document.documentElement).colorScheme==='dark'");
    assert_eq!(
        p.eval("document.documentElement.scrollWidth<=innerWidth")
            .unwrap(),
        true
    );
    theme(&mut p, "light");
    screenshot(&mut p, "out/issue-appearance", "light-detail");
}
