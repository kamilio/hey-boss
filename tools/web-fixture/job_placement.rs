use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn job_machine_placement_survives_reload_and_controls_fanout() {
    let f = fixture();
    let project = "Placement";
    f.init(project).unwrap();
    seed::seed(&f, Scenario::JobsUi, project).unwrap();
    let _supervisor = agents::supervisor(&f).unwrap();
    let (_web, base) = web(&f, project);
    let mut p = Page::open("about:blank");
    let machines = json!([{"node":"primary","host":"Mac Studio","state":"connected"},{"node":"fallback","host":"Build server","state":"disconnected"}]);
    let runtime =
        json!({"ok":true,"machine":"fixture","catalog":{"models":[]},"machines":machines});
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":format!("const realFetch=window.fetch;window.fetch=(url,init)=>url==='/api/jobs/runtime'?Promise.resolve(Response.json({runtime})):realFetch(url,init);addEventListener('error',e=>sessionStorage.setItem('pageError',e.message));")})).unwrap();
    p.command(
        "Page.navigate",
        json!({"url":format!("{base}/jobs#project=named%3APlacement")}),
    )
    .unwrap();
    let click = |p: &mut Page, sel: &str| {
        let node = format!("document.querySelector({})", json!(sel));
        p.wait(&format!("{node}?.disabled===false"));
        p.eval(&format!("{node}.click()")).unwrap();
    };
    let mode = |p: &mut Page, value: &str| {
        p.eval(&format!("document.querySelector('#job-placement').value='{value}';document.querySelector('#job-placement').dispatchEvent(new Event('change'))")).unwrap();
    };
    let save = |p: &mut Page, revision: u8| {
        click(p, "#job-save");
        p.wait(&format!("document.querySelector('.job-panel-subtitle')?.textContent.startsWith('Revision {revision} ·')"));
    };
    let reload = |p: &mut Page, value: &str| {
        p.eval("window.oldJobPage=true").unwrap();
        p.command("Page.reload", json!({})).unwrap();
        p.wait(&format!(
            "!window.oldJobPage && document.querySelector('#job-placement')?.value==='{value}'"
        ));
    };
    click(&mut p, "#job-new");
    p.wait("document.querySelector('#job-placement')?.value==='auto' && document.querySelector('#job-targets').hidden");
    p.eval(&format!("document.querySelector('#job-name').value='Machine priority';document.querySelector('#job-markdown').value={};document.querySelector('#job-model').value='fixture-model';document.querySelector('#job-enabled').checked=false",json!(include_str!("../../src/jobs/fixtures/original.md")))).unwrap();
    mode(&mut p, "specific");
    p.wait("!!document.querySelector('[data-machine-choice=primary]')");
    click(&mut p, "#job-machine-add");
    click(&mut p, "#job-machine-add");
    assert_eq!(
        p.eval("document.querySelector('[data-machine=\"0\"]').value")
            .unwrap(),
        "primary"
    );
    assert_eq!(p.eval("document.querySelector('[data-machine=\"1\"] option:checked').textContent.includes('Offline')").unwrap(),true);
    click(&mut p, "[data-machine-up=\"1\"]");
    save(&mut p, 1);
    p.wait("document.querySelector('[data-machine=\"0\"]')?.value==='fallback'");
    reload(&mut p, "specific");
    assert_eq!(
        p.eval("Array.from(document.querySelectorAll('[data-machine]'),el=>el.value)")
            .unwrap(),
        json!(["fallback", "primary"])
    );
    for width in [1440, 390] {
        for theme in ["light", "dark"] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":950,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            p.command(
                "Emulation.setEmulatedMedia",
                json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
            )
            .unwrap();
            p.eval("document.querySelector('#job-targets').scrollIntoView({block:'center'})")
                .unwrap();
            p.wait("document.documentElement.scrollWidth<=innerWidth && document.querySelector('#job-panel').scrollWidth<=document.querySelector('#job-panel').clientWidth");
            screenshot(
                &mut p,
                "out/job-placement",
                &format!("priority-{width}-{theme}"),
            );
        }
    }
    mode(&mut p, "auto");
    save(&mut p, 2);
    reload(&mut p, "auto");
    mode(&mut p, "all");
    p.wait("document.querySelector('#job-targets').hidden && document.querySelector('#job-all-help').checkVisibility()");
    save(&mut p, 3);
    reload(&mut p, "all");
    p.eval("document.querySelector('#job-placement').scrollIntoView({block:'center'})")
        .unwrap();
    screenshot(&mut p, "out/job-placement", "all-mobile");
    for (label, count) in [("All machines", 1), ("Selected machines", 2)] {
        if count == 2 {
            mode(&mut p, "selected");
            p.wait("document.querySelector('#job-machine-choices').checkVisibility() && document.querySelector('#job-targets').hidden");
            for node in ["primary", "fallback"] {
                click(&mut p, &format!("[data-machine-choice={node}]"));
            }
            p.wait("document.querySelector('#job-selected-count').textContent.includes('2 machines selected')");
            save(&mut p, 4);
            reload(&mut p, "selected");
            for node in ["primary", "fallback"] {
                p.wait(&format!(
                    "document.querySelector('[data-machine-choice={node}]')?.checked"
                ));
                click(&mut p, &format!("[data-machine-choice={node}]"));
            }
            click(&mut p, "#job-save");
            p.wait("document.querySelector('#jobs-error').textContent.includes('Select at least one machine')");
            for node in ["primary", "fallback"] {
                click(&mut p, &format!("[data-machine-choice={node}]"));
            }
            save(&mut p, 5);
            p.eval(
                "document.querySelector('#job-machine-choices').scrollIntoView({block:'center'})",
            )
            .unwrap();
            p.wait("document.documentElement.scrollWidth<=innerWidth");
            screenshot(&mut p, "out/job-placement", "selected-mobile");
        }
        click(&mut p, "#job-run");
        p.wait("document.querySelector('#job-stop')?.hidden===false");
        click(&mut p, "#job-history-refresh");
        p.wait(&format!("document.querySelector('#job-history').textContent.includes('0 of {count} machines finished') && document.querySelector('#job-history').textContent.includes('{label}')"));
        if count == 1 {
            assert_eq!(
                p.eval("document.querySelectorAll('#job-history .job-run').length")
                    .unwrap(),
                2
            );
        }
        click(&mut p, "#job-stop");
        p.wait("document.querySelector('#job-stop')?.hidden && document.querySelector('#job-history').textContent.includes('cancelled')");
    }
    assert_eq!(
        p.eval("sessionStorage.getItem('pageError')").unwrap(),
        Value::Null
    );
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn jobs_and_issues_share_themes_and_project_menu_behavior() {
    let f = fixture();
    f.init("Jobs UI QA").unwrap();
    let (_server, base) = web(&f, "Jobs UI QA");
    let mut p = Page::open("about:blank");
    for width in [1440, 390] {
        for theme in ["light", "dark"] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":1000,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            p.command(
                "Emulation.setEmulatedMedia",
                json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
            )
            .unwrap();
            let mut expected = None;
            for path in ["/", "/issues", "/jobs"] {
                let url = format!("{base}{path}#project=named%3AJobs%20UI%20QA");
                p.command("Page.navigate", json!({"url":url})).unwrap();
                p.wait(&format!("location.href==={} && document.querySelector('#project-name')?.textContent==='Jobs UI QA'",json!(url)));
                p.eval("document.querySelector('#project-trigger').click()")
                    .unwrap();
                p.wait("document.querySelector('#project-menu').checkVisibility()");
                let styles=p.eval(r#"Object.fromEntries(['body','.app-header','.app-navigation','#project-trigger','#project-menu','.navigation-more-links','.button.primary'].map(s=>{
                const el=document.querySelector(s);const css=el&&getComputedStyle(el);
                return [s,css&&['backgroundColor','backgroundImage','color','borderColor','borderRadius','fontFamily','fontSize','fontWeight','boxShadow'].map(k=>css[k])];
            }))"#).unwrap();
                if let Some(reference) = &expected {
                    assert_eq!(&styles, reference, "{path} {width} {theme}");
                } else {
                    expected = Some(styles);
                }
                p.wait("document.querySelector('#project-menu').getBoundingClientRect().left>=0 && document.querySelector('#project-menu').getBoundingClientRect().right<=innerWidth");
                for event in ["keyDown", "keyUp"] {
                    p.command("Input.dispatchKeyEvent",json!({"type":event,"key":"Escape","code":"Escape","windowsVirtualKeyCode":27})).unwrap();
                }
                p.wait("document.activeElement.id==='project-trigger' && document.documentElement.scrollWidth<=innerWidth");
                if path == "/jobs" {
                    let mut clip =
                        p.command("Page.getLayoutMetrics", json!({})).unwrap()["cssContentSize"]
                            .clone();
                    clip["scale"] = 1.into();
                    capture(
                        &mut p,
                        "out/job-theme",
                        &format!("{width}-{theme}"),
                        json!({"format":"png","captureBeyondViewport":true,"clip":clip}),
                    );
                }
            }
        }
    }
}
