//! Project links, Back/Forward and draft persistence in the shipped desktop UI.
use super::*;

fn open() -> Page {
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error', e=>window.name=e.message); addEventListener('unhandledrejection', e=>window.name=String(e.reason))"})).unwrap();
    p
}
fn go(p: &mut Page, url: &str) {
    p.command("Page.navigate", json!({"url":url})).unwrap();
}
fn click(p: &mut Page, selector: &str) {
    p.eval(&format!(
        "document.querySelector({}).click()",
        json!(selector)
    ))
    .unwrap();
}
fn selected(p: &mut Page, path: &str, name: &str) {
    p.wait(&format!(
        "location.pathname==={} && document.querySelector('#project-name')?.textContent==={}",
        json!(path),
        json!(name)
    ));
}
fn reload(p: &mut Page) {
    p.eval("window.previousDocument=true").unwrap();
    p.command("Page.reload", json!({})).unwrap();
    p.wait("window.previousDocument===undefined && document.readyState==='complete'");
}
fn pick(p: &mut Page, path: &str, name: &str) {
    click(p, "#project-trigger");
    click(p, &format!("[data-project='named:{name}']"));
    selected(p, path, name);
}
fn no_warning(p: &mut Page) {
    assert_eq!(
        p.eval("window.dispatchEvent(new Event('beforeunload',{cancelable:true}))")
            .unwrap(),
        true
    );
}
fn escape(p: &mut Page) {
    for kind in ["keyDown", "keyUp"] {
        p.command(
            "Input.dispatchKeyEvent",
            json!({"type":kind,"key":"Escape","code":"Escape","windowsVirtualKeyCode":27}),
        )
        .unwrap();
    }
    p.wait("!document.querySelector('#editor-dialog').open");
}
#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn leaving_an_issue_restores_comments_and_editors_without_a_warning() {
    let f = fixture();
    f.init("Drafts").unwrap();
    run(f.issue(
        "Drafts",
        &[
            "create",
            "--title",
            "Original",
            "--body",
            "Saved description",
        ],
    ))
    .unwrap();
    let (_web, base) = web(&f, "Drafts");
    let mut p = open();
    let detail = format!("{base}/#project=named%3ADrafts&issue=1");
    go(&mut p, &detail);
    p.wait("!!document.querySelector('#comment-body')");
    no_warning(&mut p);
    // Set values without input events so only the real pagehide flush can save them.
    p.eval("document.querySelector('#comment-body').value='Unsent comment'")
        .unwrap();
    no_warning(&mut p);
    reload(&mut p);
    p.wait("document.querySelector('#comment-body')?.value==='Unsent comment'");
    click(&mut p, "[data-back]");
    p.wait("model.route.issue===null && !!model.signature");
    no_warning(&mut p);
    reload(&mut p);
    go(&mut p, &detail);
    p.wait("document.querySelector('#comment-body')?.value==='Unsent comment'");
    click(&mut p, "[data-edit]");
    p.eval("document.querySelector('#editor-subject').value='Edited title';document.querySelector('#editor-body').value='Edited body';document.querySelector('#comment-body').value='Second comment'").unwrap();
    no_warning(&mut p);
    reload(&mut p);
    p.wait("document.querySelector('#comment-body')?.value==='Second comment'");
    click(&mut p, "[data-edit]");
    p.wait("document.querySelector('#editor-dialog').open && document.querySelector('#editor-subject').value==='Edited title' && document.querySelector('#editor-body').value==='Edited body'");
    escape(&mut p);
    click(&mut p, "[data-back]");
    p.wait("!!model.signature && !document.querySelector('#list-view').hidden");
    click(&mut p, "#new-issue");
    no_warning(&mut p);
    p.eval("document.querySelector('#editor-subject').value='New title'; document.querySelector('#editor-body').value='New description'").unwrap();
    no_warning(&mut p);
    reload(&mut p);
    p.wait("!!model.signature");
    for _ in 0..2 {
        click(&mut p, "#new-issue");
        p.wait("document.querySelector('#editor-subject').value==='New title' && document.querySelector('#editor-body').value==='New description'");
        escape(&mut p);
    }
    click(&mut p, "#new-issue");
    for theme in ["light", "dark"] {
        p.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        for width in [1440, 768, 390, 320] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            p.wait("document.querySelector('#editor-dialog').scrollWidth<=document.querySelector('#editor-dialog').clientWidth && document.querySelector('#editor-submit').checkVisibility()");
            screenshot(&mut p, "out/navigation", &format!("editor-{theme}-{width}"));
        }
    }
    click(&mut p, "#editor-submit");
    p.wait("!document.querySelector('#editor-dialog').open && document.querySelector('.issue-created')!==null");
    reload(&mut p);
    p.wait("!!model.signature");
    click(&mut p, "#new-issue");
    p.wait("document.querySelector('#editor-dialog').open && document.querySelector('#editor-subject').value===''");
    escape(&mut p);
    p.eval("Storage.prototype.setItem=()=>{throw new DOMException('Storage full','QuotaExceededError')}; undefined").unwrap();
    click(&mut p, "#new-issue");
    p.eval("document.querySelector('#editor-subject').value='No storage';document.querySelector('#editor-subject').dispatchEvent(new Event('input'))").unwrap();
    no_warning(&mut p);
    reload(&mut p);
    p.wait("!!model.signature");
    assert_eq!(p.beforeunload_dialogs, 0);
    assert_eq!(p.eval("window.name").unwrap(), "");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn project_picker_carries_context_across_tabs_reloads_and_browser_history() {
    let f = fixture();
    let a = "Navigation Alpha";
    let b = "Navigation Beta & more";
    for name in [a, b] {
        f.init(name).unwrap();
        run(f.issue(name, &["settings", "set", "--prs-enabled"])).unwrap();
    }
    let _fleet = agents::supervisor(&f).unwrap();
    let (_web, base) = web(&f, a);
    let mut p = open();
    let start = |path| format!("{base}{path}#project=named%3ANavigation%20Alpha");
    go(&mut p, &start("/"));
    selected(&mut p, "/", a);
    click(&mut p, "#nav-artifacts");
    selected(&mut p, "/artifacts", a);
    pick(&mut p, "/artifacts", b);
    click(&mut p, "#nav-inbox");
    selected(&mut p, "/", b);
    p.wait("!document.querySelector('#inbox-view').hidden");
    click(&mut p, "#nav-issues");
    p.wait("!document.querySelector('#list-view').hidden");
    click(&mut p, "#navigation-more");
    click(&mut p, "#nav-workers");
    selected(&mut p, "/workers", b);
    click(&mut p, "#nav-artifacts");
    selected(&mut p, "/artifacts", b);
    click(&mut p, "#nav-issues");
    selected(&mut p, "/", b);
    for path in ["/", "/workers", "/artifacts"] {
        go(&mut p, &format!("{base}{path}"));
        selected(&mut p, path, b);
        reload(&mut p);
        selected(&mut p, path, b);
    }
    pick(&mut p, "/artifacts", a);
    p.eval("history.back()").unwrap();
    selected(&mut p, "/artifacts", b);
    p.eval("history.forward()").unwrap();
    selected(&mut p, "/artifacts", a);
    for path in [
        "/jobs",
        "/artifacts",
        "/merged-prs",
        "/skills",
        "/admin",
        "/secrets",
        "/workers",
    ] {
        go(&mut p, &start(path));
        selected(&mut p, path, a);
        pick(&mut p, path, b);
        reload(&mut p);
        selected(&mut p, path, b);
        go(&mut p, &format!("{base}{path}"));
        selected(&mut p, path, b);
        click(&mut p, "#nav-jobs");
        selected(&mut p, "/jobs", b);
        click(&mut p, "#nav-issues");
        selected(&mut p, "/", b);
    }
    go(&mut p, &start("/workers"));
    selected(&mut p, "/workers", a);
    pick(&mut p, "/workers", b);
    click(&mut p, "#nav-inbox");
    p.wait("document.querySelector('#inbox-view')?.hidden===false");
    click(&mut p, "#nav-artifacts");
    selected(&mut p, "/artifacts", b);
    go(&mut p, &start("/artifacts"));
    selected(&mut p, "/artifacts", a);
    go(&mut p, &format!("{base}/"));
    selected(&mut p, "/", a);
    assert_eq!(p.eval("window.name").unwrap(), "");
}
#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn explicit_project_links_work_without_browser_storage() {
    let f = fixture();
    let b = "Navigation Beta & more";
    f.init(b).unwrap();
    let (_web, base) = web(&f, b);
    let mut p = open();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"Storage.prototype.getItem=Storage.prototype.setItem=()=>{throw Error('Storage unavailable')}"})).unwrap();
    go(
        &mut p,
        &format!("{base}/artifacts#project=named%3ANavigation%20Beta%20%26%20more"),
    );
    selected(&mut p, "/artifacts", b);
    assert_eq!(
        p.eval("document.querySelector('#nav-workers').getAttribute('href')")
            .unwrap(),
        "/workers"
    );
    click(&mut p, "#nav-issues");
    selected(&mut p, "/", b);
    click(&mut p, "#nav-jobs");
    selected(&mut p, "/jobs", b);
    reload(&mut p);
    selected(&mut p, "/jobs", b);
    click(&mut p, "#nav-artifacts");
    selected(&mut p, "/artifacts", b);
    assert_eq!(p.eval("window.name").unwrap(), "");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn project_links_obey_feature_flags_and_keep_issue_filters() {
    let f = fixture();
    f.init("Routes").unwrap();
    let (_web, base) = web(&f, "Routes");
    let mut p = open();
    go(&mut p, &format!("{base}/#project=named%3ARoutes"));
    selected(&mut p, "/", "Routes");
    p.wait("!!model.signature");
    p.wait("!!document.querySelector('.app-navigation #project-more-links #nav-prompts') && !document.querySelector('.app-navigation #nav-inbox') && !!(document.querySelector('#nav-skills').compareDocumentPosition(document.querySelector('#navigation-more')) & Node.DOCUMENT_POSITION_FOLLOWING)");
    for view in ["issues", "inbox"] {
        let route = json!({"view":view,"project":"named:Design & QA","host":"fixture-remote","issue":41,"notice":"notice-123","state":"closed","owner":"human:boss","label":"ready","search":"navigation"});
        let links=p.eval(&format!("(()=>{{const prev=model.route;try{{model.route={route};updateAppNavigation();return [model.route.issue,...['#nav-issues','#nav-inbox'].map(s=>Object.fromEntries(new URLSearchParams(document.querySelector(s).hash.slice(1))))]}}finally{{model.route=prev}}}})()")).unwrap();
        assert_eq!(links[0], 41);
        for (i, target) in [(1, "issues"), (2, "inbox")] {
            assert_eq!(links[i]["view"], target);
            assert!(links[i].get("issue").is_none() && links[i].get("notice").is_none());
            for key in ["project", "host", "state", "owner", "label", "search"] {
                assert_eq!(links[i][key], route[key], "{key}");
            }
        }
    }
    // Issue and Inbox links on the issue page are owned by its list filters.
    p.eval("document.querySelector('#nav-issues').href='#project=mobile&state=closed'; document.querySelector('#nav-inbox').href='#project=mobile&view=inbox&inbox_state=archive'; HeyBossUI.projectNavigation('mobile')").unwrap();
    assert_eq!(
        p.eval("['#nav-issues','#nav-inbox'].map(s=>document.querySelector(s).hash)")
            .unwrap(),
        json!([
            "#project=mobile&state=closed",
            "#project=mobile&view=inbox&inbox_state=archive"
        ])
    );
    go(&mut p, &format!("{base}/artifacts#project=named%3ARoutes"));
    selected(&mut p, "/artifacts", "Routes");
    for (settings, selection, hidden) in [
        (
            json!({"artifacts_enabled":false,"prs_enabled":false}),
            json!({}),
            [true, true],
        ),
        (
            json!({}),
            json!({"artifacts_enabled":true,"prs_enabled":true}),
            [false, false],
        ),
        (json!({}), json!({}), [false, true]),
    ] {
        let result=p.eval(&format!("(()=>{{const id='named:Routes';HeyBossUI.ProjectPicker.prototype.update.call({{}},[{{id,...{settings}}}],{{id,name:'Routes',...{selection}}});return ['#nav-artifacts','#nav-merged-prs'].map(s=>document.querySelector(s).hidden)}})()")).unwrap();
        assert_eq!(result, json!(hidden));
    }
    let chosen = "named:Beta & more / #2";
    for mobile in [false, true] {
        let anchors=p.eval(&format!("history.replaceState(null,'','/artifacts#project=old&host=fixture-remote'); if({mobile})document.documentElement.dataset.issueMobile='true';HeyBossUI.projectNavigation({});Array.from(document.querySelectorAll('.app-navigation a'),a=>Object.fromEntries(new URLSearchParams(a.hash.slice(1))))",json!(chosen))).unwrap();
        assert_eq!(
            p.eval("new URLSearchParams(location.hash.slice(1)).get('project')")
                .unwrap(),
            chosen
        );
        assert!(!anchors.as_array().unwrap().is_empty());
        for anchor in anchors.as_array().unwrap() {
            assert_eq!(anchor["project"], chosen);
            assert_eq!(
                anchor["host"].as_str(),
                (!mobile).then_some("fixture-remote")
            );
            assert!(anchor.get("view").is_none());
        }
        for id in ["workers", "secrets"] {
            assert_eq!(
                p.eval(&format!(
                    "document.querySelector('#nav-{id}').getAttribute('href')"
                ))
                .unwrap(),
                format!("/{id}")
            );
        }
    }
    p.eval("delete document.documentElement.dataset.issueMobile;history.replaceState(null,'','/artifacts')").unwrap();
    let fallback = "github.com/example/default";
    for (saved, expected) in [
        ("local:machine:/Users/test", fallback),
        ("local:machine:/home/test", fallback),
        ("local:machine:/root", fallback),
        ("github.com/example/chosen", "github.com/example/chosen"),
    ] {
        let result=p.eval(&format!("localStorage.setItem('hey-boss-issues-project',JSON.stringify({}));HeyBossUI.projectId({})",json!(saved),json!(fallback))).unwrap();
        assert_eq!(result, expected);
    }
    p.eval("history.replaceState(null,'','#project=github.com/example/link')")
        .unwrap();
    assert_eq!(
        p.eval("HeyBossUI.projectId('fallback')").unwrap(),
        "github.com/example/link"
    );
    for (path, hash, pinned) in [
        ("/artifacts", "", true),
        ("/workers", "", false),
        ("/secrets", "", false),
        ("/secrets", "#project=old", true),
    ] {
        let actual=p.eval(&format!("history.replaceState(null,'',{0});HeyBossUI.projectNavigation({1});new URLSearchParams(location.hash.slice(1)).get('project')",json!(format!("{path}{hash}")),json!(chosen))).unwrap();
        assert_eq!(actual, if pinned { json!(chosen) } else { Value::Null });
    }
    assert_eq!(p.eval("window.name").unwrap(), "");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn a_cancelled_host_switch_cannot_move_a_local_draft() {
    let f = fixture();
    f.init("Hosts").unwrap();
    run(f.issue("Hosts", &["create", "--title", "Local draft"])).unwrap();
    let (_web, base) = web(&f, "Hosts");
    let mut p = open();
    go(&mut p, &format!("{base}/#project=named%3AHosts&issue=1"));
    p.wait("!!document.querySelector('#comment-body')");
    // Stand in for a second host: use the same API data, but hold its projects reply
    // until the browser has finished returning to the local issue.
    p.eval("const fetchLocal=window.fetch; window.fetch=async(path,opts)=>{if(path==='/api/action' && opts?.body){const b=JSON.parse(opts.body); if(b.host==='fixture-peer' && b.operation?.action==='projects'){b.host=null;const response=await fetchLocal(path,{...opts,body:JSON.stringify(b)});await new Promise(done=>window.releasePeer=done);return response}}return fetchLocal(path,opts)};document.querySelector('#comment-body').value='Local comment'; window.hostSwitch=navigate({host:'fixture-peer'});undefined").unwrap();
    p.wait("typeof window.releasePeer==='function'");
    assert_eq!(
        p.eval("document.querySelector('#comment-body').value")
            .unwrap(),
        "Local comment"
    );
    p.eval("navigate({host:model.defaultHost||''})").unwrap();
    let assert_local = |p: &mut Page| {
        p.wait("model.activeHost===(model.defaultHost||'') && model.route.host===model.activeHost && model.detail?.issue.number===1");
        assert_eq!(
            p.eval("storage.get(draftKey('comment','named:Hosts',1,'fixture-peer'))")
                .unwrap(),
            Value::Null
        );
        assert_eq!(
            p.eval("document.querySelector('#comment-body').value")
                .unwrap(),
            "Local comment"
        );
    };
    assert_local(&mut p);
    p.eval("releasePeer();hostSwitch").unwrap();
    assert_local(&mut p);
    assert_eq!(p.eval("window.name").unwrap(), "");
}

fn shortcut(p: &mut Page, key: &str, code: &str) {
    for kind in ["keyDown", "keyUp"] {
        p.command(
            "Input.dispatchKeyEvent",
            json!({"type":kind,"key":key,"code":code}),
        )
        .unwrap();
    }
}
#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn slow_issue_lists_leave_keyboard_navigation_and_drafts_responsive() {
    let f = fixture();
    f.init("Keyboard").unwrap();
    run(f.issue("Keyboard", &["create", "--title", "Issue navigation"])).unwrap();
    let (_web, base) = web(&f, "Keyboard");
    let mut p = open();
    let url = format!("{base}/#project=named%3AKeyboard&issue=1");
    go(&mut p, &url);
    p.wait("!!document.querySelector('#comment-body')");
    p.eval("document.querySelector('#comment-body').value='Unsent during navigation'; const nativeFetch=window.fetch; let releaseList; const gate=new Promise(done=>releaseList=done); let lists=0; window.fetch=async(path,opts)=>{const r=await nativeFetch(path,opts);if(path==='/api/action' && opts?.body && JSON.parse(opts.body).operation?.action==='list'){lists++;await gate}return r};undefined").unwrap();
    let start=p.eval("const seq=model.sequence;document.querySelector('[data-back]').click();document.body.focus();document.body.dispatchEvent(new KeyboardEvent('keydown',{key:'/',bubbles:true,cancelable:true}));[model.route.issue,document.querySelector('#list-view').hidden,document.activeElement.id,model.sequence-seq]").unwrap();
    assert_eq!(start, json!([null, false, "issue-search", 1]));
    let draft = |p: &mut Page, i: usize| {
        p.eval("document.querySelector('#new-issue').focus()")
            .unwrap();
        shortcut(p, "n", "KeyN");
        p.wait("document.querySelector('#editor-dialog').open");
        let title = json!(format!("Draft {i}"));
        p.eval(&format!(
            "document.querySelector('#editor-subject').value={title}"
        ))
        .unwrap();
        escape(p);
        shortcut(p, "n", "KeyN");
        p.wait(&format!("document.querySelector('#editor-dialog').open && document.querySelector('#editor-subject').value==={title}"));
        escape(p);
        shortcut(p, "/", "Slash");
        p.wait("document.activeElement.id==='issue-search'");
    };
    draft(&mut p, 0);
    p.wait("lists>0 && !model.signature");
    p.eval("releaseList();window.fetch=nativeFetch;undefined")
        .unwrap();
    p.wait("!!model.signature");
    p.eval("history.back()").unwrap();
    p.wait("model.detail?.issue.number===1 && document.querySelector('#comment-body')?.value==='Unsent during navigation'");
    p.eval("history.forward()").unwrap();
    p.wait("model.route.issue===null && !document.querySelector('#list-view').hidden");
    for i in 1..=20 {
        go(&mut p, &url);
        p.wait("model.detail?.issue.number===1");
        click(&mut p, "[data-back]");
        draft(&mut p, i);
    }
    p.eval("location.hash+='&issue=1'").unwrap();
    p.wait("model.detail?.issue.number===1");
    assert_eq!(p.eval("window.name").unwrap(), "");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn worker_aliases_preserve_the_selected_project_and_keyboard_navigation() {
    let f = fixture();
    let (project, _) = Scenario::AgentsNavigation.destination();
    f.init(project).unwrap();
    seed::seed(&f, Scenario::AgentsNavigation, project).unwrap();
    let (_web, base) = web(&f, project);
    let mut p = open();
    go(&mut p, &format!("{base}/workers#project=named%3AAtlas"));
    selected(&mut p, "/workers", "Atlas");
    p.wait("document.querySelector('#worker-board')?.checkVisibility()");
    assert_eq!(p.eval("!document.querySelector('.fleet-tabs') && !document.querySelector('.app-navigation #nav-workers') && document.querySelector('#nav-workers').getAttribute('href')==='/workers'").unwrap(),true);
    pick(&mut p, "/workers", "Beacon");
    p.wait("document.querySelector('#worker-board')?.checkVisibility()");
    for width in [1280, 390, 320] {
        p.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":false}),
        )
        .unwrap();
        for route in [
            "/agents",
            "/agents#view=conversations",
            "/workers",
            "/agents/session#project=named%3ABeacon&issue=42",
        ] {
            go(&mut p, &format!("{base}{route}"));
            p.wait("document.querySelector('#project-name')?.textContent==='Beacon'");
            if route.starts_with("/agents/session") {
                p.wait("document.querySelector('#back')?.href.includes('issue=42')");
            } else {
                p.wait("document.querySelector('#worker-config')?.checkVisibility()");
            }
            assert_eq!(
                p.eval("document.documentElement.scrollWidth<=innerWidth")
                    .unwrap(),
                true,
                "{route} {width}"
            );
            let layout=p.eval(r#"Array.from(document.querySelectorAll('.app-navigation .navigation-inner > a')).filter(a=>a.checkVisibility()).map(a=>{
                a.scrollIntoView({block:'nearest',inline:'nearest'}); const box=a.getBoundingClientRect();
                const fits=[...a.childNodes].filter(n=>n.nodeType===Node.TEXT_NODE&&n.textContent.trim()).every(n=>{const r=document.createRange();r.selectNodeContents(n);const t=r.getBoundingClientRect();return t.left>=box.left&&t.right<=box.right});
                return box.left>=0 && box.right<=innerWidth+1 && fits;
            })"#).unwrap();
            let links = layout.as_array().unwrap();
            assert!(
                !links.is_empty() && links.iter().all(|ok| ok == true),
                "{route} {width}: {links:?}"
            );
            if width < 560 {
                p.eval("document.querySelector('#navigation-more').focus()")
                    .unwrap();
                key(&mut p, "Enter", "Enter", 13, "\r");
                p.wait("document.querySelector('#navigation-more-links').matches(':popover-open')");
                key(&mut p, "Tab", "Tab", 9, "");
                p.wait("(()=>{const a=document.querySelector('#nav-workers'),r=a.getBoundingClientRect();return a===document.activeElement && r.left>=0 && r.right<=innerWidth})()");
                key(&mut p, "Escape", "Escape", 27, "");
            }
        }
    }
    assert_eq!(p.eval("window.name").unwrap(), "");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn worker_broadcast_keeps_drafts_and_reports_partial_or_empty_delivery() {
    let f = fixture();
    let (project, _) = Scenario::AgentsNavigation.destination();
    f.init(project).unwrap();
    seed::seed(&f, Scenario::AgentsNavigation, project).unwrap();
    let (_web, base) = web(&f, project);
    let mut p = open();
    go(&mut p, &format!("{base}/workers"));
    p.wait("document.querySelector('#broadcast-open')?.checkVisibility()");
    p.eval("window.sent=[];const realFetch=window.fetch.bind(window);window.fetch=(url,opts)=>url==='/api/fleet/broadcast'?(sent.push(JSON.parse(opts.body)),Promise.resolve(Response.json(reply))):realFetch(url,opts)").unwrap();
    for (width, theme, total, queued, offline) in [
        (1280, "light", 3, 3, 0),
        (390, "dark", 3, 1, 1),
        (320, "light", 0, 0, 0),
    ] {
        p.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":800,"deviceScaleFactor":1,"mobile":false}),
        )
        .unwrap();
        p.command(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
        )
        .unwrap();
        p.eval(&format!(
            "window.reply={}",
            json!({"ok":true,"total":total,"queued":queued,"offline":offline})
        ))
        .unwrap();
        pointer_click(&mut p, "#broadcast-open");
        p.wait("document.querySelector('#broadcast-dialog')?.open");
        p.eval("document.querySelector('#broadcast-text').value='First line\\nSecond line'")
            .unwrap();
        assert_eq!(p.eval("(()=>{const d=document.querySelector('#broadcast-dialog');return d.scrollWidth<=d.clientWidth && d.getBoundingClientRect().right<=innerWidth})()").unwrap(),true);
        key(&mut p, "Escape", "Escape", 27, "");
        p.wait("document.activeElement===document.querySelector('#broadcast-open')");
        pointer_click(&mut p, "#broadcast-open");
        assert_eq!(
            p.eval("document.querySelector('#broadcast-text').value")
                .unwrap(),
            "First line\nSecond line"
        );
        click(&mut p, "#broadcast-form [type=submit]");
        let msg = if total == 0 {
            "No active agents on connected machines.".into()
        } else {
            format!("Queued for {queued} of {total} active agents.")
        };
        p.wait(&format!(
            "document.querySelector('#broadcast-status').innerText.includes({})",
            json!(msg)
        ));
        if offline > 0 {
            assert_eq!(p.eval("document.querySelector('#broadcast-status').innerText.includes('Offline machines: 1')").unwrap(),true);
        }
    }
    assert_eq!(
        p.eval("sent").unwrap(),
        json!(vec![json!({"text":"First line\nSecond line"}); 3])
    );
    assert_eq!(p.eval("window.name").unwrap(), "");
}
