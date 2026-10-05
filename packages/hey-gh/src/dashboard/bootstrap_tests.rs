use super::*;

fn row(number: u64, title: &str, open: bool) -> Value {
    json!({"pullRequest": {"id":format!("PR_{number}"),"number":number,
        "repository":{"nameWithOwner":"Acme/Demo"},"title":title,
        "state":if open {"OPEN"} else {"CLOSED"},"removed":!open,
        "body":"x".repeat(600),"complete":true,"sourceErrors":{}}})
}

async fn observe(client: &Client, number: u64, title: &str, open: bool) {
    client
        .observe(
            &format!("pr-status://github.com/Acme/Demo/{number}"),
            &row(number, title, open),
        )
        .await
        .unwrap();
}

fn config(dir: &tempfile::TempDir) -> crate::Config {
    crate::Config {
        cache_path: dir.path().join("cache.sqlite"),
        max_collection_bytes: 256,
        ..Default::default()
    }
}

#[tokio::test]
async fn paged_pr_bootstrap_resumes_after_restart_and_replays_concurrent_changes() {
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(config(&dir), "synthetic".into()).unwrap();
    for number in [2, 4, 6] {
        observe(&client, number, "before", true).await;
    }
    let first = client
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .expect("a large roster must page within the original byte budget");
    assert!(first.has_more);
    assert_eq!(first.pull_requests.len(), 1);
    assert_eq!(first.pull_requests[0]["number"], 2);
    client
        .validate_pr_status_cursor(Some("ACME/DEMO"), &first.cursor)
        .await
        .unwrap();
    assert!(
        client
            .validate_pr_status_cursor(None, &first.cursor)
            .await
            .is_err()
    );
    observe(&client, 2, "closed during scan", false).await;
    observe(&client, 1, "inserted before scan position", true).await;
    observe(&client, 4, "updated during scan", true).await;
    drop(client);
    let client = Client::with_token(config(&dir), "synthetic".into()).unwrap();
    let mut mirror: BTreeMap<u64, Value> = first
        .pull_requests
        .into_iter()
        .map(|row| (row["number"].as_u64().unwrap(), row))
        .collect();
    let mut cursor = first.cursor;
    let mut drained = false;
    for _ in 0..20 {
        let page = client
            .pr_status_page(Some("ACME/DEMO"), Some(&cursor), 1000, Duration::ZERO)
            .await
            .unwrap();
        assert!(
            page.pull_requests.is_empty(),
            "continuations use the existing change feed contract"
        );
        for change in page.changes {
            let number = change.pull_request["number"].as_u64().unwrap();
            if change.pull_request["removed"] == true {
                mirror.remove(&number);
            } else {
                mirror.insert(number, change.pull_request);
            }
        }
        cursor = page.cursor;
        if !page.has_more {
            drained = true;
            break;
        }
    }
    assert!(drained);
    assert_eq!(mirror.keys().copied().collect::<Vec<_>>(), [1, 4, 6]);
    assert_eq!(mirror[&4]["title"], "updated during scan");
    assert!(
        client
            .pr_status_page(Some("acme/demo"), Some(&cursor), 1000, Duration::ZERO)
            .await
            .unwrap()
            .changes
            .is_empty()
    );
}

#[tokio::test]
async fn paged_pr_bootstrap_projection_keeps_raw_byte_boundaries_and_single_row_limit() {
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(config(&dir), "synthetic".into()).unwrap();
    for number in [2, 4, 6] {
        observe(&client, number, "before", true).await;
    }
    let first = client
        .pr_status_page_projected(None, None, 1000, Duration::ZERO, Some(&["number"]))
        .await
        .unwrap();
    assert!(first.has_more);
    assert_eq!(
        first.pull_requests.len(),
        1,
        "small output cannot bypass raw storage byte limits"
    );
    assert!(first.pull_requests[0].get("body").is_none());
    let mut oversized = row(7, "oversized", true);
    oversized["pullRequest"]["body"] = json!("x".repeat(5000));
    client
        .observe("pr-status://github.com/Acme/Demo/7", &oversized)
        .await
        .unwrap();
    let mut cursor = first.cursor;
    let mut rejected = false;
    for _ in 0..10 {
        match client
            .pr_status_page_projected(None, Some(&cursor), 1000, Duration::ZERO, Some(&["number"]))
            .await
        {
            Ok(page) => {
                cursor = page.cursor;
                if !page.has_more {
                    break;
                }
            }
            Err(Error::Invalid(message)) => {
                assert!(message.contains("byte limit"));
                rejected = true;
                break;
            }
            Err(error) => panic!("unexpected error: {error}"),
        }
    }
    assert!(
        rejected,
        "an indivisible oversized row still fails explicitly"
    );
}

