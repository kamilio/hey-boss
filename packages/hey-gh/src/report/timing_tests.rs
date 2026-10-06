use crate::{Client, Config, Error, Freshness};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_report_reads_log_the_wait_even_when_the_caller_cancels() {
    // Isolate INFO callsite interest from concurrently untraced tests.
    const CHILD: &str = "HEY_GH_REPORT_TIMING_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "report::timing_tests::blocked_report_reads_log_the_wait_even_when_the_caller_cancels",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let captured = Logs(Arc::new(Mutex::new(Vec::new())));
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(
        Config {
            cache_path: dir.path().join("cache.sqlite"),
            report_timeout: Duration::from_millis(250),
            ..Config::default()
        },
        "private-test-token".into(),
    )
    .unwrap();
    for kind in ["pr", "ci"] {
        captured.0.lock().unwrap().clear();
        let suffix = if kind == "ci" { ":ci" } else { "" };
        let lock = client.report_lock(&format!("private-org/private-repo#7{suffix}"));
        let _owner = lock.lock().await;
        let client = &client;
        let read = |repo| async move {
            if kind == "ci" {
                client
                    .ci_for_pr(repo, 7, Freshness::default())
                    .await
                    .map(|_| ())
            } else {
                client
                    .pr_report(repo, 7, Freshness::default())
                    .await
                    .map(|_| ())
            }
        };
        assert!(matches!(
            read("private-org/private-repo").await,
            Err(Error::Deadline)
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(150), read("PRIVATE-ORG/PRIVATE-REPO"))
                .await
                .is_err()
        );
        let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        let reads: Vec<_> = logs
            .lines()
            .filter(|line| line.contains("PR evidence read finished"))
            .collect();
        assert_eq!(
            reads.len(),
            2,
            "deadline/cancellation lost phase evidence: {logs}"
        );
        assert!(reads[0].contains("outcome=\"error\""), "{logs}");
        assert!(reads[0].contains("error_code=\"deadline\""), "{logs}");
        assert!(reads[1].contains("outcome=\"interrupted\""), "{logs}");
        let target = crate::digest("private-org/private-repo#7");
        let mut ids = Vec::new();
        for line in reads {
            assert!(line.contains(&format!("target_key={target}")), "{line}");
            assert!(line.contains(&format!("kind=\"{kind}\"")), "{line}");
            assert!(line.contains("phase=\"report_lock\""), "{line}");
            assert!(
                line.contains("seed_ms=0") && line.contains("collection_ms=0"),
                "{line}"
            );
            let field = |name: &str| {
                line.split_whitespace()
                    .find_map(|part| part.strip_prefix(name))
                    .unwrap()
            };
            assert!(field("lock_ms=").parse::<u64>().unwrap() >= 100, "{line}");
            let id = field("read_id=");
            assert_eq!(id.len(), 32);
            assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
            ids.push(id);
        }
        assert_ne!(ids[0], ids[1]);
        assert!(
            !logs.contains("private-test-token")
                && !logs.contains("private-org")
                && !logs.contains("PRIVATE-ORG"),
            "{logs}"
        );
    }
    assert_collection_wait_is_logged(&captured).await;
}

async fn assert_collection_wait_is_logged(captured: &Logs) {
    use serde_json::json;
    let head = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let base = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let router = axum::Router::new().fallback(move |uri: axum::http::Uri| async move {
        let path = uri.path();
        axum::Json(if path.ends_with("/pulls/7") {
            json!({"node_id":"PR_7","number":7,"state":"open","merged":false,"mergeable":true,
                "head":{"sha":head},"base":{"ref":"main","sha":base},"merge_commit_sha":null})
        } else if path.contains("/actions/runs") {
            tokio::time::sleep(Duration::from_millis(1100)).await;
            json!({"workflow_runs":[]})
        } else if path.ends_with("/check-runs") {
            json!({"check_runs":[]})
        } else if path.ends_with("/status") {
            json!({"statuses":[]})
        } else if path.ends_with("/graphql") {
            json!({"data":{"repository":{"pullRequest":{
                "reviewThreads":{"nodes":[],"pageInfo":{"hasNextPage":false}},
                "timelineItems":{"nodes":[],"pageInfo":{"hasNextPage":false}}
            }}}})
        } else {
            json!([])
        })
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_token(
        Config {
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            cache_path: dir.path().join("cache.sqlite"),
            min_spacing: Duration::ZERO,
            ..Config::default()
        },
        "private-test-token".into(),
    )
    .unwrap();
    captured.0.lock().unwrap().clear();
    let first = client
        .pr_report("private-org/private-repo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(first.complete, "{first:?}");
    let returned = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    captured.0.lock().unwrap().clear();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(200),
            client.pr_report("private-org/private-repo", 7, Freshness::Revalidate)
        )
        .await
        .is_err()
    );
    let cancelled = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    server.abort();
    for (logs, outcome, minimum) in [
        (&returned, "returned", 1000),
        (&cancelled, "interrupted", 100),
    ] {
        for kind in ["ci", "pr"] {
            let line = logs
                .lines()
                .find(|line| {
                    line.contains("PR evidence read finished")
                        && line.contains(&format!("kind=\"{kind}\""))
                })
                .unwrap_or_else(|| panic!("{logs}"));
            assert!(line.contains(&format!("outcome=\"{outcome}\"")), "{line}");
            assert!(line.contains("attempts=1"), "{line}");
            let collection: u64 = line
                .split_whitespace()
                .find_map(|p| p.strip_prefix("collection_ms="))
                .unwrap()
                .parse()
                .unwrap();
            assert!(collection >= minimum, "{line}");
            if outcome == "interrupted" {
                assert!(
                    line.contains("phase=\"collection\"") && line.contains("confirmation_ms=0"),
                    "{line}"
                );
            } else {
                assert!(line.contains("complete=true"), "{line}");
            }
        }
    }
}
