//! PR and commit links in the shipped issue list, detail view and merge history.
use super::*;
use rusqlite::{Connection, params};

const PROJECT: &str = "Links";
const REPO: &str = "https://github.com/example/repo";

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn commit_attachments_survive_switching_to_a_pr_workflow() {
    let f = fixture();
    f.init(PROJECT).unwrap();
    run(f.issue(PROJECT, &["create", "--title", "Linked work"])).unwrap();
    let (_web, base) = web(&f, PROJECT);
    let list = format!("{base}/#project=named%3ALinks&view=issues&state=open&owner=all");
    let mut p = Page::open(&list);
    p.wait("!!document.querySelector('.issue-title')");
    assert_eq!(
        p.eval("document.querySelectorAll('.issue-commit-chip').length")
            .unwrap(),
        0
    );
    let db = Connection::open(&f.db).unwrap();
    let add = |i| {
        let sha = format!("abcdef{i}{}", "1".repeat(33));
        db.execute("INSERT INTO issue_commits(project_id,issue_number,sha,url,added_by,created_at) VALUES('named:Links',1,?1,?2,'codex:example',?3)",params![sha, format!("{REPO}/commit/{sha}"), i]).unwrap();
        sha
    };
    let mut shas = vec![add(0)];
    p.command("Page.reload", json!({})).unwrap();
    p.wait("document.querySelector('.issue-commit-link')?.textContent==='abcdef0'");
    assert_eq!(
        p.eval("document.querySelector('.issue-commits-more')===null")
            .unwrap(),
        true
    );
    for i in 1..6 {
        shas.push(add(i));
    }
    for prs in [false, true] {
        if prs {
            run(f.issue(PROJECT, &["settings", "set", "--prs-enabled"])).unwrap();
        }
        if prs {
            p.eval("document.querySelector('#detail-view [data-back]').click()")
                .unwrap();
            p.wait("model.route.issue === null && !!document.querySelector('.issue-title') && !document.querySelector('#list-view').hidden");
        }
        p.command("Page.reload", json!({})).unwrap();
        p.wait("document.querySelector('.issue-commit-link')?.textContent==='abcdef5'");
        assert_eq!(
            p.eval("document.querySelectorAll('.issue-commit-chip').length")
                .unwrap(),
            1
        );
        let more = p
            .eval("document.querySelector('.issue-commits-more').outerHTML")
            .unwrap();
        for text in [
            "+5 more",
            "data-issue=\"1\"",
            "project=named%3ALinks",
            "issue=1",
        ] {
            assert!(more.as_str().unwrap().contains(text), "{more}");
        }
        p.eval("document.querySelector('.issue-commits-more').click()")
            .unwrap();
        p.wait("document.querySelector('.commit-count')?.textContent==='6'");
        assert_eq!(
            p.eval("document.querySelector('.commit-target').checkVisibility()")
                .unwrap(),
            true
        );
        assert_eq!(
            p.eval("document.querySelector('.commit-summary').innerText.includes('Commits')")
                .unwrap(),
            true
        );
        assert_eq!(p.eval("!!document.querySelector('#pr-form')").unwrap(), prs);
        assert_eq!(
            p.eval("!!document.querySelector('#commit-form')").unwrap(),
            true
        );
        let targets = p
            .eval("[...document.querySelectorAll('.commit-target')].map(a=>a.href)")
            .unwrap();
        assert_eq!(
            targets,
            json!(
                shas.iter()
                    .rev()
                    .map(|s| format!("{REPO}/commit/{s}"))
                    .collect::<Vec<_>>()
            )
        );
        p.wait("document.querySelector('.commit-target').target==='_blank' && document.querySelector('.commit-trace-link').getAttribute('aria-label')==='Open agent trace for commit abcdef5'");
        assert_eq!(
            p.eval("document.querySelector('.issue-review a').href === document.querySelector('.commit-target').href && document.querySelector('.issue-review [data-action=ready]').checkVisibility() && !document.querySelector('.issue-review [data-remove-commit]')").unwrap(),
            true
        );
        let trace = p
            .eval("document.querySelector('.commit-trace-link').getAttribute('href')")
            .unwrap();
        assert!(
            trace.as_str().unwrap().starts_with("/agents/session#")
                && trace.as_str().unwrap().contains("agent=codex%3Aexample")
        );
        assert_eq!(p.eval("document.querySelector('.commit-section .commit-trace-row')===null && !/Deduplicated commits|View agent trace/.test(document.querySelector('.commit-section').textContent)").unwrap(),true);
    }
    run(f.issue(PROJECT, &["delete", "1"])).unwrap();
    p.command("Page.reload", json!({})).unwrap();
    p.wait("!!document.querySelector('[data-action=restore]') && document.querySelectorAll('.commit-target').length===6");
    assert_eq!(
        p.eval("!document.querySelector('#commit-form, [data-remove-commit]')")
            .unwrap(),
        true
    );
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn pr_purpose_and_status_are_distinct_in_the_list_and_details() {
    let f = fixture();
    f.init(PROJECT).unwrap();
    run(f.issue(PROJECT, &["create", "--title", "PR purposes"])).unwrap();
    let (_web, base) = web(&f, PROJECT);
    let mut p = Page::open(&format!(
        "{base}/#project=named%3ALinks&view=issues&state=open&owner=all"
    ));
    p.wait("!!document.querySelector('.issue-title')");
    let url = format!("{REPO}/pull/123");
    for (purpose, label) in [
        (None, None),
        (Some(Value::Null), None),
        (Some(json!("")), None),
        (Some(json!("unknown")), None),
        (Some(json!("unspecified")), None),
        (Some(json!("fix")), Some("Fix")),
        (Some(json!("prerequisite")), Some("Prerequisite")),
        (
            Some(json!("supporting-evidence")),
            Some("Supporting evidence"),
        ),
    ] {
        let mut pr = json!({"url":url});
        if let Some(v) = purpose {
            pr["purpose"] = v;
        }
        let issue = json!({"number":1,"pull_requests":[pr]});
        let html = p.eval(&format!("listPullRequests({issue})")).unwrap();
        let html = html.as_str().unwrap();
        if matches!(label, Some("Prerequisite" | "Supporting evidence")) {
            assert!(html.is_empty());
        } else {
            let suffix = label.map_or(String::new(), |s| format!(" · {s}"));
            assert!(
                html.contains(">#123</span>")
                    && html.contains(&format!("aria-label=\"Open pull request #123{suffix}\""))
                    && html.contains(&format!("title=\"{url}{suffix}\""))
            );
            assert_eq!(html.contains("pr-purpose-label"), label.is_some());
            assert!(!html.contains("Unspecified"));
        }
        let detail = p
            .eval(&format!(
                "renderPullRequests({{...{issue},deleted_at:1}},true)"
            ))
            .unwrap();
        assert!(detail.as_str().unwrap().contains(&format!(
            "\"pr-purpose-label\">{}</span>",
            label.unwrap_or("Unspecified")
        )));
    }
    assert_eq!(p.eval("listPullRequests({})").unwrap(), "");
    let empty = json!({"number":1});
    let linked = json!({"number":1,"pull_requests":[{"url":url,"purpose":"fix"}]});
    let deleted = json!({"number":1,"deleted_at":1,"pull_requests":linked["pull_requests"]});
    for issue in [&empty, &linked, &deleted] {
        assert_eq!(
            p.eval(&format!("renderPullRequests({issue},false)"))
                .unwrap(),
            ""
        );
    }
    assert!(
        p.eval(&format!("renderPullRequests({empty},true)"))
            .unwrap()
            .as_str()
            .unwrap()
            .contains("No pull requests attached")
    );
    for (issue, mutable) in [(&linked, true), (&deleted, false)] {
        let html = p
            .eval(&format!("renderPullRequests({issue},true)"))
            .unwrap();
        let html = html.as_str().unwrap();
        assert!(html.contains(&url));
        for control in ["id=\"pr-form\"", "data-pr-purpose=", "data-remove-pr="] {
            assert_eq!(html.contains(control), mutable);
        }
    }
    for (status, minutes, error, stale) in [
        ("open", 4, false, false),
        ("open", 8, false, true),
        ("closed", 20, false, false),
        ("closed", 33, false, true),
        ("merged", 1440, false, false),
        ("unknown", 0, false, false),
        ("open", 0, true, true),
    ] {
        let html=p.eval(&format!("listPullRequests({{pull_requests:[{{url:{},status:{},checked_at:Date.now()-{},error:{error}}}]}})",json!(url),json!(status),minutes*60000)).unwrap();
        assert!(
            html.as_str()
                .unwrap()
                .contains(&format!("pr-status-{status}"))
        );
        assert_eq!(html.as_str().unwrap().contains("pr-status-stale"), stale);
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn merged_prs_use_local_calendar_days_and_preserve_link_context() {
    let f = fixture();
    f.init(PROJECT).unwrap();
    run(f.issue(PROJECT, &["create", "--title", "Supporting evidence"])).unwrap();
    run(f.issue(PROJECT, &["settings", "set", "--prs-enabled"])).unwrap();
    let db = Connection::open(&f.db).unwrap();
    db.execute(
        "UPDATE global_settings SET github_user_id=42 WHERE id=1",
        [],
    )
    .unwrap();
    db.execute("INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at,purpose,status,author_id,pr_title,checked_at)
        VALUES('named:Links',1,?1,'human:qa',1,'supporting-evidence','merged',42,'<script>bad</script>',1000)",[format!("{REPO}/pull/7")]).unwrap();
    let (_web, base) = web(&f, PROJECT);
    let mut p = Page::open(&format!("{base}/merged-prs#project=named%3ALinks"));
    p.command(
        "Emulation.setTimezoneOverride",
        json!({"timezoneId":"America/Chicago"}),
    )
    .unwrap();
    p.wait("document.querySelector('.merge-title')?.textContent === '<script>bad</script>'");
    assert_eq!(
        p.eval("document.querySelector('.merge-title').href")
            .unwrap(),
        format!("{REPO}/pull/7")
    );
    p.wait("document.querySelector('.merge-time').textContent.startsWith('Observed') && !document.querySelector('#merged-list script')");
    let rows:Vec<_>=[Some(0),Some(2),Some(181),None].into_iter().enumerate().map(|(i,offset)|json!({
        "url":format!("{REPO}/pull/{}",i+1), "merged_at":offset.map(|n|1_790_744_340_000_i64+n*60000)
    })).collect();
    let groups = p
        .eval(&format!(
            "HeyBossMergedPRs.groups({},new Date('2026-09-30T12:00:00Z'))",
            json!(rows)
        ))
        .unwrap();
    let groups = groups.as_array().unwrap();
    assert_eq!(groups.len(), 3);
    for (group, label, urls) in [
        (0, "Today", vec![3, 2]),
        (1, "Yesterday", vec![1]),
        (2, "Merge date unavailable", vec![4]),
    ] {
        assert_eq!(groups[group]["label"], label);
        assert_eq!(
            groups[group]["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| &r["url"])
                .collect::<Vec<_>>(),
            urls.iter()
                .map(|n| &rows[*n - 1]["url"])
                .collect::<Vec<_>>()
        );
    }
    let records = json!([{"url":format!("{REPO}/pull/7"),"title":"<script>bad</script>","observed_at":1000,"issues":[{"number":3,"title":"Fix"}]}]);
    let rendered = p
        .eval(&format!(
            "HeyBossMergedPRs.render({records},{{project:'github.com/example/repo',host:'box'}})"
        ))
        .unwrap();
    let html = rendered.as_str().unwrap();
    for expected in [
        "&lt;script&gt;bad&lt;/script&gt;",
        "1 PR",
        "example/repo#7",
        "issue=3",
        "host=box",
        "Observed",
    ] {
        assert!(html.contains(expected), "{expected}");
    }
    assert!(!html.contains("<script>"));
    assert_eq!(p.eval("new DOMParser().parseFromString(HeyBossMergedPRs.render([{url:'javascript:void(0)',issues:[]}],{}),'text/html').querySelector('a')===null").unwrap(),true);
}
