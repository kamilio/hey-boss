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
    assert!(
        call("parse", json!(["Fix @poe-code", duplicates, current]))
            .unwrap_err()
            .contains("ambiguous")
    );
    assert!(
        call("parse", json!(["Fix", projects, null]))
            .unwrap_err()
            .contains("project")
    );
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
        assert!(
            p.eval("document.querySelector('#quick-issue-error').innerText")
                .unwrap()
                .as_str()
                .unwrap()
                .contains(error)
        );
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
