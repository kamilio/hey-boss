use super::*;

fn conflicting(data: &mut Data) {
    data.rest["mergeable"] = json!(false);
    data.rest["merge_commit_sha"] = Value::Null;
    let node = &mut data.graph["data"]["repository"]["pullRequest"];
    node["mergeable"] = json!("CONFLICTING");
    node["potentialMergeCommit"] = Value::Null;
}

#[tokio::test]
async fn confirmed_conflict_uses_personal_selectors_without_renewing_rest_metadata() {
    for installation in [false, true] {
        let f = Fixture::with_installation(installation).await;
        conflicting(&mut f.data.lock().unwrap());
        let old = f.seed().await;
        f.data.lock().unwrap().stall_rest = true;
        let report = tokio::time::timeout(
            Duration::from_secs(1),
            f.client
                .required_checks_for_pr("acme/demo", 7, Freshness::default()),
        )
        .await
        .expect("confirmed conflict waited for REST metadata")
        .unwrap();
        // Required checks can pass while the PR has a merge conflict. This is
        // evidence about the head's checks, never a claim of merge readiness.
        assert_eq!(report.state, "satisfied", "{:?}", report.errors);
        assert_eq!(report.merge_sha, None);
        assert!(
            report
                .checks
                .iter()
                .all(|check| check.sha.as_deref() == Some(HEAD))
        );
        assert!(
            report
                .validations
                .iter()
                .any(|v| v.resource.ends_with("/graphql") && v.validated_at_ms > old)
        );
        assert!(
            report
                .validations
                .iter()
                .all(|v| !v.resource.ends_with("/pulls/7"))
        );
        let cached = f
            .client
            .pull_request("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert_eq!(cached.validated_at_ms, old);
        assert_eq!(cached.data["mergeable"], false);
        let data = f.data.lock().unwrap();
        assert_eq!(data.calls.len(), 1, "{:?}", data.calls);
        assert_eq!(data.calls[0].0, "/graphql");
        assert!(
            data.tokens
                .iter()
                .all(|token| token == "Bearer synthetic-token")
        );
    }
}

#[tokio::test]
async fn ambiguous_or_changed_conflict_requires_rest_without_using_a_retained_merge_ref() {
    for (field, value) in [
        ("mergeable", json!("UNKNOWN")),
        ("mergeable", json!("MERGEABLE")),
        ("mergeable", Value::Null),
        ("potentialMergeCommit", json!({"oid":MERGE})),
        ("headRefOid", json!(MERGE)),
        ("baseRefOid", json!(HEAD)),
        ("baseRefName", json!("other")),
        ("baseRepository", Value::Null),
        ("id", json!("another_pr")),
        ("number", json!(8)),
        ("state", json!("CLOSED")),
        ("merged", json!(true)),
        ("stack", json!({"id":"native"})),
        ("stackEntry", json!({"id":"native_entry"})),
        ("missing_merge", Value::Null),
        ("missing_stack", Value::Null),
        ("missing_entry", Value::Null),
    ] {
        let f = Fixture::new().await;
        conflicting(&mut f.data.lock().unwrap());
        f.seed().await;
        {
            let mut data = f.data.lock().unwrap();
            let node = &mut data.graph["data"]["repository"]["pullRequest"];
            match field {
                "missing_merge" => {
                    node.as_object_mut().unwrap().remove("potentialMergeCommit");
                }
                "missing_stack" => {
                    node.as_object_mut().unwrap().remove("stack");
                }
                "missing_entry" => {
                    node.as_object_mut().unwrap().remove("stackEntry");
                }
                _ => node[field] = value,
            }
            data.deny_rest = true;
        }
        assert!(
            matches!(
                f.client
                    .required_checks_for_pr("acme/demo", 7, Freshness::default())
                    .await,
                Err(hey_gh::Error::GitHub { status: 403, .. })
            ),
            "{field}"
        );
        let data = f.data.lock().unwrap();
        assert_eq!(
            data.calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
            ["/graphql", "/repos/acme/demo/pulls/7"],
            "{field}"
        );
    }
}

#[tokio::test]
async fn conflict_selector_access_denial_cannot_fall_back_to_an_old_seed() {
    let f = Fixture::new().await;
    conflicting(&mut f.data.lock().unwrap());
    f.seed().await;
    f.data.lock().unwrap().graph =
        json!({"errors":[{"type":"FORBIDDEN","message":"Access denied"}]});
    assert!(matches!(
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await,
        Err(hey_gh::Error::GraphQL {
            access_denied: true,
            ..
        })
    ));
    let data = f.data.lock().unwrap();
    assert_eq!(data.calls.len(), 1);
    assert_eq!(data.calls[0].0, "/graphql");
}

#[tokio::test]
async fn resolved_conflict_recollects_the_new_merge_commit() {
    let f = Fixture::new().await;
    conflicting(&mut f.data.lock().unwrap());
    f.seed().await;
    {
        let mut data = f.data.lock().unwrap();
        data.rest = metadata();
        data.graph = selectors();
    }
    let report = f
        .client
        .required_checks_for_pr("acme/demo", 7, Freshness::default())
        .await
        .unwrap();
    assert_eq!(report.state, "satisfied", "{:?}", report.errors);
    assert_eq!(report.merge_sha.as_deref(), Some(MERGE));
    assert!(
        report
            .checks
            .iter()
            .all(|check| check.sha.as_deref() == Some(MERGE))
    );
    let data = f.data.lock().unwrap();
    assert!(data.calls.iter().any(|call| call.0.ends_with("/pulls/7")));
    assert!(data.calls.iter().any(|call| call.0.contains(MERGE)));
    assert!(!data.calls.iter().any(|call| call.0.contains("/git/ref/")));
}

#[tokio::test]
async fn unknown_or_missing_conflict_seed_requires_rest() {
    for kind in ["unknown", "missing_merge", "closed"] {
        let f = Fixture::new().await;
        {
            let mut data = f.data.lock().unwrap();
            conflicting(&mut data);
            match kind {
                "unknown" => data.rest["mergeable"] = Value::Null,
                "missing_merge" => {
                    data.rest
                        .as_object_mut()
                        .unwrap()
                        .remove("merge_commit_sha");
                }
                "closed" => data.rest["state"] = json!("closed"),
                _ => unreachable!(),
            }
        }
        f.seed().await;
        f.data.lock().unwrap().deny_rest = true;
        assert!(
            matches!(
                f.client
                    .required_checks_for_pr("acme/demo", 7, Freshness::default())
                    .await,
                Err(hey_gh::Error::GitHub { status: 403, .. })
            ),
            "{kind}"
        );
        let data = f.data.lock().unwrap();
        assert_eq!(
            data.calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
            ["/repos/acme/demo/pulls/7"],
            "{kind}"
        );
    }
}
