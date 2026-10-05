use super::*;

pub(super) async fn seeded(app: bool) -> (Harness, Client) {
    let h = Harness::new().await;
    h.mode("account-ci-selectors-counted-checks");
    h.phase(2);
    let c = Client::with_token(
        if app {
            ci_app_selectors::app_config(&h)
        } else {
            h.config()
        },
        "synthetic-token".into(),
    )
    .unwrap();
    assert!(
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    let before = h.calls().len();
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    assert!(
        h.calls()[before..]
            .iter()
            .all(|c| c.path == "/graphql" && c.token == "Bearer synthetic-token")
    );
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute(
            "DELETE FROM cache WHERE key LIKE '%/commits/%/check-runs?%'",
            [],
        )
        .unwrap();
    (h, c)
}

#[tokio::test]
async fn discovery_counts_avoid_head_and_merge_check_reads_online_and_after_offline_restart() {
    for app in [false, true] {
        let (h, c) = seeded(app).await;
        let before = h.calls().len();
        let report = c
            .ci_for_pr("acme/demo", 7, Freshness::default())
            .await
            .unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        assert!(report.data.check_runs.is_empty());
        assert_eq!(
            h.calls().len(),
            before,
            "discovery must avoid check REST and additional GraphQL reads"
        );
        let proofs: Vec<_> = report
            .validations
            .iter()
            .filter(|v| {
                v.resource.starts_with("my-open-prs://") && v.resource.contains("#check-runs:")
            })
            .collect();
        assert_eq!(proofs.len(), 2);
        drop(c);
        let c = Client::with_token(
            if app {
                ci_app_selectors::app_config(&h)
            } else {
                h.config()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        let offline = c
            .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        assert!(offline.complete, "{:?}", offline.data.errors);
        assert!(offline.data.check_runs.is_empty());
        for proof in proofs {
            assert!(offline.validations.iter().any(
                |v| v.resource == proof.resource && v.validated_at_ms == proof.validated_at_ms
            ));
        }
        assert_eq!(
            h.calls().len(),
            before,
            "restart must not mint a token or read GitHub"
        );
    }
}

#[tokio::test]
async fn discovery_check_counts_preserve_explicit_refresh_and_reject_missing_or_invalid_evidence() {
    for case in [
        "refresh",
        "missing",
        "negative",
        "string",
        "nonempty",
        "wrong-sha",
        "expired",
        "future",
    ] {
        let (h, c) = seeded(false).await;
        ci_discovery_status::edit_discovery(&h, |r| {
            if matches!(case, "expired" | "future") {
                let at = if case == "expired" { 1 } else { u64::MAX };
                r["validated_at_ms"] = json!(at);
                if let Some(clocks) = r["data"]["validatedAtByPr"].as_object_mut() {
                    for clock in clocks.values_mut() {
                        *clock = json!(at);
                    }
                }
            }
            let nodes = if r["data"]["pulls"].is_array() {
                r["data"]["pulls"].as_array_mut().unwrap()
            } else {
                r["data"]["data"]["viewer"]["pullRequests"]["nodes"]
                    .as_array_mut()
                    .unwrap()
            };
            for n in nodes
                .iter_mut()
                .filter(|n| n["repository"]["nameWithOwner"] == "acme/demo")
            {
                let commit = &mut n["commits"]["nodes"][0]["commit"];
                match case {
                    "missing" => {
                        commit.as_object_mut().unwrap().remove("statusCheckRollup");
                    }
                    "negative" => {
                        commit["statusCheckRollup"]["contexts"]["checkRunCount"] = json!(-1)
                    }
                    "string" => {
                        commit["statusCheckRollup"]["contexts"]["checkRunCount"] = json!("0")
                    }
                    "nonempty" => {
                        commit["statusCheckRollup"]["contexts"]["checkRunCount"] = json!(1)
                    }
                    "wrong-sha" => commit["oid"] = json!(NEW_HEAD),
                    _ => {}
                }
            }
        });
        let before = h.calls().len();
        let report = c
            .ci_for_pr(
                "acme/demo",
                7,
                if case == "refresh" {
                    Freshness::Revalidate
                } else {
                    Freshness::default()
                },
            )
            .await
            .unwrap();
        assert!(report.complete, "{case}: {:?}", report.data.errors);
        assert!(
            h.calls()[before..]
                .iter()
                .any(|c| c.path.contains(HEAD) && c.path.ends_with("/check-runs")),
            "{case}"
        );
        if case == "refresh" {
            assert_eq!(
                h.calls()[before..]
                    .iter()
                    .filter(|c| c.path.ends_with("/check-runs"))
                    .count(),
                2
            );
        }
    }
}
