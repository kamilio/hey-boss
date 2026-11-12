//! Set HEY_BOSS_TEST_BINARY and optionally HEY_BOSS_EXPECTED_BUILD to check an installation.
use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn assignment_cards_open_the_exact_saved_trace_with_keyboard_or_pointer() {
    let f = installed_fixture();
    f.init("Assignment QA").unwrap();
    fleet::assigned_agent(&f, "Assignment QA").unwrap();
    let (_web, base) = web(&f, "Assignment QA");
    let list = format!("{base}/#project=named%3AAssignment+QA");
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error',e=>window.name=e.message);addEventListener('unhandledrejection',e=>window.name=String(e.reason))"})).unwrap();
    let key = |p: &mut Page, key: &str, vk: u8| {
        for kind in ["keyDown", "keyUp"] {
            p.command("Input.dispatchKeyEvent",json!({"type":kind,"key":key,"code":key,"windowsVirtualKeyCode":vk,"text":if key=="Enter" && kind=="keyDown" {"\r"} else {""}})).unwrap();
        }
    };
    let card = |p: &mut Page, n| {
        p.eval(&format!(
            "document.querySelector('[data-assignment-card=assignment-card-{n}]').focus()"
        ))
        .unwrap();
        key(p, "ArrowDown", 40);
        p.wait(&format!(
            "document.querySelector('#assignment-card-{n}').matches(':popover-open')"
        ));
    };
    for (width, height) in [(1440, 1000), (390, 844)] {
        for theme in ["light", "dark"] {
            p.command(
                "Emulation.setEmulatedMedia",
                json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
            )
            .unwrap();
            let size = |p: &mut Page, h| {
                p.command(
                    "Emulation.setDeviceMetricsOverride",
                    json!({"width":width,"height":h,"deviceScaleFactor":1,"mobile":false}),
                )
                .unwrap();
            };
            size(&mut p, height);
            p.command("Page.navigate", json!({"url":list})).unwrap();
            p.wait("!!document.querySelector('[data-assignment-card=assignment-card-1]')");
            assert_eq!(p.eval("document.querySelectorAll('#assignment-card-2 .assignment-card-primary,#assignment-card-3 .assignment-card-primary').length").unwrap(),0);
            card(&mut p, 1);
            p.eval("window.trace=document.querySelector('#assignment-card-1 .assignment-card-primary');trace.focus()").unwrap();
            p.wait("(()=>{const r=trace.getBoundingClientRect();return trace.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)) && document.documentElement.scrollWidth<=innerWidth})()");
            if width == 390 {
                p.wait("trace.getBoundingClientRect().width>=44 && trace.getBoundingClientRect().height>=44");
            }
            size(&mut p, height - 1);
            p.wait("document.activeElement===trace");
            screenshot(&mut p, "out/worker-assignment", &format!("{width}-{theme}"));
            key(&mut p, "Enter", 13);
            p.wait("location.hash.includes('run=assigned-run') && location.hash.includes('host=local') && document.body.innerText.includes('The correct assigned session is open.')");
            p.wait("document.documentElement.scrollWidth<=innerWidth");
            let mut clip =
                p.command("Page.getLayoutMetrics", json!({})).unwrap()["cssContentSize"].clone();
            clip["scale"] = 1.into();
            capture(
                &mut p,
                "out/worker-assignment",
                &format!("{width}-{theme}-conversation"),
                json!({"format":"png","captureBeyondViewport":true,"clip":clip}),
            );
            p.eval("history.back()").unwrap();
            p.wait("!!document.querySelector('[data-assignment-card=assignment-card-1]') && model.route.owner==='all'");
        }
    }
    card(&mut p, 1);
    p.eval("document.querySelector('#assignment-card-1 a:not(.assignment-card-primary)').click()")
        .unwrap();
    p.wait("model.route.owner==='codex:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee' && model.issues.length===1");
    p.command("Page.navigate", json!({"url":list})).unwrap();
    p.wait("!!document.querySelector('[data-assignment-card=assignment-card-4]')");
    card(&mut p, 4);
    p.eval("document.querySelector('#assignment-card-4 .assignment-card-primary').click()")
        .unwrap();
    p.wait("document.querySelector('#session-title')?.textContent==='Conversation unavailable' && document.querySelector('#session-status').textContent.includes('no longer has a saved Codex session')");
    p.command("Page.navigate",json!({"url":format!("{base}/agents/session#project=named%3AAssignment+QA&issue=5&agent=codex%3Aaaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")})).unwrap();
    p.wait("location.hash.includes('run=remote-run') && location.hash.includes('host=devbox')");
    assert_eq!(p.eval("window.name").unwrap(), "");
}
