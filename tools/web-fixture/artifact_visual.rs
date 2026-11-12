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
            page.eval("document.querySelector('#artifact-comments-toggle').click()")
                .unwrap();
            page.wait("document.querySelector('#artifact-comments').checkVisibility()");
            shot(&mut page, "comments");
            page.eval("document.querySelector('.artifact-menu summary').click();document.querySelector('#artifact-edit-browser').click();scrollTo(0,0)").unwrap();
            page.wait("document.querySelector('#artifact-body')?.checkVisibility()");
            shot(&mut page, "editor");
            page.eval("document.querySelector('#artifact-preview').click()")
                .unwrap();
            page.wait("document.querySelector('#artifact-edit-preview strong')?.checkVisibility()");
            shot(&mut page, "preview");
            page.eval("document.querySelector('#artifact-cancel').click()")
                .unwrap();
        }
    }
}
