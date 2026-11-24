use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn artifact_publishing_cli_links_and_one_sync_button() {
    let f = installed_fixture();
    f.init("Publishing QA").unwrap();
    let root = f.root.path();
    let script = root.join("publisher.sh");
    fs::write(&script, "#!/bin/sh\nif test -f \"$(dirname \"$0\")/fail\"; then echo 'Notion unavailable' >&2; exit 1; fi\nprintf '{\"id\":\"remote\",\"url\":\"https://example.com/plan\",\"version\":\"1\"}'\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("publishing.yaml"),json!({"notion":{"content":"artifact","auto_sync":false,"audience":"private","command":script,"create":["create","{source}"],"read":["read","{id}"],"update":["update","{source}"],"fields":{"id":"/id","url":"/url","version":"/version"}}}).to_string()).unwrap();
    let created = run(f.command(&[
        "artifact",
        "create",
        "--project",
        "Publishing QA",
        "--title",
        "A plan worth sharing",
        "--body",
        "A focused document with a stable link.",
        "--json",
    ]))
    .unwrap();
    let id = created["artifact"]["id"].as_str().unwrap();
    let (_server, base) = web(&f, "Publishing QA");
    let mut p = Page::open(&format!(
        "{base}/artifacts#project=named%3APublishing+QA&artifact={id}"
    ));
    p.wait("document.querySelector('#artifact-reading')!==null");
    assert_eq!(p.eval("document.querySelector('#artifact-share')===null && document.querySelector('#artifact-sync')===null").unwrap(), true);
    run(f.command(&[
        "artifact",
        "share",
        id,
        "--project",
        "Publishing QA",
        "--provider",
        "notion",
        "--link",
        "https://example.com/plan",
        "--json",
    ]))
    .unwrap();
    p.command("Page.reload", json!({})).unwrap();
    p.wait(
        "document.querySelector('.artifact-publication a')?.textContent.includes('Open in notion')",
    );
    assert_eq!(p.eval("document.querySelector('.artifact-publication').textContent.includes('Manual · Up to date') && document.querySelectorAll('.artifact-publications button').length===1").unwrap(),true);
    run(f.command(&[
        "artifact",
        "edit",
        id,
        "--project",
        "Publishing QA",
        "--body",
        "A new revision.",
        "--json",
    ]))
    .unwrap();
    p.command("Page.reload", json!({})).unwrap();
    p.wait(
        "document.querySelector('.artifact-publication')?.textContent.includes('Changes pending')",
    );
    fs::write(root.join("fail"), "").unwrap();
    p.eval("document.querySelector('#artifact-sync').click()")
        .unwrap();
    p.wait("document.querySelector('.artifact-publication-error')?.textContent.includes('Notion unavailable')");
    assert_eq!(
        p.eval("document.querySelector('.artifact-publication a').getAttribute('href')")
            .unwrap(),
        "https://example.com/plan"
    );
    for width in [1440, 390, 320] {
        p.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":false}),
        )
        .unwrap();
        assert_eq!(
            p.eval("document.documentElement.scrollWidth<=innerWidth+1")
                .unwrap(),
            true
        );
        screenshot(&mut p, ".notion-sync-check", &format!("sync-{width}"));
    }
    fs::remove_file(root.join("fail")).unwrap();
    p.eval("document.querySelector('#artifact-sync').click()")
        .unwrap();
    p.wait("document.querySelector('.artifact-publication').textContent.includes('Up to date') && !document.querySelector('.artifact-publication-error')");
}
