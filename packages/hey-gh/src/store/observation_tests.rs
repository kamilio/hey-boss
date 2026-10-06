use super::*;
use std::time::Duration;

#[tokio::test(flavor = "current_thread")]
async fn observation_phases_explain_external_writer_wait_after_caller_cancellation() {
    const CHILD: &str = "HEY_GH_OBSERVATION_TIMING_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "store::observation_tests::observation_phases_explain_external_writer_wait_after_caller_cancellation", "--test-threads=1"])
            .env(CHILD, "1").output().await.unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    #[derive(Clone)]
    struct Logs(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Logs {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let logs = Logs(Arc::new(Mutex::new(Vec::new())));
    let writer = logs.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let store = Store::open(&path, Duration::from_secs(3600), 100, 4096).unwrap();
    let external = Connection::open(&path).unwrap();
    external.execute_batch("BEGIN IMMEDIATE").unwrap();
    let value = serde_json::json!({"private_body":"private_token"});
    let mut cancelled = Box::pin(store.observe("private_scope", "private_resource", &value));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(cancelled.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(cancelled);
    tokio::time::sleep(Duration::from_millis(400)).await;
    external.execute_batch("COMMIT").unwrap();
    store.run(|_| Ok(())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if String::from_utf8_lossy(&logs.0.lock().unwrap())
                .contains("Cache observation phases finished")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled observation lost phase diagnostics");
    assert_eq!(
        store
            .snapshot("private_scope", "private_resource")
            .await
            .unwrap(),
        Some(value)
    );
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    let line = text
        .lines()
        .find(|line| line.contains("Cache observation phases finished"))
        .unwrap();
    let field = |name: &str| -> u64 {
        line.split_whitespace()
            .find_map(|part| part.strip_prefix(name))
            .unwrap()
            .parse()
            .unwrap()
    };
    assert!(field("transaction_ms=") >= 250, "{text}");
    assert!(field("encode_ms=") < field("transaction_ms="), "{text}");
    for field in [
        "read_ms=",
        "write_ms=",
        "prune_ms=",
        "commit_ms=",
        "maintenance_ms=",
        "payload_bytes=",
    ] {
        assert!(line.contains(field), "{text}");
    }
    for private in [
        "private_scope",
        "private_resource",
        "private_body",
        "private_token",
    ] {
        assert!(!text.contains(private), "{text}");
    }
}