#[tokio::test]
async fn paged_pr_bootstrap_rejects_expired_and_other_account_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&dir);
    config.max_change_events = 2;
    let client = Client::with_token(config.clone(), "synthetic".into()).unwrap();
    for number in [2, 4, 6] {
        observe(&client, number, "before", true).await;
    }
    let first = client
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(first.has_more);
    let other = Client::with_token(config, "other-synthetic".into()).unwrap();
    assert!(
        other
            .validate_pr_status_cursor(None, &first.cursor)
            .await
            .is_err()
    );
    assert!(
        other
            .pr_status_page(None, Some(&first.cursor), 1000, Duration::ZERO)
            .await
            .is_err()
    );
    for number in [2, 4, 6] {
        observe(&client, number, "after", true).await;
    }
    assert!(matches!(
        client.validate_pr_status_cursor(None, &first.cursor).await,
        Err(Error::CursorExpired)
    ));
    assert!(matches!(
        client
            .pr_status_page(None, Some(&first.cursor), 1000, Duration::ZERO)
            .await,
        Err(Error::CursorExpired)
    ));
}

#[tokio::test]
async fn paged_pr_bootstrap_crosses_http_with_projected_continuations() {
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(config(&dir), "synthetic".into()).unwrap();
    for number in [2, 4, 6] {
        observe(&client, number, "before", true).await;
    }
    client
        .observe(&client.roster_resource(), &json!([]))
        .await
        .unwrap();
    let api = crate::api::Api::new(client.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sdk = crate::ApiClient::new(
        format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, api.router()).await.unwrap();
    });
    let mut cursor = None;
    let mut numbers = Vec::new();
    let mut drained = false;
    for _ in 0..10 {
        let page = sdk
            .pr_status_selected(
                crate::PrStatusSelection {
                    repository: Some("ACME/DEMO"),
                    cursor: cursor.as_deref(),
                    fields: Some(&["number"]),
                },
                1000,
                Duration::ZERO,
                Freshness::CachedOnly,
            )
            .await
            .unwrap();
        assert!(page.complete);
        assert_eq!(page.coverage.as_ref().unwrap().returned_rows, 1);
        for row in page
            .pull_requests
            .iter()
            .chain(page.changes.iter().map(|c| &c.pull_request))
        {
            assert!(row.get("body").is_none());
            assert_eq!(row["complete"], true);
            numbers.push(row["number"].as_u64().unwrap());
        }
        cursor = Some(page.cursor);
        if !page.has_more {
            drained = true;
            break;
        }
    }
    server.abort();
    assert!(drained);
    assert_eq!(numbers, [2, 4, 6]);
    assert!(client.watches().await.unwrap().is_empty());
}

#[tokio::test]
async fn paged_pr_bootstrap_bounds_row_count_even_when_bodies_fit() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&dir);
    config.max_collection_bytes = 1024 * 1024;
    let client = Client::with_token(config, "synthetic".into()).unwrap();
    let rows: Vec<_> = (1..=1001)
        .map(|number| {
            (
                format!("pr-status://github.com/Acme/Demo/{number}"),
                row(number, "before", true),
            )
        })
        .collect();
    client.observe_many(&rows).await.unwrap();
    let first = client
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(first.pull_requests.len(), 1000);
    assert!(first.has_more);
    let last = client
        .pr_status_page(None, Some(&first.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(last.changes.len(), 1);
    assert!(!last.has_more);
    let numbers: BTreeSet<_> = first
        .pull_requests
        .iter()
        .chain(last.changes.iter().map(|c| &c.pull_request))
        .map(|row| row["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers.len(), 1001);
}
