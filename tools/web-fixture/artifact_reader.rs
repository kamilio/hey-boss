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
