//! Exercise the shipped list and detail renderers on the real Issues page.
use super::*;

const PR: &str = "https://github.com/o/r/pull/1";
const OTHER: &str = "https://github.com/o/r/pull/2";
const HELPERS: &str = "{actorName,icon,bossName:model.boss.name}";

fn with_page(check: impl FnOnce(&mut Page)) {
    let f = fixture();
    f.init("Assignment QA").unwrap();
    let (_web, base) = web(&f, "Assignment QA");
    let mut page = Page::open(&format!("{base}/#project=named%3AAssignment%20QA"));
    page.wait("model.project?.id === 'named:Assignment QA'");
    page.eval("HeyBossUI.rememberActors({actor_models:{'codex:session':'gpt-6-astra'}})")
        .unwrap();
    check(&mut page);
}

fn issue(assignment: Value) -> Value {
    json!({"number":7,"state":"open","version":1,"pull_requests":[{"url":PR}],"assignment":assignment})
}
fn html(page: &mut Page, expr: String) -> String {
    page.eval(&expr).unwrap().as_str().unwrap().to_owned()
}
fn render(page: &mut Page, issue: &Value) -> String {
    html(
        page,
        format!(
            "IssueAssignments.render({{issue:{issue},project:model.project,assignment_machines:[{{id:'box',name:'Devbox'}}]}},{HELPERS})"
        ),
    )
}
fn status(page: &mut Page, issue: &Value) -> String {
    html(page, format!("IssueAssignments.status({issue},{HELPERS})"))
}
fn badge(page: &mut Page, issue: &Value) -> String {
    html(page, format!("listAssignment({issue})"))
}
#[track_caller]
fn shows(html: &str, present: &[&str], absent: &[&str]) {
    for text in present {
        assert!(html.contains(text), "missing {text:?} in {html}");
    }
    for text in absent {
        assert!(!html.contains(text), "unexpected {text:?} in {html}");
    }
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn assignment_controls_distinguish_owner_destination_and_stopped_watchers() {
    with_page(|page| {
        for (a, label, hint, selected) in [
            (
                json!({"kind":"machine","machine":"box","machine_name":"Devbox"}),
                "Devbox",
                "Waiting for an agent",
                "machine:box",
            ),
            (
                json!({"kind":"agent","actor":"codex:session","machine_name":"Devbox"}),
                "Codex · gpt-6-astra",
                "Working on Devbox",
                "active",
            ),
            (
                json!({"kind":"github","waiting":true}),
                "GitHub PR watcher",
                "Waiting for new GitHub findings.",
                "github",
            ),
            (
                json!({"kind":"github","waiting":false}),
                "Unassigned",
                "queued",
                "active",
            ),
            (
                json!({"kind":"github","actor":"codex:session","machine_name":"Devbox"}),
                "Codex · gpt-6-astra",
                "working on Devbox",
                "active",
            ),
            (
                json!({"kind":"github","waiting":false,"machine":"box","machine_name":"Devbox"}),
                "Devbox",
                "queued",
                "machine:box",
            ),
        ] {
            let i = issue(a);
            let description = page
                .eval(&format!("IssueAssignments.describe({i},{HELPERS})"))
                .unwrap();
            assert_eq!(description["label"], label);
            shows(description["detail"].as_str().unwrap(), &[hint], &[]);
            let output = render(page, &i);
            let doc = format!(
                "new DOMParser().parseFromString({},'text/html')",
                json!(output)
            );
            assert_eq!(
                page.eval(&format!("{doc}.querySelector('select').value"))
                    .unwrap(),
                selected
            );
            assert_eq!(
                page.eval(&format!(
                    "{doc}.querySelector('option:checked').textContent"
                ))
                .unwrap(),
                label
            );
            if i["assignment"]["kind"] == "github" {
                shows(
                    &output,
                    &[
                        "value=\"github\"",
                        "data-action=\"refresh_github\"",
                        "Unassigned (stop monitoring)",
                    ],
                    &[],
                );
            }
            if !i["assignment"]["actor"].is_null() {
                shows(&output, &["agent=codex%3Asession"], &[]);
            }
        }
        let mut i = issue(json!({"kind":"boss"}));
        i["pull_requests"] = json!([]);
        shows(
            &render(page, &i),
            &[
                "data-assignment-select",
                "value=\"boss\" selected",
                "value=\"github\" disabled",
                "Attach a PR",
                "value=\"machine:box\"",
            ],
            &["Fleet allocation", "Release reservation"],
        );
        for pr in [
            json!({"url":"https://example.com/o/r/pull/1"}),
            json!({"url":"https://github.com/o/r/pull/0"}),
            json!({"url":"https://github.com/../r/pull/1"}),
            json!({"url":"https://github.com/o/./pull/1"}),
            json!({"url":"https://github.com/o/r/pull/18446744073709551616"}),
            json!({"url":PR,"status":"closed"}),
        ] {
            i["pull_requests"] = json!([pr]);
            shows(&render(page, &i), &["value=\"github\" disabled"], &[]);
        }
        i = issue(json!({"kind":"unassigned"}));
        i["attempt_hold"] = json!({"attempt_id":"retained"});
        let output = render(page, &i);
        shows(
            &output,
            &["Pickup is paused until the retained attempt is reconciled"],
            &["An available machine can pick this up"],
        );
        assert!(
            output
                .split("<select ")
                .nth(1)
                .unwrap()
                .split('>')
                .next()
                .unwrap()
                .contains(" disabled")
        );
        let mut i = issue(json!({"kind":"github","waiting":true}));
        shows(
            &render(page, &i),
            &[
                "popovertarget=\"assignment-help\"",
                "id=\"assignment-help\"",
                "popover",
                "Awaiting first fetch",
            ],
            &["<p>Waiting for required", "Not recorded yet", "Last fetch"],
        );
        assert_eq!(status(page, &i), "");
        i["state"] = json!("closed");
        i["github_status"] =
            json!({"monitoring":false,"stopped_reason":"no_open_pull_requests","prs":{}});
        shows(
            &render(page, &i),
            &["Monitoring stopped"],
            &["data-action=\"refresh_github\""],
        );
        shows(
            &status(page, &i),
            &["No open GitHub pull requests remain"],
            &["Waiting for the first GitHub status"],
        );
    });
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn github_status_separates_confirmed_failures_from_missing_or_stale_evidence() {
    with_page(|page| {
        let mut i = issue(json!({"kind":"github"}));
        let failure = json!([{"context":"tests","state":"failure"}]);
        for (evidence, present, absent) in [
            (
                json!({"conflicts":"conflicting","complete":false}),
                vec!["github-check-summary failed\">Merge conflicts"],
                vec![],
            ),
            (
                json!({"conflicts":"conflicting","required":failure}),
                vec!["Merge conflicts"],
                vec![],
            ),
            (
                json!({"conflicts":"conflicting","policy_errors":[{"message":"Policy unavailable"}]}),
                vec!["Merge conflicts", "Policy unavailable"],
                vec![],
            ),
            (
                json!({"conflicts":"conflicting","sources_match":false}),
                vec!["Refreshing changed pull request"],
                vec![">Merge conflicts"],
            ),
            (
                json!({"complete":true,"ci_settled":true,"ci_complete":false,"has_checks":false,"reviews":[{"body":"Please fix the race"}]}),
                vec!["No checks reported", "Please fix the race"],
                vec!["All checks finished", "Checks in progress"],
            ),
            (
                json!({"required":failure,"policy_errors":[{"message":"Policy access denied <retry>"}]}),
                vec![
                    "1 required check failed (policy incomplete)",
                    "Policy access denied &lt;retry&gt;",
                ],
                vec!["<retry>", "All checks finished"],
            ),
            (
                json!({"sources_match":false,"required":failure}),
                vec![
                    "Refreshing changed pull request",
                    "Last recorded required checks",
                ],
                vec![
                    "1 required check failed",
                    "All checks finished",
                    "github-check-summary failed",
                ],
            ),
            (
                json!({"truncated":true,"omitted":{"checks":80,"reviews":7}}),
                vec!["80 checks", "7 reviews", "GitHub"],
                vec![],
            ),
            (
                json!({"truncated":true,"omitted":{"required":236},"required_counts":{"total":300,"failure":200,"satisfied":100},"required":failure}),
                vec!["200 required checks failed", "236 required checks omitted"],
                vec![],
            ),
        ] {
            i["github_status"] = json!({"prs":{(PR):{"evidence":evidence}}});
            shows(&status(page, &i), &present, &absent);
        }
        i["github_status"] = json!({"omitted_prs":6,"prs":{(PR):{"evidence":{"truncated":true,"omitted":{"source_errors":2}}}}});
        shows(
            &status(page, &i),
            &[
                "6 additional pull requests",
                "Status incomplete",
                "2 source errors",
            ],
            &[],
        );
        i["github_status"] = json!({"error":"GitHub unavailable","prs":{}});
        shows(
            &status(page, &i),
            &["Status unavailable"],
            &["All checks finished", "Ready to merge"],
        );
        i["github_status"] = json!({"prs":{(PR):{"error":"Rate limited","evidence":{"complete":true}},(OTHER):{"evidence":{"complete":false}}}});
        shows(
            &status(page, &i),
            &["Rate limited", "Status unavailable"],
            &["All checks finished"],
        );
        i["github_status"] =
            json!({"prs":{(PR):{"error":"Review access denied","evidence":{"required":failure}}}});
        shows(
            &status(page, &i),
            &["1 required check failed", "Review access denied"],
            &[],
        );
        i = issue(json!({"kind":"machine","machine":"bad","machine_name":"<img src=x>"}));
        i["github_status"] = json!({"prs":{"javascript:bad()":{"evidence":{"required":[{"context":"<script>bad()</script>","state":"failure"}],"reviews":[{"body":"<img onerror=bad()>","html_url":"javascript:bad()"}]}}}});
        shows(&render(page, &i), &["&lt;img"], &["<img"]);
        shows(
            &status(page, &i),
            &["&lt;script", "&lt;img"],
            &["<img", "<script", "href=\"javascript:"],
        );
    });
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn github_fetch_controls_show_each_pr_and_allow_retry_after_interruption() {
    with_page(|page| {
        let now = page.eval("Date.now()").unwrap().as_i64().unwrap();
        let mut i = issue(json!({"kind":"github","waiting":true}));
        i["github_status"] = json!({"monitoring":true,"fetches":{(PR):{"finished_at":now-60000,"next_at":now+30000}}});
        shows(
            &render(page, &i),
            &[
                "Last fetch",
                "<time datetime=",
                "data-action=\"refresh_github\"",
                "Fetch now",
            ],
            &[],
        );
        i["github_status"]["fetches"][PR] = json!({"requested_at":now});
        shows(
            &render(page, &i),
            &["Fetch queued", "refresh_github\" disabled"],
            &[],
        );
        i["assignment"] = json!({"kind":"github","actor":"codex:session","machine_name":"Devbox"});
        i["github_status"]["fetches"][PR] =
            json!({"started_at":now-180000,"finished_at":now-240000,"error":"retry <unsafe>"});
        shows(
            &render(page, &i),
            &[
                "working on Devbox",
                "Fetch interrupted",
                "retry &lt;unsafe&gt;",
            ],
            &["refresh_github\" disabled", "<unsafe>"],
        );
        i["pull_requests"] = json!([{"url":PR},{"url":OTHER}]);
        i["github_status"]["fetches"][OTHER] =
            json!({"finished_at":now-200000,"next_at":now+300000,"error":"Rate limited"});
        let output = render(page, &i);
        shows(
            &output,
            &["PR #1", "PR #2", "Retry after", "Rate limited"],
            &[],
        );
        assert_eq!(output.matches("Last fetch").count(), 2);
    });
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn badges_link_to_current_ownership_and_real_agent_sessions() {
    with_page(|page| {
        page.eval("Object.assign(model.route,{host:'devbox',search:'keep',label:'bug',owner:'all'}); void 0").unwrap();
        for (a, owner, symbol) in [
            (json!({"kind":"boss"}), "human%3Aboss", "hat"),
            (
                json!({"kind":"machine","machine":"box","machine_name":"Devbox"}),
                "machine%3Abox",
                "monitor",
            ),
            (
                json!({"kind":"agent","actor":"codex:session"}),
                "codex%3Asession",
                "codex",
            ),
            (
                json!({"kind":"github","waiting":true}),
                "watcher%3Agithub",
                "pull-request",
            ),
        ] {
            let i = issue(a);
            let output = badge(page, &i);
            shows(
                &output,
                &[
                    "class=\"assignment-badge\"",
                    &format!("data-assignment-icon=\"{symbol}\""),
                    "popover=\"manual\"",
                    &format!("owner={owner}"),
                    "host=devbox",
                    "search=keep",
                    "Filter by",
                ],
                &[],
            );
            match symbol {
                "codex" => shows(&output, &["Codex · gpt-6-astra", "Open agent"], &[]),
                "pull-request" => shows(&output, &["data-open-watcher=\"7\""], &[]),
                _ => (),
            }
        }
        let mut i = issue(json!({"kind":"github","actor":"codex:session"}));
        shows(
            &badge(page, &i),
            &[
                "data-assignment-kind=\"agent\"",
                "data-assignment-icon=\"codex\"",
                "owner=codex%3Asession",
                "agent=codex%3Asession",
                "Open agent",
            ],
            &["Filter by GitHub PR watcher"],
        );
        i["assignment"] = json!({"kind":"github","waiting":false});
        shows(
            &badge(page, &i),
            &[
                "data-assignment-kind=\"unassigned\"",
                "owner=unassigned",
                ">Unassigned</strong>",
                "data-open-watcher=\"7\"",
            ],
            &["Open agent conversation", "Filter by GitHub PR watcher"],
        );
        i = issue(json!({"kind":"machine","machine":"box","machine_name":"<img src=x>"}));
        shows(&badge(page, &i), &["&lt;img"], &["<img"]);
        i = issue(json!({"kind":"unassigned"}));
        assert_eq!(badge(page, &i), "");
        for state in ["open", "closed"] {
            i["state"] = json!(state);
            for field in [
                json!({"closed_by":"codex:old"}),
                json!({"commits":[{"added_by":"codex:old"}]}),
            ] {
                let mut old = i.clone();
                old.as_object_mut()
                    .unwrap()
                    .extend(field.as_object().unwrap().clone());
                assert_eq!(badge(page, &old), "");
            }
        }
        i = issue(json!({"kind":"agent","actor":"job:run"}));
        i["job_run_id"] = json!("run");
        shows(
            &badge(page, &i),
            &["Scheduled job", "dedicated job service"],
            &[
                "Agent is working",
                "Open agent conversation",
                "agents/session",
            ],
        );
        shows(
            &render(page, &i),
            &[],
            &["Agent conversation", "agents/session"],
        );
        i["assignment"] = json!({"kind":"unassigned"});
        i["closed_by"] = json!("job:run");
        assert_eq!(badge(page, &i), "");
        shows(
            &render(page, &i),
            &["dedicated job service"],
            &["pick this up"],
        );
        i["assignment"] = json!({"kind":"agent","actor":"codex:exact-session"});
        shows(&badge(page, &i), &["agent=codex%3Aexact-session"], &[]);
        i = issue(json!({"kind":"unassigned"}));
        i["remote_attempts"] = json!([{"id":"run","actor":"codex:remote","machine":"remote","host":"Devbox <2>","started_at":12}]);
        shows(
            &badge(page, &i),
            &[
                "data-assignment-kind=\"agent\"",
                "Devbox &lt;2&gt;",
                "agent=codex%3Aremote",
                "class=\"assignment-activity\"",
                "data-assignment-agent=\"codex:remote\"",
                "Elapsed time",
            ],
            &["Filter by", "<2>"],
        );
        i["state"] = json!("closed");
        assert_eq!(badge(page, &i), "");
        i["state"] = json!("open");
        i["assignment"] = json!({"kind":"agent","actor":"codex:local"});
        shows(
            &badge(page, &i),
            &["agent=codex%3Alocal"],
            &["agent=codex%3Aremote"],
        );
    });
}
