use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn quiet_hours_validate_save_and_recover_a_concurrent_edit() {
    let f = fixture();
    let (project, _) = Scenario::QuietHours.destination();
    f.init(project).unwrap();
    let (_server, base) = web(&f, project);
    let mut p = Page::open(&format!("{base}/#project=named%3AQuiet%20hours%20QA"));
    p.wait("model.project?.id==='named:Quiet hours QA'");
    p.eval("window.problems=[];addEventListener('error',e=>problems.push(e.message));window.byId=id=>document.getElementById(id)").unwrap();
    let open = |p: &mut Page| {
        p.eval("byId('self-avatar').click();byId('global-settings-trigger').click()")
            .unwrap();
        p.wait("byId('global-settings-dialog').open && !byId('global-boss-name').disabled");
    };
    let change = |p: &mut Page, id: &str, value: &str| {
        p.eval(&format!("(()=>{{const input=byId({});input.value={};for(const event of ['input','change'])input.dispatchEvent(new Event(event,{{bubbles:true}}))}})()",json!(id),json!(value))).unwrap();
    };
    let save = |p: &mut Page| {
        p.eval("byId('global-settings-submit').click()").unwrap();
        p.wait("!byId('global-settings-dialog').open");
    };
    open(&mut p);
    assert_eq!(p.eval("[byId('global-quiet-enabled').checked,byId('global-quiet-start').value,byId('global-quiet-end').value,byId('global-settings-submit').disabled]").unwrap(),json!([true,"22:00","07:00",true]));
    for (width, height, theme) in [
        (1280, 960, "light"),
        (390, 844, "light"),
        (320, 568, "light"),
        (390, 844, "dark"),
    ] {
        p.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
        )
        .unwrap();
        p.command(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
        )
        .unwrap();
        assert_eq!(p.eval("document.documentElement.scrollWidth<=innerWidth && byId('global-settings-dialog').scrollWidth<=byId('global-settings-dialog').clientWidth+1").unwrap(),true);
        p.eval("byId('global-quiet-zone').scrollIntoView({block:'nearest'})")
            .unwrap();
        screenshot(&mut p, "out/quiet-hours", &format!("{theme}-{width}"));
    }
    change(&mut p, "global-quiet-start", "21:30");
    change(&mut p, "global-quiet-end", "21:30");
    p.eval("byId('global-settings-submit').click()").unwrap();
    assert_eq!(
        p.eval("byId('global-settings-dialog').open && !byId('global-quiet-end').validity.valid")
            .unwrap(),
        true
    );
    change(&mut p, "global-quiet-end", "08:15");
    change(&mut p, "global-quiet-zone", "Europe/Warsaw");
    save(&mut p);
    open(&mut p);
    assert_eq!(p.eval("[byId('global-quiet-start').value,byId('global-quiet-end').value,byId('global-quiet-zone').value]").unwrap(),json!(["21:30","08:15","Europe/Warsaw"]));
    p.eval("byId('global-quiet-enabled').click()").unwrap();
    assert_eq!(p.eval("byId('global-quiet-start').matches(':disabled')").unwrap(), true);
    save(&mut p);
    open(&mut p);
    assert_eq!(
        p.eval("byId('global-quiet-enabled').checked").unwrap(),
        false
    );
    p.eval("byId('global-quiet-enabled').click()").unwrap();
    change(&mut p, "global-quiet-start", "20:00");
    p.eval("api({action:'configure_global',boss_name:'Changed elsewhere'})")
        .unwrap();
    p.eval("byId('global-settings-submit').click()").unwrap();
    p.wait("byId('global-settings-reload').checkVisibility() && byId('global-settings-error').checkVisibility()");
    p.eval("byId('global-settings-reload').click()").unwrap();
    p.wait("byId('global-settings-reload').hidden && byId('global-quiet-start').value==='20:00'");
    save(&mut p);
    open(&mut p);
    assert_eq!(p.eval("byId('global-quiet-start').value").unwrap(), "20:00");
    key(&mut p, "Escape", "Escape", 27, "");
    p.wait("!byId('global-settings-dialog').open && document.activeElement===byId('self-avatar')");
    assert_eq!(p.eval("problems").unwrap(), json!([]));
    p.command(
        "Page.navigate",
        json!({"url":format!("{base}/?phone=1#settings=1")}),
    )
    .unwrap();
    p.wait("document.getElementById('global-settings-dialog')?.checkVisibility()");
}
