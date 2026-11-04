use super::*;
use crate::pages;
use std::{fs, io::Read};

fn peer(f: &Fixture, name: &str) -> Connection {
    let db = f.db();
    install(&db, name).unwrap();
    db.execute("UPDATE fleet_meta SET role='peer',node=?1", [name])
        .unwrap();
    db.execute_batch("UPDATE distributed_meta SET active=1,controller='a';")
        .unwrap();
    db
}
fn publish(f: &Fixture, text: &str) -> pages::Page {
    let source = f.0.join("report.html");
    fs::write(&source, text).unwrap();
    pages::publish(&f.0.join("issues.db"), &source, "report").unwrap()
}
fn body(f: &Fixture) -> String {
    let database = f.0.join("issues.db");
    let page = pages::view(&database, "report").unwrap();
    assert!(!page.syncing);
    let mut text = String::new();
    pages::content(&database, &page.deployment, "index.html")
        .unwrap()
        .reader
        .read_to_string(&mut text)
        .unwrap();
    text
}
#[test]
fn pages_publish_offline_replicate_through_a_peer_and_converge_after_concurrent_updates_and_deletion()
 {
    let fa = Fixture::new();
    let fb = Fixture::new();
    let fc = Fixture::new();
    let a = peer(&fa, "a");
    let b = peer(&fb, "b");
    let c = peer(&fc, "c");
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&fb.0.join("sync.db")).unwrap();
    let mut ec = hey_boss_sync::Engine::open(&fc.0.join("sync.db")).unwrap();
    publish(&fb, "Offline from b"); // No supervisor or network is running.
    assert_eq!(body(&fb), "Offline from b");
    capture_all(&b, &mut eb);
    transfer(&eb, &mut ec);
    capture_all(&c, &mut ec);
    assert_eq!(body(&fc), "Offline from b");
    transfer(&ec, &mut ea);
    capture_all(&a, &mut ea);
    assert_eq!(body(&fa), "Offline from b");
    let head = eb.head().unwrap();
    publish(&fb, "Offline from b");
    capture_all(&b, &mut eb);
    assert_eq!(
        eb.head().unwrap(),
        head,
        "Identical publish must not resend file facts"
    );
    publish(&fa, "Concurrent a");
    publish(&fb, "Concurrent b");
    capture_all(&a, &mut ea);
    capture_all(&b, &mut eb);
    transfer(&ea, &mut eb);
    transfer(&eb, &mut ea);
    capture_all(&a, &mut ea);
    capture_all(&b, &mut eb);
    assert_eq!(body(&fa), body(&fb));
    pages::delete(&fa.0.join("issues.db"), "report").unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    assert!(pages::view(&fb.0.join("issues.db"), "report").is_err());
    // A stale offline replica rejoins after deletion. Its old files must not revive the page.
    transfer(&ec, &mut eb);
    capture_all(&b, &mut eb);
    assert!(pages::list(&fb.0.join("issues.db"), 0).unwrap().is_empty());
    transfer(&eb, &mut ec);
    capture_all(&c, &mut ec);
    assert!(pages::view(&fc.0.join("issues.db"), "report").is_err());
}

#[test]
fn interrupted_out_of_order_delivery_keeps_the_previous_complete_deployment() {
    let fa = Fixture::new();
    let fb = Fixture::new();
    let a = peer(&fa, "a");
    let b = peer(&fb, "b");
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&fb.0.join("sync.db")).unwrap();
    publish(&fa, "First");
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    assert_eq!(body(&fb), "First");
    let before = ea.head().unwrap();
    publish(&fa, "Second");
    capture_all(&a, &mut ea);
    let mut facts = ea.pending(before).unwrap();
    facts.reverse();
    for (_, _, payload) in &facts {
        let fact: Revision = serde_json::from_str(payload).unwrap();
        if fact.table != "published_chunks" {
            eb.put(payload).unwrap();
        }
    }
    capture_all(&b, &mut eb);
    let page = pages::view(&fb.0.join("issues.db"), "report").unwrap();
    assert!(page.syncing);
    let mut text = String::new();
    pages::content(&fb.0.join("issues.db"), &page.deployment, "index.html")
        .unwrap()
        .reader
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "First");
    for (_, _, payload) in &facts {
        eb.put(payload).unwrap();
    }
    capture_all(&b, &mut eb);
    assert_eq!(body(&fb), "Second");
}

