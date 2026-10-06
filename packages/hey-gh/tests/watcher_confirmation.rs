use hey_gh::{RequiredChecksReport, watcher::confirmed_metadata};
use serde_json::{Value, json};

fn policy() -> Value {
    let head = "a".repeat(40);
    let base = "b".repeat(40);
    let merge = "c".repeat(40);
    json!({
        "repository":"o/r","pull_number":7,"head_sha":head,"base_branch":"main",
        "base_sha":base,"pr_base_sha":base,"merge_sha":merge,
        "policy_identity":{"branch":"main","stack":null},
        "state":"satisfied","strict":false,"up_to_date":true,
        "checks":[],"rules":[],"errors":[],"cursor":"unused","pull_request_state":"open",
        "pull_request_confirmation":{"validated_at_ms":1,"selectors":{
            "node_id":"PR_7","number":7,"state":"open","merged":false,"mergeable":true,
            "head":{"sha":head},"base":{"ref":"main","sha":base,
                "repo":{"id":1,"node_id":"R_1","full_name":"o/r"}},
            "merge_commit_sha":merge,"stack":null
        }}
    })
}

fn accepted(value: Value) -> bool {
    let policy: RequiredChecksReport = serde_json::from_value(value).unwrap();
    confirmed_metadata("o/r", 7, &policy).is_some()
}

#[test]
fn confirmation_requires_complete_matching_policy_selectors() {
    let initial = policy();
    assert!(accepted(initial.clone()));
    for (pointer, replacement) in [
        ("/repository", json!("o/other")),
        ("/pull_number", json!(8)),
        ("/head_sha", json!("d".repeat(40))),
        ("/pr_base_sha", Value::Null),
        ("/pr_base_sha", json!("d".repeat(40))),
        ("/merge_sha", Value::Null),
        ("/merge_sha", json!("d".repeat(40))),
        ("/base_branch", json!("other")),
        ("/policy_identity", Value::Null),
        ("/policy_identity/branch", json!("other")),
        ("/pull_request_state", json!("closed")),
        ("/pull_request_confirmation", Value::Null),
        ("/pull_request_confirmation/selectors/node_id", json!("")),
        ("/pull_request_confirmation/selectors/number", json!(8)),
        (
            "/pull_request_confirmation/selectors/state",
            json!("closed"),
        ),
        ("/pull_request_confirmation/selectors/merged", Value::Null),
        ("/pull_request_confirmation/selectors/merged", json!(true)),
        (
            "/pull_request_confirmation/selectors/mergeable",
            Value::Null,
        ),
        (
            "/pull_request_confirmation/selectors/head/sha",
            json!("bad"),
        ),
        (
            "/pull_request_confirmation/selectors/base/sha",
            json!("bad"),
        ),
        ("/pull_request_confirmation/selectors/base/ref", Value::Null),
        (
            "/pull_request_confirmation/selectors/base/repo/id",
            json!(0),
        ),
        (
            "/pull_request_confirmation/selectors/base/repo/node_id",
            Value::Null,
        ),
        (
            "/pull_request_confirmation/selectors/base/repo/full_name",
            json!("o/other"),
        ),
    ] {
        let mut changed = initial.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert!(!accepted(changed), "accepted {pointer}");
    }
    for field in [
        "node_id",
        "number",
        "state",
        "merged",
        "mergeable",
        "head",
        "base",
        "merge_commit_sha",
        "stack",
    ] {
        let mut changed = initial.clone();
        changed["pull_request_confirmation"]["selectors"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(!accepted(changed), "accepted missing {field}");
    }
}

#[test]
fn head_only_conflict_and_native_stack_require_exact_confirmation() {
    let mut value = policy();
    value["merge_sha"] = Value::Null;
    value["pull_request_confirmation"]["selectors"]["merge_commit_sha"] = Value::Null;
    value["pull_request_confirmation"]["selectors"]["mergeable"] = json!(false);
    assert!(accepted(value.clone()));
    value["pull_request_confirmation"]["selectors"]
        .as_object_mut()
        .unwrap()
        .remove("merge_commit_sha");
    assert!(!accepted(value));

    let mut value = policy();
    let stack = json!({"id":12,"number":4,"position":2,"size":2,"base":{"ref":"main","sha":"d".repeat(40)}});
    value["policy_identity"]["stack"] = stack.clone();
    value["base_branch"] = json!("layer");
    value["pull_request_confirmation"]["selectors"]["base"]["ref"] = json!("layer");
    value["pull_request_confirmation"]["selectors"]["stack"] = stack;
    assert!(accepted(value.clone()));
    for (field, replacement) in [
        ("id", json!(13)),
        ("position", json!(1)),
        ("size", json!(3)),
        ("base", json!({"ref":"other","sha":"d".repeat(40)})),
    ] {
        let mut changed = value.clone();
        changed["pull_request_confirmation"]["selectors"]["stack"][field] = replacement;
        assert!(!accepted(changed), "accepted changed native stack {field}");
    }
}

#[test]
fn legacy_reports_deserialize_without_claiming_confirmation() {
    let mut value = policy();
    value
        .as_object_mut()
        .unwrap()
        .remove("pull_request_confirmation");
    assert!(!accepted(value));
}
