use super::*;
use serde_json::json;

fn fact(clock: i64, table: &str, row: Value) -> String {
    serde_json::to_string(&Revision {
        protocol: PROTOCOL,
        origin: "remote".into(),
        clock,
        table: table.into(),
        before: None,
        after: Some(row),
        resource: None,
        archive: None,
    })
    .unwrap()
}
fn active() -> (Fixture, Connection, hey_boss_sync::Engine) {
    let f = Fixture::new();
    let db = f.db();
    install(&db, "a").unwrap();
    db.execute("UPDATE distributed_meta SET active=1", [])
        .unwrap();
    let e = hey_boss_sync::Engine::open(&f.0.join("sync.db")).unwrap();
    (f, db, e)
}
fn count(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn incompatible_row_is_retained_and_replayed_with_its_later_edit() {
    let (_f, db, mut e) = active();
    for (clock, row) in [
        (
            1,
            json!({"id":"held","metadata":"{}","last_seen":1,"future":9}),
        ),
        (2, json!({"id":"held","metadata":"{}","last_seen":2})),
        (3, json!({"id":"free","metadata":"{}","last_seen":3})),
    ] {
        e.put(&fact(clock, "agents", row)).unwrap();
    }
    capture_all(&db, &mut e);
    assert_eq!(count(&db, "SELECT count(*) FROM agents WHERE id='free'"), 1);
    assert_eq!(count(&db, "SELECT count(*) FROM agents WHERE id='held'"), 0);
    assert_eq!(count(&db, "SELECT count(*) FROM distributed_deferred"), 2);
    db.execute_batch(
        "ALTER TABLE agents ADD COLUMN future INTEGER; UPDATE distributed_deferred SET retry_at=0",
    )
    .unwrap();
    for _ in 0..4 {
        import(&db, &e).unwrap();
    }
    assert_eq!(
        count(
            &db,
            "SELECT last_seen FROM agents WHERE id='held' AND future=9"
        ),
        2
    );
    assert_eq!(count(&db, "SELECT count(*) FROM distributed_deferred"), 0);
}

#[test]
fn incoming_rows_are_visible_while_remote_traffic_continues() {
    let (_f, db, mut e) = active();
    for i in 0..129 {
        e.put(&fact(
            i,
            "agents",
            json!({"id":format!("test:{i}"),"metadata":"{}","last_seen":i}),
        ))
        .unwrap();
    }
    assert_eq!(import(&db, &e).unwrap(), 128);
    assert_eq!(
        count(&db, "SELECT count(*) FROM agents WHERE id LIKE 'test:%'"),
        128
    );
}

#[test]
fn recovered_parent_outside_first_rejected_page_is_retried() {
    let (_f, db, mut e) = active();
    for i in 0..129 {
        e.put(&fact(i+1,"artifacts",json!({"project_id":if i==128 {"later"} else {"absent"},"id":format!("a-{i:03}"),"title":"document","body":"x","version":1,"archived":0,"created_at":1,"updated_at":1}))).unwrap();
    }
    for _ in 0..5 {
        import(&db, &e).unwrap();
    }
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM distributed_rejections WHERE table_name='artifacts'"
        ),
        129
    );
    e.put(&fact(130, "projects", json!({"id":"later","name":"Later"})))
        .unwrap();
    for _ in 0..4 {
        import(&db, &e).unwrap();
    }
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM artifacts WHERE project_id='later'"
        ),
        1
    );
}

#[test]
fn large_review_message_edit_exports_even_from_an_old_outbox() {
    let (_f, db, mut e) = active();
    let snapshot = json!({"patch":"\\".repeat(5*1024*1024)}).to_string();
    db.execute(
        "INSERT INTO commit_reviews VALUES('p',1,'abc',?1,'first')",
        [&snapshot],
    )
    .unwrap();
    while export(&db, &mut e).unwrap() > 0 {}
    db.execute("UPDATE commit_reviews SET message='second'", [])
        .unwrap();
    assert!(
        count(
            &db,
            "SELECT length(before_json)+length(after_json) FROM distributed_outbox WHERE table_name='commit_reviews' ORDER BY clock DESC LIMIT 1"
        ) < 1024
    );
    // Simulate a full update already captured by an older binary.
    db.execute("UPDATE distributed_outbox SET before_json=json_set(before_json,'$.snapshot',?1),after_json=json_set(after_json,'$.snapshot',?1) WHERE table_name='commit_reviews' AND before_json IS NOT NULL",[&snapshot]).unwrap();
    db.execute("INSERT INTO agents VALUES('later','{}',0)", [])
        .unwrap();
    while export(&db, &mut e).unwrap() > 0 {}
    let (_g, other, mut r) = active();
    transfer(&e, &mut r);
    capture_all(&other, &mut r);
    assert_eq!(
        count(&other, "SELECT count(*) FROM agents WHERE id='later'"),
        1
    );
    let stored: String = other
        .query_row(
            "SELECT snapshot FROM commit_reviews WHERE message='second'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, snapshot);
}
