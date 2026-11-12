//! Check the progressively rendered document against the server's complete HTML.
use super::*;

fn document(f: &Fixture, body: &str) -> (String, String) {
    f.init("Reading").unwrap();
    fs::write(f.root.path().join("document.md"), body).unwrap();
    let saved = run(f.command(&[
        "artifact",
        "create",
        "--project",
        "Reading",
        "--title",
        "Large document",
        "--file",
        "document.md",
        "--json",
    ]))
    .unwrap();
    let doc = &saved["artifact"];
    (
        doc["id"].as_str().unwrap().into(),
        doc["body_html"].as_str().unwrap().into(),
    )
}

fn complete(p: &mut Page) {
    p.wait("document.querySelector('#artifact-reading')?.hasAttribute('aria-busy')===false");
}

fn check_html(p: &mut Page, html: &str) {
    let matches=p.eval(&format!("(() => {{ const t=document.createElement('template'); t.innerHTML={}; return document.querySelector('#artifact-reading').innerHTML===t.innerHTML }})()", json!(html))).unwrap();
    assert_eq!(
        matches, true,
        "Progressive rendering changed canonical HTML"
    );
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn maximum_checklist_keeps_every_item_and_frames_responsive() {
    let f = installed_fixture();
    let mut body = String::from("# Maximum checklist\n\n");
    for i in 0..28000 {
        use std::fmt::Write;
        writeln!(body, "- [x] Checklist item {i}.").unwrap();
    }
    let (id, html) = document(&f, &body);
    let (_web, base) = web(&f, "Reading");
    let mut p = Page::open("about:blank");
    p.command(
        "Page.navigate",
        json!({"url":format!("{base}/artifacts#project=named%3AReading")}),
    )
    .unwrap();
    p.wait("!!document.querySelector('.artifact-row-link')");
    p.eval("window.probe={gap:0,count:0,stop:false}; requestAnimationFrame(function tick(t){ if (probe.last){probe.gap=Math.max(probe.gap,t-probe.last);probe.count++} probe.last=t; if(!probe.stop)requestAnimationFrame(tick) });").unwrap();
    p.wait("probe.count>0");
    p.eval("document.querySelector('.artifact-row-link').click()")
        .unwrap();
    complete(&mut p);
    let gap = p
        .eval("probe.stop=true;probe.gap")
        .unwrap()
        .as_f64()
        .unwrap();
    assert!(gap <= 150.0, "Reader stalled for {gap:.1} ms");
    assert!(
        p.eval("location.hash")
            .unwrap()
            .as_str()
            .unwrap()
            .contains(&id)
    );
    assert_eq!(
        p.eval("document.querySelectorAll('#artifact-reading ul > li').length")
            .unwrap(),
        28000
    );
    check_html(&mut p, &html);
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn large_documents_preserve_lists_code_footnotes_and_selection_comments() {
    for short_lists in [false, true] {
        let mut body = String::from("# Long reading\n\n");
        use std::fmt::Write;
        if short_lists {
            for i in 0..1500 {
                writeln!(body,"## Section {i}\n\nContext for decision {i}.\n\n- First choice\n- Second choice\n- Keep the useful context\n").unwrap();
            }
        } else {
            for i in 0..2000 {
                writeln!(body, "{}. Checklist item {i}", i + 3).unwrap();
            }
            body.push_str("\n| Item | Decision |\n| --- | --- |\n");
            for i in 0..1000 {
                writeln!(body, "| Row {i} | Keep **context** |").unwrap();
            }
        }
        body.push_str("\nA final **selected passage** with a source.[^end]\n\n```text\nliteral <script>\n```\n\n[^end]: Final source explanation.\n");
        let f = installed_fixture();
        let (id, html) = document(&f, &body);
        let (_web, base) = web(&f, "Reading");
        let route = format!("{base}/artifacts#project=named%3AReading&artifact={id}");
        for &width in if short_lists {
            &[390, 1440][..]
        } else {
            &[390][..]
        } {
            let mut p = Page::open("about:blank");
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            p.command("Page.navigate", json!({"url":route})).unwrap();
            complete(&mut p);
            check_html(&mut p, &html);
            if short_lists {
                assert_eq!(
                    p.eval("document.querySelectorAll('#artifact-reading ul').length")
                        .unwrap(),
                    1500
                );
            } else {
                assert_eq!(p.eval("const r=document.querySelector('#artifact-reading'); [r.querySelectorAll('ol > li').length,r.querySelector('ol').start,r.querySelectorAll('tbody tr').length]").unwrap(),json!([2000,3,1000]));
            }
            p.eval("window.passage=Array.from(document.querySelectorAll('#artifact-reading strong')).find(el=>el.textContent==='selected passage'); passage.scrollIntoView({block:'center'})").unwrap();
            p.wait("passage.getBoundingClientRect().top>=0 && passage.getBoundingClientRect().bottom<=innerHeight");
            p.eval("document.querySelector('#artifact-reading .footnote-reference a').click()")
                .unwrap();
            p.wait("(()=>{const r=document.querySelector('.footnote-definition').getBoundingClientRect();return r.top>=0 && r.top<innerHeight})()");
            assert_eq!(p.eval("location.href").unwrap(), route);
            p.eval(
                "document.querySelector('#artifact-reading pre').scrollIntoView({block:'center'})",
            )
            .unwrap();
            p.wait("document.querySelector('#artifact-reading pre code').innerText==='literal <script>\\n'");
            p.eval("passage.scrollIntoView({block:'center'});const range=document.createRange();range.selectNodeContents(passage.firstChild);getSelection().removeAllRanges();getSelection().addRange(range);document.querySelector('#artifact-reading').dispatchEvent(new Event('pointerup'))").unwrap();
            p.wait("document.querySelector('#artifact-selection-comment').checkVisibility()");
            p.eval("document.querySelector('#artifact-selection-comment').click()")
                .unwrap();
            let comment = format!("Reachable selection {width}");
            p.eval(&format!("document.querySelector('#artifact-comment').value={};document.querySelector('#artifact-comment-form').requestSubmit()",json!(comment))).unwrap();
            p.wait(&format!(
                "document.querySelector('#artifact-threads').innerText.includes({})",
                json!(comment)
            ));
            assert_eq!(
                p.eval("document.querySelector('#artifact-threads blockquote').innerText")
                    .unwrap(),
                "selected passage"
            );
            assert_eq!(p.eval("document.querySelector('#artifact-threads').innerText.includes('Outdated selection') || !passage.isConnected || document.querySelector('#artifact-reading').hasAttribute('aria-busy')").unwrap(),false);
            if short_lists {
                screenshot(
                    &mut p,
                    "out/artifact-reader",
                    &format!("short-list-end-{width}"),
                );
            }
        }
    }
}
