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
async fn blocked_policy_reads_log_the_wait_even_when_the_caller_cancels() {
    // Isolate INFO callsite interest from concurrently untraced tests.
    const CHILD: &str = "HEY_GH_POLICY_TIMING_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "policy::timing_tests::blocked_policy_reads_log_the_wait_even_when_the_caller_cancels",
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
    let lock = client.report_lock("required-checks:private-org/private-repo#7");
    let _owner = lock.lock().await;
    assert!(matches!(
        client
            .required_checks_for_pr("private-org/private-repo", 7, Freshness::CachedOnly)
            .await,
        Err(Error::Deadline)
    ));
    assert!(
        tokio::time::timeout(
            Duration::from_millis(150),
            client.required_checks_for_pr("PRIVATE-ORG/PRIVATE-REPO", 7, Freshness::CachedOnly)
        )
        .await
        .is_err()
    );
    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let reads: Vec<_> = logs
        .lines()
        .filter(|line| line.contains("Required-check read finished"))
        .collect();
    assert_eq!(
        reads.len(),
        2,
        "deadline/cancellation dropped the local wait evidence: {logs}"
    );
    assert!(reads[0].contains("outcome=\"error\""), "{logs}");
    assert!(reads[0].contains("error_code=\"deadline\""), "{logs}");
    assert!(reads[1].contains("outcome=\"interrupted\""), "{logs}");
    let target = crate::digest("required-checks:private-org/private-repo#7");
    let mut ids = Vec::new();
    for line in reads {
        assert!(line.contains(&format!("target_key={target}")), "{line}");
        assert!(line.contains("phase=\"report_lock\""), "{line}");
        assert!(
            line.contains("seed_ms=0") && line.contains("ci_ms=0"),
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