#[test]
fn published_cdn_stress_partitions_duplicate_delivery_restarts_and_concurrent_names() {
    use serde_json::json;
    let fixtures: Vec<_> = (0..4).map(|_| Fixture::new()).collect();
    let databases: Vec<_> = fixtures
        .iter()
        .enumerate()
        .map(|(i, f)| peer(f, &format!("replica-{i}")))
        .collect();
    let mut engines: Vec<_> = fixtures
        .iter()
        .map(|f| hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap())
        .collect();
    // More than a journal page, shared multi-chunk assets, and many immutable versions.
    let mut seed = 0x91ab43u64;
    for round in 0..48 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let writer = seed as usize % fixtures.len();
        let f = &fixtures[writer];
        let name = format!("site-{}", round % 5);
        if round % 9 == 8 && pages::view(&f.0.join("issues.db"), &name).is_ok() {
            pages::delete(&f.0.join("issues.db"), &name).unwrap();
        } else {
            let source = f.0.join("site");
            fs::create_dir_all(source.join("assets")).unwrap();
            fs::write(
                source.join("index.html"),
                format!("<h1>Replica {writer}, revision {round}</h1>"),
            )
            .unwrap();
            // Unchanged shared content must not be resent on each deployment.
            fs::write(
                source.join("assets/shared.bin"),
                vec![77; pages::CHUNK_SIZE * 3 + 11],
            )
            .unwrap();
            for file in 0..12 {
                fs::write(
                    source.join(format!("assets/{file}.txt")),
                    format!("asset-{file}-version-{}", round / 12),
                )
                .unwrap();
            }
            let published = pages::publish(&f.0.join("issues.db"), &source, &name).unwrap();
            assert!(!published.syncing);
        }
        capture_all(&databases[writer], &mut engines[writer]);
        // Replica 3 remains partitioned for most of the run. Other connections flap.
        if round % 3 == 0 {
            let source = round % 3;
            let target = (source + 1) % 3;
            let batch = engines[source]
                .batch(engines[target].received(engines[source].id()).unwrap())
                .unwrap();
            engines[target].ingest(&batch).unwrap();
            engines[target].ingest(&batch).unwrap(); // lost acknowledgement / replay
            capture_all(&databases[target], &mut engines[target]);
        }
        if round % 7 == 0 {
            // Drop/reopen the engine with pending local and remote changes.
            engines[writer] = hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap();
        }
    }
    // Reconnect a ring, not a star: every replica must receive forwarded facts.
    for _ in 0..5 {
        for source in 0..4 {
            let target = (source + 1) % 4;
            let (from, to) = if source < target {
                let (left, right) = engines.split_at_mut(target);
                (&left[source], &mut right[0])
            } else {
                let (left, right) = engines.split_at_mut(source);
                (&right[0], &mut left[target])
            };
            transfer(from, to);
            capture_all(&databases[target], to);
        }
    }
    let snapshot = |db: &Connection| {
        replica::rows(db, "SELECT * FROM published_pages ORDER BY name", &[]).unwrap()
    };
    let expected = snapshot(&databases[0]);
    for i in 0..4 {
        assert_eq!(snapshot(&databases[i]), expected);
        assert_eq!(
            replica::rows(
                &databases[i],
                "SELECT * FROM published_chunks ORDER BY hash",
                &[]
            )
            .unwrap(),
            replica::rows(
                &databases[0],
                "SELECT * FROM published_chunks ORDER BY hash",
                &[]
            )
            .unwrap()
        );
        assert_eq!(
            databases[i]
                .query_row("SELECT count(*) FROM distributed_rejections", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        for page in pages::list(&fixtures[i].0.join("issues.db"), 0).unwrap() {
            assert!(!page.syncing);
            let out = fixtures[i].0.join(format!("download-{}", page.name));
            pages::download(&fixtures[i].0.join("issues.db"), &page.name, &out).unwrap();
            assert_eq!(
                fs::read(out.join("assets/shared.bin")).unwrap(),
                vec![77; pages::CHUNK_SIZE * 3 + 11]
            );
        }
    }
    // Replaying the entire journal after checkpoint loss is idempotent.
    databases[3]
        .execute("UPDATE distributed_meta SET projected=0", [])
        .unwrap();
    capture_all(&databases[3], &mut engines[3]);
    assert_eq!(snapshot(&databases[3]), expected);
    // Five pages × 48 publications do not each get copies of the shared blob.
    assert_eq!(
        databases[0]
            .query_row(
                "SELECT count(*) FROM published_chunks WHERE hash=?1",
                [hey_boss_sync::digest(&vec![77; pages::CHUNK_SIZE])],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    eprintln!(
        "CDN stress: {}",
        json!({"replicas":4,"offline_mutations":48,"live_pages":expected.iter().filter(|p|p["deleted"]==0).count(),"journal_records":engines[0].head().unwrap()})
    );
}

#[test]
fn missing_and_corrupt_chunks_do_not_advance_export_and_repair_can_resume() {
    let f = Fixture::new();
    let db = peer(&f, "a");
    let mut engine = hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap();
    publish(&f, "Recoverable content");
    let hash = hey_boss_sync::digest(b"Recoverable content");
    let path = pages::root(&f.0.join("issues.db")).join(hash);
    let saved = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    let checkpoint: i64 = db
        .query_row("SELECT exported FROM distributed_meta", [], |r| r.get(0))
        .unwrap();
    assert!(export(&db, &mut engine).is_err());
    assert_eq!(
        db.query_row("SELECT exported FROM distributed_meta", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        checkpoint
    );
    fs::write(&path, b"corrupt").unwrap();
    assert!(export(&db, &mut engine).is_err());
    fs::write(&path, saved).unwrap();
    capture_all(&db, &mut engine);
    assert_eq!(body(&f), "Recoverable content");
}

#[test]
fn corrupted_incoming_page_resource_is_rejected_before_checkpoint_or_visibility() {
    let fa = Fixture::new();
    let fb = Fixture::new();
    let a = peer(&fa, "a");
    let b = peer(&fb, "b");
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let mut eb = hey_boss_sync::Engine::open(&fb.0.join("sync.db")).unwrap();
    publish(&fa, "Verified content");
    capture_all(&a, &mut ea);
    for (_, _, payload) in ea.pending(0).unwrap() {
        let mut value: Value = serde_json::from_str(&payload).unwrap();
        if value["table"] == "published_chunks" {
            value["resource"]["bytes"] = serde_json::json!("Y29ycnVwdA==");
            eb.put(&value.to_string()).unwrap();
            break;
        }
    }
    assert!(import(&b, &eb).is_err());
    assert_eq!(
        b.query_row("SELECT projected FROM distributed_meta", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        b.query_row("SELECT count(*) FROM published_chunks", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(pages::view(&fb.0.join("issues.db"), "report").is_err());
}
