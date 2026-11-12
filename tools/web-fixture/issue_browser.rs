//! Exercise the shipped issue UI, including current and historical assignment data.
use super::*;
use rusqlite::Connection;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn watcher_from_assignment_card_preserves_filters_and_fetch_state() {
    let f = fixture();
    let p = "Assignment QA";
    f.init(p).unwrap();
    seed::seed(&f, Scenario::AssignmentWatch, p).unwrap();
    let (_web, base) = web(&f, p);
    let url = format!("{base}/#project=named%3AAssignment%20QA&view=issues&state=open&owner=all");
    let mut page = Page::open(&url);
    let open = |page: &mut Page, n: i64| {
        let badge = format!("[data-assignment-card=assignment-card-{n}]");
        page.wait(&format!("!!document.querySelector('{badge}')"));
        page.eval(&format!("document.querySelector('{badge}').focus()"))
            .unwrap();
        page.wait(&format!(
            "document.getElementById('assignment-card-{n}').matches(':popover-open')"
        ));
        page.eval(&format!(
            "document.querySelector('[data-open-watcher=\"{n}\"]').click()"
        ))
        .unwrap();
        page.wait("document.querySelector('.github-watcher-dialog').open && !!document.querySelector('.github-watcher-dialog .github-fetch')");
    };
    open(&mut page, 7);
    assert_eq!(page.eval("location.href").unwrap(), url);
    let text = page
        .eval("document.querySelector('.github-watcher-dialog').innerText")
        .unwrap();
    for expected in ["Last fetch", "Retry after", "GitHub rate limit reached"] {
        assert!(text.as_str().unwrap().contains(expected), "{text}");
    }
    page.eval("document.querySelector('.github-fetch-now').click()")
        .unwrap();
    page.wait("document.querySelector('.github-fetch-now')?.disabled && document.querySelector('.github-fetch-state')?.textContent === 'Fetch queued'");
    assert!(Connection::open(&f.db).unwrap().query_row("SELECT requested_at IS NOT NULL FROM github_fetch_status WHERE url='https://github.com/example/project/pull/7'",[],|r|r.get::<_,bool>(0)).unwrap());
    for kind in ["keyDown", "keyUp"] {
        page.command(
            "Input.dispatchKeyEvent",
            json!({"type":kind,"key":"Escape","code":"Escape","windowsVirtualKeyCode":27}),
        )
        .unwrap();
    }
    page.wait("!document.querySelector('.github-watcher-dialog').open && document.activeElement.dataset.assignmentCard==='assignment-card-7'");
    open(&mut page, 3);
    let href = page
        .eval("document.querySelector('.github-watcher-dialog .assignment-detail + p a').href")
        .unwrap();
    assert!(href.as_str().unwrap().contains("agent=codex%3Afixture"));
    page.eval("document.querySelector('.github-pr-status details > summary').click()")
        .unwrap();
    page.wait("document.querySelector('.github-checks a')?.checkVisibility() && document.querySelector('.github-checks').innerText.includes('Unit tests')");
    // Change the evidence between polls; the newly rendered disclosure must stay open.
    Connection::open(&f.db).unwrap().execute(r#"UPDATE issue_github_watches SET status= json_set(status,'$.prs."https://github.com/example/project/pull/3".evidence.required[0].context','Unit tests (updated)') WHERE issue_number=3"#,[]).unwrap();
    page.wait("document.querySelector('.github-checks a')?.checkVisibility() && document.querySelector('.github-checks').innerText.includes('Unit tests (updated)')");
    page.eval("document.querySelector('.github-watcher-dialog [data-close]').click()")
        .unwrap();
    open(&mut page, 6);
    page.eval("document.querySelector('.github-feedback summary').click()")
        .unwrap();
    page.wait("document.querySelector('.github-feedback article')?.checkVisibility() && document.querySelector('.github-feedback article').innerText.includes('Please handle a connection drop')");
    for theme in ["light", "dark"] {
        page.command(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
        )
        .unwrap();
        for width in [1440, 390, 320] {
            page.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":800,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            page.wait("Array.from(document.querySelectorAll('.github-watcher-dialog[open]')).some(el=>{const r=el.getBoundingClientRect();return r.left>=0 && r.right<=innerWidth && r.top>=0 && r.bottom<=innerHeight && el.scrollWidth<=el.clientWidth})");
            if width == 390 {
                screenshot(&mut page, "out/issue-browser", &format!("watcher-{theme}"));
            }
        }
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn issue_work_context_and_readiness_use_the_actual_client() {
    let f = fixture();
    let p = "Issue Design QA";
    f.init(p).unwrap();
    issue_design::architecture(&f, p).unwrap();
    let (_web, base) = web(&f, p);
    let mut page = Page::open(&format!(
        "{base}/#project=named%3AIssue%20Design%20QA&issue=2"
    ));
    page.wait("document.querySelector('.issue-work [data-draft-action]') !== null");
    assert_eq!(
        page.eval("document.querySelector('.issue-work').getAttribute('aria-label')")
            .unwrap(),
        "Work"
    );
    for selector in [
        ".issue-assignment",
        ".issue-readiness",
        ".tag-section",
        ".issue-blockers",
    ] {
        assert_eq!(
            page.eval(&format!(
                "!!document.querySelector('.issue-work {selector}')"
            ))
            .unwrap(),
            true
        );
    }
    assert_eq!(
        page.eval("document.querySelector('.issue-context').open")
            .unwrap(),
        false
    );
    assert_eq!(
        page.eval("document.querySelector('.issue-context > summary').textContent")
            .unwrap(),
        "Issue details"
    );
    assert_eq!(page.eval("!!document.querySelector('.issue-context [data-issue-version]') && !document.querySelector('.issue-work [data-issue-version],.issue-work [data-action=delete]')").unwrap(),true);
    for (changes, agent_link) in [
        (json!({"closed_by":"human:boss"}), None),
        (
            json!({"closed_by":"codex:previous"}),
            Some("agent=codex%3Aprevious"),
        ),
        (
            json!({"commits":[{"added_by":"codex:previous"}]}),
            Some("agent=codex%3Aprevious"),
        ),
        (
            json!({"commits":[{"origin":{"kind":"codex","session_id":"recorded","host":"local","actor_id":"codex:previous"}}]}),
            Some("host=local&run=session%3Arecorded"),
        ),
    ] {
        let links=page.eval(&format!("new DOMParser().parseFromString(renderIssueContext({{...model.detail.issue,...{changes}}}), 'text/html').querySelector('a[href*=session]')?.getAttribute('href') || null")).unwrap();
        if let Some(expected) = agent_link {
            assert!(
                links.as_str().is_some_and(|link| link.contains(expected)),
                "{changes}: {links}"
            )
        } else {
            assert!(links.is_null())
        }
    }
    for (state, extra, enabled, draft) in [
        ("open", json!({}), true, true),
        ("blocked", json!({}), true, true),
        ("closed", json!({}), true, false),
        ("open", json!({"assignee":"codex:assigned"}), true, false),
        ("open", json!({"draft":true}), true, false),
        ("open", json!({}), false, false),
        ("open", json!({"deleted_at":1}), true, false),
    ] {
        let actual=page.eval(&format!("new DOMParser().parseFromString(renderReadiness({{issue:{{state:'{state}',...{extra}}}, drafts_enabled:{enabled}}}),'text/html').querySelector('[data-draft-action=draft]') !== null")).unwrap();
        assert_eq!(actual, draft, "{state} {extra} drafts_enabled={enabled}");
    }
    for number in [1, 2, 3, 4, 5, 6, 8] {
        page.command(
            "Page.navigate",
            json!({"url":format!("{base}/?qa={number}#project=named%3AIssue%20Design%20QA&issue={number}")}),
        )
        .unwrap();
        page.wait(&format!(
            "document.querySelector('.detail-number')?.textContent === '#{number}'"
        ));
        for theme in ["light", "dark"] {
            page.command("Emulation.setEmulatedMedia",json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).unwrap();
            for width in [1440, 768, 390, 320] {
                page.command(
                    "Emulation.setDeviceMetricsOverride",
                    json!({"width":width,"height":960,"deviceScaleFactor":1,"mobile":false}),
                )
                .unwrap();
                let parent = if width <= 640 {
                    ".detail-main"
                } else {
                    ".sidebar"
                };
                page.wait(&format!(
                    "!!document.querySelector('{parent} > .issue-work')"
                ));
                assert_eq!(page.eval("document.documentElement.scrollWidth <= innerWidth && !document.querySelector('.issue-context').open && !!document.querySelector('.issue-description + .issue-progress-card')").unwrap(),true,"issue {number} at {width}px");
                if width <= 640 {
                    assert_eq!(page.eval("['.issue-description','.issue-progress-card','.issue-work','.issue-discussion'].map(s=>document.querySelector(s).getBoundingClientRect()).every((r,i,a)=>!i || r.top>=a[i-1].bottom)").unwrap(),true,"issue {number} at {width}px");
                }
                if number == 1 && (width == 1440 || width == 390) {
                    screenshot(
                        &mut page,
                        "out/issue-browser",
                        &format!("issue-{theme}-{width}"),
                    );
                }
            }
        }
    }
    page.command(
        "Page.navigate",
        json!({"url":format!("{base}/#project=named%3AIssue%20Design%20QA&issue=1")}),
    )
    .unwrap();
    page.wait(
        "model.detail?.issue.number === 1 && !!document.querySelector('#comments .issue-activity')",
    );
    let draft = "An unsent finding survives opening supporting details.";
    page.eval(&format!("document.querySelector('#comment-body').value={};document.querySelector('#comment-body').dispatchEvent(new Event('input',{{bubbles:true}}))",json!(draft))).unwrap();
    for selector in [".issue-context > summary", ".pr-add:has(#pr-url) > summary"] {
        page.eval(&format!("document.querySelector('{selector}').focus()"))
            .unwrap();
        for kind in ["keyDown", "keyUp"] {
            page.command(
                "Input.dispatchKeyEvent",
                json!({"type":kind,"key":"Enter","code":"Enter","text":if kind=="keyDown" {"\r"}else{""},"windowsVirtualKeyCode":13}),
            )
            .unwrap();
        }
    }
    page.wait("document.querySelector('[data-issue-version]').checkVisibility() && document.querySelector('#pr-url').checkVisibility()");
    assert_eq!(
        page.eval("document.querySelector('#comment-body').value")
            .unwrap(),
        draft
    );
    page.eval("document.querySelector('.progress-history summary').click()")
        .unwrap();
    page.wait("document.querySelectorAll('.progress-history-list li').length === 20 && document.querySelector('.progress-history-more').checkVisibility()");
    page.eval("document.querySelector('#pr-url').value='https://github.com/example/hey-boss/pull/43';document.querySelector('#pr-purpose').value='supporting-evidence';document.querySelector('#pr-url').closest('form').requestSubmit()").unwrap();
    page.wait("!!document.querySelector('[data-pr-purpose$=\"/43\"]')");
    assert_eq!(
        page.eval("document.querySelector('#comment-body').value")
            .unwrap(),
        draft
    );
    page.eval("document.querySelector('#comment-submit').click()")
        .unwrap();
    page.wait(&format!("document.querySelector('#comment-body')?.value === '' && document.querySelector('#comments')?.textContent.includes({})",json!(draft)));
    page.eval("document.querySelector('.issue-overflow > summary').click()")
        .unwrap();
    page.wait("document.querySelector('.issue-overflow [data-action=delete]').checkVisibility()");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn timeline_groups_preserve_order_and_escape_event_data() {
    let f = fixture();
    let p = "Timeline QA";
    f.init(p).unwrap();
    issue_design::timeline(&f, p).unwrap();
    let (_web, base) = web(&f, p);
    let mut page = Page::open(&format!("{base}/#project=named%3ATimeline%20QA&issue=1"));
    page.wait("!!document.querySelector('#comments .activity-group')");
    assert_eq!(
        page.eval("document.querySelectorAll('#comments .activity-group[open]').length")
            .unwrap(),
        0
    );
    assert_eq!(
        page.eval("document.querySelectorAll('#comments > .comment-card').length")
            .unwrap(),
        2
    );
    assert_eq!(
        page.eval("document.querySelectorAll('#comments img').length")
            .unwrap(),
        0
    );
    let order=page.eval("Array.from(document.querySelector('#comments').children).map(n=>n.matches('.activity-group')?'group':n.dataset.commentId?'comment':null).filter(Boolean)").unwrap();
    assert_eq!(
        order,
        json!(["group", "comment", "group", "comment", "group"])
    );
    for (action, data, parts) in [
        (
            "assigned",
            json!({"target":"github","previous_assignee":"human:boss"}),
            &["GitHub PR watcher", "Boss"][..],
        ),
        (
            "claimed",
            json!({"assignee":"human:boss"}),
            &["assigned", "Boss"],
        ),
        (
            "unassigned",
            json!({"previous_assignee":"human:boss"}),
            &["unassigned", "Boss"],
        ),
        ("assigned", json!({"target":"machine:devbox"}), &["devbox"]),
        (
            "edited",
            json!({"before":{"labels":["old"],"title":"Old"},"after":{"labels":["new","<script>"],"title":"New"}}),
            &[
                "added", "new", "<script>", "removed", "old", "renamed", "Old", "New",
            ],
        ),
        (
            "edited",
            json!({"before":{"draft":true},"after":{"draft":false}}),
            &["ready for work"],
        ),
        (
            "triaged",
            json!({"before":{"assignee":"human:boss","labels":[]},"after":{"assignee":null,"labels":["bug"]}}),
            &["unassigned", "Boss", "added", "bug"],
        ),
        (
            "closed",
            json!({}),
            &["Codex", "astra", "closed this issue"],
        ),
        ("future_event", json!({}), &["updated this issue"]),
        (
            "worker_selected",
            json!({"worker_name":"Review worker"}),
            &["Review worker", "selected this issue"],
        ),
    ] {
        let actor = if action == "worker_selected" {
            "reservation:internal-id"
        } else {
            "codex:session"
        };
        let html=page.eval(&format!("issueActivity({{id:1,actor:'{actor}',created_at:1,action:'{action}',data:{{actor_model:'astra',...{data}}}}})")).unwrap();
        let text=page.eval(&format!("new DOMParser().parseFromString({html},'text/html').querySelector('summary').textContent")).unwrap();
        let mut rest = text.as_str().unwrap();
        for part in parts {
            rest = rest
                .split_once(part)
                .unwrap_or_else(|| panic!("missing {part} in {text}"))
                .1;
        }
        assert!(!html.as_str().unwrap().contains("<script>"));
        assert!(!text.as_str().unwrap().contains("reservation:internal-id"));
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn pagination_preserves_every_result_and_resets_only_changed_filters() {
    let f = fixture();
    f.init("Pages").unwrap();
    let (_web, base) = web(&f, "Pages");
    let mut page = Page::open(&format!(
        "{base}/#project=named%3APages&view=issues&state=open&owner=all"
    ));
    page.wait("!!model.signature");
    for value in [
        "undefined",
        "''",
        "'0'",
        "'-2'",
        "'1.5'",
        "'Infinity'",
        "'NaN'",
        "'9007199254740992'",
        "'3'",
    ] {
        assert_eq!(
            page.eval(&format!("HeyBossPagination.number({value})"))
                .unwrap(),
            if value == "'3'" { 3 } else { 1 }
        );
    }
    for total in [0_usize, 1, 999, 1000, 1001, 2000, 2005] {
        let pages = total.div_ceil(1000).max(1);
        let items = format!("Array.from({{length:{total}}},(_,i)=>i)");
        for current in 1..=pages {
            let result = page
                .eval(&format!("HeyBossPagination.page({items},{current})"))
                .unwrap();
            assert_eq!(result["current"], current);
            assert_eq!(result["pages"], pages);
            assert_eq!(result["total"], total);
            let expected: Vec<_> = ((current - 1) * 1000..(current * 1000).min(total)).collect();
            assert_eq!(result["items"], json!(expected));
        }
        assert_eq!(
            page.eval(&format!("HeyBossPagination.page({items},999).current"))
                .unwrap(),
            pages
        );
    }
    assert_eq!(
        page.eval("HeyBossPagination.page([2000,2001,2002,2003,2004],3).items[0]")
            .unwrap(),
        2000
    );
    // Keep the current route intact. Only construct the URL a detail link or filter would use.
    let lookup = |page: &mut Page, key: &str, change: Value| {
        page.eval(&format!(
        "new URLSearchParams(routeHash({{...model.route,page:3,inbox_page:2,...{change}}}).slice(1)).get({})",json!(key)
    )).unwrap()
    };
    for (key, change, expected) in [
        ("page", json!({"issue":2001}), json!("3")),
        ("inbox_page", json!({"notice":"notice-1001"}), json!("2")),
        ("page", json!({"page":1}), Value::Null),
        ("page", json!({"page":2}), json!("2")),
    ] {
        assert_eq!(lookup(&mut page, key, change), expected);
    }
    for (key, value) in [
        ("project", "named:Other"),
        ("host", "remote"),
        ("state", "closed"),
        ("search", "needle"),
        ("owner", "mine"),
        ("label", "bug"),
        ("blocked", "hold"),
        ("min_launches", "5"),
        ("draft", "1"),
        ("created_by", "human:boss"),
        ("inbox_state", "archive"),
        ("inbox_project", "Other"),
        ("inbox_search", "needle"),
    ] {
        let (reset, keep, expected) = if key.starts_with("inbox_") {
            ("inbox_page", "page", "3")
        } else {
            ("page", "inbox_page", "2")
        };
        assert_eq!(
            lookup(&mut page, reset, json!({key:value})),
            Value::Null,
            "{key}"
        );
        assert_eq!(
            lookup(&mut page, keep, json!({key:value})),
            expected,
            "{key}"
        );
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn blocked_issue_views_distinguish_holds_from_dependency_chains() {
    let f = fixture();
    f.init("Blockers").unwrap();
    let (_web, base) = web(&f, "Blockers");
    let mut page = Page::open(&format!(
        "{base}/#project=named%3ABlockers&view=issues&state=open&owner=all"
    ));
    page.wait("!!model.signature");
    for (issue, kind, hold, deps) in [
        (
            json!({"state":"blocked","manual_blocked":true}),
            "hold",
            true,
            false,
        ),
        (
            json!({"state":"blocked","blocked_by":[{"number":1}]}),
            "dependencies",
            false,
            true,
        ),
        (
            json!({"state":"blocked","manual_blocked":true,"blocked_by":[{"number":1}]}),
            "hold",
            true,
            true,
        ),
        (
            json!({"state":"open","manual_blocked":true}),
            "",
            false,
            false,
        ),
        (
            json!({"state":"blocked","manual_blocked":true,"deleted_at":1}),
            "",
            false,
            false,
        ),
    ] {
        let actual=page.eval(&format!("[IssueBlockers.kind({issue}), ...['hold','dependencies',''].map(filter=>IssueBlockers.matches({issue},filter))]")).unwrap();
        assert_eq!(actual, json!([kind, hold, deps, true]));
    }
    for (state, disabled) in [("closed", false), ("blocked", true)] {
        let issue = json!({"state":state,"blocked_by":[{"number":1}]});
        assert_eq!(page.eval(&format!("new DOMParser().parseFromString(issueStateActions({issue}),'text/html').querySelector('[data-action=reopen]').disabled")).unwrap(),disabled);
    }
    let chain = json!({"number":2,"title":"API layer","state":"open","blocked_by":[],"blocker_links":[{"number":1,"title":"Storage layer","state":"ready","satisfied":true,"pull_requests":[{"url":"https://github.com/example/repo/pull/101"}]}],"blocking":[{"number":3,"title":"Web UI","state":"blocked","actively_blocked":true,"unblocks_on_release":true}]});
    let html = page.eval(&format!("IssueBlockers.card({chain})")).unwrap();
    for expected in [
        "dep-chain-graph",
        "PR #101",
        "Unblocks on Ready/Close",
        "data-create-dependent=\"2\"",
    ] {
        assert!(html.as_str().unwrap().contains(expected), "{expected}");
    }
}
