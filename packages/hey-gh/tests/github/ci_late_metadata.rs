use super::*;

struct Queued {
    h: Harness,
    c: Client,
    gate: JoinHandle<Result<hey_gh::Response, Error>>,
    read: JoinHandle<Result<hey_gh::CiObservation, Error>>,
    before: usize,
}

async fn queued(app: bool, changed: bool) -> Queued {
    let (h, c) = ci_status_versions::seeded(app).await;
    ci_status_versions::age_statuses(&h, 1.max(ci_status_versions::proof_clock(&h) - 60_000));
    ci_app_selectors::expire_metadata(&h);
    ci_selectors::edit_selector_cache(&h, |r| r["validated_at_ms"] = json!(1));
    h.mode("ci-point-status-versions-late");
    if changed {
        h.phase(3);
    }
    let before = h.calls().len();
    let gate = tokio::spawn({
        let c = c.clone();
        async move { c.get("slow", Freshness::Revalidate).await }
    });
    until(|| h.calls()[before..].iter().any(|call| call.path == "/slow")).await;
    let read = tokio::spawn({
        let c = c.clone();
        async move { c.ci_for_pr("acme/demo", 7, Freshness::default()).await }
    });
    // One blocked REST socket, the held selector response, and both status
    // lists waiting for REST. Metadata must arrive after those reads began.
    until(|| {
        c.status().outstanding_requests >= 4
            && h.calls()[before..]
                .iter()
                .any(|call| call.path == "/graphql")
    })
    .await;
    Queued {
        h,
        c,
        gate,
        read,
        before,
    }
}

#[tokio::test]
async fn late_ci_metadata_releases_queued_status_reads_without_spending_rest_quota() {
    for app in [false, true] {
        let Queued {
            h,
            c,
            gate,
            mut read,
            before,
        } = queued(app, false).await;
        h.mock.selectors_release.notify_one();
        let early = tokio::time::timeout(Duration::from_secs(1), &mut read).await;
        // Always release the owned socket, including for the expected red run.
        h.mock.release.notify_one();
        gate.await.unwrap().unwrap();
        let early = match early {
            Ok(result) => Some(result.unwrap().unwrap()),
            Err(_) => {
                let _ = read.await;
                None
            }
        };
        until(|| c.status().outstanding_requests == 0).await;
        let report = early
            .expect("fresh selector versions must release status reads while REST remains queued");
        assert!(report.complete, "{:?}", report.data.errors);
        assert_eq!(report.data.commit_statuses.len(), 2);
        assert_eq!(
            report
                .validations
                .iter()
                .filter(|v| v.resource.contains("#commit-status-versions:"))
                .count(),
            2
        );
        assert!(report.validations.iter().all(|v| v.validated_at_ms > 1));
        assert!(
            report
                .validations
                .iter()
                .all(|v| !v.resource.ends_with("/status?per_page=100")),
            "abandoned REST clocks are not evidence"
        );
        let calls = &h.calls()[before..];
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.path.ends_with("/status"))
                .count(),
            0
        );
        let selectors: Vec<_> = calls
            .iter()
            .filter(|call| call.path == "/graphql")
            .collect();
        assert_eq!(selectors.len(), 1, "no additional query for late evidence");
        assert_eq!(
            selectors[0].token,
            if app {
                "Bearer synthetic-app-token"
            } else {
                "Bearer synthetic-token"
            }
        );
    }
}

#[tokio::test]
async fn late_ci_metadata_changed_versions_keep_the_existing_rest_reads() {
    for app in [false, true] {
        let Queued {
            h,
            c,
            gate,
            mut read,
            before,
        } = queued(app, true).await;
        h.mock.selectors_release.notify_one();
        let early = tokio::time::timeout(Duration::from_millis(200), &mut read).await;
        h.mock.release.notify_one();
        gate.await.unwrap().unwrap();
        assert!(
            early.is_err(),
            "changed versions cannot certify the cached list"
        );
        let report = read.await.unwrap().unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        assert!(
            report
                .data
                .commit_statuses
                .iter()
                .all(|s| s["state"] == "failure")
        );
        assert_eq!(
            h.calls()[before..]
                .iter()
                .filter(|call| call.path.ends_with("/status"))
                .count(),
            2
        );
        until(|| c.status().outstanding_requests == 0).await;
    }
}

#[tokio::test]
async fn late_ci_metadata_preserves_a_coalesced_rest_reader() {
    for app in [false, true] {
        let Queued {
            h,
            c,
            gate,
            mut read,
            before,
        } = queued(app, false).await;
        let coalesced = c.status().coalesced_requests;
        let other = tokio::spawn({
            let c = c.clone();
            async move {
                c.get(
                    &format!("repos/acme/demo/commits/{HEAD}/status?per_page=100"),
                    Freshness::Revalidate,
                )
                .await
            }
        });
        until(|| c.status().coalesced_requests > coalesced).await;
        h.mock.selectors_release.notify_one();
        let early = tokio::time::timeout(Duration::from_secs(1), &mut read).await;
        let other_waiting = !other.is_finished();
        h.mock.release.notify_one();
        gate.await.unwrap().unwrap();
        let response = other.await.unwrap().unwrap();
        let report = match early {
            Ok(result) => result.unwrap().unwrap(),
            Err(_) => {
                let _ = read.await;
                panic!("CI must complete independently of its coalesced REST reader");
            }
        };
        assert!(report.complete);
        assert!(other_waiting);
        assert!(matches!(
            response.source,
            Source::Network | Source::Revalidated
        ));
        assert_eq!(response.data["total_count"], 1);
        until(|| c.status().outstanding_requests == 0).await;
        assert_eq!(
            h.calls()[before..]
                .iter()
                .filter(|call| call.path.ends_with("/status"))
                .count(),
            1
        );
    }
}
