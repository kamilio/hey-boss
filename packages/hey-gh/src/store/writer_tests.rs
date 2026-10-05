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
