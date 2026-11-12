use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn diagrams_keep_readable_labels_safe_source_and_working_zoom() {
    let f = installed_fixture();
    let (project, _) = Scenario::ArtifactDiagrams.destination();
    f.init(project).unwrap();
    seed::seed(&f, Scenario::ArtifactDiagrams, project).unwrap();
    let (_server, base) = web(&f, project);
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"window.problems=[];addEventListener('error',e=>problems.push(e.message));document.addEventListener('securitypolicyviolation',e=>problems.push(e.violatedDirective));window.el=s=>document.querySelector(s);window.fig=i=>document.querySelectorAll('.artifact-diagram')[i]"})).unwrap();
    p.command(
        "Page.navigate",
        json!({"url":format!("{base}/artifacts#project=named%3AFlowchart+QA")}),
    )
    .unwrap();
    p.wait("!!el('.artifact-row-link')");
    p.eval("el('.artifact-row-link').click()").unwrap();
    p.wait("fig(0)?.dataset.state==='ready'");
    assert_eq!(p.eval("[document.querySelectorAll('.artifact-diagram').length,document.querySelectorAll('pre > code.language-js').length]").unwrap(),json!([4,1]));
    assert_eq!(p.eval("(()=>{const s=getComputedStyle(fig(0).querySelector('.node text'));return s.stroke==='none' && parseFloat(s.fontSize)>=14 && fig(0).querySelectorAll('svg text').length>10 && fig(0).querySelectorAll('.edgePath,.edgePaths path').length>10})()").unwrap(),true);
    p.eval("fig(2).scrollIntoView({block:'center'})").unwrap();
    p.wait("fig(2).dataset.state==='error' && fig(2).querySelector('details').open");
    assert_eq!(
        p.eval("fig(2).querySelector('code').textContent.trim()")
            .unwrap(),
        "flowchart LR\n A[Broken"
    );
    p.eval("fig(3).scrollIntoView({block:'center'})").unwrap();
    p.wait("['ready','error'].includes(fig(3).dataset.state)");
    assert_eq!(p.eval("!window.diagramPwned && !fig(3).querySelector('svg script,svg foreignObject,svg a[href^=javascript]')").unwrap(),true);
    let expand = |p: &mut Page| {
        p.eval("fig(0).querySelector('[data-expand]').click()")
            .unwrap();
        p.wait("el('.artifact-diagram-dialog')?.open");
    };
    let close = |p: &mut Page| {
        key(p, "Escape", "Escape", 27, "");
        p.wait("!el('.artifact-diagram-dialog') && document.activeElement===fig(0).querySelector('[data-expand]') && !!fig(0).querySelector('svg')");
    };
    for theme in ["light", "dark"] {
        p.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        for (width, height) in [(1440, 1000), (820, 1180), (390, 844), (320, 740)] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":height,"mobile":false,"deviceScaleFactor":1}),
            )
            .unwrap();
            p.eval("fig(0).scrollIntoView({block:'center'})").unwrap();
            p.wait("fig(0).dataset.state==='ready' && document.documentElement.scrollWidth<=innerWidth+1");
            let size = p.command("Page.getLayoutMetrics", json!({})).unwrap();
            capture(
                &mut p,
                "out/artifact-diagrams",
                &format!("{theme}-{width}"),
                json!({"format":"png","captureBeyondViewport":true,"clip":{"x":0,"y":0,"width":size["cssContentSize"]["width"],"height":size["cssContentSize"]["height"],"scale":1}}),
            );
            expand(&mut p);
            p.wait("(()=>{const r=el('.artifact-diagram-dialog').getBoundingClientRect();return r.left>=0 && r.right<=innerWidth+1 && r.top>=0 && r.bottom<=innerHeight+1})()");
            assert_eq!(p.eval("new Set([...document.querySelectorAll('[id]')].map(n=>n.id)).size===document.querySelectorAll('[id]').length").unwrap(),true);
            screenshot(
                &mut p,
                "out/artifact-diagrams",
                &format!("{theme}-{width}-expanded"),
            );
            p.eval("window.zoom=()=>parseInt(el('.artifact-diagram-dialog output').textContent);window.fit=zoom();el('.artifact-diagram-dialog [data-in]').click()").unwrap();
            assert_eq!(p.eval("zoom()>fit").unwrap(), true);
            for (button, expected) in [("Actual size", "100"), ("Fit", "fit")] {
                p.eval(&format!("Array.from(el('.artifact-diagram-dialog').querySelectorAll('button')).find(b=>(b.getAttribute('aria-label')||b.textContent).trim()==={}).click()",json!(button))).unwrap();
                assert_eq!(p.eval(&format!("zoom()==={expected}")).unwrap(), true);
            }
            close(&mut p);
        }
    }
    p.eval("el('.artifact-menu summary').click();el('#artifact-edit-browser').click();el('#artifact-preview').click()").unwrap();
    p.wait("!!el('#artifact-edit-preview .artifact-diagram svg')");
    p.eval("el('#artifact-preview').click()").unwrap();
    assert_eq!(
        p.eval("el('#artifact-body').value").unwrap(),
        include_str!("recovery-flow.md")
    );
    p.eval("el('#artifact-cancel').click()").unwrap();
    p.wait("fig(0)?.dataset.state==='ready'");
    p.eval("const heading=[...document.querySelectorAll('#artifact-reading h2')].find(n=>n.textContent==='Ordinary code'); heading.scrollIntoView({block:'center'}); new Promise(done=>requestAnimationFrame(()=>requestAnimationFrame(done)))").unwrap();
    p.eval("const range=document.createRange();range.selectNodeContents(heading);getSelection().removeAllRanges();getSelection().addRange(range);el('#artifact-reading').dispatchEvent(new Event('pointerup'));el('#artifact-selection-comment').click();el('#artifact-comment').value='Diagram-safe anchor';el('#artifact-comment-form').requestSubmit()").unwrap();
    p.wait("el('#artifact-threads').innerText.includes('Diagram-safe anchor')");
    assert_eq!(p.eval("el('#artifact-threads blockquote').innerText==='Ordinary code' && !el('#artifact-threads').innerText.includes('Outdated selection')").unwrap(),true);
    assert_eq!(p.eval("problems").unwrap(), json!([]));
}
