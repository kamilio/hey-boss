use super::*;
use std::time::Duration;

#[test]
fn queued_writers_leave_blocking_threads_for_reads_and_survive_paused_or_cancelled_callers() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(2)
        .build()
        .unwrap();
    runtime.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("cache.sqlite"),
            Duration::from_secs(3600),
            100,
            4096,
        )
        .unwrap();
        let value = serde_json::json!({"value":"available during a write"});
        store.observe("scope", "source", &value).await.unwrap();
        store
            .run(|conn| {
                conn.execute_batch(
                    "CREATE TABLE writer_test_events(seq INTEGER PRIMARY KEY,name TEXT)",
                )
                .map_err(storage)
            })
            .await
            .unwrap();

        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let owner = tokio::spawn({
            let store = store.clone();
            async move {
                store
                    .run(move |conn| {
                        let tx = conn.transaction().map_err(storage)?;
                        tx.execute("INSERT INTO writer_test_events(name) VALUES('owner')", [])
                            .map_err(storage)?;
                        entered.send(()).unwrap();
                        held.recv_timeout(Duration::from_secs(5)).map_err(storage)?;
                        tx.commit().map_err(storage)
                    })
                    .await
            }
        });
        ready.await.unwrap();

        let queued = |name: &'static str| {
            store.run(move |conn| {
                conn.execute("INSERT INTO writer_test_events(name) VALUES(?1)", [name])
                    .map_err(storage)?;
                Ok(())
            })
        };
        let mut paused = Box::pin(queued("paused"));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(paused.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        let mut cancelled = Box::pin(queued("cancelled"));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(cancelled.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        drop(cancelled);

        // One thread owns the write transaction. Queued writers must leave the
        // other thread available to the independent WAL reader and JSON decoder.
        let read =
            tokio::time::timeout(Duration::from_secs(1), store.snapshot("scope", "source")).await;
        release.send(()).unwrap();
        owner.await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(2), queued("peer"))
            .await
            .expect("a paused writer caller reserved the writer connection")
            .unwrap();
        paused.await.unwrap();
        assert_eq!(
            read.expect("queued writers consumed the reader's blocking threads")
                .unwrap(),
            Some(value)
        );
        let events = store
            .read(|conn| {
                conn.prepare("SELECT name FROM writer_test_events ORDER BY seq")
                    .map_err(storage)?
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(storage)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(storage)
            })
            .await
            .unwrap();
        assert_eq!(events, ["owner", "paused", "cancelled", "peer"]);
    });
}

#[tokio::test(flavor = "current_thread")]
async fn slow_writer_logs_separate_queue_time_and_survive_caller_cancellation() {
    const CHILD: &str = "HEY_GH_WRITER_TIMING_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "store::writer_tests::slow_writer_logs_separate_queue_time_and_survive_caller_cancellation", "--test-threads=1"])
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
    let captured = Logs(Arc::new(Mutex::new(Vec::new())));
    let writer = captured.clone();
    // Only this isolated child owns the global subscriber, including blocking
    // writer threads whose caller may have already been cancelled.
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
    let store = Store::open(
        &dir.path().join("cache.sqlite"),
        Duration::from_secs(3600),
        100,
        4096,
    )
    .unwrap();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let owner = tokio::spawn({
        let store = store.clone();
        async move {
            store
                .run(move |_| {
                    entered.send(()).unwrap();
                    held.recv_timeout(Duration::from_secs(5)).map_err(storage)?;
                    Ok(())
                })
                .await
        }
    });
    ready.await.unwrap();
    let response = Response {
        data: serde_json::json!({"private-body":"private-token"}),
        fetched_at_ms: 1,
        validated_at_ms: 1,
        source: Source::Network,
        etag: None,
        last_modified: None,
        link: None,
    };
    let mut cancelled = Box::pin(store.put("private-scope", "private-resource", &response));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(cancelled.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(cancelled);
    tokio::time::sleep(Duration::from_millis(350)).await;
    release.send(()).unwrap();
    owner.await.unwrap().unwrap();
    store.run(|_| Ok(())).await.unwrap();
    assert_eq!(
        store
            .get("private-scope", "private-resource")
            .await
            .unwrap()
            .unwrap()
            .data,
        response.data
    );
    // The writer turn is released before logging. A later barrier can finish
    // first, so wait for the independent cancelled writer's diagnostic too.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let ready = String::from_utf8_lossy(&captured.0.lock().unwrap())
                .lines()
                .any(|line| line.contains("Store::put") && line.contains("work_ms="));
            if ready {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled writer never logged its completion");
    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let slow: Vec<_> = logs
        .lines()
        .filter(|line| line.contains("Cache writer operation finished"))
        .collect();
    assert!(
        slow.len() >= 2,
        "cancelled caller lost writer timing: {logs}"
    );
    let field = |line: &str, name: &str| -> u64 {
        line.split_whitespace()
            .find_map(|p| p.strip_prefix(name))
            .unwrap()
            .parse()
            .unwrap()
    };
    let owner_log = slow
        .iter()
        .find(|line| line.contains("slow_writer_logs"))
        .unwrap();
    let queued_log = slow
        .iter()
        .find(|line| line.contains("Store::put"))
        .unwrap();
    assert!(field(owner_log, "work_ms=") >= 300, "{logs}");
    assert!(field(queued_log, "queue_ms=") >= 300, "{logs}");
    assert!(queued_log.contains("succeeded=true"), "{logs}");
    for private in [
        "private-scope",
        "private-resource",
        "private-body",
        "private-token",
    ] {
        assert!(!logs.contains(private), "{logs}");
    }
}
