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

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn project_workflow_links_open_their_markdown_files_and_recover_a_failed_save() {
    let f = installed_fixture();
    let project = "Workflow QA";
    f.init(project).unwrap();
    let settings = || run(f.issue(project, &["settings", "show"])).unwrap();
    let defaults = settings();
    let (_web, base) = web(&f, project);
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"window.el=s=>document.querySelector(s);window.field=()=>el('.fx-paper > div')?.shadowRoot?.querySelector('.cm-content');window.lines=()=>[...field().querySelectorAll('.cm-line')].map(e=>e.textContent).join('\\n')"})).unwrap();
    for (file, section) in [
        ("worker.md", "worker"),
        ("delivery-pr.md", "prs"),
        ("delivery-main.md", "main"),
        ("plan.md", "plan"),
        ("workspace-worktree.md", "worktree"),
        ("workspace-checkout.md", "checkout"),
    ] {
        p.command(
            "Page.navigate",
            json!({"url":format!("{base}/#project=named%3AWorkflow+QA")}),
        )
        .unwrap();
        p.wait("model.project?.id==='named:Workflow QA' && !!model.signature");
        p.eval("el('#project-more').click();el('#project-settings-trigger').click()")
            .unwrap();
        p.wait("el('#project-settings-dialog')?.open && !el('#project-worktree').disabled");
        assert_eq!(p.eval("['#editor-kind','#quick-issue-kind'].map(s=>[...el(s).options].map(o=>o.textContent))").unwrap(),json!([["Implement","Plan"],["Implement","Plan"]]));
        p.eval(&format!("el('[data-prompt-file=\"{file}\"]').click()"))
            .unwrap();
        p.wait(&format!(
            "el('.fx-document h3')?.textContent==='{file}' && !!field()"
        ));
        assert_eq!(
            p.eval("new URLSearchParams(location.hash.slice(1)).get('project')")
                .unwrap(),
            "named:Workflow QA"
        );
        let original = if section == "worker" {
            &defaults["worker_prompt_default"]
        } else {
            &defaults["prompt_defaults"][section]
        }
        .as_str()
        .unwrap()
        .trim_end();
        assert_eq!(p.eval("lines().trimEnd()").unwrap(), original);
        if matches!(section, "checkout" | "main" | "prs") {
            continue;
        }
        for theme in ["light", "dark"] {
            p.command(
                "Emulation.setEmulatedMedia",
                json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
            )
            .unwrap();
            for width in [1440, 768, 390, 320] {
                p.command(
                    "Emulation.setDeviceMetricsOverride",
                    json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":false}),
                )
                .unwrap();
                assert_eq!(p.eval("el('.file-explorer').scrollWidth<=el('.file-explorer').clientWidth+1 && document.documentElement.scrollWidth<=innerWidth+1 && el('[data-save]').checkVisibility()").unwrap(),true,"{file} {theme}/{width}");
                screenshot(
                    &mut p,
                    "out/prompt-files",
                    &format!("{section}-{theme}-{width}"),
                );
            }
        }
        // Insert into the real editor, keeping the shipped prompt as the fixture.
        p.eval("field().focus();window.confirm=()=>false").unwrap();
        p.command("Input.insertText", json!({"text":"!"})).unwrap();
        p.wait("el('[data-save]')?.disabled===false");
        let draft = p.eval("lines()").unwrap();
        assert_ne!(draft.as_str().unwrap().trim_end(), original);
        p.eval("el('.fx-row[title=\"delivery-pr.md\"]').click()")
            .unwrap();
        assert_eq!(p.eval("el('.fx-document h3').textContent").unwrap(), file);
        assert_eq!(p.eval("lines()").unwrap(), draft);
        p.eval("const send=window.fetch.bind(window);window.fetch=(url,opts)=>{if(url==='/api/action' && JSON.parse(opts.body).operation.action==='configure_project'){window.fetch=send;return Promise.resolve(Response.json({ok:false,error:{message:'Save interrupted'}},{status:503}))}return send(url,opts)};el('[data-save]').click()").unwrap();
        p.wait("el('.fx-error')?.textContent.includes('Save interrupted') && !el('[data-save]').disabled");
        assert_eq!(p.eval("lines()").unwrap(), draft);
        p.eval("el('[data-save]').click()").unwrap();
        p.wait("el('.fx-document [role=status]')?.textContent==='Saved'");
        let saved = settings();
        assert_eq!(
            (if section == "worker" {
                &saved["prompt"]
            } else {
                &saved["prompt_overrides"][section]
            })
            .as_str()
            .unwrap()
            .trim_end(),
            draft.as_str().unwrap().trim_end()
        );
        p.eval("window.loaded=true").unwrap();
        p.command("Page.reload", json!({})).unwrap();
        p.wait("!window.loaded && !!field()");
        assert_eq!(
            p.eval("lines()").unwrap().as_str().unwrap().trim_end(),
            draft.as_str().unwrap().trim_end()
        );
        p.eval("window.confirm=()=>true;[...el('.fx-actions').children].find(b=>b.textContent==='Restore default…').click()").unwrap();
        p.wait("el('.fx-document [role=status]')?.textContent==='Default restored'");
        assert_eq!(p.eval("lines().trimEnd()").unwrap(), original);
        let reset = settings();
        if section == "worker" {
            assert_eq!(reset["prompt"].as_str().unwrap().trim_end(), original);
        } else {
            assert!(reset["prompt_overrides"][section].is_null());
        }
    }
}
