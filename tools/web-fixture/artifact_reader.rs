use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn disabled_artifacts_keep_existing_documents_accessible_on_mobile() {
    let f = installed_fixture();
    for project in ["Empty Resources", "Existing Resources"] {
        f.init(project).unwrap();
        if project == "Existing Resources" {
            run(f.command(&[
                "artifact",
                "create",
                "--project",
                project,
                "--title",
                "Existing document",
                "--body",
                "Preserved content",
                "--json",
            ]))
            .unwrap();
        }
        run(f.issue(project, &["settings", "set", "--no-artifacts"])).unwrap();
    }
    let (_server, base) = web(&f, "Empty Resources");
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error',e=>window.name=e.message);addEventListener('unhandledrejection',e=>window.name=String(e.reason))"})).unwrap();
    p.command(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":false}),
    )
    .unwrap();
    let navigate = |p: &mut Page, hash: &str| {
        p.command(
            "Page.navigate",
            json!({"url":format!("{base}/artifacts#project=named%3A{hash}")}),
        )
        .unwrap();
    };
    let no_create = |p: &mut Page| {
        assert_eq!(p.eval("Array.from(document.querySelectorAll('button')).filter(b=>b.checkVisibility() && (b.getAttribute('aria-label') || b.textContent).trim()==='New artifact').length").unwrap(),0);
        assert_eq!(p.eval("window.name").unwrap(), "");
    };
    navigate(&mut p, "Empty+Resources");
    p.wait(
        "document.querySelector('.artifact-empty h2')?.textContent==='Artifacts are turned off'",
    );
    p.wait("document.querySelector('.artifact-empty p')?.innerText==='This project is not using artifacts. Use regular tasks. Existing documents remain available.'");
    no_create(&mut p);
    navigate(&mut p, "Empty+Resources&new=1");
    p.wait(r#"document.body.innerText.includes('Project "Empty Resources" is not using artifacts. Use regular tasks.')"#);
    assert_eq!(
        p.eval("document.querySelector('#artifact-title')===null")
            .unwrap(),
        true
    );
    navigate(&mut p, "Existing+Resources");
    p.wait("document.querySelector('.artifact-row-link')?.innerText.includes('Existing document')");
    no_create(&mut p);
    p.eval("document.querySelector('.artifact-row-link').click()")
        .unwrap();
    p.wait("document.querySelector('#artifact-reading')?.innerText.trim()==='Preserved content'");
    p.wait("document.querySelector('#artifact-edit')?.checkVisibility() && document.documentElement.scrollWidth<=innerWidth+1");
    no_create(&mut p);
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn missing_artifact_returns_to_library_and_document_links_keep_the_reader_route() {
    let f = installed_fixture();
    let project = "Reading";
    f.init(project).unwrap();
    let body = format!(
        "# Notes\n\nA footnote.[^source]\n\n{}\n```text\n{}\n```\n\n[^source]: Source explanation.",
        (0..50)
            .map(|i| format!("Paragraph {i} with some useful context.\n\n"))
            .collect::<String>(),
        "Wide code with useful context. ".repeat(10)
    );
    run(f.command(&[
        "artifact",
        "create",
        "--project",
        project,
        "--title",
        "Document navigation",
        "--body",
        &body,
        "--json",
    ]))
    .unwrap();
    let (_server, base) = web(&f, project);
    let mut p = Page::open("about:blank");
    p.command(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":false}),
    )
    .unwrap();
    let missing = format!("{base}/artifacts#project=named%3AReading&artifact=missing-document");
    p.command("Page.navigate", json!({"url":missing})).unwrap();
    p.wait("document.querySelector('#artifact-error')?.checkVisibility()");
    assert_eq!(
        p.eval("document.querySelectorAll('.artifact-loading').length")
            .unwrap(),
        0
    );
    p.eval("Array.from(document.querySelectorAll('a')).find(a=>a.innerText.trim()==='All artifacts' && a.checkVisibility()).click()").unwrap();
    p.wait(
        "document.querySelector('.artifact-row-link')?.innerText.includes('Document navigation')",
    );
    p.eval("document.querySelector('.artifact-row-link').click()")
        .unwrap();
    p.wait("!!document.querySelector('#artifact-reading pre')");
    let route = p.eval("location.href").unwrap();
    p.eval("document.querySelector('.skip-link').focus()")
        .unwrap();
    key(&mut p, "Enter", "Enter", 13, "\r");
    p.wait("document.activeElement.id==='artifact-main'");
    assert_eq!(p.eval("location.href").unwrap(), route);
    p.eval("document.querySelector('#artifact-reading .footnote-reference a').click()")
        .unwrap();
    p.wait("(() => {const top=document.querySelector('#artifact-reading .footnote-definition').getBoundingClientRect().top; return top>=0 && top<innerHeight})()");
    assert_eq!(p.eval("location.href").unwrap(), route);
    screenshot(&mut p, "out/artifact-reader", "footnote-navigation");
    p.eval("document.querySelector('#artifact-reading pre').focus()")
        .unwrap();
    key(&mut p, "ArrowRight", "ArrowRight", 39, "");
    p.wait("document.activeElement.matches('#artifact-reading pre') && document.activeElement.scrollLeft>0");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn selecting_passages_and_editing_long_drafts_keeps_controls_reachable() {
    let f = installed_fixture();
    let project = "Reading";
    f.init(project).unwrap();
    let body = format!(
        "# A calmer workspace\n\n{}",
        "Several lines of project notes.\n\n".repeat(40)
    );
    let doc = run(f.command(&[
        "artifact",
        "create",
        "--project",
        project,
        "--title",
        "Workspace design notes",
        "--body",
        &body,
        "--json",
    ]))
    .unwrap();
    let (_server, base) = web(&f, project);
    let route = format!(
        "{base}/artifacts#project=named%3AReading&artifact={}",
        doc["artifact"]["id"].as_str().unwrap()
    );
    for (width, height) in [(1440, 1000), (390, 844)] {
        let mut p = Page::open("about:blank");
        p.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
        )
        .unwrap();
        p.command(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-reduced-motion","value":"reduce"}]}),
        )
        .unwrap();
        p.command("Page.navigate", json!({"url":route})).unwrap();
        p.wait("document.querySelector('#artifact-reading h1')?.innerText==='A calmer workspace'");
        let visible = |selector: &str| {
            format!(
                "(()=>{{const el=document.querySelector({});if(!el?.checkVisibility())return false;const r=el.getBoundingClientRect();return r.top>=0 && r.bottom<=innerHeight}})()",
                json!(selector)
            )
        };
        let box_expr = "(()=>{const r=document.querySelector('#artifact-reading').getBoundingClientRect();return [r.x,r.width]})()";
        let before = p.eval(box_expr).unwrap();
        p.eval("const range=document.createRange();range.selectNodeContents(document.querySelector('#artifact-reading h1').firstChild);getSelection().removeAllRanges();getSelection().addRange(range);document.querySelector('#artifact-reading').dispatchEvent(new Event('pointerup'))").unwrap();
        p.wait("document.querySelector('#artifact-selection-comment')?.checkVisibility()");
        assert_eq!(
            p.eval("!!document.querySelector('#artifact-comments')?.checkVisibility()")
                .unwrap(),
            false
        );
        assert_eq!(p.eval(box_expr).unwrap(), before);
        p.eval("document.querySelector('#artifact-selection-comment').click()")
            .unwrap();
        p.wait("document.querySelector('#artifact-quote')?.innerText==='A calmer workspace' && document.activeElement.id==='artifact-comment'");
        if width == 390 {
            p.wait(&visible("#artifact-comment"));
        }
        screenshot(
            &mut p,
            "out/artifact-reader",
            &format!("selected-passage-{width}"),
        );
        p.eval("document.querySelector('#artifact-clear-quote').click();document.querySelector('#artifact-comments-close').click()").unwrap();
        if width == 390 {
            p.wait(&visible(".artifact-document-heading"));
            p.eval("document.querySelector('#artifact-comments-toggle').click()")
                .unwrap();
            p.wait(&visible(".artifact-comments-heading"));
            p.eval("document.querySelector('#artifact-comments-close').click()")
                .unwrap();
            p.wait(&visible(".artifact-document-heading"));
            p.eval("document.querySelector('.artifact-menu summary').click();document.querySelector('#artifact-edit-browser').click()")
                .unwrap();
            p.wait("document.querySelector('#artifact-body')?.checkVisibility()");
            p.eval(&format!("const field=document.querySelector('#artifact-body'); field.value={}; field.dispatchEvent(new Event('input',{{bubbles:true}}))",json!(body.repeat(2)))).unwrap();
            p.wait("field.scrollHeight<=field.clientHeight+2");
            p.eval("scrollTo(0,document.documentElement.scrollHeight)")
                .unwrap();
            p.wait(&visible("#artifact-editor button[type=submit]"));
        }
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn native_edits_refresh_the_reader_without_overwriting_a_browser_draft() {
    let f = installed_fixture();
    let (project, _) = Scenario::NativeArtifactEditor.destination();
    f.init(project).unwrap();
    let info = desktop::seed(&f, Scenario::NativeArtifactEditor, project).unwrap();
    let id = info["id"].as_str().unwrap();
    let (_web, base) = web(&f, project);
    let mut p = Page::open(&format!(
        "{base}/artifacts#project=named%3ANative%20editor%20QA&artifact={id}"
    ));
    p.wait("document.querySelector('#artifact-edit')?.checkVisibility()");
    p.eval("document.querySelector('#artifact-edit').click()")
        .unwrap();
    let launched = "document.querySelector('#artifact-status').textContent==='Opened in native editor · auto-saving'";
    p.wait(launched);
    let count = || {
        serde_json::from_slice::<Value>(&fs::read(f.root.path().join("launches.json")).unwrap())
            .unwrap()["count"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(count(), 1);
    assert_eq!(
        p.eval("document.querySelector('#artifact-editor')===null")
            .unwrap(),
        true
    );
    for event in ["keyDown", "keyUp"] {
        p.command(
            "Input.dispatchKeyEvent",
            json!({"type":event,"key":"e","code":"KeyE","windowsVirtualKeyCode":69,"modifiers":4}),
        )
        .unwrap();
    }
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while count() != 2 {
        assert!(
            std::time::Instant::now() < end,
            "shortcut did not open native editor"
        );
        thread::sleep(Duration::from_millis(30));
    }
    let edit = |version: &str, body: &str| {
        run(f.command(&[
            "artifact",
            "edit",
            id,
            "--project",
            project,
            "--if-version",
            version,
            "--body",
            body,
            "--json",
        ]))
        .unwrap()
    };
    edit("1", "Native save visible in reader.");
    p.wait("document.querySelector('#artifact-reading').innerText.trim()==='Native save visible in reader.'");
    p.eval("document.querySelector('.artifact-menu summary').click();document.querySelector('#artifact-edit-browser').click();const field=document.querySelector('#artifact-body');field.value='Unsent browser draft';field.dispatchEvent(new Event('input',{bubbles:true}))").unwrap();
    edit("2", "Newer native edit.");
    p.eval("dispatchEvent(new Event('focus')); new Promise(resolve=>setTimeout(resolve,1700))")
        .unwrap();
    assert_eq!(p.eval("field.isConnected && field.value==='Unsent browser draft' && !document.querySelector('#artifact-editor-native')").unwrap(),true);
    assert_eq!(count(), 2);
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn reply_drafts_survive_resolution_and_retry_an_unacknowledged_save_once() {
    let f = installed_fixture();
    let project = "Reading";
    f.init(project).unwrap();
    let created = run(f.command(&[
        "artifact",
        "create",
        "--project",
        project,
        "--title",
        "Decisions",
        "--body",
        "A discussion worth keeping.",
        "--json",
    ]))
    .unwrap();
    let id = created["artifact"]["id"].as_str().unwrap();
    run(f.command(&[
        "artifact",
        "comment",
        id,
        "--project",
        project,
        "--body",
        "Can we review this decision?",
        "--json",
    ]))
    .unwrap();
    let (_web, base) = web(&f, project);
    let mut p = Page::open(&format!(
        "{base}/artifacts#project=named%3AReading&artifact={id}"
    ));
    fn click(p: &mut Page, selector: &str) {
        p.wait(&format!(
            "document.querySelector({})?.checkVisibility()",
            json!(selector)
        ));
        p.eval(&format!(
            "document.querySelector({}).click()",
            json!(selector)
        ))
        .unwrap();
    }
    fn type_reply(p: &mut Page, text: &str) {
        p.eval(&format!("(()=>{{const f=document.querySelector('[data-reply] textarea');f.value={};f.dispatchEvent(new Event('input',{{bubbles:true}}))}})()",json!(text))).unwrap();
    }
    click(&mut p, "#artifact-comments-toggle");
    click(&mut p, ".artifact-reply summary");
    type_reply(&mut p, "My unfinished reply");
    click(&mut p, "[data-resolve]");
    for reload in [false, true] {
        if reload {
            p.command("Page.reload", json!({})).unwrap();
            p.wait("document.querySelector('#artifact-comments-toggle')?.getAttribute('aria-expanded')==='false'");
            click(&mut p, "#artifact-comments-toggle");
        }
        click(&mut p, "details.artifact-thread > summary");
        click(&mut p, ".artifact-reply summary");
        assert_eq!(
            p.eval("document.querySelector('[data-reply] textarea').value")
                .unwrap(),
            "My unfinished reply"
        );
    }
    click(&mut p, "[data-reply] button");
    p.wait("document.querySelector('#artifact-status').innerText==='Reply saved'");
    click(&mut p, "details.artifact-thread > summary");
    assert_eq!(p.eval("document.querySelector('.artifact-thread .reply').innerText.includes('My unfinished reply')").unwrap(),true);
    click(&mut p, "[data-resolve]");
    click(&mut p, "section.artifact-thread .artifact-reply summary");
    assert_eq!(
        p.eval("document.querySelector('[data-reply] textarea').value")
            .unwrap(),
        ""
    );
    type_reply(&mut p, "Reply after an interrupted acknowledgment");
    p.eval("const send=window.fetch.bind(window);window.fetch=async (url,opts)=>{const response=await send(url,opts);if(url==='/api/action'&&response.ok&&JSON.parse(opts.body)?.operation?.operation?.parent){window.fetch=send;throw new TypeError('lost acknowledgment')}return response}").unwrap();
    click(&mut p, "[data-reply] button");
    p.wait("document.querySelector('#artifact-status').innerText==='Reply pending · retry after reconnecting'");
    p.command("Page.reload", json!({})).unwrap();
    p.wait("document.querySelector('#artifact-comments-toggle')?.getAttribute('aria-expanded')==='false'");
    click(&mut p, "#artifact-comments-toggle");
    click(&mut p, ".artifact-reply summary");
    assert_eq!(
        p.eval("document.querySelector('[data-reply] textarea').readOnly")
            .unwrap(),
        true
    );
    click(&mut p, "[data-reply] button");
    p.wait("document.querySelector('#artifact-status').innerText==='Reply saved'");
    let saved = run(f.command(&["artifact", "view", id, "--project", project, "--json"])).unwrap();
    assert_eq!(
        saved["comments"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["body"] == "Reply after an interrupted acknowledgment")
            .count(),
        1
    );
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn external_links_preserve_the_document_and_isolate_the_opened_tab() {
    let f = installed_fixture();
    f.init("Reading").unwrap();
    let (_web, base) = web(&f, "Reading");
    let destination = tiny_http::Server::http(("127.0.0.1", 0)).unwrap();
    let external = format!("http://{}", destination.server_addr());
    let body=format!("# Reference notes\n\n[Source]({external}/source) · <https://example.org/automatic> · https://example.org/bare\n\n[Protocol relative](//example.org/relative) · [Local library](/artifacts) · [Same site]({base}/artifacts) · [Email](mailto:hello@example.org)\n\nA footnote.[^note]\n\n[^note]: Keep the document open.");
    let client = reqwest::blocking::Client::new();
    let boot: Value = client
        .get(format!("{base}/api/bootstrap"))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let action = |operation: Value| {
        let response:Value=client.post(format!("{base}/api/action")).header("X-Hey-Boss-CSRF",boot["csrf"].as_str().unwrap()).json(&json!({"project":"named:Reading","request_id":uuid::Uuid::new_v4().to_string(),"operation":{"action":"artifact","operation":operation}})).send().unwrap().json().unwrap();
        assert_eq!(response["ok"], true, "{response}");
        response
    };
    let doc = action(json!({"command":"create","title":"References","body":body}));
    let id = doc["artifact"]["id"].as_str().unwrap();
    run(f.command(&[
        "artifact",
        "comment",
        id,
        "--project",
        "Reading",
        "--body",
        "[Discussion source](https://example.org/discussion)",
        "--json",
    ]))
    .unwrap();
    let url = format!("{base}/artifacts#project=named%3AReading&artifact={id}");
    let mut p = Page::open(&url);
    let audit = |p: &mut Page, root: &str| {
        let links=p.eval(&format!("Array.from(document.querySelector({}).querySelectorAll('a'),a=>({{href:a.href,external:['http:','https:'].includes(a.protocol)&&a.origin!==location.origin,blank:a.target==='_blank',safe:a.relList.contains('noopener')&&a.relList.contains('noreferrer')}}))",json!(root))).unwrap();
        let links = links.as_array().unwrap();
        assert!(links.iter().any(|a| a["external"] == true));
        for a in links {
            assert_eq!(a["external"], a["blank"], "{a}");
            if a["external"] == true {
                assert_eq!(a["safe"], true, "{a}");
            }
        }
    };
    let popup = |p: &mut Page, keyboard: bool| {
        let point=p.eval("(()=>{const a=document.querySelector('#artifact-reading a[href$=\"/source\"]');a.scrollIntoView({block:'center'});a.focus();const r=a.getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2,a.href]})()").unwrap();
        assert_eq!(point[2], format!("{external}/source"));
        if keyboard {
            key(p, "Enter", "Enter", 13, "\r")
        } else {
            for event in ["mousePressed", "mouseReleased"] {
                p.command(
                    "Input.dispatchMouseEvent",
                    json!({"type":event,"x":point[0],"y":point[1],"button":"left","clickCount":1}),
                )
                .unwrap();
            }
        }
        let request = destination
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("external tab did not load");
        assert_eq!(request.url(), "/source");
        assert!(!request.headers().iter().any(|h| h.field.equiv("Referer")));
        request.respond(tiny_http::Response::from_string("<link rel='icon' href='data:,'><script>document.title=window.opener===null?'Isolated source':'Has opener'</script>").with_header(tiny_http::Header::from_bytes("Content-Type","text/html").unwrap())).unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(5);
        let target = loop {
            let targets = p.command("Target.getTargets", json!({})).unwrap();
            if let Some(t) = targets["targetInfos"].as_array().unwrap().iter().find(|t| {
                t["url"] == format!("{external}/source") && t["title"] == "Isolated source"
            }) {
                break t["targetId"].clone();
            }
            assert!(
                std::time::Instant::now() < end,
                "Popup was not isolated: {targets}"
            );
            thread::sleep(Duration::from_millis(30));
        };
        p.command("Target.closeTarget", json!({"targetId":target}))
            .unwrap();
        assert_eq!(p.eval("location.href").unwrap(), url);
    };
    for theme in ["light", "dark"] {
        p.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        for width in [1440, 390, 320] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            p.wait("!!document.querySelector('#artifact-reading .footnote-reference a')");
            audit(&mut p, "#artifact-reading");
            if theme == "light" && width == 1440 {
                popup(&mut p, false);
            }
            p.eval("document.querySelector('#artifact-reading .footnote-reference a').click();if(document.querySelector('#artifact-comments').hidden)document.querySelector('#artifact-comments-toggle').click()").unwrap();
            assert_eq!(p.eval("location.href").unwrap(), url);
            audit(&mut p, "#artifact-threads");
            assert_eq!(
                p.eval("document.documentElement.scrollWidth<=innerWidth+1")
                    .unwrap(),
                true
            );
            screenshot(
                &mut p,
                "out/artifact-reader",
                &format!("external-{theme}-{width}"),
            );
            p.eval("document.querySelector('.artifact-menu summary').click();document.querySelector('#artifact-edit-browser').click();document.querySelector('#artifact-preview').click()").unwrap();
            p.wait("!!document.querySelector('#artifact-edit-preview .footnote-reference a')");
            audit(&mut p, "#artifact-edit-preview");
            p.eval("document.querySelector('#artifact-cancel').click()")
                .unwrap();
        }
    }
    let large = body
        + "\n\n"
        + &"Paragraph with [another source](https://example.org/large).\n\n".repeat(900);
    action(json!({"command":"edit","id":id,"if_version":1,"body":large}));
    p.command("Page.reload", json!({})).unwrap();
    p.wait("document.querySelectorAll('#artifact-reading a[href=\"https://example.org/large\"]').length===900 && !document.querySelector('#artifact-reading').hasAttribute('aria-busy')");
    audit(&mut p, "#artifact-reading");
    popup(&mut p, true);
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn quoted_drafts_survive_reload_and_publish_with_mouse_keyboard_and_touch() {
    let f = installed_fixture();
    f.init("Reading").unwrap();
    let body = (1..=32)
        .map(|i| {
            format!("## Passage {i}\n\nKeep the original context when reviewing this document.\n\n")
        })
        .collect::<String>();
    let doc = run(f.command(&[
        "artifact",
        "create",
        "--project",
        "Reading",
        "--title",
        "Review notes",
        "--body",
        &body,
        "--json",
    ]))
    .unwrap();
    let id = doc["artifact"]["id"].as_str().unwrap();
    let (_web, base) = web(&f, "Reading");
    let route = format!("{base}/artifacts#project=named%3AReading&artifact={id}");
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"window.el=s=>document.querySelector(s);window.passage=()=>document.querySelectorAll('#artifact-reading h2')[20];window.readerBox=()=>{const r=el('#artifact-reading').getBoundingClientRect();return [r.x,r.width]};addEventListener('error',e=>window.name=e.message);addEventListener('unhandledrejection',e=>window.name=String(e.reason))"})).unwrap();
    let visible="el('#artifact-quote')?.innerText==='Passage' && document.activeElement===el('#artifact-comment') && (()=>{const r=document.activeElement.getBoundingClientRect();return r.top>=0 && r.bottom<=innerHeight && document.documentElement.scrollWidth<=innerWidth+1})()";
    let select = |p: &mut Page, touch: bool| {
        p.wait("!!passage()");
        let point=p.eval("(()=>{passage().scrollIntoView({block:'center'});const r=passage().getBoundingClientRect();return [r.x+32,r.y+r.height/2]})()").unwrap();
        if touch {
            p.eval("const range=document.createRange();range.setStart(passage().firstChild,0);range.setEnd(passage().firstChild,7);getSelection().removeAllRanges();getSelection().addRange(range)").unwrap();
        } else {
            for clicks in [1, 2] {
                for event in ["mousePressed", "mouseReleased"] {
                    p.command("Input.dispatchMouseEvent",json!({"type":event,"x":point[0],"y":point[1],"button":"left","clickCount":clicks})).unwrap();
                }
            }
        }
        p.wait("el('#artifact-selection-comment')?.checkVisibility()");
    };
    let tap = |p: &mut Page, selector: &str| {
        let xy=p.eval(&format!("(()=>{{const r=el({}).getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2]}})()",json!(selector))).unwrap();
        p.command(
            "Input.dispatchTouchEvent",
            json!({"type":"touchStart","touchPoints":[{"x":xy[0],"y":xy[1]}]}),
        )
        .unwrap();
        p.command(
            "Input.dispatchTouchEvent",
            json!({"type":"touchEnd","touchPoints":[]}),
        )
        .unwrap();
    };
    for theme in ["light", "dark"] {
        p.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        for (width, height, touch) in [
            (1440, 1000, false),
            (820, 1180, false),
            (390, 844, false),
            (320, 568, false),
            (390, 844, true),
        ] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":touch}),
            )
            .unwrap();
            p.command(
                "Emulation.setTouchEmulationEnabled",
                json!({"enabled":touch}),
            )
            .unwrap();
            p.command("Page.navigate", json!({"url":route})).unwrap();
            p.wait("!!passage()");
            p.eval("if(!el('#artifact-comments').hidden)el('#artifact-comments-close').click()")
                .unwrap();
            let before = p.eval("readerBox()").unwrap();
            select(&mut p, touch);
            assert_eq!(p.eval("el('#artifact-comments').hidden").unwrap(), true);
            assert_eq!(p.eval("readerBox()").unwrap(), before);
            if touch {
                tap(&mut p, "#artifact-selection-comment");
            } else if width == 1440 {
                p.eval("el('#artifact-selection-comment').focus()").unwrap();
                key(&mut p, "Enter", "Enter", 13, "\r");
                p.wait(visible);
                p.eval("el('#artifact-clear-quote').click()").unwrap();
                assert_eq!(p.eval("el('#artifact-quote').hidden").unwrap(), true);
                select(&mut p, false);
                p.eval("el('#artifact-selection-comment').focus()").unwrap();
                key(&mut p, " ", "Space", 32, " ");
            } else {
                p.eval("el('#artifact-selection-comment').click()").unwrap();
            }
            p.wait(visible);
            let label = format!("{theme}-{width}-{}", if touch { "touch" } else { "mouse" });
            screenshot(&mut p, "out/artifact-reader", &format!("composer-{label}"));
            p.eval(&format!("el('#artifact-comment').value={};el('#artifact-comment').dispatchEvent(new Event('input',{{bubbles:true}}));window.beforeReload=true",json!(label))).unwrap();
            p.command("Page.reload", json!({})).unwrap();
            p.wait(&format!("!window.beforeReload && el('#artifact-quote')?.innerText==='Passage' && el('#artifact-comment')?.value==={}",json!(label)));
            if touch {
                p.eval("el('#artifact-comment-form button[type=submit]').scrollIntoView({block:'center'})")
                    .unwrap();
                tap(&mut p, "#artifact-comment-form button[type=submit]");
            } else {
                p.eval("el('#artifact-comment-form button[type=submit]').click()")
                    .unwrap();
            }
            p.wait(&format!(
                "el('#artifact-threads')?.textContent.includes({})",
                json!(label)
            ));
            let saved = run(f.command(&["artifact", "view", id, "--project", "Reading", "--json"]))
                .unwrap();
            let c = saved["comments"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["body"] == label)
                .unwrap();
            assert_eq!(c["quote"], "Passage");
            assert_eq!(c["outdated"], false);
            assert_eq!(p.eval("window.name").unwrap(), "");
        }
    }
}
