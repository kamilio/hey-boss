//! Exercise the list controls on the page that ships them.
use super::*;

fn with_page(check: impl FnOnce(&mut Page)) {
    let f = fixture();
    f.init("List QA").unwrap();
    run(f.issue(
        "List QA",
        &["create", "--title", "Draft from another browser", "--draft"],
    ))
    .unwrap();
    let (_server, base) = web(&f, "List QA");
    let mut p = Page::open(&format!("{base}/#project=named%3AList%20QA"));
    p.wait("model.issues.some(i=>i.number===1)");
    check(&mut p);
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn filters_combine_threshold_owner_creator_and_blocked_reason() {
    with_page(|p| {
        for (input, expected) in [
            ("", ""),
            ("0", ""),
            ("-1", ""),
            ("1.5", ""),
            ("5x", ""),
            ("05", ""),
            ("9007199254740992", ""),
            ("5", "5"),
        ] {
            assert_eq!(
                p.eval(&format!(
                    "IssueFilters.read(new URLSearchParams({{min_launches:{}}})).min_launches",
                    json!(input)
                ))
                .unwrap(),
                expected
            );
        }
        for (state, expected) in [("open", ""), ("blocked", "hold")] {
            assert_eq!(
                p.eval(&format!(
                    "IssueFilters.read(new URLSearchParams('state={state}&blocked=hold')).blocked"
                ))
                .unwrap(),
                expected
            );
        }
        let cases = json!([
            {"state":"blocked","manual_blocked":true,"agent_launch_count":4},
            {"state":"blocked","manual_blocked":true,"agent_launch_count":5},
            {"state":"blocked","blocked_by":[{"number":9}],"agent_launch_count":8},
            {"state":"blocked","manual_blocked":true,"blocked_by":[{"number":9}],"agent_launch_count":10},
            {"state":"blocked","attempt_hold":{"active":true},"agent_launch_count":5},
            {"state":"open"},{"state":"blocked","manual_blocked":true,"agent_launch_count":8,"deleted_at":123}
        ]);
        let matches = |p: &mut Page, issues: &Value, query: &str| {
            p.eval(&format!("{issues}.map(i=>IssueFilters.matches(i,IssueFilters.read(new URLSearchParams({}))))",json!(query))).unwrap()
        };
        assert_eq!(
            matches(p, &cases, "state=blocked&min_launches=5&blocked=hold"),
            json!([false, true, false, true, true, false, false])
        );
        assert_eq!(
            matches(
                p,
                &cases,
                "state=blocked&min_launches=5&blocked=dependencies"
            ),
            json!([false, false, true, true, false, false, false])
        );
        let drafts = json!([{"draft":true,"created_by":"human:boss"},{"draft":false,"created_by":"human:boss"},{"draft":true,"created_by":"agent:other","assignee":"human:boss"},{}]);
        for (query, expected) in [
            ("draft=1", json!([true, false, true, false])),
            (
                "draft=1&created_by=human%3Aboss",
                json!([true, false, false, false]),
            ),
            ("draft=other", json!([true, true, true, true])),
            ("", json!([true, true, true, true])),
        ] {
            assert_eq!(matches(p, &drafts, query), expected);
        }
        for state in ["open", "blocked", "ready", "closed", "deleted", "all"] {
            let mut route = json!({"state":state,"owner":"all","min_launches":"","search":"keep","label":"bug"});
            let presets = p.eval(&format!("IssueFilters.presets({route})")).unwrap();
            let presets = presets.as_array().unwrap();
            assert_eq!(presets.len(), if state == "blocked" { 4 } else { 3 });
            for (key, expected) in [
                ("min_launches", "5"),
                ("owner", "human:boss"),
                ("min_launches", ""),
                ("owner", "all"),
            ] {
                if expected == "all" {
                    route["owner"] = json!("mine");
                }
                let option = presets.iter().find(|v| v["key"] == key).unwrap();
                let update = p
                    .eval(&format!("IssueFilters.toggle({route},{option})"))
                    .unwrap();
                assert_eq!(update, json!({(key):expected}));
                route[key] = update[key].clone();
                if key == "min_launches" && expected.is_empty() {
                    assert_eq!(route["owner"], "human:boss");
                }
            }
        }
    });
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn actions_match_issue_state_and_project_draft_setting() {
    with_page(|p| {
        for (fields, expected) in [
            (json!({}), "assign_boss draft close block"),
            (json!({"assignee":"human:boss"}), "unassign close block"),
            (
                json!({"assignee":"codex:other"}),
                "assign_boss unassign close block",
            ),
            (json!({"draft":true}), "undraft close block"),
            (json!({"state":"blocked"}), "reopen draft close"),
            (json!({"state":"blocked","draft":true}), "reopen close"),
            (json!({"state":"closed"}), "reopen"),
            (json!({"deleted_at":1}), "restore"),
            (
                json!({"state":"ready","assignee":"human:boss"}),
                "reopen unassign close",
            ),
            (json!({"state":"ready","draft":true}), "reopen close"),
            (
                json!({"state":"blocked","manual_blocked":true,"blocked_by":[{"number":2}]}),
                "clear_manual_hold draft close",
            ),
            (json!({"attempt_hold":{"host":"test-machine"}}), ""),
        ] {
            let result = p
                .eval(&format!(
                    "issueListActions({{state:'open',labels:[],...{fields}}}).map(a=>a.action)"
                ))
                .unwrap();
            let expected: Vec<_> = ["view", "copy"]
                .into_iter()
                .chain(expected.split_whitespace())
                .collect();
            assert_eq!(result, json!(expected), "{fields}");
        }
        for (fields, key, label, disabled) in [
            (json!({"state":"closed"}), "reopen", "Reopen issue", false),
            (
                json!({"state":"blocked","blocked_by":[{"number":2}]}),
                "reopen",
                "Unblock issue",
                true,
            ),
            (
                json!({"state":"blocked","manual_blocked":true}),
                "clear_manual_hold",
                "Release hold",
                false,
            ),
        ] {
            let a = p
                .eval(&format!(
                    "issueListActions({fields}).find(a=>a.action==='{}')",
                    key
                ))
                .unwrap();
            assert_eq!(a["label"], label);
            assert_eq!(a["disabled"].as_bool().unwrap_or(false), disabled);
        }
        p.eval("model.drafts_enabled=false").unwrap();
        assert_eq!(
            p.eval("issueListActions({state:'open'}).map(a=>a.action)")
                .unwrap(),
            json!(["view", "copy", "assign_boss", "close", "block"])
        );
        assert_eq!(
            p.eval("issueListActions({state:'open',draft:true}).some(a=>a.action==='undraft')")
                .unwrap(),
            true
        );
    });
}

// Capture only mutation requests. Reads still go through the fixture server.
// Rust sets the reply before each case; the page uses its real request and retry code.
fn record_writes(p: &mut Page) {
    p.eval(
        r#"window.writes=[]; window.writeReply={ok:true,issue:{state:'open'}};
        const originalFetch=window.fetch;
        window.fetch=(url,init)=>{
            const body=url==='/api/action' && JSON.parse(init.body);
            if(!body?.request_id) return originalFetch(url,init);
            writes.push(body);
            if(writeReply===null) return Promise.reject(new TypeError('Disconnected'));
            return Promise.resolve(Response.json(writeReply,{status:writeReply.ok?200:409}));
        }; void 0"#,
    )
    .unwrap();
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn list_mutations_keep_the_captured_target_version_and_owner_confirmation() {
    with_page(|p| {
        p.eval("location.hash='project=named%3AList%20QA&issue=1'")
            .unwrap();
        p.wait("model.detail?.issue.draft && !!document.querySelector('#comment-body')");
        record_writes(p);
        p.eval("window.targetRow={issue:{number:12,version:7,assignee:'codex:worker'},project:'named:Original',host:'original-host',sequence:-1,trigger:document.createElement('button')}; document.querySelector('#comment-body').value='Unrelated draft'").unwrap();
        for (action, confirm, operation) in [
            (
                "unassign",
                Some(true),
                json!({"action":"unassign","number":12,"force":true}),
            ),
            ("close", Some(false), Value::Null),
            (
                "reopen",
                None,
                json!({"action":"reopen","number":12,"if_version":7}),
            ),
            (
                "block",
                Some(true),
                json!({"action":"block","number":12,"comment":null,"force":false}),
            ),
            (
                "clear_manual_hold",
                None,
                json!({"action":"reopen","number":12,"if_version":7,"clear_manual_hold":true}),
            ),
            (
                "undraft",
                None,
                json!({"action":"undraft","number":12,"if_version":7}),
            ),
        ] {
            if action == "block" {
                p.eval("targetRow.issue.assignee=null").unwrap();
            }
            p.eval(&format!(
                "writes.length=0;window.operationDone=runListAction('{action}',targetRow);void 0"
            ))
            .unwrap();
            if let Some(yes) = confirm {
                p.wait("document.querySelector('#confirm-dialog').open");
                assert_eq!(p.eval("writes.length").unwrap(), 0);
                p.eval(if yes {
                    "document.querySelector('#confirm-submit').click()"
                } else {
                    "document.querySelector('#confirm-cancel').click()"
                })
                .unwrap();
            }
            p.eval("operationDone").unwrap();
            let calls = p.eval("writes").unwrap();
            if operation.is_null() {
                assert_eq!(calls, json!([]));
                continue;
            }
            assert_eq!(calls.as_array().unwrap().len(), 1);
            assert_eq!(calls[0]["project"], "named:Original");
            assert_eq!(calls[0]["host"], "original-host");
            assert_eq!(calls[0]["operation"], operation);
            assert_eq!(p.eval("model.project.id").unwrap(), "named:List QA");
            assert_eq!(
                p.eval("document.querySelector('#toast').hidden").unwrap(),
                true,
                "a different project must not announce success"
            );
        }
        p.eval("writeReply=null").unwrap();
        let lost = p
            .eval("performAction('close',targetRow.trigger,targetRow)")
            .unwrap_err();
        assert!(lost.contains("Connection lost"), "{lost}");
        p.wait(
            "model.detail?.issue.draft && !!document.querySelector('[data-draft-action=ready]')",
        );
        let version = p.eval("model.detail.issue.version").unwrap();
        p.eval("writes.length=0;document.querySelector('[data-draft-action=ready]').click()")
            .unwrap();
        p.wait("writes.length===1 && !document.querySelector('#draft-error').hidden");
        assert_eq!(
            p.eval("writes[0].operation").unwrap(),
            json!({"action":"undraft","number":1,"if_version":version})
        );
    });
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn mutation_retries_reuse_only_uncertain_requests_and_keep_discussion_in_scope() {
    with_page(|p| {
        record_writes(p);
        for code in [
            "fleet_reserved",
            "fleet_allocation_expired",
            "fleet_allocation_missing",
            "network_error",
        ] {
            let unknown = code == "network_error";
            let reply = if unknown {
                Value::Null
            } else {
                json!({"ok":false,"error":{"code":code,"message":code}})
            };
            p.eval(&format!("window.writeReply={reply}; writes.length=0"))
                .unwrap();
            let call = "mutate({action:'edit',number:1,draft:true})";
            let error = p.eval(call).unwrap_err();
            assert!(
                error.contains(if unknown { "Connection lost" } else { code }),
                "{error}"
            );
            assert_eq!(
                p.eval("pendingMutation.size").unwrap(),
                usize::from(unknown)
            );
            p.eval("writeReply={ok:true}").unwrap();
            assert_eq!(p.eval(call).unwrap()["ok"], true);
            let ids = p.eval("writes.map(w=>w.request_id)").unwrap();
            assert_eq!(ids.as_array().unwrap().len(), 2);
            assert!(ids[0].is_string() && ids[1].is_string());
            assert_eq!(ids[0] == ids[1], unknown);
            assert_eq!(p.eval("pendingMutation.size").unwrap(), 0);
        }
        // Start with the real detail page. A tunnel mutation only returns the
        // updated issue; null sections in that reply must not erase local detail.
        p.eval("location.hash='project=named%3AList%20QA&issue=1'")
            .unwrap();
        p.wait("model.detail?.issue.number===1");
        p.eval("model.detail.comments=[{body:'Keep this discussion'}];detailCache.clear(); writeReply={ok:true,store:{host:'supervisor'},issue:{number:1,version:17,draft:false},comments:null,subtasks:null}").unwrap();
        let commit = "mutate({action:'undraft',number:1,if_version:16})";
        p.eval(commit).unwrap();
        let cached = p
            .eval("detailCache.get(detailKey(model.project.id,1)).value")
            .unwrap();
        assert_eq!(cached["issue"]["version"], 17);
        assert_eq!(cached["issue"]["draft"], false);
        assert_eq!(cached["comments"], json!([{"body":"Keep this discussion"}]));
        p.eval("detailCache.clear();model.route.issue=2").unwrap();
        p.eval(commit).unwrap();
        assert_eq!(p.eval("detailCache.size").unwrap(), 0);
    });
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn closing_an_issue_in_the_current_list_confirms_and_persists() {
    with_page(|p| {
        p.eval("document.querySelector('[data-list-actions=\"1\"]').click()")
            .unwrap();
        p.wait("listActionMenu.matches(':popover-open')");
        p.eval("listActionMenu.querySelector('[data-list-action=close]').click()")
            .unwrap();
        p.wait("!listActionBusy && !document.querySelector('[data-list-actions=\"1\"]')");
        assert!(
            p.eval("document.querySelector('#toast').innerText")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("Issue closed")
        );
        assert_eq!(
            p.eval("api({action:'view',number:1}).then(r=>r.issue.state)")
                .unwrap(),
            "closed"
        );
    });
}
