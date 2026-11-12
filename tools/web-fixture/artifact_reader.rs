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
