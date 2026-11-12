//! Capture the shipped library, reader and editor in each supported layout.
use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn artifact_library_reader_comments_and_editor_fit_each_screen() {
    let f = installed_fixture();
    let project = "Artifact Studio";
    f.init(project).unwrap();
    let body = "# A calmer workspace\n\nKeep the important things close. Give everything else room to breathe.\n\n## What we’re building\n\nA simple home for **project documents**, decisions, and conversations.\n\n- [x] A clear reading surface\n- [ ] Review the mobile experience\n\n> Good tools make the next step obvious.";
    let create = |title: &str, body: &str| {
        run(f.command(&[
            "artifact",
            "create",
            "--project",
            project,
            "--title",
            title,
            "--body",
            body,
            "--json",
        ]))
        .unwrap()["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let id = create("Workspace design notes", body);
    create(
        "Decisions from review",
        "Put decisions in writing so teammates can find them later.",
    );
    run(f.command(&[
        "artifact",
        "comment",
        &id,
        "--project",
        project,
        "--body",
        "The mobile reading view is ready for review.",
        "--json",
    ]))
    .unwrap();
    let (_web, base) = web(&f, project);
    let library = format!("{base}/artifacts#project=named%3AArtifact+Studio");
    let mut page = Page::open("about:blank");
    page.command("Page.enable", json!({})).unwrap();
    page.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"window.problems=[];addEventListener('error',e=>problems.push(e.message));addEventListener('unhandledrejection',e=>problems.push(String(e.reason)))"})).unwrap();
    for theme in ["light", "dark"] {
        page.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        for (width, height) in [(1440, 1000), (820, 1180), (390, 844), (320, 740)] {
            page.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            page.command("Page.navigate", json!({"url":library}))
                .unwrap();
            page.wait("document.querySelectorAll('.artifact-row-link').length===2");
            let shot = |page: &mut Page, state: &str| {
                let name = format!("{theme}-{width}-{state}");
                assert_eq!(
                    page.eval("document.documentElement.scrollWidth<=innerWidth+1")
                        .unwrap(),
                    true,
                    "horizontal overflow in {name}"
                );
                assert_eq!(
                    page.eval("problems").unwrap(),
                    json!([]),
                    "script errors in {name}"
                );
                let size = page.command("Page.getLayoutMetrics", json!({})).unwrap();
                capture(
                    page,
                    "out/artifact-layout",
                    &name,
                    json!({"format":"png","captureBeyondViewport":true,"clip":{"x":0,"y":0,"width":size["cssContentSize"]["width"],"height":size["cssContentSize"]["height"],"scale":1}}),
                );
            };
            shot(&mut page, "library");
            page.eval("[...document.querySelectorAll('.artifact-row-link')].find(a=>a.innerText.includes('Workspace design notes')).click()").unwrap();
            page.wait("document.querySelector('#artifact-reading strong')?.innerText==='project documents'");
            shot(&mut page, "reader");
            assert_eq!(page.eval("!document.querySelector('#artifact-heading').checkVisibility() && ![...document.querySelectorAll('button')].some(b=>b.checkVisibility() && b.textContent.trim()==='Archive')").unwrap(),true);
            page.eval("document.querySelector('.artifact-menu summary').focus()")
                .unwrap();
            key(&mut page, "Enter", "Enter", 13, "\r");
            page.wait("document.querySelector('#artifact-export').checkVisibility()");
            key(&mut page, "Escape", "Escape", 27, "");
            page.wait("!document.querySelector('#artifact-export').checkVisibility()");

            page.eval("document.querySelector('#artifact-comments-toggle').click()")
                .unwrap();
            page.wait("document.querySelector('#artifact-comments').checkVisibility()");
            shot(&mut page, "comments");
            assert_eq!(
                page.eval("document.querySelector('[data-reply] textarea').checkVisibility()")
                    .unwrap(),
                false
            );
            page.eval("document.querySelector('.artifact-reply summary').click()")
                .unwrap();
            page.wait("document.querySelector('[data-reply] textarea').checkVisibility()");

            page.eval("document.querySelector('.artifact-menu summary').click();document.querySelector('#artifact-edit-browser').click();scrollTo(0,0)").unwrap();
            page.wait("document.querySelector('#artifact-body')?.checkVisibility()");
            shot(&mut page, "editor");
            if width == 390 {
                assert_eq!(page.eval("(()=>{const r=document.querySelector('#nav-artifacts').getBoundingClientRect();return r.left>=0 && r.right<=innerWidth && parseFloat(getComputedStyle(document.querySelector('#artifact-body')).fontSize)>=16})()").unwrap(),true);
            }
            page.eval(r"var field=document.querySelector('#artifact-body');field.value='# Preview check\n\n**Readable** draft.';field.dispatchEvent(new Event('input',{bubbles:true}))").unwrap();

            page.eval("document.querySelector('#artifact-preview').click()")
                .unwrap();
            page.wait("document.querySelector('#artifact-edit-preview strong')?.checkVisibility()");
            shot(&mut page, "preview");
            assert_eq!(page.eval("!document.querySelector('#artifact-body').checkVisibility() && document.querySelector('#artifact-edit-preview strong').innerText==='Readable'").unwrap(),true);
            page.eval("document.querySelector('#artifact-preview').click()")
                .unwrap();
            assert_eq!(
                page.eval("document.querySelector('#artifact-body').value")
                    .unwrap(),
                "# Preview check\n\n**Readable** draft."
            );

            page.eval("document.querySelector('#artifact-cancel').click()")
                .unwrap();
        }
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn library_cards_preserve_excerpts_across_search_archive_and_pagination() {
    let f = installed_fixture();
    let project = "Preview Studio";
    f.init(project).unwrap();
    let create = |title: &str, body: &str| {
        run(f.command(&[
            "artifact",
            "create",
            "--project",
            project,
            "--title",
            title,
            "--body",
            body,
            "--json",
        ]))
        .unwrap()
    };
    let main = create(
        "A calmer workspace",
        "# Decisions\n\nBring the work that matters into focus.",
    );
    for (title, body) in [
        ("Release notes", "What shipped yesterday."),
        ("Untitled ideas", ""),
        ("Markup", "An inline `<img>` stays text."),
        (
            "Reading list",
            "[References](https://example.org) to revisit.",
        ),
        ("Long preview", &"Read the important context. ".repeat(22)),
    ] {
        create(title, body);
    }
    let (_web, base) = web(&f, project);
    let library = format!("{base}/artifacts#project=named%3APreview+Studio");
    let mut p = Page::open("about:blank");
    p.command("Page.navigate", json!({"url":library})).unwrap();
    p.wait("document.querySelectorAll('.artifact-row').length===6");
    p.eval("window.el=s=>document.querySelector(s);window.rows=()=>[...document.querySelectorAll('.artifact-row')];window.search=q=>{el('#artifact-search').value=q;el('#artifact-search').dispatchEvent(new Event('input',{bubbles:true}))}").unwrap();
    assert_eq!(p.eval("rows().filter(r=>r.querySelector('.artifact-row-preview').innerText.includes('Bring the work that matters')).length===1 && !el('.artifact-row-preview img') && document.querySelectorAll('.artifact-row-preview.is-empty').length===1").unwrap(),true);
    for theme in ["light", "dark"] {
        p.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        for (width, height) in [(1440, 1100), (820, 1180), (390, 844), (320, 740)] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            assert_eq!(p.eval("document.documentElement.scrollWidth<=innerWidth+1 && rows().every(r=>{const x=r.querySelector('.artifact-row-preview');return x.clientHeight<=3*parseFloat(getComputedStyle(x).lineHeight)+1 && [...r.querySelectorAll('.artifact-row-actions button')].every(b=>{const box=b.getBoundingClientRect();return box.width>=44 && box.height>=44 && box.right<=innerWidth && box.left>=0})})").unwrap(),true,"card geometry {theme} {width}");
            let size = p.command("Page.getLayoutMetrics", json!({})).unwrap();
            capture(
                &mut p,
                "out/artifact-cards",
                &format!("{theme}-{width}"),
                json!({"format":"png","captureBeyondViewport":true,"clip":{"x":0,"y":0,"width":size["cssContentSize"]["width"],"height":size["cssContentSize"]["height"],"scale":1}}),
            );
            if width == 320 {
                p.eval("rows().at(-1).scrollIntoView({block:'center'})")
                    .unwrap();
                p.wait("(()=>{const r=rows().at(-1).getBoundingClientRect();return r.top>=0 && r.bottom<=innerHeight})()");
                screenshot(
                    &mut p,
                    "out/artifact-cards",
                    &format!("{theme}-{width}-scrolled"),
                );
                p.eval("scrollTo(0,0)").unwrap();
            }
        }
    }
    p.command(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":1440,"height":1100,"deviceScaleFactor":1,"mobile":false}),
    )
    .unwrap();
    p.command("Page.bringToFront", json!({})).unwrap();
    key(&mut p, "Tab", "Tab", 9, "");
    p.eval("el('.artifact-row-link').focus()").unwrap();
    assert_eq!(p.eval("document.activeElement===el('.artifact-row-link') && document.activeElement.matches(':focus-visible')").unwrap(),true);
    key(&mut p, "Enter", "Enter", 13, "\r");
    p.wait("el('#artifact-reading')?.checkVisibility()");
    p.eval("[...document.links].find(a=>a.innerText.trim()==='All artifacts' && a.checkVisibility()).click()").unwrap();
    p.wait("rows().length===6");
    p.eval("search('yesterday')").unwrap();
    p.wait("rows().length===1 && el('.artifact-row-preview').innerText.includes('yesterday')");
    p.eval("search('no-matching-document')").unwrap();
    p.wait("el('.artifact-empty h2')?.textContent==='No matching artifacts' && el('#artifact-empty-action').innerText==='Clear search'");
    p.eval("el('#artifact-empty-action').click()").unwrap();
    p.wait("rows().length===6");
    let id = main["artifact"]["id"].as_str().unwrap();
    let archive = format!("document.querySelector('[data-archive=\"{id}\"]').click()");
    p.eval(&archive).unwrap();
    p.wait("rows().length===5");
    p.eval("el('#artifact-archived').click()").unwrap();
    p.wait("rows().length===1 && el('.artifact-row-preview').innerText.includes('Bring the work')");
    p.eval(&archive).unwrap();
    p.wait("el('.artifact-empty h2')?.textContent==='No archived artifacts'");
    p.eval("el('#artifact-empty-action').click()").unwrap();
    p.wait("rows().length===6");
    p.eval("rows().find(r=>r.innerText.includes('Untitled ideas')).querySelector('[data-delete]').click()").unwrap();
    p.wait("el('dialog[open] [data-cancel]')?.checkVisibility()");
    p.eval("el('dialog[open] [data-cancel]').click()").unwrap();
    p.wait("!el('dialog[open]') && rows().length===6");
    // Use the real list endpoint and its cursor, not a fabricated second page.
    for i in 0..45 {
        create(&format!("Page item {i}"), &format!("Bounded preview {i}"));
    }
    p.command("Page.reload", json!({})).unwrap();
    p.wait("document.querySelectorAll('.artifact-row').length===50 && document.querySelector('#artifact-more')?.checkVisibility()");
    let first = p
        .eval("document.querySelector('.artifact-row-link').href")
        .unwrap();
    p.eval("document.querySelector('#artifact-more').click()")
        .unwrap();
    p.wait("document.querySelectorAll('.artifact-row-preview').length===51 && document.querySelector('#artifact-more').hidden");
    assert_eq!(
        p.eval("new Set([...document.querySelectorAll('.artifact-row-link')].map(a=>a.href)).size")
            .unwrap(),
        51
    );
    assert_eq!(
        p.eval("document.querySelector('.artifact-row-link').href")
            .unwrap(),
        first
    );
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn empty_loading_and_failed_library_views_recover_and_rotation_keeps_the_draft() {
    let f = installed_fixture();
    f.init("Projects").unwrap();
    f.init("Empty").unwrap();
    let saved = run(f.command(&[
        "artifact",
        "create",
        "--project",
        "Projects",
        "--title",
        "Notes",
        "--body",
        "Keep these decisions.",
        "--json",
    ]))
    .unwrap();
    let (_web, base) = web(&f, "Projects");
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"window.el=s=>document.querySelector(s);if(location.search==='?hold'){const send=window.fetch.bind(window);const gate=new Promise(done=>window.releaseList=done);window.fetch=async (url,opts)=>{if(url==='/api/action' && JSON.parse(opts.body)?.operation?.operation?.command==='list'){await gate;window.fetch=send}return send(url,opts)}}"})).unwrap();
    let resize = |p: &mut Page, width: u64, height: u64| {
        p.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
        )
        .unwrap();
    };
    let visit = |p: &mut Page, suffix: &str| {
        p.command(
            "Page.navigate",
            json!({"url":format!("{base}/artifacts{suffix}")}),
        )
        .unwrap();
    };
    for theme in ["light", "dark"] {
        resize(&mut p, 390, 844);
        p.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        let shot = |p: &mut Page, name: &str| {
            assert_eq!(
                p.eval("document.documentElement.scrollWidth<=innerWidth+1")
                    .unwrap(),
                true,
                "{name}"
            );
            screenshot(p, "out/artifact-layout", &format!("{theme}-{name}"));
        };
        visit(&mut p, "#project=named%3AEmpty");
        p.wait("el('.artifact-empty h2')?.innerText==='Your documents start here'");
        shot(&mut p, "empty-library");
        visit(&mut p, "?hold#project=named%3AProjects");
        p.wait("el('.artifact-loading')?.checkVisibility() && typeof releaseList==='function'");
        shot(&mut p, "library-loading");
        p.eval("releaseList()").unwrap();
        p.wait("!!el('.artifact-row-link')");
        p.eval("el('#artifact-search').value='no-matching-doc';el('#artifact-search').dispatchEvent(new Event('input',{bubbles:true}))").unwrap();
        p.wait("el('.artifact-empty h2')?.innerText==='No matching artifacts'");
        shot(&mut p, "no-results");
        p.eval("el('#artifact-empty-action').click()").unwrap();
        p.wait("!!el('.artifact-row-link')");
        visit(
            &mut p,
            "#project=named%3AProjects&artifact=missing-document",
        );
        p.wait("el('#artifact-error')?.checkVisibility()");
        shot(&mut p, "document-error");
        visit(
            &mut p,
            &format!(
                "#project=named%3AProjects&artifact={}",
                saved["artifact"]["id"].as_str().unwrap()
            ),
        );
        p.wait("el('#artifact-reading')?.innerText.trim()==='Keep these decisions.'");
        p.eval("el('.artifact-menu summary').focus()").unwrap();
        key(&mut p, "Enter", "Enter", 13, "\r");
        p.wait("el('#artifact-export').checkVisibility()");
        shot(&mut p, "keyboard-actions");
        key(&mut p, "Escape", "Escape", 27, "");
        p.wait("!el('.artifact-menu').open");
        p.eval("el('.artifact-menu summary').click();el('#artifact-edit-browser').click();el('#artifact-body').value='# Rotating a draft\\n\\nKeep this text exactly.';el('#artifact-body').dispatchEvent(new Event('input',{bubbles:true}))").unwrap();
        for (name, width, height) in [
            ("landscape", 844, 390),
            ("zoom-equivalent", 720, 450),
            ("portrait", 390, 844),
        ] {
            resize(&mut p, width, height);
            assert_eq!(
                p.eval("el('#artifact-body').value").unwrap(),
                "# Rotating a draft\n\nKeep this text exactly."
            );
            shot(&mut p, name);
        }
        p.eval("el('#artifact-cancel').click()").unwrap();
    }
}
