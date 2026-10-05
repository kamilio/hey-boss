use super::*;

fn edit_discovery(h: &Harness, mut edit: impl FnMut(&mut Value)) {
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    let rows: Vec<(String,String)> = db.prepare("SELECT key,response FROM cache WHERE key='account-discovery-complete:v1' OR json_type(response,'$.data.data.viewer.pullRequests.nodes')='array'")
        .unwrap().query_map([], |r| Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    for (key, raw) in rows {
        let mut response: Value = serde_json::from_str(&raw).unwrap();
        edit(&mut response);
        db.execute(
            "UPDATE cache SET response=?1 WHERE key=?2",
            [response.to_string(), key],
        )
        .unwrap();
    }
}

async fn seeded() -> (Harness, Client) {
    let h = Harness::new().await;
    h.mode("account-ci-selectors-empty");
    h.phase(2);
    let c = h.client();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute(
        "DELETE FROM cache WHERE key LIKE '%/commits/%/status?%'",
        [],
    )
    .unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',0) WHERE key LIKE '%/pulls/7'", []).unwrap();
    (h, c)
}

#[tokio::test]
async fn discovery_empty_statuses_avoid_rest_and_point_graphql_requests() {
    let (h, c) = seeded().await;
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    assert!(report.complete, "{:?}", report.data.errors);
    assert!(report.data.commit_statuses.is_empty());
    assert_eq!(
        h.calls().len(),
        before,
        "fresh discovery should supply both status sources without more upstream reads"
    );
    assert_eq!(
        report
            .validations
            .iter()
            .filter(|v| v.resource.starts_with("my-open-prs://")
                && v.resource.contains("#commit-statuses:"))
            .count(),
        2
    );
}

#[tokio::test]
async fn discovery_empty_statuses_survive_offline_restart() {
    let (h, c) = seeded().await;
    drop(c);
    let before = h.calls().len();
    let report = h
        .client()
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(report.complete, "{:?}", report.data.errors);
    assert!(report.data.commit_statuses.is_empty());
    assert_eq!(h.calls().len(), before);
}

#[tokio::test]
async fn discovery_empty_statuses_preserve_forced_refresh() {
    let (h, c) = seeded().await;
    let before = h.calls().len();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    assert_eq!(
        h.calls()[before..]
            .iter()
            .filter(|call| call.path.ends_with("/status"))
            .count(),
        2
    );
}

#[tokio::test]
async fn newer_discovery_statuses_cannot_be_hidden_by_older_empty_evidence() {
    for freshness in [
        Freshness::CachedOnly,
        Freshness::MaxAge(Duration::from_secs(30)),
    ] {
        let (h, c) = seeded().await;
        let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
        db.execute(
            "DELETE FROM cache WHERE key='account-discovery-complete:v1'",
            [],
        )
        .unwrap();
        h.mode("ci-point-empty-status");
        c.ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
            .await
            .unwrap();
        for sha in [HEAD, MERGE] {
            c.get(
                &format!("repos/acme/demo/commits/{sha}/status?per_page=100"),
                Freshness::Revalidate,
            )
            .await
            .unwrap();
        }
        h.mode("account-ci-selectors-nonempty");
        c.all_my_open_pull_requests(Freshness::Revalidate)
            .await
            .unwrap();
        let before = h.calls().len();
        let report = c.ci_for_pr("acme/demo", 7, freshness).await.unwrap();
        if matches!(freshness, Freshness::CachedOnly) {
            assert!(
                !report.complete,
                "new statuses have no complete cached REST payload yet"
            );
            assert_eq!(report.data.errors.len(), 2);
            assert_eq!(h.calls().len(), before);
        } else {
            assert!(report.complete, "{:?}", report.data.errors);
            assert_eq!(report.data.commit_statuses.len(), 1);
            assert_eq!(
                h.calls()[before..]
                    .iter()
                    .filter(|call| call.path.ends_with("/status"))
                    .count(),
                2
            );
        }
    }
}

#[tokio::test]
async fn discovery_empty_statuses_require_explicit_fields_identity_and_valid_clocks() {
    for case in [
        "head-missing",
        "head-oid",
        "head-duplicate",
        "merge-missing",
        "merge-oid",
        "id",
        "number",
        "repository",
        "duplicate",
        "zero",
        "future",
    ] {
        let (h, c) = seeded().await;
        edit_discovery(&h, |response| {
            if matches!(case, "zero" | "future") {
                let at = if case == "zero" { 0 } else { u64::MAX };
                response["validated_at_ms"] = json!(at);
                if let Some(clocks) = response["data"]["validatedAtByPr"].as_object_mut() {
                    for clock in clocks.values_mut() {
                        *clock = json!(at);
                    }
                }
                return;
            }
            let nodes = if response["data"]["pulls"].is_array() {
                response["data"]["pulls"].as_array_mut().unwrap()
            } else {
                response["data"]["data"]["viewer"]["pullRequests"]["nodes"]
                    .as_array_mut()
                    .unwrap()
            };
            let Some(pr) = nodes
                .iter_mut()
                .find(|node| node["repository"]["nameWithOwner"] == "acme/demo")
            else {
                return;
            };
            match case {
                "head-missing" => {
                    pr["commits"]["nodes"][0]["commit"]
                        .as_object_mut()
                        .unwrap()
                        .remove("status");
                }
                "head-oid" => pr["commits"]["nodes"][0]["commit"]["oid"] = json!(NEW_HEAD),
                "head-duplicate" => {
                    let node = pr["commits"]["nodes"][0].clone();
                    pr["commits"]["nodes"].as_array_mut().unwrap().push(node);
                }
                "merge-missing" => {
                    pr["potentialMergeCommit"]
                        .as_object_mut()
                        .unwrap()
                        .remove("status");
                }
                "merge-oid" => pr["potentialMergeCommit"]["oid"] = json!(NEW_HEAD),
                "id" => pr["id"] = json!("PR_replaced"),
                "number" => pr["number"] = json!(8),
                "repository" => pr["repository"]["nameWithOwner"] = json!("acme/unrelated"),
                "duplicate" => {
                    let duplicate = pr.clone();
                    nodes.push(duplicate);
                }
                _ => unreachable!(),
            }
        });
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert!(!report.complete, "{case}");
        assert_eq!(
            report.data.errors.len(),
            if case.starts_with("head-") || case.starts_with("merge-") {
                1
            } else {
                2
            },
            "{case}"
        );
        assert!(
            report
                .data
                .errors
                .iter()
                .all(|error| error.source.starts_with("commit_statuses:")),
            "{case}"
        );
        assert_eq!(h.calls().len(), before);
    }
}

#[tokio::test]
async fn discovery_empty_statuses_use_page_clocks_without_extending_freshness() {
    for freshness in [
        Freshness::CachedOnly,
        Freshness::MaxAge(Duration::from_secs(30)),
    ] {
        let (h, c) = seeded().await;
        edit_discovery(&h, |response| {
            response["validated_at_ms"] = json!(1);
            if let Some(clocks) = response["data"]["validatedAtByPr"].as_object_mut() {
                for clock in clocks.values_mut() {
                    *clock = json!(1);
                }
            }
        });
        c.pull_request("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        let before = h.calls().len();
        let report = c.ci_for_pr("acme/demo", 7, freshness).await.unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        if matches!(freshness, Freshness::CachedOnly) {
            assert_eq!(h.calls().len(), before);
            let stamps: Vec<_> = report
                .validations
                .iter()
                .filter(|v| v.resource.contains("#commit-statuses:"))
                .map(|v| v.validated_at_ms)
                .collect();
            assert_eq!(stamps, [1, 1]);
        } else {
            assert_eq!(
                h.calls()[before..]
                    .iter()
                    .filter(|call| call.path.ends_with("/status"))
                    .count(),
                2
            );
        }
    }
}

#[tokio::test]
async fn partial_discovery_page_can_supply_empty_statuses_without_completing_the_roster() {
    let (h, c) = seeded().await;
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.data.validatedAtByPr.\"acme/demo/7\"',1,'$.data.pulls[0].commits.nodes[0].commit.status',json('{\"id\":\"S_old\"}'),'$.data.pulls[0].potentialMergeCommit.status',json('{\"id\":\"S_old_merge\"}')) WHERE key='account-discovery-complete:v1'", []).unwrap();
    let memo = || {
        db.query_row(
            "SELECT response FROM cache WHERE key='account-discovery-complete:v1'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    };
    let before_memo = memo();
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    assert!(report.complete, "{:?}", report.data.errors);
    assert!(report.data.commit_statuses.is_empty());
    assert_eq!(h.calls().len(), before);
    assert_eq!(memo(), before_memo);
    assert!(
        report
            .validations
            .iter()
            .filter(|v| v.resource.contains("#commit-statuses:"))
            .all(|v| v.validated_at_ms > 1)
    );
}

#[tokio::test]
async fn discovery_empty_statuses_do_not_hide_newer_rest_statuses() {
    let (h, c) = seeded().await;
    h.mode("account-ci-selectors");
    for sha in [HEAD, MERGE] {
        c.get(
            &format!("repos/acme/demo/commits/{sha}/status?per_page=100"),
            Freshness::Revalidate,
        )
        .await
        .unwrap();
    }
    let before = h.calls().len();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(report.complete);
    assert_eq!(report.data.commit_statuses.len(), 1);
    assert_eq!(h.calls().len(), before);
    assert!(
        report
            .validations
            .iter()
            .all(|v| !v.resource.contains("#commit-statuses:"))
    );
}

#[tokio::test]
async fn contradictory_rest_and_discovery_status_evidence_remains_incomplete() {
    let (h, c) = seeded().await;
    h.mode("account-ci-selectors-nonempty");
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    h.mode("account-ci-selectors-empty");
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    assert!(!report.complete);
    assert_eq!(report.data.errors.len(), 2);
    assert!(
        report
            .data
            .errors
            .iter()
            .all(|error| error.message.contains("disagree"))
    );
}
