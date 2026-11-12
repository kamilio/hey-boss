use super::*;
use crate::markdown_attachments::{remove_sources, svg, write_sources, CSV, IMPORT, PROJECT};
fn cli(f: &Fixture, args: &[&str]) -> Value {
    run(f.command(&[args, &["--project", PROJECT, "--json"]].concat())).unwrap()
}
fn rejected(f: &Fixture, args: &[&str], message: &str) {
    let out = f
        .command(&[args, &["--project", PROJECT]].concat())
        .output()
        .unwrap();
    assert!(
        out.status.code().is_some_and(|n| n != 0)
            && String::from_utf8_lossy(&out.stderr).contains(message),
        "{args:?}: {out:?}"
    );
}

#[test]
fn installed_artifact_actions_and_guarded_deletion() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    let (_server, base) = web(&f, PROJECT);
    let asset = |path| {
        reqwest::blocking::get(format!("{base}/{path}"))
            .unwrap()
            .error_for_status()
            .unwrap()
            .text()
            .unwrap()
    };
    let script = asset("artifacts.js");
    assert!(script.contains("data-delete=") && script.contains("function confirmDelete("));
    let css = asset("artifacts.css");
    assert!(css.contains(".artifact-delete-dialog") && css.contains("width: 44px; height: 44px;"));
    assert!(asset("artifacts").contains("id=\"artifact-archived\""));
    let created = cli(
        &f,
        &[
            "artifact",
            "create",
            "--title",
            "Disposable scratch",
            "--body",
            "Delete this test document",
        ],
    );
    let id = created["artifact"]["id"].as_str().unwrap();
    assert_eq!(created["artifact"]["version"], 1);
    cli(&f, &["artifact", "archive", id, "--if-version", "1"]);
    let list = || cli(&f, &["artifact", "list", "--archived"]);
    assert!(list()["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["id"] == id && a["archived"] == true));
    cli(&f, &["artifact", "restore", id, "--if-version", "2"]);
    assert_eq!(cli(&f, &["artifact", "view", id])["artifact"]["version"], 3);
    rejected(
        &f,
        &["artifact", "delete", id, "--if-version", "1"],
        "Artifact changed",
    );
    let args = [
        "artifact",
        "delete",
        id,
        "--if-version",
        "3",
        "--request-id",
        "verified-delete",
    ];
    let deleted = cli(&f, &args);
    assert_eq!(deleted["deleted"], id);
    assert_eq!(cli(&f, &args), deleted);
    rejected(&f, &["artifact", "view", id], "Artifact not found");
    assert!(!list()["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["id"] == id));
}

#[test]
fn installed_markdown_import_keeps_files_after_source_removal() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    let root = f.root.path();
    write_sources(&f).unwrap();
    let created = cli(&f, IMPORT);
    assert_eq!(cli(&f, IMPORT), created);
    let id = created["artifact"]["id"].as_str().unwrap();
    let files = || {
        cli(&f, &["attachment", "list", "--artifact", id])["attachments"]
            .as_array()
            .unwrap()
            .clone()
    };
    let attachments = files();
    assert_eq!(attachments.len(), 2);
    let body = created["artifact"]["body"].as_str().unwrap();
    for file in &attachments {
        assert!(
            body.contains(&format!("/attachments/{}", file["id"].as_str().unwrap())),
            "{body}"
        );
    }
    assert!(body.contains("`![example](missing.png)`"));
    let csv = attachments
        .iter()
        .find(|a| a["name"] == "data.csv")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let download = |name| {
        cli(&f, &["attachment", "download", csv, "--output", name]);
        assert_eq!(fs::read(root.join(name)).unwrap(), CSV.as_bytes());
    };
    download("download.csv");
    fs::write(root.join("missing.md"), "![missing](absent.png)").unwrap();
    rejected(
        &f,
        &[
            "artifact",
            "create",
            "--title",
            "Missing",
            "--file",
            "missing.md",
        ],
        "absent.png",
    );
    rejected(
        &f,
        &[
            "artifact",
            "edit",
            id,
            "--file",
            "report.md",
            "--if-version",
            "99",
        ],
        "Artifact changed",
    );
    assert_eq!(files(), attachments);
    remove_sources(&f).unwrap();
    download("after-source-removal.csv");

    let (_service, base) = web(&f, PROJECT);
    let get = |path| {
        reqwest::blocking::get(format!("{base}/{path}"))
            .unwrap()
            .error_for_status()
            .unwrap()
    };
    let page = get("artifacts");
    let csp = page
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp
        .split(';')
        .any(|s| s.trim_start().starts_with("img-src ") && s.contains("blob:")));
    assert!(get("attachments.js")
        .text()
        .unwrap()
        .contains("function hydrate"));
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn markdown_attachments_render_download_preview_and_retry() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    let info = crate::markdown_attachments::seed(&f).unwrap();
    let (_service, base) = web(&f, PROJECT);
    let url = crate::markdown_attachments::url(&base, &info["id"]);
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error', e => sessionStorage.setItem('page-error',e.message)); addEventListener('unhandledrejection', e => sessionStorage.setItem('page-error',String(e.reason)))"})).unwrap();
    p.command("Page.navigate", json!({"url":url})).unwrap();
    let reading = |width| {
        format!("document.querySelector('#artifact-reading img')?.naturalWidth === {width}")
    };
    p.wait(&reading(720));
    p.command("Page.reload", json!({})).unwrap();
    p.wait(&reading(720));
    let directory = "out/markdown-attachments";
    for theme in ["light", "dark"] {
        p.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
        for width in [1440, 768, 390, 320] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            p.wait("document.documentElement.scrollWidth<=innerWidth+1 && document.querySelector('#artifact-reading img').getBoundingClientRect().width>=100 && document.querySelector('#artifact-reading img').getBoundingClientRect().width<=innerWidth");
            screenshot(&mut p, directory, &format!("{theme}-{width}"));
        }
    }
    let downloads = f.root.path().join("downloads");
    fs::create_dir(&downloads).unwrap();
    p.command(
        "Browser.setDownloadBehavior",
        json!({"behavior":"allow","downloadPath":downloads}),
    )
    .unwrap();
    p.eval("Array.from(document.querySelectorAll('#artifact-reading a')).find(a=>a.textContent==='Download measurements').click()").unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !downloads.join("data.csv").is_file() {
        assert!(
            std::time::Instant::now() < until,
            "Chrome did not download data.csv"
        );
        thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(
        fs::read(downloads.join("data.csv")).unwrap(),
        CSV.as_bytes()
    );

    p.command(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":false}),
    )
    .unwrap();
    p.eval("document.querySelector('.artifact-menu summary').click();document.querySelector('#artifact-edit-browser').click()")
        .unwrap();
    let root = f.root.path();
    let choose = |p: &mut Page, files: &[&str]| {
        let doc = p.command("DOM.getDocument", json!({})).unwrap();
        let input = p
            .command(
                "DOM.querySelector",
                json!({"nodeId":doc["root"]["nodeId"],"selector":"#artifact-import"}),
            )
            .unwrap();
        let paths: Vec<_> = files.iter().map(|s| root.join(s)).collect();
        p.command(
            "DOM.setFileInputFiles",
            json!({"nodeId":input["nodeId"],"files":paths}),
        )
        .unwrap();
    };
    let draft = "document.querySelector('#artifact-body').value";
    p.wait("!!document.querySelector('#artifact-import')");
    let original = p.eval(draft).unwrap();
    fs::write(root.join("missing.md"), "![missing](absent.png)").unwrap();
    choose(&mut p, &["missing.md"]);
    p.wait("document.querySelector('#artifact-error').innerText.includes('Select the linked file “absent.png”')");
    assert_eq!(p.eval(draft).unwrap(), original);
    fs::write(root.join("report.md"),"# Updated evidence\n\n![Browser diagram](diagram.svg)\n\n[Measurements](data.csv)\n\n`[literal](missing.csv)`").unwrap();
    fs::write(root.join("diagram.svg"), svg(500)).unwrap();
    fs::write(root.join("data.csv"), "value\n42\n").unwrap();
    choose(&mut p, &["report.md", "diagram.svg", "data.csv"]);
    p.wait("document.querySelector('#artifact-status').textContent === 'Imported Markdown and 2 linked files. Save to upload.' && document.querySelector('#artifact-error').hidden");
    p.eval("document.querySelector('#artifact-preview').click()")
        .unwrap();
    p.wait("document.querySelector('#artifact-edit-preview img')?.naturalWidth === 500");
    screenshot(&mut p, directory, "import-preview-phone");
    p.eval("document.querySelector('#artifact-editor button[type=submit]').click()")
        .unwrap();
    p.wait(&reading(500));
    p.command("Page.reload", json!({})).unwrap();
    p.wait(&reading(500));
    screenshot(&mut p, directory, "import-saved-phone");

    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":r#"const originalFetch=window.fetch;let offline=true;window.fetch=function(input,opts){if(offline && String(input).endsWith('/api/action') && JSON.parse(opts.body).operation?.operation?.command==='download'){offline=false;return Promise.resolve(new Response('{"ok":false,"error":{"message":"Synthetic offline attachment"}}',{status:503,headers:{'Content-Type':'application/json'}}))}return originalFetch.call(this,input,opts)}"#})).unwrap();
    p.command("Page.reload", json!({})).unwrap();
    p.wait("document.querySelector('.attachment-image-retry')?.textContent === 'Browser diagram — retry loading'");
    p.eval("document.querySelector('.attachment-image-retry').click()")
        .unwrap();
    p.wait(&reading(500));
    assert_eq!(
        p.eval("sessionStorage.getItem('page-error')").unwrap(),
        Value::Null
    );
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn empty_library_has_one_create_button_and_back_returns_to_library() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    let (_service, base) = web(&f, PROJECT);
    let mut p = Page::open(&format!(
        "{base}/artifacts#project=named%3AMarkdown%20attachments"
    ));
    p.command(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":false}),
    )
    .unwrap();
    let empty = "document.querySelector('.artifact-empty h2')?.innerText==='Your documents start here' && !!document.querySelector('#artifact-empty-action')?.getClientRects().length";
    p.wait(empty);
    assert_eq!(p.eval("Array.from(document.querySelectorAll('button')).filter(b=>b.getClientRects().length && b.innerText.trim()==='New artifact').length").unwrap(),1);
    p.eval("document.querySelector('#artifact-empty-action').click()")
        .unwrap();
    p.wait("!!document.querySelector('#artifact-title')?.getClientRects().length");
    p.eval("document.querySelector('#artifact-cancel').click()")
        .unwrap();
    p.wait(empty);
    assert_eq!(
        p.eval("document.documentElement.scrollWidth<=innerWidth+1")
            .unwrap(),
        true
    );
}
#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn validation_leaves_artifact_draft_editable() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    let created = cli(
        &f,
        &[
            "artifact",
            "create",
            "--title",
            "Corrections",
            "--body",
            "Original text",
        ],
    );
    let (_service, base) = web(&f, PROJECT);
    let mut p = Page::open(&crate::markdown_attachments::url(
        &base,
        &created["artifact"]["id"],
    ));
    p.wait("!!document.querySelector('#artifact-reading')");
    p.eval("document.querySelector('.artifact-menu summary').click();document.querySelector('#artifact-edit-browser').click()").unwrap();
    p.wait("!!document.querySelector('#artifact-body')");
    p.eval(r#"const fetchOriginal=window.fetch; window.fetch=function(input,opts) {
        if(String(input).endsWith('/api/action') && JSON.parse(opts.body).operation?.operation?.command==='edit'){
            window.fetch=fetchOriginal;
            return Promise.resolve(new Response('{"ok":false,"error":{"code":"invalid_input","message":"Correct this document before saving"}}',{status:400,headers:{'Content-Type':'application/json'}}));
        }return fetchOriginal.call(this,input,opts);
    };const field=document.querySelector('#artifact-body');field.value='Keep this correction draft.';field.dispatchEvent(new Event('input',{bubbles:true}));document.querySelector('#artifact-editor').requestSubmit()"#).unwrap();
    p.wait("document.querySelector('#artifact-error').innerText.includes('Correct this document before saving')");
    assert_eq!(p.eval("!document.querySelector('#artifact-load-latest') && !field.disabled && !field.readOnly && field.value==='Keep this correction draft.'").unwrap(),true);
    p.eval("field.value+=' Corrected.';field.dispatchEvent(new Event('input',{bubbles:true}));document.querySelector('#artifact-editor').requestSubmit()").unwrap();
    p.wait("document.querySelector('#artifact-reading')?.innerText.includes('Keep this correction draft. Corrected.')===true");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn retrying_the_library_and_saving_an_import_with_the_keyboard() {
    let f = installed_fixture();
    let project = "Library recovery";
    f.init(project).unwrap();
    run(f.issue(project, &["create", "--title", "Referring task"])).unwrap();
    run(f.command(&[
        "artifact",
        "create",
        "--project",
        project,
        "--title",
        "Existing note",
        "--body",
        "Prior decision.",
        "--json",
    ]))
    .unwrap();
    let (_web, base) = web(&f, project);
    let library = format!("{base}/artifacts#project=named%3ALibrary+recovery");
    let mut p = Page::open("about:blank");
    p.command(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":false}),
    )
    .unwrap();
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"window.el=s=>document.querySelector(s);const send=window.fetch.bind(window);window.fetch=(url,opts)=>url==='/api/action' && !sessionStorage.getItem('online') && JSON.parse(opts.body)?.operation?.operation?.command==='list'?Promise.resolve(Response.json({error:'Connection unavailable'},{status:503})):send(url,opts)"})).unwrap();
    p.command("Page.navigate", json!({"url":library})).unwrap();
    p.wait("el('#artifact-error p')?.innerText==='Connection unavailable'");
    p.eval("sessionStorage.setItem('online','1');el('#artifact-error button').click()")
        .unwrap();
    p.wait("!!el('.artifact-row-link') && el('#artifact-error').hidden");
    assert_eq!(p.eval("location.href").unwrap(), library);
    p.command(
        "Page.navigate",
        json!({"url":format!("{library}&new=1&issue=1")}),
    )
    .unwrap();
    p.wait("el('#artifact-cancel')?.checkVisibility()");
    p.eval("el('#artifact-cancel').click()").unwrap();
    p.wait("location.pathname==='/' && new URLSearchParams(location.hash.slice(1)).get('issue')==='1' && model.project?.name==='Library recovery'");
    p.command("Page.navigate", json!({"url":format!("{library}&new=1")}))
        .unwrap();
    p.wait("el('#artifact-title')?.checkVisibility()");
    p.eval("el('#artifact-title').value='Imported verification'")
        .unwrap();
    let source = "# Imported notes\n\n**Keep this** when exporting.\n";
    let input = f.root.path().join("verification.md");
    fs::write(&input, source).unwrap();
    let node = p.command("DOM.getDocument", json!({})).unwrap()["root"]["nodeId"].clone();
    let input_node = p
        .command(
            "DOM.querySelector",
            json!({"nodeId":node,"selector":"#artifact-import"}),
        )
        .unwrap()["nodeId"]
        .clone();
    p.command(
        "DOM.setFileInputFiles",
        json!({"nodeId":input_node,"files":[input]}),
    )
    .unwrap();
    p.wait(&format!("el('#artifact-body').value==={}", json!(source)));
    p.eval("el('#artifact-body').focus()").unwrap();
    for event in ["keyDown", "keyUp"] {
        p.command("Input.dispatchKeyEvent",json!({"type":event,"key":"s","code":"KeyS","windowsVirtualKeyCode":83,"modifiers":if cfg!(target_os="macos"){4}else{2}})).unwrap();
    }
    p.wait("el('#artifact-reading strong')?.innerText==='Keep this'");
    let downloads = f.root.path().join("downloads");
    fs::create_dir(&downloads).unwrap();
    p.command(
        "Browser.setDownloadBehavior",
        json!({"behavior":"allow","downloadPath":downloads}),
    )
    .unwrap();
    p.eval("el('.artifact-menu summary').click();el('#artifact-export').click()")
        .unwrap();
    let file = downloads.join("Imported verification.md");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !file.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "export missing: {:?}",
            fs::read_dir(&downloads).unwrap().collect::<Vec<_>>()
        );
        thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(fs::read_to_string(file).unwrap(), source);
    p.eval("el('.artifact-menu summary').click();el('#artifact-archive').click()")
        .unwrap();
    p.wait("el('#artifact-archive')?.innerText==='Restore' || el('#artifact-archive')?.textContent.trim()==='Restore'");
    p.eval("[...document.links].find(a=>a.innerText.trim()==='All artifacts' && a.checkVisibility()).click()").unwrap();
    p.wait("el('#artifact-archived')?.checkVisibility() && !el('#artifact-archived').disabled");
    p.eval("el('#artifact-archived').click()").unwrap();
    p.wait("el('.artifact-row')?.innerText.includes('Imported verification') && el('.artifact-card-label').innerText.toLowerCase().includes('archived')");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn issue_attachments_keep_the_same_document_after_unlink_and_rename() {
    let f = installed_fixture();
    f.init(PROJECT).unwrap();
    run(f.issue(PROJECT, &["create", "--title", "Design request"])).unwrap();
    cli(
        &f,
        &[
            "artifact",
            "create",
            "--title",
            "Unrelated note",
            "--body",
            "A separate document",
        ],
    );
    let (_server, base) = web(&f, PROJECT);
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"window.el=s=>document.querySelector(s);window.fill=(s,v)=>{const e=el(s);e.value=v;e.dispatchEvent(new Event('input',{bubbles:true}))}"})).unwrap();
    p.command(
        "Page.navigate",
        json!({"url":format!("{base}/#project=named%3AMarkdown+attachments&issue=1")}),
    )
    .unwrap();
    p.wait("el('#issue-artifacts [data-create-artifact]')?.checkVisibility()");
    p.eval("el('[data-create-artifact]').click()").unwrap();
    p.wait("el('#artifact-title')?.checkVisibility()");
    assert_eq!(
        p.eval("Object.fromEntries(new URLSearchParams(location.hash.slice(1)))")
            .unwrap(),
        json!({"new":"1","issue":"1","project":format!("named:{PROJECT}")})
    );
    p.eval("fill('#artifact-title','Shared plan');fill('#artifact-body','# Shared design');el('#artifact-editor').requestSubmit()").unwrap();
    p.wait("el('.artifact-backlinks a')?.textContent.includes('#1')");
    let url = p.eval("location.href").unwrap();
    let id = p
        .eval("new URLSearchParams(location.hash.slice(1)).get('artifact')")
        .unwrap();
    p.eval("el('.artifact-backlinks a').click()").unwrap();
    p.wait("el('.artifact-attachment-list a')?.textContent==='Shared plan'");
    p.eval("el('#issue-artifacts [data-unlink]').click()")
        .unwrap();
    p.wait("!!el('[data-create-artifact]') && !el('.artifact-attachment-list a')");
    p.eval("el('[data-attach]').click();fill('[data-attach-form] input','missing document')")
        .unwrap();
    p.wait("el('[data-attach-status]')?.textContent==='No matching artifacts'");
    assert_eq!(
        p.eval("el('[data-attach-form] [type=submit]').disabled")
            .unwrap(),
        true
    );
    p.eval("el('[data-attach-cancel]').click()").unwrap();
    assert_eq!(
        p.eval("!el('[data-attach-form] input') && el('[data-attach]')===document.activeElement")
            .unwrap(),
        true
    );
    p.eval("el('[data-attach]').click();fill('[data-attach-form] input','Shared plan')")
        .unwrap();
    p.wait("el('[data-attach-form] select')?.options.length===1 && !el('[data-attach-form] [type=submit]').disabled");
    assert_eq!(p.eval("el('[data-attach-form] select').value").unwrap(), id);
    p.eval("el('[data-attach-form] [type=submit]').click()")
        .unwrap();
    p.wait("el('.artifact-attachment-list a')?.textContent==='Shared plan'");
    p.eval("el('.artifact-attachment-list a').click()").unwrap();
    p.wait("el('.artifact-backlinks a')?.textContent.includes('#1')");
    assert_eq!(p.eval("location.href").unwrap(), url);
    p.eval("el('.artifact-backlinks a').click()").unwrap();
    p.wait("el('.artifact-attachment-list a')?.textContent==='Shared plan'");
    cli(
        &f,
        &[
            "artifact",
            "edit",
            id.as_str().unwrap(),
            "--title",
            "Reviewed plan",
            "--if-version",
            "1",
        ],
    );
    p.eval("el('#refresh').click()").unwrap();
    p.wait("el('.artifact-attachment-list a')?.textContent==='Reviewed plan'");
    assert_eq!(
        p.eval("el('.artifact-attachment-list a').href").unwrap(),
        url
    );
}
