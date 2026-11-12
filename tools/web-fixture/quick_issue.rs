//! Exercise the quick-add client in the page that ships it.
use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn mentions_and_task_labels() {
    let f = fixture();
    f.init("Quick").unwrap();
    let (_web, base) = web(&f, "Quick");
    let mut p = Page::open(&format!("{base}/workers"));
    p.wait("typeof HeyBossQuickIssue !== 'undefined'");
    let mut call =
        |name: &str, args: Value| p.eval(&format!("HeyBossQuickIssue.{name}(...{args})"));
    let projects = json!([
        {"id":"github.com/kamilio/hey-boss","name":"hey-boss"},
        {"id":"github.com/kamilio/poe-code","name":"poe-code"},
        {"id":"named:Design Team","name":"Design Team"},
        {"id":"named:café","name":"café"}
    ]);
    let mut duplicates = projects.clone();
    duplicates
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"github.com/other/poe-code","name":"poe-code"}));
    let current = &projects[0];
    for (text, title, project) in [
        ("Fix reconnect", "Fix reconnect", 0),
        ("@poe-code Fix reconnect", "Fix reconnect", 1),
        ("Fix @poe-code reconnect", "Fix reconnect", 1),
        ("Fix reconnect @poe-code", "Fix reconnect", 1),
        ("Fix (@poe-code), reconnect", "Fix (), reconnect", 1),
        ("Fix @POE-CODE reconnect", "Fix reconnect", 1),
        (
            "Fix @github.com/kamilio/poe-code reconnect",
            "Fix reconnect",
            1,
        ),
        (r#"Fix @"Design Team" reconnect"#, "Fix reconnect", 2),
        ("Fix @'Design Team' reconnect", "Fix reconnect", 2),
        ("Fix @café reconnect", "Fix reconnect", 3),
        ("Fix @cafe\u{301} reconnect", "Fix reconnect", 3),
        ("Fix\n@poe-code\t reconnect", "Fix reconnect", 1),
        ("Fix @poe-code and @poe-code", "Fix and", 1),
        ("Email boss@poe-code.com", "Email boss@poe-code.com", 0),
        ("Email 𐐀@poe-code.com", "Email 𐐀@poe-code.com", 0),
        (
            r"Document \@poe-code syntax",
            "Document @poe-code syntax",
            0,
        ),
        ("Fix @poe-code. Now", "Fix . Now", 1),
    ] {
        assert_eq!(
            call("parse", json!([text, projects, current])).unwrap(),
            json!({"title":title,"project":projects[project]}),
            "{text}"
        );
    }
    for text in [
        "@missing Fix",
        "@poe-code-extra Fix",
        "@poe-code @hey-boss Fix",
        "@\"Design Team Fix",
        "@ Fix",
        "@poe-code",
        "   ",
    ] {
        assert!(
            call("parse", json!([text, projects, current])).is_err(),
            "{text}"
        );
    }
    assert!(call("parse", json!(["Fix @poe-code", duplicates, current]))
        .unwrap_err()
        .contains("ambiguous"));
    assert!(call("parse", json!(["Fix", projects, null]))
        .unwrap_err()
        .contains("project"));
    for (text, caret, start, end, query) in [
        ("Fix @po reconnect", 7, 4, 7, "po"),
        ("Fix @poe-code reconnect", 7, 4, 13, "po"),
        ("@", 1, 0, 1, ""),
        ("Fix @po. Now", 7, 4, 7, "po"),
        ("Fix @\"Design T\" reconnect", 14, 4, 15, "Design T"),
        ("@𐐀team", 7, 0, 7, "𐐀team"),
        ("@cafe\u{301}", 6, 0, 6, "cafe\u{301}"),
        ("@\"Design @Team\"", 13, 0, 15, "Design @Tea"),
    ] {
        assert_eq!(
            call("mentionAt", json!([text, caret])).unwrap(),
            json!({"start":start,"end":end,"query":query}),
            "{text}"
        );
    }
    for (text, caret) in [
        ("boss@po", 7),
        ("𐐀@po", 5),
        (r"Fix \@po", 8),
        ("Fix @po ", 8),
        ("Fix @\"Design Team\" ", 19),
        ("Fix @po", 2),
        ("Fix @po. Now", 8),
    ] {
        assert_eq!(
            call("mentionAt", json!([text, caret])).unwrap(),
            Value::Null,
            "{text}"
        );
    }
    for (query, expected) in [
        ("PO", json!([projects[1]])),
        ("cafe\u{301}", json!([projects[3]])),
        ("kamilio/poe", json!([projects[1]])),
        ("unknown", json!([])),
    ] {
        assert_eq!(
            call("suggestions", json!([projects, query])).unwrap(),
            expected
        );
    }
    let ranked = json!([{"id":"long","name":"Deploy tools"},{"id":"exact","name":"Deploy"},{"id":"contains","name":"QA deploy"}]);
    assert_eq!(
        call("suggestions", json!([ranked, "deploy"])).unwrap(),
        json!([ranked[1], ranked[0], ranked[2]])
    );
    let many: Vec<_> = (0..20)
        .map(|i| json!({"id":i.to_string(),"name":format!("Project {i}")}))
        .collect();
    assert_eq!(
        call("suggestions", json!([many, ""]))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        8
    );
    let unusual = json!([{"id":"named:quote","name":r#"Team "A" \ B"#}]);
    for (text, caret, known, index, expected) in [
        (
            "Fix @po reconnect",
            7,
            &projects,
            1,
            "Fix @poe-code reconnect",
        ),
        (
            "Fix @poe-code reconnect",
            7,
            &projects,
            0,
            "Fix @hey-boss reconnect",
        ),
        ("Fix @", 5, &projects, 2, "Fix @\"Design Team\" "),
        (
            "Fix @po",
            7,
            &duplicates,
            1,
            "Fix @github.com/kamilio/poe-code ",
        ),
        (
            "Fix (@po), reconnect",
            8,
            &projects,
            1,
            "Fix (@poe-code), reconnect",
        ),
        ("Fix @po. Now", 7, &projects, 1, "Fix @poe-code. Now"),
        (
            "Fix @\"Design T\" reconnect",
            14,
            &projects,
            2,
            "Fix @\"Design Team\" reconnect",
        ),
        ("Fix @", 5, &unusual, 0, r#"Fix @"Team \"A\" \\ B" "#),
    ] {
        let mention = call("mentionAt", json!([text, caret])).unwrap();
        let result = call(
            "completeMention",
            json!([text, mention, known[index], known]),
        )
        .unwrap();
        assert_eq!(result["text"], expected);
        assert!(result["caret"].as_u64().unwrap() > mention["start"].as_u64().unwrap());
        assert_eq!(
            call("parse", json!([expected, known, current])).unwrap()["project"],
            known[index]
        );
    }
    for (labels, kind) in [
        (json!(["ready", "task:plan"]), "plan"),
        (json!(["task:plan", "task:research"]), "plan"),
        (json!(["task:research"]), "plan"),
        (json!(["plan", "research"]), "implement"),
    ] {
        assert_eq!(call("taskKind", json!([labels])).unwrap(), kind);
    }
    for (labels, kind, expected) in [
        (
            json!(["ready", "task:research", "ready"]),
            "plan",
            json!(["ready", "task:plan"]),
        ),
        (json!(["ready"]), "research", json!(["ready"])),
        (json!(["bug", "task:research"]), "implement", json!(["bug"])),
    ] {
        assert_eq!(call("taskLabels", json!([labels, kind])).unwrap(), expected);
    }
}

fn key(p: &mut Page, key: &str, code: u32, modifiers: u32) {
    for event in ["keyDown", "keyUp"] {
        p.command(
            "Input.dispatchKeyEvent",
            json!({"type":event,"key":key,"windowsVirtualKeyCode":code,"modifiers":modifiers}),
        )
        .unwrap();
    }
}
fn open(p: &mut Page) {
    key(p, "K", 75, 10);
    p.wait("document.querySelector('#quick-issue-dialog').open && document.activeElement.id==='quick-issue-title' && document.querySelector('#quick-issue-context').textContent.startsWith('Create in')");
}
fn fill(p: &mut Page, text: &str) {
    p.eval(&format!("document.querySelector('#quick-issue-title').value={}; document.querySelector('#quick-issue-title').dispatchEvent(new Event('input'))",json!(text))).unwrap();
}
fn saved(p: &mut Page, f: &Fixture, project: &str, title: &str) {
    p.wait("!document.querySelector('#quick-issue-dialog').open && !!document.querySelector('#quick-issue-status a')");
    let route=p.eval("Object.fromEntries(new URLSearchParams(document.querySelector('#quick-issue-status a').hash.slice(1)))").unwrap();
    assert_eq!(route["project"], format!("named:{project}"));
    let result = run(f.issue(project, &["view", route["issue"].as_str().unwrap()])).unwrap();
    assert_eq!(result["issue"]["title"], title);
    assert_eq!(result["issue"]["body"], "");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn create_from_each_surface_and_retry_a_lost_reply() {
    let f = fixture();
    for project in ["Quick", "Other"] {
        f.init(project).unwrap();
    }
    let (_web, base) = web(&f, "Quick");
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    let navigate = |p: &mut Page, path: &str| {
        p.command("Page.navigate", json!({"url":format!("{base}{path}")}))
            .unwrap();
    };
    navigate(&mut p, "/#project=named%3AQuick&quick-issue=1");
    p.wait("document.querySelector('#quick-issue-dialog')?.open && !location.hash.includes('quick-issue')");
    key(&mut p, "Escape", 27, 0);
    for (path, surface) in [
        ("/#project=named%3AQuick&view=issues", "Issues"),
        ("/#project=named%3AQuick&view=inbox", "Inbox"),
        ("/workers#project=named%3AQuick", "Workers"),
    ] {
        navigate(&mut p, path);
        p.wait("document.readyState==='complete' && !!document.querySelector('#quick-issue-open')");
        open(&mut p);
        let title = format!("Created from {surface}");
        fill(&mut p, &format!("{title} @Ot"));
        key(&mut p, "Enter", 13, 0);
        assert_eq!(
            p.eval("document.activeElement.value").unwrap(),
            format!("{title} @Other ")
        );
        key(&mut p, "Enter", 13, 0);
        saved(&mut p, &f, "Other", &title);
    }

    navigate(&mut p, "/#project=named%3AQuick&view=issues");
    p.wait("typeof model !== 'undefined' && !!model.signature");
    p.eval("document.querySelector('#new-issue').click(); document.querySelector('#editor-subject').focus(); document.activeElement.value='Preserved editor draft'").unwrap();
    open(&mut p);
    fill(&mut p, "Quick while editing");
    key(&mut p, "Enter", 13, 0);
    saved(&mut p, &f, "Quick", "Quick while editing");
    p.wait("document.querySelector('#editor-dialog').open && document.activeElement.id==='editor-subject' && document.activeElement.value==='Preserved editor draft'");
    open(&mut p);
    for (input, error) in [
        ("Fix @missing", "Unknown project"),
        ("@Quick @Other Fix", "only one project"),
    ] {
        fill(&mut p, input);
        key(&mut p, "Enter", 13, 0);
        assert!(p
            .eval("document.querySelector('#quick-issue-error').innerText")
            .unwrap()
            .as_str()
            .unwrap()
            .contains(error));
    }
    key(&mut p, "Escape", 27, 0);
    p.wait("document.querySelector('#editor-dialog').open && !document.querySelector('#quick-issue-dialog').open");
    p.eval("document.querySelector('#editor-cancel').click()")
        .unwrap();
    open(&mut p);
    assert_eq!(
        p.eval("document.activeElement.value").unwrap(),
        "@Quick @Other Fix"
    );
    fill(&mut p, "IME draft");
    p.eval("document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,isComposing:true}))").unwrap();
    assert_eq!(
        p.eval(
            "document.querySelector('#quick-issue-dialog').open && !document.activeElement.disabled"
        )
        .unwrap(),
        true
    );
    for (width, height) in [(390, 844), (1280, 800)] {
        key(&mut p, "Escape", 27, 0);
        p.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
        )
        .unwrap();
        p.eval("document.querySelector('#quick-issue-open').click()")
            .unwrap();
        p.wait("document.querySelector('#quick-issue-dialog').open && document.querySelector('#quick-issue-context').textContent.startsWith('Create in')");
        fill(&mut p, "Fix @Ot");
        p.wait("document.querySelector('#quick-issue-title').getAttribute('aria-expanded')==='true' && document.querySelector('#quick-issue-dialog').getBoundingClientRect().right <= innerWidth");
        screenshot(&mut p, "out/quick-issue", &format!("picker-{width}"));
        key(&mut p, "Escape", 27, 0); // dismiss the picker
    }
    // Both requests reach the real server. Drop the first reply after commit and
    // hold the second until the user has pressed Enter again.
    p.eval(r#"window.requests=[]; const normal=window.fetch; window.fetch=async (...args)=>{
        if(args[0]!='/api/action' || JSON.parse(args[1].body).operation.action!=='create') return normal(...args);
        requests.push(JSON.parse(args[1].body).request_id);
        if(requests.length===1){const response=await normal(...args); if(!(await response.json()).ok)throw Error('create failed'); throw TypeError('reply lost');}
        await new Promise(resolve=>window.releaseReply=resolve); return normal(...args);
    }"#).unwrap();
    fill(&mut p, "@Other Retry only once");
    key(&mut p, "Enter", 13, 0);
    p.wait("document.querySelector('#quick-issue-error').innerText.includes('reply lost') && document.activeElement.value==='@Other Retry only once'");
    key(&mut p, "Enter", 13, 0);
    p.wait("typeof releaseReply==='function'");
    key(&mut p, "Enter", 13, 0);
    p.eval("releaseReply()").unwrap();
    saved(&mut p, &f, "Other", "Retry only once");
    assert_eq!(
        p.eval("requests.length===2 && !!requests[0] && requests[0]===requests[1]")
            .unwrap(),
        true
    );
    let db = rusqlite::Connection::open(&f.db).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM issues WHERE title='Retry only once'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn task_intent_and_queue_placement_survive_drafts_and_retries() {
    let f = installed_fixture();
    let project = "Task intent";
    f.init(project).unwrap();
    run(f.issue(project, &["create", "--title", "Existing"])).unwrap();
    let (_web, base) = web(&f, project);
    let url = format!("{base}/#project=named%3ATask+intent");
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"window.el=s=>document.querySelector(s);window.click=s=>el(s).click();window.setField=(s,v)=>{const e=el(s);e.value=v;e.dispatchEvent(new Event(e.tagName==='SELECT'?'change':'input',{bubbles:true}))};window.issueOrder=()=>[...document.querySelectorAll('#list-view .issue-row[data-issue-number]')].map(e=>Number(e.dataset.issueNumber));addEventListener('error',e=>window.name=e.message)"})).unwrap();
    p.command("Page.navigate", json!({"url":url})).unwrap();
    let order = |p: &mut Page, numbers: &[u32]| {
        p.wait(&format!(
            "JSON.stringify(issueOrder())==='{}'",
            json!(numbers)
        ))
    };
    let view = |number: &str| run(f.issue(project, &["view", number])).unwrap()["issue"].clone();
    order(&mut p, &[1]);
    for (kind, number) in [("implement", "2"), ("plan", "3")] {
        p.eval("click('#new-issue')").unwrap();
        p.wait("el('#editor-dialog').open && !el('#editor-bottom').checked && el('#editor-kind').value==='implement'");
        p.eval(&format!("setField('#editor-kind',{0});setField('#editor-subject','Review '+{0});setField('#editor-body','Keep the requirements.');setField('#editor-labels','ready, design');el('#editor-subject').focus()",json!(kind))).unwrap();
        if kind == "implement" {
            key(&mut p, "B", 66, 12);
            assert_eq!(p.eval("el('#editor-bottom').checked").unwrap(), true);
            p.eval("click('#editor-cancel');click('#new-issue')")
                .unwrap();
            p.wait(
                "el('#editor-bottom').checked && el('#editor-subject').value==='Review implement'",
            );
        }
        p.eval("click('#editor-submit')").unwrap();
        p.wait("!el('#editor-dialog').open");
        order(
            &mut p,
            if kind == "implement" {
                &[1, 2]
            } else {
                &[3, 1, 2]
            },
        );
        p.wait(&format!(
            "el('.issue-created')?.dataset.issueNumber==='{number}'"
        ));
        let issue = view(number);
        assert_eq!(issue["body"], "Keep the requirements.");
        let mut labels = issue["labels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        labels.sort();
        assert_eq!(
            labels,
            if kind == "plan" {
                vec!["design", "ready", "task:plan"]
            } else {
                vec!["design", "ready"]
            }
        );
    }
    p.eval("click('[data-issue-number=\"3\"] .issue-title')")
        .unwrap();
    p.wait("el('[data-edit]')?.checkVisibility()");
    p.eval("click('[data-edit]')").unwrap();
    assert_eq!(p.eval("el('#editor-kind').value==='plan' && !el('#editor-tags .tag-chips').textContent.includes('task:') && !el('#editor-bottom-control').checkVisibility()").unwrap(),true);
    key(&mut p, "B", 66, 10);
    assert_eq!(p.eval("el('#editor-bottom').checked").unwrap(), false);
    p.eval("setField('#editor-kind','implement');click('#editor-cancel');click('[data-edit]')")
        .unwrap();
    assert_eq!(p.eval("el('#editor-kind').value").unwrap(), "implement");
    p.eval("click('#editor-submit')").unwrap();
    p.wait("!el('#editor-dialog').open");
    let issue = view("3");
    assert_eq!(issue["body"], "Keep the requirements.");
    assert_eq!(issue["labels"].as_array().unwrap().len(), 2);
    assert!(["ready", "design"]
        .iter()
        .all(|name| issue["labels"].as_array().unwrap().contains(&json!(name))));
    p.wait("el('[data-edit]')?.checkVisibility()");
    p.eval("click('[data-edit]');setField('#editor-kind','plan');click('#editor-submit')")
        .unwrap();
    p.wait("!el('#editor-dialog').open");
    p.command("Page.navigate", json!({"url":url})).unwrap();
    order(&mut p, &[3, 1, 2]);
    open(&mut p);
    assert_eq!(p.eval("el('#quick-issue-bottom').checked").unwrap(), false);
    fill(&mut p, "Quick plan");
    p.eval("setField('#quick-issue-kind','plan')").unwrap();
    key(&mut p, "B", 66, 10);
    key(&mut p, "Escape", 27, 0);
    open(&mut p);
    p.wait("el('#quick-issue-bottom').checked && el('#quick-issue-kind').value==='plan'");
    p.eval("window.creates=[];const send=window.fetch.bind(window);window.fetch=(url,options)=>{if(url==='/api/action'){const req=JSON.parse(options.body);if(req.operation.action==='create'){creates.push(req);if(creates.length===1)return Promise.resolve(Response.json({ok:false,error:{message:'Interrupted'}},{status:503}))}}return send(url,options)}").unwrap();
    p.eval("click('#quick-issue-submit')").unwrap();
    p.wait("el('#quick-issue-error').innerText.includes('Interrupted') && el('#quick-issue-title').value==='Quick plan' && el('#quick-issue-kind').value==='plan'");
    p.eval("click('#quick-issue-submit')").unwrap();
    order(&mut p, &[3, 1, 2, 4]);
    assert_eq!(p.eval("creates.length===2 && !!creates[0].request_id && JSON.stringify(creates[0])===JSON.stringify(creates[1])").unwrap(),true);
    assert_eq!(view("4")["labels"], json!(["task:plan"]));
    open(&mut p);
    assert_eq!(
        p.eval("!el('#quick-issue-bottom').checked && el('#quick-issue-kind').value==='implement'")
            .unwrap(),
        true
    );
    fill(&mut p, "Quick implementation");
    key(&mut p, "Enter", 13, 0);
    order(&mut p, &[5, 3, 1, 2, 4]);
    p.eval("window.beforeReload=true").unwrap();
    p.command("Page.reload", json!({})).unwrap();
    p.wait("!window.beforeReload && el('#list-view')?.checkVisibility()");
    order(&mut p, &[5, 3, 1, 2, 4]);
    for theme in ["light", "dark"] {
        p.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        for width in [1440, 768, 390, 320] {
            p.command("Emulation.setDeviceMetricsOverride",json!({"width":width,"height":if width<=390{844}else{1000},"deviceScaleFactor":1,"mobile":false})).unwrap();
            assert_eq!(
                p.eval("document.querySelectorAll('#list-view .task-kind-badge').length")
                    .unwrap(),
                2
            );
            screenshot(&mut p, "out/task-intent", &format!("list-{theme}-{width}"));
            for (prefix, open, placement) in [
                ("editor", "#new-issue", "#editor-bottom-control"),
                ("quick-issue", "#quick-issue-open", "#quick-issue-bottom"),
            ] {
                p.eval(&format!("click({})", json!(open))).unwrap();
                p.wait(&format!(
                    "el('#{prefix}-dialog').open && !el('#{prefix}-kind').disabled"
                ));
                p.eval(&format!(
                    "setField('#{prefix}-kind','plan');el('#{prefix}-kind').focus()"
                ))
                .unwrap();
                assert_eq!(p.eval(&format!("(()=>{{const e=el('#{prefix}-kind'),d=el('#{prefix}-dialog'),r=d.getBoundingClientRect();return el('{placement}').checkVisibility() && e===document.activeElement && e.labels.length>0 && (innerWidth>390||parseFloat(getComputedStyle(e).fontSize)>=16) && r.left>=0 && r.right<=innerWidth+1 && d.scrollWidth<=d.clientWidth+1}})()")).unwrap(),true,"{prefix} {theme}/{width}");
                if prefix == "editor" {
                    assert_eq!(
                        p.eval("el('#editor-kind-help').textContent").unwrap(),
                        "A concrete plan, saved as a linked artifact."
                    );
                }
                screenshot(
                    &mut p,
                    "out/task-intent",
                    &format!("{prefix}-{theme}-{width}"),
                );
                key(&mut p, "Escape", 27, 0);
                p.wait(&format!("!el('#{prefix}-dialog').open"));
            }
            assert_eq!(
                p.eval("document.documentElement.scrollWidth<=innerWidth+1 && !window.name")
                    .unwrap(),
                true
            );
        }
    }
}
