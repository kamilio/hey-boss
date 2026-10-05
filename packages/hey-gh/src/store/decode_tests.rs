use super::*;
use std::sync::{
    Condvar,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[tokio::test]
async fn payload_decoding_releases_the_reader_and_keeps_its_captured_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("cache.sqlite"),
        Duration::from_secs(3600),
        100,
        4096,
    )
    .unwrap();
    let resource = "metadata://github.com/acme/repo/7";
    let before = serde_json::json!({"value":"before"});
    store.observe("scope", resource, &before).await.unwrap();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let decoding = tokio::spawn({
        let store = store.clone();
        async move {
            store
                .read_decode(
                    move |conn| {
                        conn.query_row(
                            "SELECT data,hash FROM snapshots WHERE scope='scope' AND resource=?1",
                            [resource],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                        )
                        .map_err(storage)
                    },
                    move |(data, hash)| {
                        entered.send(()).unwrap();
                        held.recv_timeout(Duration::from_secs(5)).map_err(storage)?;
                        Ok((serde_json::from_str::<Value>(&data).map_err(storage)?, hash))
                    },
                )
                .await
        }
    });
    ready.await.unwrap();
    let after = serde_json::json!({"value":"after"});
    store.observe("scope", resource, &after).await.unwrap();
    let lookup = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(
            store.pr_resource_key("scope", resource),
            store.repository_generation("scope", "acme/repo")
        )
    })
    .await;
    release.send(()).unwrap();
    let captured = decoding.await.unwrap().unwrap();
    let (key, generation) = lookup.expect("JSON decoding held the shared identity/metadata reader");
    assert_eq!(key.unwrap(), resource);
    assert_eq!(generation.unwrap(), 0);
    assert_eq!(captured, (before.clone(), digest(&before.to_string())));
    assert_eq!(
        store.snapshot("scope", resource).await.unwrap(),
        Some(after)
    );
}

#[tokio::test]
async fn decoding_admission_is_bounded_and_progresses_after_cancelled_or_paused_callers() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("cache.sqlite"),
        Duration::from_secs(3600),
        100,
        4096,
    )
    .unwrap();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let fetched = Arc::new(AtomicUsize::new(0));
    let decoding = Arc::new(AtomicUsize::new(0));
    let mut readers = Vec::new();
    for number in 0..8 {
        let (store, gate, fetched, decoding) = (
            store.clone(),
            gate.clone(),
            fetched.clone(),
            decoding.clone(),
        );
        readers.push(tokio::spawn(async move {
            store
                .read_decode(
                    move |conn| {
                        let _: i64 = conn
                            .query_row("SELECT 1", [], |row| row.get(0))
                            .map_err(storage)?;
                        fetched.fetch_add(1, Ordering::SeqCst);
                        Ok(number)
                    },
                    move |number| {
                        decoding.fetch_add(1, Ordering::SeqCst);
                        let (lock, wake) = &*gate;
                        let (_guard, timeout) = wake
                            .wait_timeout_while(
                                lock.lock().unwrap(),
                                Duration::from_secs(5),
                                |released| !*released,
                            )
                            .unwrap();
                        assert!(!timeout.timed_out());
                        Ok(number)
                    },
                )
                .await
        }));
    }
    let admitted = tokio::time::timeout(Duration::from_secs(2), async {
        while decoding.load(Ordering::SeqCst) < 4 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let bounded = fetched.load(Ordering::SeqCst);
    readers[0].abort();
    let mut paused = Box::pin(store.get("scope", "paused"));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(paused.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    {
        let (lock, wake) = &*gate;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }
    for (index, reader) in readers.into_iter().enumerate() {
        let result = reader.await;
        if index != 0 {
            result.unwrap().unwrap();
        }
    }
    assert!(
        admitted.is_ok(),
        "decoding did not release the database reader"
    );
    assert_eq!(
        bounded, 4,
        "read buffers must be bounded before fetching payloads"
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(1), store.get("scope", "next"))
            .await
            .expect("paused decoder admission blocked a peer")
            .unwrap()
            .is_none()
    );
    assert!(paused.await.unwrap().is_none());
}
