use axum::{
    Router,
    extract::State,
    http::{StatusCode, Uri},
    response::IntoResponse,
};
use hey_gh::{Client, Config, Freshness};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const TRUNK: &str = "cccccccccccccccccccccccccccccccccccccccc";
const MERGE: &str = "dddddddddddddddddddddddddddddddddddddddd";

fn stack(branch: &str) -> Value {
    json!({"id":12,"number":4,"position":2,"size":2,"base":{"ref":branch,"sha":TRUNK}})
}

struct Fixture {
    membership: Value,
    next_membership: Option<Value>,
    denied: bool,
    classic: bool,
    strict: bool,
    protection: Value,
    next_protection: Option<Value>,
    calls: Vec<String>,
}
async fn handler(State(state): State<Arc<Mutex<Fixture>>>, uri: Uri) -> impl IntoResponse {
    let mut s = state.lock().unwrap();
    let path = uri.path();
    s.calls.push(path.into());
    let value = if path == "/user" {
        json!({"id":1,"login":"me"})
    } else if path == "/graphql" {
        json!({"data":{"viewer":{"pullRequests":{"totalCount":1,"nodes":[{"id":"PR_demo_7","number":7,"title":"Native stack layer","url":"https://github.com/acme/demo/pull/7","state":"OPEN","headRefOid":HEAD,"baseRefOid":BASE,"baseRefName":"layer","repository":{"nameWithOwner":"acme/demo"}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}})
    } else if path.ends_with("/pulls/7") {
        let membership = s.membership.clone();
        if let Some(next) = s.next_membership.take() {
            s.membership = next;
        }
        json!({"node_id":"PR_demo_7","number":7,"state":"open","head":{"sha":HEAD},"base":{"ref":"layer","sha":BASE},"merge_commit_sha":MERGE,"stack":membership})
    } else if path.contains("/protection/required_status_checks") {
        if s.classic && path.contains("/main/") {
            json!({"strict":s.strict,"checks":[{"context":"classic","app_id":15368}]})
        } else {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(json!({"message":"Branch not protected"})),
            );
        }
    } else if path.contains("/rules/branches/") {
        if s.denied && !path.ends_with("/layer") {
            return (
                StatusCode::FORBIDDEN,
                axum::Json(json!({"message":"Policy inaccessible"})),
            );
        }
        if path.ends_with("/layer") {
            json!([])
        } else {
            json!([{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":s.strict,"required_status_checks":[{"context":"pre-commit","integration_id":15368}]}}])
        }
    } else if path.contains("/branches/") {
        let protection = s.protection.clone();
        if let Some(next) = s.next_protection.take() {
            s.protection = next;
            s.classic = true;
        }
        json!({"protected":!path.ends_with("/layer"),"protection":protection,"commit":{"sha":if path.ends_with("/layer") {BASE} else {TRUNK}}})
    } else if path.contains("/compare/") {
        json!({"merge_base_commit":{"sha":TRUNK}})
    } else if path.ends_with("/check-runs") {
        json!({"total_count":1,"check_runs":[{"id":if path.contains(MERGE) {11} else {10},"name":"pre-commit","app":{"id":15368},"head_sha":if path.contains(MERGE) {MERGE} else {HEAD},"status":"completed","conclusion":if path.contains(MERGE) {"failure"} else {"success"}}]})
    } else if path.ends_with("/status") {
        json!({"statuses":[]})
    } else if path.ends_with("/actions/runs") {
        json!({"workflow_runs":[]})
    } else {
        json!([])
    };
    (StatusCode::OK, axum::Json(value))
}

async fn fixture(
    membership: Value,
) -> (
    Client,
    Arc<Mutex<Fixture>>,
    tempfile::TempDir,
    tokio::task::JoinHandle<()>,
) {
    let state = Arc::new(Mutex::new(Fixture {
        membership,
        next_membership: None,
        denied: false,
        classic: false,
        strict: false,
        protection: Value::Null,
        next_protection: None,
        calls: vec![],
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(
        Config {
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            cache_path: dir.path().join("cache.sqlite"),
            min_spacing: Duration::ZERO,
            ..Config::default()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    let app = Router::new().fallback(handler).with_state(state.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (client, state, dir, task)
}

#[tokio::test]
async fn explicit_disabled_classic_checks_avoid_redundant_404_but_keep_trunk_rules() {
    let (c, s, _dir, task) = fixture(stack("main")).await;
    s.lock().unwrap().protection = json!({"enabled":false,"required_status_checks":{
        "enforcement_level":"off","contexts":[],"checks":[]
    }});
    let report = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(report.state, "failure");
    assert_eq!(report.checks[0].context, "pre-commit");
    assert_eq!(report.checks[0].sha.as_deref(), Some(MERGE));
    assert!(report.errors.is_empty());
    let calls = &s.lock().unwrap().calls;
    assert!(
        calls
            .iter()
            .any(|p| p == "/repos/acme/demo/rules/branches/main")
    );
    assert_eq!(
        calls
            .iter()
            .filter(|p| p.contains("/protection/required_status_checks"))
            .count(),
        0,
        "fresh branch metadata already proves classic checks are disabled"
    );
    task.abort();
}

#[tokio::test]
async fn absent_enabled_or_inconsistent_classic_metadata_keeps_the_policy_probe() {
    for protection in [
        Value::Null,
        json!({"enabled":false}),
        json!({"enabled":true,"required_status_checks":{"enforcement_level":"off","contexts":[],"checks":[]}}),
        json!({"enabled":"false","required_status_checks":{"enforcement_level":"off","contexts":[],"checks":[]}}),
        json!({"enabled":false,"required_status_checks":{"enforcement_level":"off","contexts":["classic"],"checks":[]}}),
        json!({"enabled":false,"required_status_checks":{"enforcement_level":"off","contexts":[],"checks":[{"context":"classic"}]}}),
        json!({"enabled":false,"required_status_checks":{"enforcement_level":"off","contexts":[],"checks":[],"strict":true}}),
    ] {
        let (c, s, _dir, task) = fixture(stack("main")).await;
        {
            let mut state = s.lock().unwrap();
            state.protection = protection.clone();
            state.classic = true;
        }
        let report = c
            .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert!(
            report.checks.iter().any(|c| c.context == "classic"),
            "{protection}"
        );
        assert_eq!(
            s.lock()
                .unwrap()
                .calls
                .iter()
                .filter(|p| p.contains("/protection/required_status_checks"))
                .count(),
            1
        );
        task.abort();
    }
}

#[tokio::test]
async fn disabled_classic_metadata_does_not_hide_denied_rulesets() {
    let (c, s, _dir, task) = fixture(stack("main")).await;
    {
        let mut state = s.lock().unwrap();
        state.protection = json!({"enabled":false,"required_status_checks":{
            "enforcement_level":"off","contexts":[],"checks":[]
        }});
        state.denied = true;
    }
    let report = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(report.state, "unknown");
    assert!(report.errors.iter().any(|e| e.source == "rulesets"));
    assert!(
        !s.lock()
            .unwrap()
            .calls
            .iter()
            .any(|p| p.contains("/protection/required_status_checks"))
    );
    task.abort();
}

#[tokio::test]
async fn classic_policy_enabled_without_a_commit_change_is_recollected() {
    let (c, s, _dir, task) = fixture(stack("main")).await;
    {
        let mut state = s.lock().unwrap();
        state.protection = json!({"enabled":false,"required_status_checks":{
            "enforcement_level":"off","contexts":[],"checks":[]
        }});
        state.next_protection = Some(json!({"enabled":true}));
    }
    let report = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(report.checks.iter().any(|c| c.context == "classic"));
    assert_eq!(report.policy_sha.as_deref(), Some(TRUNK));
    assert!(
        s.lock()
            .unwrap()
            .calls
            .iter()
            .filter(|p| p.as_str() == "/repos/acme/demo/branches/main")
            .count()
            >= 4
    );
    task.abort();
}

#[tokio::test]
async fn native_stack_inherits_trunk_rules_and_keeps_diff_and_merge_identity() {
    let (c, s, _dir, task) = fixture(stack("main")).await;
    let r = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(r.state, "failure");
    assert_eq!(r.base_branch, "layer");
    assert_eq!(r.base_sha.as_deref(), Some(BASE));
    assert_eq!(r.checks[0].context, "pre-commit");
    assert_eq!(r.checks[0].app_id, Some(15368));
    assert_eq!(r.checks[0].sha.as_deref(), Some(MERGE));
    let value = serde_json::to_value(&r).unwrap();
    assert_eq!(value["policy_identity"]["branch"], "main");
    assert_eq!(value["policy_identity"]["stack"], stack("main"));
    assert_eq!(value["policy_sha"], TRUNK);
    assert!(!r.strict);
    assert!(
        !s.lock()
            .unwrap()
            .calls
            .iter()
            .any(|p| p.contains("/compare/"))
    );
    let calls = s.lock().unwrap().calls.len();
    let cached = c
        .required_checks_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(cached.state, "failure");
    assert!(cached.oldest_validation_at_ms <= r.observed_at_ms);
    assert_eq!(
        s.lock().unwrap().calls.len(),
        calls,
        "Cached policy reads never fetch"
    );
    task.abort();
}

#[tokio::test]
async fn native_stack_registration_and_trunk_changes_retry_with_unchanged_head() {
    for (before, after) in [
        (Value::Null, stack("main")),
        (stack("main"), stack("release")),
        (stack("main"), Value::Null),
        (
            stack("main"),
            json!({"id":12,"number":4,"position":2,"size":3,"base":{"ref":"main","sha":TRUNK}}),
        ),
    ] {
        let (c, s, _dir, task) = fixture(before).await;
        s.lock().unwrap().next_membership = Some(after.clone());
        let r = c
            .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        let value = serde_json::to_value(&r).unwrap();
        assert_eq!(value["policy_identity"]["stack"], after);
        assert_eq!(
            r.state,
            if after.is_null() {
                "not_required"
            } else {
                "failure"
            }
        );
        assert_eq!(
            s.lock()
                .unwrap()
                .calls
                .iter()
                .filter(|p| p.ends_with("/pulls/7"))
                .count(),
            3,
            "reuse the changed confirmation as the retry seed, then confirm again"
        );
        task.abort();
    }
}

#[tokio::test]
async fn expired_policy_seed_rechecks_native_membership_before_returning() {
    for (before, after) in [
        (Value::Null, stack("main")),
        (stack("main"), stack("release")),
        (stack("main"), Value::Null),
        (
            stack("main"),
            json!({"id":12,"number":4,"position":2,"size":3,"base":{"ref":"main","sha":TRUNK}}),
        ),
    ] {
        let (c, s, dir, task) = fixture(before).await;
        c.required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        rusqlite::Connection::open(dir.path().join("cache.sqlite")).unwrap().execute(
            "UPDATE cache SET response=json_set(response,'$.validated_at_ms',0) WHERE key LIKE '%/pulls/7'", [],
        ).unwrap();
        s.lock().unwrap().membership = after.clone();
        let calls = s.lock().unwrap().calls.len();
        let report = c
            .required_checks_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&report).unwrap()["policy_identity"]["stack"],
            after
        );
        assert_eq!(
            report.state,
            if after.is_null() {
                "not_required"
            } else {
                "failure"
            }
        );
        assert!(report.validations.iter().all(|v| v.validated_at_ms > 0));
        assert_eq!(
            s.lock().unwrap().calls[calls..]
                .iter()
                .filter(|p| p.ends_with("/pulls/7"))
                .count(),
            2
        );
        task.abort();
    }
}

#[tokio::test]
async fn native_stack_missing_metadata_and_denied_policy_never_imply_readiness() {
    for membership in [
        json!({}),
        json!({"id":12}),
        json!(false),
        json!({"id":12,"number":4,"base":{"ref":"main"}}),
    ] {
        let (c, _s, _dir, task) = fixture(membership).await;
        assert!(
            c.required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
                .await
                .is_err()
        );
        task.abort();
    }
    let (c, s, _dir, task) = fixture(stack("main")).await;
    s.lock().unwrap().denied = true;
    let r = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(r.state, "unknown");
    assert!(r.errors.iter().any(|e| e.source == "rulesets"));
    task.abort();
}

#[tokio::test]
async fn native_stack_classic_policy_and_strictness_use_trunk_tip() {
    let (c, s, _dir, task) = fixture(stack("main")).await;
    {
        let mut s = s.lock().unwrap();
        s.classic = true;
        s.strict = true;
    }
    let r = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(r.strict);
    assert_eq!(r.up_to_date, Some(true));
    assert!(r.checks.iter().any(|c| c.context == "classic"));
    assert!(
        s.lock()
            .unwrap()
            .calls
            .iter()
            .any(|p| p.contains(&format!("/compare/{TRUNK}...{HEAD}")))
    );
    task.abort();
}

#[tokio::test]
async fn native_stack_registration_cannot_borrow_cached_unprotected_base_policy() {
    let (c, s, _dir, task) = fixture(Value::Null).await;
    let plain = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(plain.state, "not_required");
    assert_eq!(plain.policy_identity.as_ref().unwrap().branch, "layer");
    assert!(plain.policy_identity.as_ref().unwrap().stack.is_none());
    s.lock().unwrap().membership = stack("main");
    c.pull_request("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let calls = s.lock().unwrap().calls.len();
    let cold = c
        .required_checks_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(cold.state, "unknown");
    assert!(cold.errors.iter().any(|e| e.source == "rulesets"));
    assert_eq!(cold.policy_identity.as_ref().unwrap().branch, "main");
    assert_eq!(s.lock().unwrap().calls.len(), calls);
    task.abort();
}

#[tokio::test]
async fn native_stack_api_dashboard_and_watcher_keep_policy_and_invalidate_registration() {
    let (c, s, _dir, task) = fixture(stack("main")).await;
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sdk = hey_gh::ApiClient::new(
        format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, api.router()).await.unwrap() });
    let r = sdk
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(r.state, "failure");
    let observation = hey_gh::watcher::observe_required(&r);
    assert_eq!(observation.evidence["policy_identity"]["branch"], "main");
    assert_eq!(observation.evidence["source_merges"]["required"], MERGE);
    assert_eq!(observation.blocking.len(), 1);
    let ci = c
        .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let pr = c
        .pull_request("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    let observed = hey_gh::watcher::observe_ci("acme/demo", 7, &pr.data, &ci.data, &r);
    assert_eq!(observed.evidence["sources_match"], true);
    assert_eq!(observed.blocking.len(), 1);
    let page = c
        .pr_status_page(Some("acme/demo"), None, 100, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(page.pull_requests[0]["requiredChecks"]["state"], "failure");
    assert_eq!(
        page.pull_requests[0]["requiredChecks"]["policy_identity"]["branch"],
        "main"
    );
    s.lock().unwrap().membership = stack("release");
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let changed = c
        .pull_request("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    let stale = hey_gh::watcher::observe_ci("acme/demo", 7, &changed.data, &ci.data, &r);
    assert_eq!(stale.evidence["sources_match"], false);
    assert!(stale.blocking.is_empty());
    let page = c
        .pr_status_page(Some("acme/demo"), None, 100, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        page.pull_requests[0]["requiredChecks"].is_null(),
        "Same head and direct base cannot keep policy after a trunk change"
    );
    let r = sdk
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(r.policy_identity.as_ref().unwrap().branch, "release");
    assert_ne!(
        hey_gh::watcher::observe_required(&r).evidence["policy_fingerprint"],
        observation.evidence["policy_fingerprint"]
    );
    let page = c
        .pr_status_page(Some("acme/demo"), None, 100, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        page.pull_requests[0]["requiredChecks"]["policy_identity"]["branch"],
        "release"
    );
    server.abort();
    task.abort();
}
