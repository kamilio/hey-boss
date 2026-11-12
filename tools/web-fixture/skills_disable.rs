use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn disabled_skills_retry_offline_and_restore_complete_archives() {
    let f = fixture();
    f.init("Skills QA").unwrap();
    let root = f.root.path();
    let homes = [root.join("home"), root.join("studio")];
    let installed: Vec<_> = homes
        .iter()
        .flat_map(|home| {
            [".codex", ".agents", ".claude", ".hey-boss"]
                .map(|agent| home.join(agent).join("skills/example"))
        })
        .collect();
    let text = "---\nname: example\ndescription: Fixture\n---\n";
    for dir in &installed {
        put(dir.join("SKILL.md"), text);
        put(dir.join(".hidden"), [0, 128, 255]);
        put(dir.join("script"), "#!/bin/sh\nexit 0\n");
        fs::set_permissions(dir.join("script"), fs::Permissions::from_mode(0o751)).unwrap();
    }
    install_transport(root);
    put(root.join("config.yaml"), "ssh_hosts: [studio]\n");
    let api = Api::start(&f);
    api.post(&json!({"action":"scan"}), 200);
    let healthy = |report: &Value| {
        assert!(
            report["machines"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["error"].as_str().unwrap_or("").is_empty()),
            "{report}"
        )
    };
    healthy(&api.idle());

    let mut page = Page::open("about:blank");
    page.command("Page.enable", json!({})).unwrap();
    page.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error',e=>sessionStorage.setItem('pageError',e.message));addEventListener('unhandledrejection',e=>sessionStorage.setItem('pageError',String(e.reason)))"})).unwrap();
    let open = |page: &mut Page, api: &Api| {
        let url = reqwest::Url::parse(&api.url)
            .unwrap()
            .join("/skills#skill=global%3Aexample")
            .unwrap();
        page.command("Page.navigate", json!({"url":url.as_str()}))
            .unwrap();
    };
    open(&mut page, &api);
    page.wait("document.querySelector('[data-disable-skill=example]')?.disabled === false");
    for path in ["/", "/skills", "/jobs", "/secrets"] {
        let nav=page.eval(&format!("fetch('{path}').then(r=>r.text()).then(html=>{{const d=new DOMParser().parseFromString(html,'text/html');return [d.querySelector('.global-navigation > #nav-skills')?.getAttribute('href'), d.querySelector('.app-navigation #nav-skills') !== null]}})")).unwrap();
        assert_eq!(nav, json!(["/skills", false]), "{path}");
    }
    let offline = homes[1].join(".offline");
    put(offline.clone(), "");
    page.eval("document.querySelector('[data-disable-skill=example]').click()")
        .unwrap();
    page.wait("document.querySelector('[data-disable-skill=example]')?.textContent === 'Retry disable' && !document.querySelector('[data-disable-skill=example]').disabled");
    let report = api.idle();
    assert!(
        report["disabled"]
            .as_array()
            .unwrap()
            .contains(&json!("example"))
    );
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("needs attention")
    );
    assert!(
        machine(&report, "studio")["error"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("offline")
    );
    for dir in &installed {
        assert_eq!(dir.exists(), dir.starts_with(&homes[1]));
    }

    fs::remove_file(offline).unwrap();
    page.eval("document.querySelector('[data-disable-skill=example]').click()")
        .unwrap();
    page.wait("document.querySelector('[data-enable-skill=example]')?.disabled === false && !document.querySelector('[data-disable-skill=example]')");
    let report = api.idle();
    healthy(&report);
    assert!(installed.iter().all(|p| !p.exists()));
    api.post(
        &json!({"action":"save_file","skill":"example","content":text}),
        409,
    );
    api.post(
        &json!({"action":"distribute","selected":["example"],"revision":report["revision"]}),
        409,
    );
    let mut sync = f.command(&["skill", "sync", "example"]);
    configure(&mut sync, &f);
    assert!(sync.status().unwrap().success());
    assert!(!installed[0].exists());
    assert_eq!(
        page.eval("sessionStorage.getItem('pageError')").unwrap(),
        Value::Null
    );
    drop(api);
    let api = Api::start(&f);
    open(&mut page, &api);
    page.wait("!!document.querySelector('[data-enable-skill=example]')");
    page.eval("document.querySelector('[data-filter=disabled]').click()")
        .unwrap();
    page.wait("document.querySelector('[data-enable-skill=example]')?.checkVisibility() === true");
    let shots =
        std::env::var("SKILLS_SCREENSHOTS").unwrap_or_else(|_| "out/skills-browser".to_owned());
    for (width, height, name) in [(1440, 1000, "disabled"), (390, 844, "disabled-mobile")] {
        page.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":height,"mobile":false,"deviceScaleFactor":1}),
        )
        .unwrap();
        page.wait("document.documentElement.scrollWidth <= innerWidth");
        screenshot(&mut page, &shots, name);
    }
    page.eval("document.querySelector('[data-enable-skill=example]').click()")
        .unwrap();
    page.wait(
        "document.querySelector('[data-disable-skill=example]')?.textContent === 'Disable skill'",
    );
    let report = api.idle();
    healthy(&report);
    assert!(
        !report["disabled"]
            .as_array()
            .unwrap()
            .contains(&json!("example"))
    );
    for dir in &installed {
        assert_eq!(fs::read_to_string(dir.join("SKILL.md")).unwrap(), text);
        assert_eq!(fs::read(dir.join(".hidden")).unwrap(), [0, 128, 255]);
        assert_eq!(
            fs::metadata(dir.join("script"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o751
        );
    }
    assert_eq!(
        page.eval("sessionStorage.getItem('pageError')").unwrap(),
        Value::Null
    );
}
