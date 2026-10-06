use super::*;

// Cached metadata is identity/generation fenced by the daemon. Read it with
// bounded concurrency; never revalidate or bypass the regular queue's backoff.
pub(super) async fn poll(
    ctx: &Context,
    client: &ApiClient,
    after: &mut Option<String>,
) -> Result<()> {
    let client = client
        .clone()
        .with_read_deadline(tokio::time::Instant::now() + Duration::from_secs(10));
    let mut urls: Vec<_> = Store::open(&ctx.path)?
        .tracked_pull_requests()?
        .into_iter()
        .map(|pr| pr.url)
        .collect();
    // Resume after the last admission if the bounded scan was interrupted.
    // The in-flight tail may be retried on the next lap; it cannot starve later URLs.
    let start = after
        .as_ref()
        .map_or(0, |last| urls.partition_point(|url| url <= last));
    urls.rotate_left(start);
    let mut urls = urls.into_iter();
    let mut running = tokio::task::JoinSet::new();
    loop {
        while running.len() < 4 && !ctx.stopped() {
            let Some(url) = urls.next() else { break };
            *after = Some(url.clone());
            let Some((repository, number)) = selector(&url) else {
                continue;
            };
            let ctx = ctx.clone();
            let client = client.clone();
            running.spawn(async move {
                let response = match client
                    .pull_request(&repository, number, Freshness::CachedOnly)
                    .await
                {
                    Ok(response) => response,
                    Err(hey_gh::Error::CacheMiss) => return Ok(()),
                    Err(error) => return Err(error),
                };
                // Merge is terminal. A cached closure can have since reopened;
                // only the normal watcher may publish that reversible state.
                if merged(&response.data, &repository, number)
                    && watches::timestamp(response.validated_at_ms).is_ok()
                {
                    watches::record_cached_merge(&ctx, &url, &repository, number, &response)?;
                }
                Ok(())
            });
        }
        match running.join_next().await {
            Some(result) => result??,
            None => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cancelled_scan_resumes_after_admitted_urls_then_wraps() {
        let (root, ctx, mut store) = super::super::super::context::tests::test_context();
        let request = |operation| crate::issues::Request {
            version: 1,
            project: crate::issues::Project {
                id: "named:test".into(),
                name: "test".into(),
            },
            project_override: None,
            actor: Some(ctx.actor().unwrap()),
            operation: serde_json::from_value(operation).unwrap(),
            request_id: None,
        };
        store
            .execute(&request(
                json!({"action":"create","title":"Fix","body":"","labels":[]}),
            ))
            .unwrap();
        for number in 1..=8 {
            store.execute(&request(json!({"action":"add_pull_request","number":1,"url":format!("https://github.com/o/r/pull/{number}"),"purpose":"fix"}))).unwrap();
        }
        drop(store);
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let client =
            ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
        let (admitted, ready) = tokio::sync::oneshot::channel();
        let serving = std::thread::spawn(move || {
            let receive = || {
                server
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .expect("Cached metadata read")
            };
            let held: Vec<_> = (0..4).map(|_| receive()).collect();
            admitted.send(()).unwrap();
            for expected in [5..=8, 1..=4] {
                let mut seen = Vec::new();
                let requests: Vec<_> = (0..4).map(|_| receive()).collect();
                for incoming in requests {
                    assert!(incoming.url().ends_with("/metadata?cached_only=true"));
                    let number: u64 = incoming.url().split('/').nth(5).unwrap().parse().unwrap();
                    seen.push(number);
                    let now = crate::issues::worker::now();
                    let body = json!({"data":{"number":number,"state":"open","merged":false,"base":{"repo":{"full_name":"o/r"}}},"validated_at_ms":now,"fetched_at_ms":now,"source":"cache"});
                    incoming
                        .respond(
                            tiny_http::Response::from_string(body.to_string()).with_header(
                                tiny_http::Header::from_bytes("Content-Type", "application/json")
                                    .unwrap(),
                            ),
                        )
                        .unwrap();
                }
                seen.sort_unstable();
                assert_eq!(seen, expected.collect::<Vec<_>>());
            }
            drop(held);
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut after = None;
        runtime.block_on(async {
            // Cancel with all four local reads pending, as the outer batch
            // deadline does in production. The next scan must not restart at 1.
            tokio::select! {
                result = poll(&ctx, &client, &mut after) => panic!("Scan unexpectedly completed: {result:?}"),
                result = ready => result.unwrap(),
            }
            assert_eq!(after.as_deref(), Some("https://github.com/o/r/pull/4"));
            tokio::time::timeout(Duration::from_secs(5), poll(&ctx, &client, &mut after)).await.unwrap().unwrap();
        });
        serving.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[derive(Clone, Copy)]
    enum Evidence {
        Merged,
        Stale,
        Ordinary,
        ClosedTask,
        InvalidTime,
        NewerRecord,
        Future,
        Open,
        Closed,
        WrongIdentity,
    }

    #[test]
    fn confirmed_cached_merge_bypasses_the_required_check_queue() {
        scenario(Evidence::Merged);
    }

    #[test]
    fn confirmed_cached_merge_remains_terminal_after_its_validation_ages() {
        scenario(Evidence::Stale);
    }

    #[test]
    fn ordinary_linked_pr_uses_confirmed_cached_merge() {
        scenario(Evidence::Ordinary);
    }

    #[test]
    fn closed_task_fix_uses_confirmed_cached_merge() {
        scenario(Evidence::ClosedTask);
    }

    #[test]
    fn missing_validation_cannot_publish_a_cached_merge() {
        scenario(Evidence::InvalidTime);
    }

    #[test]
    fn old_cached_merge_does_not_overwrite_a_newer_store_observation() {
        scenario(Evidence::NewerRecord);
    }

    #[test]
    fn cached_open_pr_stays_with_the_watcher() {
        scenario(Evidence::Open);
    }

    #[test]
    fn cached_closure_is_not_treated_as_an_irreversible_merge() {
        scenario(Evidence::Closed);
    }

    #[test]
    fn future_validation_cannot_publish_a_cached_merge() {
        scenario(Evidence::Future);
    }

    #[test]
    fn cached_merge_must_identify_the_requested_pr() {
        scenario(Evidence::WrongIdentity);
    }

    fn scenario(evidence: Evidence) {
        let (root, ctx, mut store) = super::super::super::context::tests::test_context();
        let request = |operation| crate::issues::Request {
            version: 1,
            project: crate::issues::Project {
                id: "named:test".into(),
                name: "test".into(),
            },
            project_override: None,
            actor: Some(ctx.actor().unwrap()),
            operation: serde_json::from_value(operation).unwrap(),
            request_id: None,
        };
        store
            .execute(&request(
                json!({"action":"create","title":"Fix","body":"","labels":[]}),
            ))
            .unwrap();
        store.execute(&request(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/1","purpose":"fix"}))).unwrap();
        if !matches!(evidence, Evidence::Ordinary | Evidence::ClosedTask) {
            let view = store
                .execute(&request(json!({"action":"view","number":1})))
                .unwrap();
            store.execute(&request(json!({"action":"assign","number":1,"target":"github","if_version":view["issue"]["version"]}))).unwrap();
        }
        if matches!(evidence, Evidence::ClosedTask) {
            store
                .execute(&request(json!({"action":"close","number":1,"force":false})))
                .unwrap();
        }
        if matches!(evidence, Evidence::NewerRecord) {
            store
                .record_pr_status(
                    "https://github.com/o/r/pull/1",
                    Some("open"),
                    crate::issues::worker::now(),
                    None,
                )
                .unwrap();
        }
        store.record_github_user(42).unwrap();
        drop(store);
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let client =
            ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
        let serving = std::thread::spawn(move || {
            let now = crate::issues::worker::now();
            let mut value = json!({"data":{"number":1,"state":"closed","merged":true,"user":{"id":42},"title":"Cached merge","merged_at":"2026-10-05T21:04:49Z","base":{"repo":{"full_name":"o/r"}}},"validated_at_ms":now,"fetched_at_ms":now,"source":"cache"});
            {
                let incoming = server
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .expect("Read cached merge metadata without waiting for policy");
                assert_eq!(incoming.url(), "/v1/prs/o/r/1/metadata?cached_only=true");
                match evidence {
                    Evidence::Merged | Evidence::Ordinary | Evidence::ClosedTask => {}
                    Evidence::InvalidTime => value["validated_at_ms"] = json!(0),
                    Evidence::Stale | Evidence::NewerRecord => {
                        value["validated_at_ms"] = json!(now - 300_000)
                    }
                    Evidence::Future => value["validated_at_ms"] = json!(now + 300_000),
                    Evidence::WrongIdentity => value["data"]["number"] = json!(2),
                    Evidence::Open | Evidence::Closed => {
                        value["data"]["state"] = json!(if matches!(evidence, Evidence::Open) {
                            "open"
                        } else {
                            "closed"
                        });
                        value["data"]["merged"] = json!(false);
                    }
                }
                incoming
                    .respond(
                        tiny_http::Response::from_string(value.to_string()).with_header(
                            tiny_http::Header::from_bytes("Content-Type", "application/json")
                                .unwrap(),
                        ),
                    )
                    .unwrap();
            }
            assert!(
                server
                    .recv_timeout(Duration::from_millis(100))
                    .unwrap()
                    .is_none()
            );
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(poll(&ctx, &client, &mut None)).unwrap();
        serving.join().unwrap();
        let mut store = Store::open(&ctx.path).unwrap();
        let view = store
            .execute(&request(json!({"action":"view","number":1})))
            .unwrap();
        let history = store
            .execute(&request(
                json!({"action":"merged_pull_requests","limit":100,"offset":0}),
            ))
            .unwrap();
        if !matches!(
            evidence,
            Evidence::Merged | Evidence::Stale | Evidence::Ordinary | Evidence::ClosedTask
        ) {
            assert_ne!(view["issue"]["state"], "closed");
            assert_eq!(view["issue"]["assignee"], "watcher:github");
            assert!(history["pull_requests"].as_array().unwrap().is_empty());
        } else {
            assert_eq!(view["issue"]["state"], "closed");
            assert_eq!(history["pull_requests"][0]["title"], "Cached merge");
            assert_eq!(history["pull_requests"][0]["merged_at"], 1791234289000_i64);
        }
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
