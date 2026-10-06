use super::*;

const REF: &str = "/repos/acme/demo/git/ref/pull/7/merge";

fn unknown_merge(data: &mut Data) {
    let node = &mut data.graph["data"]["repository"]["pullRequest"];
    node["mergeable"] = json!("UNKNOWN");
    node["potentialMergeCommit"] = Value::Null;
}

#[tokio::test]
async fn matching_ref_confirms_policy_without_refreshing_metadata_or_using_app_auth() {
    for installation in [false, true] {
        let f = Fixture::with_installation(installation).await;
        let old = f.seed().await;
        let before = f
            .client
            .bootstrap()
            .await
            .unwrap()
            .snapshots
            .into_iter()
            .find(|s| s.resource.starts_with("metadata://"))
            .unwrap();
        {
            let mut data = f.data.lock().unwrap();
            unknown_merge(&mut data);
            data.stall_rest = true;
        }
        let report = tokio::time::timeout(
            Duration::from_secs(1),
            f.client
                .required_checks_for_pr("acme/demo", 7, Freshness::default()),
        )
        .await
        .expect("matching merge ref waited for full REST metadata")
        .unwrap();
        assert_eq!(report.state, "satisfied", "{:?}", report.errors);
        assert_eq!(report.merge_sha.as_deref(), Some(MERGE));
        assert!(
            report
                .validations
                .iter()
                .any(|v| v.resource.ends_with(REF) && v.validated_at_ms > old)
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
        assert_eq!(cached.data, metadata());
        let after = f
            .client
            .bootstrap()
            .await
            .unwrap()
            .snapshots
            .into_iter()
            .find(|s| s.resource == before.resource)
            .unwrap();
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
        let data = f.data.lock().unwrap();
        assert_eq!(
            data.calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
            ["/graphql", REF]
        );
        assert!(
            data.tokens
                .iter()
                .all(|token| token == "Bearer synthetic-token")
        );
    }
}

#[tokio::test]
async fn changed_or_missing_ref_requires_full_confirmation() {
    for value in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"ref":"refs/pull/8/merge","object":{"type":"commit","sha":MERGE}}),
        json!({"ref":"refs/pull/7/merge","object":{"type":"tag","sha":MERGE}}),
        json!({"ref":"refs/pull/7/merge","object":{"type":"commit","sha":HEAD}}),
        json!({"ref":"refs/pull/7/merge","object":{"type":"commit"}}),
    ] {
        let f = Fixture::new().await;
        f.seed().await;
        {
            let mut data = f.data.lock().unwrap();
            unknown_merge(&mut data);
            data.merge_ref = value.clone();
            data.deny_rest = true;
        }
        assert!(
            matches!(
                f.client
                    .required_checks_for_pr("acme/demo", 7, Freshness::default())
                    .await,
                Err(hey_gh::Error::GitHub { status: 403, .. })
            ),
            "{value}"
        );
        let data = f.data.lock().unwrap();
        assert_eq!(
            data.calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
            ["/graphql", REF, "/repos/acme/demo/pulls/7"]
        );
    }
}

#[tokio::test]
async fn unknown_merge_cannot_bypass_changed_or_incomplete_identity() {
    for (field, value) in [
        ("id", json!("other")),
        ("number", json!(8)),
        ("state", json!("CLOSED")),
        ("merged", json!(true)),
        ("headRefOid", json!(BASE)),
        ("baseRefOid", json!(HEAD)),
        ("baseRefName", json!("release")),
        ("baseRepository/id", json!("other")),
        ("baseRepository/databaseId", json!(456)),
        ("baseRepository/nameWithOwner", json!("acme/other")),
        ("mergeable", json!("CONFLICTING")),
        ("mergeable", Value::Null),
        ("potentialMergeCommit", json!({"oid":MERGE})),
        ("stack", json!({"id":"native"})),
        ("stackEntry", json!({"id":"native"})),
        ("missing:potentialMergeCommit", Value::Null),
        ("missing:stack", Value::Null),
        ("missing:stackEntry", Value::Null),
        ("repository:id", json!("other")),
        ("repository:databaseId", json!(456)),
        ("repository:nameWithOwner", json!("acme/other")),
    ] {
        let f = Fixture::new().await;
        f.seed().await;
        {
            let mut data = f.data.lock().unwrap();
            unknown_merge(&mut data);
            data.deny_rest = true;
            if let Some(field) = field.strip_prefix("repository:") {
                data.graph["data"]["repository"][field] = value;
            } else {
                let node = &mut data.graph["data"]["repository"]["pullRequest"];
                if let Some(field) = field.strip_prefix("missing:") {
                    node.as_object_mut().unwrap().remove(field);
                } else {
                    *node.pointer_mut(&format!("/{field}")).unwrap() = value;
                }
            }
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
async fn merge_ref_access_denial_remains_explicit() {
    let f = Fixture::new().await;
    f.seed().await;
    {
        let mut data = f.data.lock().unwrap();
        unknown_merge(&mut data);
        data.deny_merge_ref = true;
    }
    assert!(matches!(
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await,
        Err(hey_gh::Error::GitHub { status: 403, .. })
    ));
    let data = f.data.lock().unwrap();
    assert_eq!(
        data.calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
        ["/graphql", REF]
    );
}

#[tokio::test]
async fn deleted_merge_ref_requires_rest_confirmation() {
    let f = Fixture::new().await;
    f.seed().await;
    {
        let mut data = f.data.lock().unwrap();
        unknown_merge(&mut data);
        data.missing_merge_ref = true;
        data.deny_rest = true;
    }
    assert!(matches!(
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await,
        Err(hey_gh::Error::GitHub { status: 403, .. })
    ));
    let data = f.data.lock().unwrap();
    assert_eq!(
        data.calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
        ["/graphql", REF, "/repos/acme/demo/pulls/7"]
    );
}

#[tokio::test]
async fn stalled_ref_is_bounded_and_falls_back_to_full_confirmation() {
    let f = Fixture::new().await;
    f.seed().await;
    {
        let mut data = f.data.lock().unwrap();
        unknown_merge(&mut data);
        data.stall_merge_ref = true;
    }
    let report = tokio::time::timeout(
        Duration::from_secs(3),
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default()),
    )
    .await
    .expect("optional ref exhausted report deadline")
    .unwrap();
    assert_eq!(report.state, "satisfied");
    let data = f.data.lock().unwrap();
    assert_eq!(
        data.calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
        ["/graphql", REF, "/repos/acme/demo/pulls/7"]
    );
}

#[tokio::test]
async fn even_recent_cached_ref_must_be_revalidated() {
    let f = Fixture::new().await;
    f.seed().await;
    f.client
        .get(REF.trim_start_matches('/'), Freshness::Revalidate)
        .await
        .unwrap();
    {
        let mut data = f.data.lock().unwrap();
        data.calls.clear();
        unknown_merge(&mut data);
        data.merge_ref["object"]["sha"] = json!(HEAD);
        data.deny_rest = true;
    }
    assert!(matches!(
        f.client
            .required_checks_for_pr("acme/demo", 7, Freshness::default())
            .await,
        Err(hey_gh::Error::GitHub { status: 403, .. })
    ));
    let data = f.data.lock().unwrap();
    assert_eq!(
        data.calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
        ["/graphql", REF, "/repos/acme/demo/pulls/7"]
    );
}
