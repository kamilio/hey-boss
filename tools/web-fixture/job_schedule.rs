use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn job_schedule_presets_preserve_cron_semantics() {
    let f = fixture();
    f.init("Schedule").unwrap();
    let (_web, base) = web(&f, "Schedule");
    let mut p = Page::open(&format!("{base}/jobs"));
    for (cron, fields, canonical, summary) in [
        (
            "25 */3 * * *",
            json!({"frequency":"hours","time":"09:00","interval":3,"minute":25}),
            "25 */3 * * *",
            "Every 3 hours at :25",
        ),
        (
            "00 */12 * * *",
            json!({"frequency":"hours","time":"09:00","interval":12,"minute":0}),
            "0 */12 * * *",
            "Every 12 hours at :00",
        ),
        (
            "0 */5 * * *",
            json!({"frequency":"hours","time":"09:00","interval":5,"minute":0}),
            "0 */5 * * *",
            "Every 5 hours at :00, restarting each day",
        ),
        (
            "30 17 * * 1-5",
            json!({"frequency":"weekdays","time":"17:30"}),
            "30 17 * * 1-5",
            "Weekdays at 17:30",
        ),
        (
            "005 009 * * *",
            json!({"frequency":"daily","time":"09:05"}),
            "5 9 * * *",
            "Every day at 09:05",
        ),
        (
            "0 * * * *",
            json!({"frequency":"hourly","time":"09:00","minute":0}),
            "0 * * * *",
            "Every hour",
        ),
        (
            "*/15 * * * *",
            json!({"frequency":"minutes","time":"09:00","interval":15}),
            "*/15 * * * *",
            "Every 15 minutes",
        ),
        (
            "*/7 * * * *",
            json!({"frequency":"minutes","time":"09:00","interval":7}),
            "*/7 * * * *",
            "Every 7 minutes, restarting each hour",
        ),
        (
            "45 * * * *",
            json!({"frequency":"hourly","time":"09:00","minute":45}),
            "45 * * * *",
            "Every hour at :45",
        ),
        (
            "5 9 * * 1,3,5",
            json!({"frequency":"weekly","time":"09:05","days":[1,3,5]}),
            "5 9 * * 1,3,5",
            "Mon, Wed, Fri at 09:05",
        ),
        (
            "5 9 31 * *",
            json!({"frequency":"monthly","time":"09:05","day":31}),
            "5 9 31 * *",
            "Day 31 of each month at 09:05",
        ),
        (
            "0 9 * * 7,0",
            json!({"frequency":"weekly","time":"09:00","days":[0]}),
            "0 9 * * 0",
            "Sun at 09:00",
        ),
        (
            "5 9 * * 1",
            json!({"frequency":"weekly","time":"09:05","days":[1]}),
            "5 9 * * 1",
            "Mondays at 09:05",
        ),
    ] {
        assert_eq!(
            p.eval(&format!("HeyBossJobs.scheduleFields({})", json!(cron)))
                .unwrap(),
            fields
        );
        assert_eq!(
            p.eval(&format!(
                "HeyBossJobs.scheduleCron({},{},{fields})",
                fields["frequency"], fields["time"]
            ))
            .unwrap(),
            canonical
        );
        assert_eq!(
            p.eval(&format!("HeyBossJobs.summary({})", json!(cron)))
                .unwrap(),
            summary
        );
    }
    for interval in [2, 3, 4, 5, 6, 12, 23] {
        for minute in [0, 25, 59] {
            let cron = format!("{minute} */{interval} * * *");
            assert_eq!(
                p.eval(&format!(
                    "HeyBossJobs.scheduleCron('hours','09:00',HeyBossJobs.scheduleFields({}))",
                    json!(cron)
                ))
                .unwrap(),
                cron
            );
        }
    }
    for expr in [
        "{interval:0,minute:25}",
        "{interval:1,minute:25}",
        "{interval:24,minute:25}",
        "{interval:2.5,minute:25}",
        "{interval:NaN,minute:25}",
        "{interval:3,minute:-1}",
        "{interval:3,minute:60}",
        "{interval:3,minute:1.5}",
        "{interval:3,minute:NaN}",
    ] {
        assert_eq!(
            p.eval(&format!("HeyBossJobs.scheduleCron('hours','09:00',{expr})"))
                .unwrap(),
            ""
        );
    }
    for cron in [
        "25 */0 * * *",
        "25 */24 * * *",
        "60 */3 * * *",
        "25 1-23/3 * * *",
        "25 */3 * * MON",
        "5 9 1 * 1",
        "*/61 * * * *",
        "0 24 * * *",
        "60 9 * * *",
        "0 9 0 * *",
        "0 9 32 * *",
        "0 9 * * 8",
        "0 9 * * * extra",
        "*/7 2-8 * JAN MON",
    ] {
        assert_eq!(
            p.eval(&format!(
                "HeyBossJobs.scheduleFields({}).frequency",
                json!(cron)
            ))
            .unwrap(),
            "custom",
            "{cron}"
        );
    }
    for (call, expected) in [
        (
            "scheduleCron('weekly','09:05',{days:[5,1,3]})",
            "5 9 * * 1,3,5",
        ),
        ("scheduleCron('weekly','09:05',{days:[]})", ""),
        ("scheduleCron('weekly','09:05')", "5 9 * * 1"),
        ("scheduleCron('daily','')", ""),
        ("summary('0 9 * * 1-5')", "Weekdays at 09:00"),
        ("summary('30 8 * * *')", "Every day at 08:30"),
        (
            "summary('*/7 2-8 * JAN MON')",
            "Minute */7 · Hour 2-8 · Day * · Month JAN · Weekday MON",
        ),
    ] {
        assert_eq!(p.eval(&format!("HeyBossJobs.{call}")).unwrap(), expected);
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn job_list_filters_runtime_availability_and_warnings() {
    let f = fixture();
    f.init("Status").unwrap();
    let (_web, base) = web(&f, "Status");
    let mut p = Page::open(&format!("{base}/jobs"));
    let jobs = json!([
        {"id":"on","enabled":true,"snapshot":{"definition":{"name":"Chief","model":"gpt","cron":"0 * * * *"}},"last_run":{"state":"failed"}},
        {"id":"off","enabled":false,"snapshot":{"definition":{"name":"Weekly review","model":"claude","cron":"0 9 * * 1"}},"active_run":{"state":"running"}},
        {"id":"gone","enabled":false,"deleted_at":123,"snapshot":{"definition":{"name":"Old","model":"gpt","cron":"0 * * * *"}}}
    ]);
    assert_eq!(
        p.eval(&format!("{jobs}.map(HeyBossJobs.scheduleState)"))
            .unwrap(),
        json!(["Enabled", "Disabled", "Deleted"])
    );
    for (filter, query, expected) in [
        ("disabled", "", "off"),
        ("enabled", " CHIEF ", "on"),
        ("all", "claude", "off"),
    ] {
        assert_eq!(
            p.eval(&format!(
                "HeyBossJobs.filterJobs({jobs},{},{}).map(j=>j.id)",
                json!(filter),
                json!(query)
            ))
            .unwrap(),
            json!([expected])
        );
    }
    let models = json!({"models":[{"selection":{"id":"exact","route":"openai"},"label":"Exact","advertised":true}]});
    assert_eq!(
        p.eval(&format!("HeyBossJobs.modelChoices({models},'custom')"))
            .unwrap(),
        json!([
        {"value":"custom","label":"custom · Not advertised","advertised":false},{"value":"openai/exact","label":"Exact","advertised":true}])
    );
    for (projects, harness, clock, available) in [
        (
            json!([{"id":"pool","projects":["p"]}]),
            "codex",
            100000,
            true,
        ),
        (json!([]), "codex", 100000, false),
        (json!([{"projects":["q"]}]), "codex", 100000, false),
        (json!([{"projects":["p"]}]), "claude", 100000, false),
        (json!([{"projects":["p"]}]), "codex", 200000, false),
    ] {
        let machine = json!({"state":"connected","heartbeat":100,"jobs":{"available":true,"workers":projects,"runtimes":{"codex":{"available":true}}},"workers":[{"intent":"pause","free":0}]});
        assert_eq!(
            p.eval(&format!(
                "HeyBossJobs.availability([{machine}],'p','{harness}',{clock})"
            ))
            .unwrap(),
            available
        );
    }
    for (job, expected) in [
        (
            json!({"enabled":true,"status":{"state":"pending","at":0}}),
            "Stalled",
        ),
        (
            json!({"enabled":true,"status":{"state":"pending","at":899000}}),
            "",
        ),
        (
            json!({"enabled":true,"status":{"state":"failed"}}),
            "Failed",
        ),
        (json!({"enabled":false,"status":{"state":"failed"}}), ""),
        (
            json!({"enabled":true,"active_run":{"state":"pending","created_at":1},"last_run":{"reason":"overlap","scheduled_at":400000},"status":{"state":"succeeded","finished_at":500000}}),
            "",
        ),
        (
            json!({"enabled":true,"active_run":{"state":"pending","created_at":1},"last_run":{"reason":"overlap","scheduled_at":550000},"status":{"state":"succeeded","finished_at":500000}}),
            "Stalled",
        ),
    ] {
        assert_eq!(
            p.eval(&format!("HeyBossUI.jobWarning({job},900000)"))
                .unwrap(),
            expected
        );
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn jobs_recover_committed_requests_without_duplicate_runs_or_revisions() {
    let f = fixture();
    let project = "Jobs Retry";
    f.init(project).unwrap();
    let _supervisor = agents::supervisor(&f).unwrap();
    let (_web, base) = web(&f, project);
    let job = |args: &[&str]| {
        let mut cmd = f.command(&["job", "--project", project, "--json"]);
        cmd.args(args);
        run(cmd).unwrap()
    };
    fs::write(
        f.root.path().join("instructions.md"),
        include_str!("../../src/jobs/fixtures/original.md"),
    )
    .unwrap();
    for (id, fallback) in [("secure", false), ("fallback", true)] {
        job(&[
            "create",
            "--id",
            id,
            "--name",
            id,
            "--cron",
            "0 9 1 JAN *",
            "--timezone",
            "UTC",
            "--harness",
            "codex",
            "--model",
            "fixture-model",
            "--instructions",
            "instructions.md",
        ]);
        let mut p = Page::open("about:blank");
        p.command("Page.enable", json!({})).unwrap();
        if fallback {
            p.command(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source":"Object.defineProperty(crypto,'randomUUID',{value:undefined})"}),
            )
            .unwrap();
        }
        // Keep writes real. Record the wire payload and discard just one reply,
        // after the server has committed, leaving the browser uncertain.
        p.command(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source":r#"const send=window.fetch;
          window.fetch=async(url,init)=>{
            const body=url==='/api/action' && JSON.parse(init.body);
            if(!body?.request_id) return send(url,init);
            const writes=JSON.parse(sessionStorage.getItem('sent')||'[]');
            writes.push(body); sessionStorage.setItem('sent',JSON.stringify(writes));
            const reply=await send(url,init);
            if(reply.ok && sessionStorage.getItem('lose')===body.operation.operation.command){
              sessionStorage.removeItem('lose');throw TypeError('Lost response');
            }
            return reply;
          };"#}),
        )
        .unwrap();
        p.command(
            "Page.navigate",
            json!({"url":format!("{base}/jobs#project=named%3AJobs%20Retry")}),
        )
        .unwrap();
        let switch = format!("document.querySelector('[data-job-toggle=\"{id}\"]')");
        p.wait(&format!(
            "{switch}?.getAttribute('aria-checked')==='true' && !{switch}.disabled"
        ));
        assert_eq!(
            p.eval("typeof crypto.randomUUID==='undefined'").unwrap(),
            fallback
        );
        p.eval(&format!(
            "sessionStorage.setItem('lose','set_enabled');{switch}.click()"
        ))
        .unwrap();
        p.wait("document.querySelector('#jobs-error').textContent==='Lost response' && !document.querySelector('#jobs-retry').hidden");
        let before = job(&["view", id]);
        assert_eq!(before["job"]["enabled"], false);
        let pending = p
            .eval("JSON.parse(sessionStorage.getItem('jobs-pending'))")
            .unwrap();
        let request_id = pending["request_id"].as_str().unwrap();
        let uuid = uuid::Uuid::parse_str(request_id).unwrap();
        assert_eq!(uuid.get_version_num(), 4);
        assert_eq!(uuid.get_variant(), uuid::Variant::RFC4122);
        p.eval("window.beforeReload=true").unwrap();
        p.command("Page.reload", json!({})).unwrap();
        p.wait(&format!("!window.beforeReload && !document.querySelector('#jobs-retry')?.hidden && {switch}?.disabled"));
        p.eval("document.querySelector('#job-retry').click()")
            .unwrap();
        p.wait(&format!("document.querySelector('#jobs-retry').hidden && {switch}?.getAttribute('aria-checked')==='false' && !{switch}.disabled"));
        assert_eq!(
            job(&["view", id])["job"]["revision"],
            before["job"]["revision"]
        );
        p.eval(&format!("{switch}.click()")).unwrap();
        p.wait(&format!(
            "{switch}?.getAttribute('aria-checked')==='true' && !{switch}.disabled"
        ));
        let writes = p
            .eval("JSON.parse(sessionStorage.getItem('sent'))")
            .unwrap();
        assert_eq!(writes[0], pending);
        assert_eq!(writes[1], pending);
        assert_ne!(writes[2]["request_id"], pending["request_id"]);
        assert_eq!(
            p.eval("sessionStorage.getItem('jobs-pending')").unwrap(),
            Value::Null
        );

        p.eval(&format!(
            "document.querySelector('.job-card[href$=\"job={id}\"]').click()"
        ))
        .unwrap();
        p.wait("document.querySelector('#job-run')?.disabled===false");
        p.eval(
            "sessionStorage.setItem('lose','run_now');document.querySelector('#job-run').click()",
        )
        .unwrap();
        p.wait("!document.querySelector('#jobs-retry').hidden && !document.querySelector('#job-retry').disabled");
        p.eval("document.querySelector('#job-retry').click()")
            .unwrap();
        p.wait("document.querySelector('#jobs-retry').hidden && document.querySelector('#job-stop')?.hidden===false");
        let writes = p
            .eval("JSON.parse(sessionStorage.getItem('sent'))")
            .unwrap();
        assert_eq!(writes[3]["operation"]["operation"]["command"], "run_now");
        assert_eq!(writes[4], writes[3]);
        assert_eq!(job(&["history", id])["runs"].as_array().unwrap().len(), 1);
    }
}
