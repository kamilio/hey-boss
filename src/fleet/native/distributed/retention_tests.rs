use super::*;
use rusqlite::params;
use serde_json::json;

fn peer(f: &Fixture, node: &str) -> (Connection, hey_boss_sync::Engine) {
    let db = f.db();
    install(&db, node).unwrap();
    db.execute("UPDATE fleet_meta SET node=?1,role='peer'", [node])
        .unwrap();
    db.execute("UPDATE distributed_meta SET active=1", [])
        .unwrap();
    (
        db,
        hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap(),
    )
}

pub(super) fn compact(f: &Fixture, db: &Connection, engine: &mut hey_boss_sync::Engine) {
    // Exercise domain replay with a small fixture. Engine tests cover the real
    // 10,000-record/64-MiB limits; no production retention setting is exposed.
    let control = rusqlite::Connection::open(f.0.join("sync.db")).unwrap();
    control
        .execute(
            "UPDATE retention SET cursor=0,cutoff=?1,head=?1",
            [engine.head().unwrap()],
        )
        .unwrap();
    while retention::collect(db, engine).unwrap() {}
}

fn state(db: &Connection) -> Vec<Value> {
    replica::rows(
        db,
        "SELECT * FROM distributed_fields ORDER BY table_name,entity,field",
        &[],
    )
    .unwrap()
}

#[test]
fn compacted_database_replays_to_new_peer_and_merges_unsent_offline_edits() {
    let fa = Fixture::new();
    let fb = Fixture::new();
    let fc = Fixture::new();
    let (a, mut ea) = peer(&fa, "a");
    let (b, mut eb) = peer(&fb, "b");
    seed(&a);
    a.execute("INSERT INTO comments(id,project_id,issue_number,author,body,created_at) VALUES(1,'p',1,'human:test','original',1)",[]).unwrap();
    capture_all(&a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    let stale = eb.batch(0).unwrap();
    // Leave the edit in the application outbox while the other peer rotates.
    b.execute(
        "UPDATE issues SET title='Offline title' WHERE project_id='p' AND number=1",
        [],
    )
    .unwrap();
    for n in 0..300 {
        a.execute(
            "UPDATE issues SET body=?1 WHERE project_id='p' AND number=1",
            [format!("requirements-{n}")],
        )
        .unwrap();
    }
    a.execute("DELETE FROM comments WHERE id=1", []).unwrap();
    capture_all(&a, &mut ea);
    let head = ea.head().unwrap();
    let expected = state(&a);
    compact(&fa, &a, &mut ea);
    let compacted = ea.batch(0).unwrap();
    assert!(compacted.compacted > 0);
    assert!(
        compacted.records.len() < 20,
        "superseded bodies must disappear"
    );
    assert_eq!(ea.head().unwrap(), head);
    drop(ea);
    let mut ea = hey_boss_sync::Engine::open(&fa.0.join("sync.db")).unwrap();
    let (c, mut ec) = peer(&fc, "c");
    transfer(&ea, &mut ec);
    capture_all(&c, &mut ec);
    assert_eq!(
        state(&c),
        expected,
        "fresh replay preserves per-field versions and deletions"
    );

    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    transfer(&eb, &mut ea);
    capture_all(&a, &mut ea);
    ea.ingest(&stale).unwrap();
    capture_all(&a, &mut ea);
    compact(&fa, &a, &mut ea);
    transfer(&ea, &mut ec);
    capture_all(&c, &mut ec);
    for db in [&a, &b, &c] {
        assert_eq!(
            db.query_row(
                "SELECT title,body FROM issues WHERE project_id='p' AND number=1",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            )
            .unwrap(),
            ("Offline title".into(), "requirements-299".into())
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(state(&a), state(&c));
}

#[test]
fn independent_fields_and_cold_before_images_keep_their_replay_sources() {
    let fa = Fixture::new();
    let fb = Fixture::new();
    let (a, mut ea) = peer(&fa, "a");
    let (b, mut eb) = peer(&fb, "b");
    seed(&a);
    capture_all(&a, &mut ea);
    // This history predates enrollment: only edit before-images supply its
    // birth, author and parent. Later edits must not discard that baseline.
    let mut row = json!({"id":"a:7","project_id":"p","issue_number":1,"author":"human:test","body":"original","created_at":1});
    for clock in 20..25 {
        let before = row.clone();
        row["body"] = json!(format!("edit-{clock}"));
        let fact = Revision {
            protocol: PROTOCOL,
            origin: "a".into(),
            clock,
            table: "comments".into(),
            before: Some(before),
            after: Some(row.clone()),
            resource: None,
            archive: None,
        };
        ea.put(&serde_json::to_string(&fact).unwrap()).unwrap();
    }
    capture_all(&a, &mut ea);
    let expected = state(&a);
    compact(&fa, &a, &mut ea);
    transfer(&ea, &mut eb);
    capture_all(&b, &mut eb);
    assert_eq!(state(&b), expected);
    assert_eq!(
        b.query_row("SELECT body FROM comments", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "edit-24"
    );
}

#[test]
fn deferred_facts_and_separate_consumers_survive_rotation() {
    let f = Fixture::new();
    let (db, mut engine) = peer(&f, "a");
    seed(&db);
    capture_all(&db, &mut engine);
    let mut old = Revision {
        protocol: PROTOCOL,
        origin: "a".into(),
        clock: 20,
        table: "agents".into(),
        before: Some(json!({"id":"human:test","metadata":"{}","last_seen":0})),
        after: Some(json!({"id":"human:test","metadata":"{}","last_seen":1})),
        resource: None,
        archive: None,
    };
    let deferred = engine.put(&serde_json::to_string(&old).unwrap()).unwrap();
    let position = engine.head().unwrap();
    old.clock = 21;
    old.after.as_mut().unwrap()["last_seen"] = json!(2);
    engine.put(&serde_json::to_string(&old).unwrap()).unwrap();
    let secret = engine
        .put(&format!(
            r#"{{"protocol":2,"kind":"{}","revision":{{}}}}"#,
            hey_boss_secrets::KIND
        ))
        .unwrap();
    old.clock = 1_000_000;
    old.before = old.after.clone();
    let watermark = engine.put(&serde_json::to_string(&old).unwrap()).unwrap();
    capture_all(&db, &mut engine);
    db.execute(
        "INSERT INTO distributed_deferred VALUES(?1,?2,'agents','', 'fixture',9223372036854775807)",
        params![position, deferred],
    )
    .unwrap();
    compact(&f, &db, &mut engine);
    assert!(engine.payload(&deferred).unwrap().is_some());
    assert!(engine.payload(&secret).unwrap().is_some());
    assert!(engine.payload(&watermark).unwrap().is_some());
}
