use super::*;

#[test]
fn installed_metadata_sync_keeps_allocated_edits_and_rejects_stale_pulls() {
    let f = installed_fixture();
    let (project, _) = Scenario::MetadataSync.destination();
    f.init(project).unwrap();
    metadata_sync::verify(&f, project).unwrap();
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn companion_metadata_denial_keeps_the_draft_and_restores_editor_focus() {
    let f = installed_fixture();
    let (project, _) = Scenario::MetadataSync.destination();
    f.init(project).unwrap();
    metadata_sync::verify(&f, project).unwrap();
    let (_server, base) = web(&f, project);
    let mut p = Page::open(&format!("{base}/#project=named%3AMetadata+sync+QA&issue=1"));
    p.wait("model.detail?.issue?.state==='closed' && !!document.querySelector('[data-edit]')");
    let original = p.eval("model.detail.issue").unwrap();
    p.eval("window.problems=[];addEventListener('error',e=>problems.push(e.message));window.el=s=>document.querySelector(s)").unwrap();
    for theme in ["light", "dark"] {
        p.command(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
        )
        .unwrap();
        p.eval("el('[data-edit]').click()").unwrap();
        for (field, value) in [
            ("#editor-subject", "Preserve this unsaved title"),
            ("#editor-body", "Keep my draft after a denied edit."),
            ("#editor-labels", "new-review-label"),
        ] {
            p.eval(&format!("(()=>{{const input=el({});input.focus();input.value={};input.dispatchEvent(new Event('input',{{bubbles:true}}))}})()",json!(field),json!(value))).unwrap();
        }
        key(&mut p, "Enter", "Enter", 13, "");
        p.eval("el('#editor-submit').click()").unwrap();
        p.wait("el('#editor-error').checkVisibility() && el('#editor-error').innerText.includes('Changes were not saved')");
        assert_eq!(p.eval("el('#editor-error').getBoundingClientRect().bottom<=el('#editor-submit').getBoundingClientRect().top && !el('#editor-conflict').checkVisibility() && pendingMutation.size===0").unwrap(),true);
        assert_eq!(
            p.eval("[el('#editor-subject').value,el('#editor-body').value]")
                .unwrap(),
            json!([
                "Preserve this unsaved title",
                "Keep my draft after a denied edit."
            ])
        );
        for width in [1440, 768, 390, 320] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            p.eval("el('#editor-error').scrollIntoView({block:'nearest'})")
                .unwrap();
            assert_eq!(p.eval("(()=>{const err=el('#editor-error'),r=el('#editor-submit').getBoundingClientRect();return document.documentElement.scrollWidth<=innerWidth && err.scrollWidth<=err.clientWidth && r.left>=0 && r.right<=innerWidth && r.top>=0 && r.bottom<=innerHeight})()").unwrap(),true,"{theme} {width}px");
            if width == 1440 || width == 320 {
                screenshot(&mut p, "out/metadata-sync", &format!("{theme}-{width}"));
            }
        }
        p.eval("el('#editor-cancel').click()").unwrap();
        p.wait("document.activeElement===el('[data-edit]')");
        assert_eq!(p.eval("model.detail.issue").unwrap(), original);
        p.eval("el('[data-action=reopen]').click()").unwrap();
        p.wait("el('#toast')?.textContent.includes('Changes were not saved') && !el('[data-action=reopen]').disabled && model.detail.issue.state==='closed'");
    }
    assert_eq!(p.eval("problems").unwrap(), json!([]));
}
