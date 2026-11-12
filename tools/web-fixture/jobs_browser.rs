use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn jobs_calendar_groups_states_and_navigates_civil_dates() {
    let f = fixture();
    f.init("Jobs UI QA").unwrap();
    let (_web, base) = web(&f, "Jobs UI QA");
    let mut page = Page::open("about:blank");
    page.command(
        "Page.navigate",
        json!({"url":format!("{base}/jobs#project=named%3AJobs%20UI%20QA")}),
    )
    .unwrap();
    page.wait("typeof HeyBossCalendar !== 'undefined'");
    for (call, expected) in [
        (
            "groupSummary({scheduled:21,succeeded:2,failed:1})",
            json!({"state":"failed","text":"1 failed · 2 succeeded · 21 scheduled"}),
        ),
        (
            "groupSummary({scheduled:24})",
            json!({"state":"scheduled","text":"24 scheduled"}),
        ),
        (
            "groupSummary({running:1,skipped:2})",
            json!({"state":"running","text":"1 running · 2 skipped"}),
        ),
        (
            "range('2026-12-31','month')",
            json!({"start":"2026-11-30","days":35}),
        ),
        (
            "range('2027-01-01','week')",
            json!({"start":"2026-12-28","days":7}),
        ),
        (
            "range('2026-03-08','agenda')",
            json!({"start":"2026-03-02","days":7}),
        ),
        ("shift('2026-01-31','month',1)", json!("2026-02-01")),
        ("shift('2026-12-01','month',1)", json!("2027-01-01")),
        ("shift('2026-03-08','week',1)", json!("2026-03-15")),
        (
            "dayKey(Date.parse('2026-01-01T00:30:00Z'),'America/Chicago')",
            json!("2025-12-31"),
        ),
        (
            "dayKey(Date.parse('2026-01-01T00:30:00Z'),'Asia/Tokyo')",
            json!("2026-01-01"),
        ),
    ] {
        assert_eq!(
            page.eval(&format!("HeyBossCalendar.{call}")).unwrap(),
            expected,
            "{call}"
        );
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn job_thread_choice_persists_across_reloads() {
    let f = fixture();
    let project = "Jobs UI QA";
    f.init(project).unwrap();
    seed::seed(&f, Scenario::JobsUi, project).unwrap();
    let _supervisor = agents::supervisor(&f).unwrap();
    let (_web, base) = web(&f, project);
    let mut page = Page::open("about:blank");
    page.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error',e=>sessionStorage.setItem('pageError',e.message));addEventListener('unhandledrejection',e=>sessionStorage.setItem('pageError',String(e.reason)))"})).unwrap();
    page.command(
        "Page.navigate",
        json!({"url":format!("{base}/jobs#project=named%3AJobs%20UI%20QA")}),
    )
    .unwrap();
    page.wait("document.querySelector('#job-new')?.disabled === false");
    page.eval("document.querySelector('#job-new').click()")
        .unwrap();
    page.wait("document.querySelector('#job-thread-mode')?.value === 'new'");
    let markdown = json!(include_str!("../../src/jobs/fixtures/original.md"));
    page.eval(&format!("document.querySelector('#job-name').value='Thread mode check'; document.querySelector('#job-model').value='fixture-model'; document.querySelector('#job-markdown').value={markdown}; document.querySelector('#job-enabled').checked=false; document.querySelector('#job-save').click()")).unwrap();
    for (revision, mode) in [(1, "new"), (2, "resume"), (3, "new")] {
        if revision > 1 {
            page.eval(&format!("document.querySelector('#job-thread-mode').value='{mode}'; document.querySelector('#job-save').click()")).unwrap();
        }
        page.wait(&format!("document.querySelector('.job-panel-subtitle')?.textContent.startsWith('Revision {revision} ·')"));
        page.eval("window.beforeJobReload = true").unwrap();
        page.command("Page.reload", json!({})).unwrap();
        page.wait("!window.beforeJobReload && document.querySelector('#job-run') !== null && document.querySelector('#job-thread-mode') !== null");
        assert_eq!(
            page.eval("document.querySelector('#job-thread-mode').value")
                .unwrap(),
            mode
        );
        if mode == "resume" {
            for width in [1280, 390] {
                page.command(
                    "Emulation.setDeviceMetricsOverride",
                    json!({"width":width,"height":900,"mobile":false,"deviceScaleFactor":1}),
                )
                .unwrap();
                page.eval(
                    "document.querySelector('#job-thread-mode').scrollIntoView({block:'center'})",
                )
                .unwrap();
                page.wait("document.documentElement.scrollWidth <= innerWidth");
                screenshot(&mut page, "out/job-thread", &width.to_string());
            }
        }
    }
    assert_eq!(
        page.eval("sessionStorage.getItem('pageError')").unwrap(),
        Value::Null
    );
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn legacy_job_issues_show_job_ownership_and_run_state() {
    let f = fixture();
    let project = "Quick action QA";
    f.init(project).unwrap();
    let records = controls::jobs(&f, project).unwrap();
    let records = records["records"].as_array().unwrap();
    assert_eq!(records.len(), 5);
    let (_web, base) = web(&f, project);
    let mut page = Page::open("about:blank");
    page.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error', e => sessionStorage.setItem('pageError', e.message));addEventListener('unhandledrejection', e => sessionStorage.setItem('pageError', String(e.reason)))"})).unwrap();
    let list = format!("{base}/#project=named%3AQuick%20action%20QA");
    for width in [1440, 390] {
        page.command("Emulation.setDeviceMetricsOverride",json!({"width":width,"height":if width==390{844}else{1000},"deviceScaleFactor":1,"mobile":false})).unwrap();
        for r in records {
            let n = r["number"].as_i64().unwrap();
            let state = r["state"].as_str().unwrap();
            page.command("Page.navigate", json!({"url":format!("{list}&issue={n}")}))
                .unwrap();
            page.wait(&format!("model.detail?.issue.number === {n} && !!document.querySelector('.issue-readiness .readiness-status')"));
            assert_eq!(
                page.eval("document.querySelector('.issue-readiness h2').textContent")
                    .unwrap(),
                "Scheduled job"
            );
            let status = page
                .eval("document.querySelector('.readiness-status').textContent")
                .unwrap();
            assert_eq!(
                status,
                format!("{}{}", &state[..1].to_uppercase(), &state[1..])
            );
            let sidebar = page
                .eval("document.querySelector('.issue-readiness').innerText")
                .unwrap();
            let sidebar = sidebar.as_str().unwrap();
            assert!(
                sidebar.contains("independently of worker slots")
                    && !sidebar.contains("Ready for agents")
                    && !sidebar.contains("Move to draft"),
                "{sidebar}"
            );
            page.wait("document.body.innerText.includes('Unicode: żółw — 日本語.') && document.documentElement.scrollWidth <= innerWidth");
            if state == "pending" {
                page.wait("!document.querySelector('.issue-assignment').innerText.includes('Agent is working') && !document.querySelector('.issue-assignment .assignment-trace')");
            }
            page.eval(
                "document.querySelector('.issue-readiness').scrollIntoView({block:'center'})",
            )
            .unwrap();
            screenshot(&mut page, "out/scheduled-jobs", &format!("{width}-{state}"));
        }
        page.command("Page.navigate", json!({"url":list})).unwrap();
        page.wait("model.issues.length > 0 && !model.detail && document.body.innerText.includes('Scheduled job')");
        screenshot(&mut page, "out/scheduled-jobs", &format!("{width}-list"));
    }
    assert_eq!(
        page.eval("renderReadiness({issue:{job_run_id:'execution',deleted_at:1}})")
            .unwrap(),
        ""
    );
    assert_eq!(
        page.eval("sessionStorage.getItem('pageError')").unwrap(),
        Value::Null
    );
}
