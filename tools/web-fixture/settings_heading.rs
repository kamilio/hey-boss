use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn project_settings_heading_fits_and_returns_focus() {
    let f = installed_fixture();
    f.init("Atlas").unwrap();
    let (_server, base) = web(&f, "Atlas");
    let mut p = Page::open("about:blank");
    p.command(
        "Page.navigate",
        json!({"url":format!("{base}/#project=named%3AAtlas")}),
    )
    .unwrap();
    p.wait("!!(model.project && model.signature)");
    let unbroken = "project_".repeat(30);
    for theme in ["light", "dark"] {
        p.command(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
        )
        .unwrap();
        for width in [1280, 768, 390, 320] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":800,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            for name in [
                "poe-code",
                "A very long project name with spaces and a useful ending",
                &unbroken,
                "日本語のプロジェクト名",
            ] {
                p.eval(&format!("document.querySelector('#project-more').click(); model.project.name={}; document.querySelector('#project-settings-trigger').click()",json!(name))).unwrap();
                p.wait("document.querySelector('#project-settings-dialog').open && !document.querySelector('#project-settings-close').disabled");
                let measured=p.eval(r#"(() => {
                    const el=id=>document.getElementById(id), dialog=el('project-settings-dialog'), title=el('project-settings-title'), name=el('project-settings-name');
                    const t=title.getBoundingClientRect(), n=name.getBoundingClientRect(), c=el('project-settings-close').getBoundingClientRect();
                    return [title.innerText.includes(name.title), name.title, n.top>=t.top && n.bottom<=t.bottom+1 && t.height<35 && t.right<=c.left && n.width>=40,
                        dialog.scrollWidth<=dialog.clientWidth+1, c.left>=0 && c.right<=innerWidth && c.top>=0];
                })()"#).unwrap();
                assert_eq!(
                    measured,
                    json!([true, name, true, true, true]),
                    "{theme}, {width}px, {name}"
                );
                p.eval("document.querySelector('#settings-tab-activity').click()")
                    .unwrap();
                p.wait("!document.querySelector('#settings-panel-activity').hidden && document.querySelector('#project-settings-title').checkVisibility()");
                p.eval("document.querySelector('#project-settings-close').click()")
                    .unwrap();
                p.wait("document.activeElement.id==='project-more' && !document.querySelector('#project-settings-dialog').open");
            }
        }
    }
}
