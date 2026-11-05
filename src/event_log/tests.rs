use super::*;
use crate::{database::Connection, issues::Store};
use serde_json::json;

struct Fixture {
    root: std::path::PathBuf,
    db: Connection,
    engine: hey_boss_sync::Engine,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-events-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let db = Store::open(&root.join("issues.db"))
            .unwrap()
            .into_database();
        install(&db).unwrap();
        let engine = hey_boss_sync::Engine::open(&root.join("sync.db")).unwrap();
        Self { root, db, engine }
    }
    fn event(&self, id: &str, time: i64) -> Event {
        Event {
            id: id.into(),
            kind: "agent.failed".into(),
            version: 1,
            origin: "devbox".into(),
            time,
            project: Some("named:poe".into()),
            subject: "run-1".into(),
            data: json!({"code":"ENOSPC"}),
        }
    }
    fn page(&self, after: Option<&str>) -> Page {
        read(&self.db, &Filter::default(), after, 100).unwrap()
    }
    fn flush(&mut self) {
        while export(&self.db, &mut self.engine).unwrap() > 0 {}
    }
    fn send(&mut self, other: &mut Self) {
        self.flush();
        loop {
            let batch = self
                .engine
                .batch(other.engine.received(self.engine.id()).unwrap())
                .unwrap();
            if batch.records.is_empty() {
                break;
            }
            other.engine.ingest(&batch).unwrap();
        }
        let mut after = 0;
        loop {
            let facts = other.engine.pending(after).unwrap();
            if facts.is_empty() {
                break;
            }
            let tx = other.db.unchecked_transaction().unwrap();
            for (position, _, payload) in facts {
                receive(&tx, &payload).unwrap();
                after = position;
            }
            tx.commit().unwrap();
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn event_and_outbox_roll_back_with_the_business_transaction() {
    let f = Fixture::new();
    let event = f.event("one", 100);
    {
        let tx = f.db.unchecked_transaction().unwrap();
        assert!(append(&tx, &event).unwrap());
    }
    assert!(f.page(None).events.is_empty());
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM operational_event_outbox", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    let tx = f.db.unchecked_transaction().unwrap();
    assert!(append(&tx, &event).unwrap());
    tx.commit().unwrap();
    assert_eq!(f.page(None).events[0].event, event);
}

#[test]
fn lost_publish_ack_is_idempotent_but_reused_identity_cannot_change_evidence() {
    let f = Fixture::new();
    let mut event = f.event("one", 100);
    assert!(append(&f.db, &event).unwrap());
    assert!(!append(&f.db, &event).unwrap());
    event.data = json!({"code":"different"});
    assert!(append(&f.db, &event).is_err());
    assert_eq!(f.page(None).events.len(), 1);
}

#[test]
fn late_offline_events_arrive_after_checkpoint_even_with_older_timestamps() {
    let mut a = Fixture::new();
    let mut b = Fixture::new();
    append(&a.db, &a.event("newer", 200)).unwrap();
    a.send(&mut b);
    let checkpoint = b.page(None).cursor;
    append(&a.db, &a.event("late", 10)).unwrap();
    a.send(&mut b);
    let page = b.page(Some(&checkpoint));
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].event.id, "late");
}

#[test]
fn concurrent_offline_writes_merge_through_relay_without_echoing_or_duplicate_rows() {
    let mut a = Fixture::new();
    let mut b = Fixture::new();
    let mut relay = Fixture::new();
    append(&a.db, &a.event("a", 100)).unwrap();
    append(&b.db, &b.event("b", 100)).unwrap();
    a.send(&mut relay);
    b.send(&mut relay);
    relay.send(&mut a);
    relay.send(&mut b);
    a.send(&mut b);
    b.send(&mut a);
    for f in [&a, &b, &relay] {
        let mut ids: Vec<_> = f
            .page(None)
            .events
            .into_iter()
            .map(|e| e.event.id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(
            f.db.query_row("SELECT count(*) FROM operational_event_outbox", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
    }
}

#[test]
fn cursor_survives_reopen_but_is_rejected_by_another_replica_or_when_ahead() {
    let f = Fixture::new();
    append(&f.db, &f.event("one", 100)).unwrap();
    let cursor = f.page(None).cursor;
    let reopened = Store::open(&f.root.join("issues.db"))
        .unwrap()
        .into_database();
    assert!(
        read(&reopened, &Filter::default(), Some(&cursor), 100)
            .unwrap()
            .events
            .is_empty()
    );
    let other = Fixture::new();
    assert!(read(&other.db, &Filter::default(), Some(&cursor), 100).is_err());
    let ahead = format!("{}:999999", cursor.rsplit_once(':').unwrap().0);
    assert!(read(&f.db, &Filter::default(), Some(&ahead), 100).is_err());
}

#[test]
fn filtered_pages_advance_past_nonmatches_without_skipping_later_matches() {
    let f = Fixture::new();
    let checkpoint = f.page(None).cursor;
    for i in 0..5 {
        let mut event = f.event(&format!("event-{i}"), i);
        if i % 2 == 0 {
            event.kind = "job.failed".into();
        }
        append(&f.db, &event).unwrap();
    }
    let filter = Filter {
        kind: Some("job.*".into()),
        ..Default::default()
    };
    let first = read(&f.db, &filter, Some(&checkpoint), 2).unwrap();
    assert_eq!(
        first
            .events
            .iter()
            .map(|e| e.event.id.as_str())
            .collect::<Vec<_>>(),
        ["event-0", "event-2"]
    );
    let second = read(&f.db, &filter, Some(&first.cursor), 2).unwrap();
    assert_eq!(second.events[0].event.id, "event-4");
    let last = read(&f.db, &filter, Some(&second.cursor), 2).unwrap();
    assert!(last.events.is_empty());
    assert_eq!(last.cursor, f.page(None).cursor);
}

#[test]
fn unknown_event_types_and_payload_versions_are_preserved_but_oversize_records_are_rejected() {
    let f = Fixture::new();
    let mut event = f.event("future", 100);
    event.kind = "future.new_kind".into();
    event.version = 9;
    append(&f.db, &event).unwrap();
    assert_eq!(get(&f.db, "future").unwrap().event, event);
    event.id = "large".into();
    event.data = json!({"body":"x".repeat(MAX_EVENT_BYTES)});
    assert!(append(&f.db, &event).is_err());
    assert_eq!(f.page(None).events.len(), 1);
}

#[test]
fn retirement_replicates_and_old_replay_cannot_resurrect_payload_or_hide_cursor_gap() {
    let mut a = Fixture::new();
    let mut b = Fixture::new();
    let mut stale = Fixture::new();
    let checkpoint = b.page(None).cursor;
    let event = a.event("expired", 100);
    append(&a.db, &event).unwrap();
    a.send(&mut b);
    a.send(&mut stale);
    retire(&a.db, &["expired".into()]).unwrap();
    a.send(&mut b);
    stale.send(&mut b);
    assert!(b.page(None).events.is_empty());
    assert!(get(&b.db, "expired").is_err());
    assert!(
        read(&b.db, &Filter::default(), Some(&checkpoint), 100)
            .unwrap_err()
            .message
            .contains("expired")
    );
    assert!(!append(&b.db, &event).unwrap());
    assert!(b.page(None).events.is_empty());
}

#[test]
fn retirement_before_original_and_journal_reclamation_keep_the_identity_retired() {
    let mut source = Fixture::new();
    let target = Fixture::new();
    let event = source.event("expired", 100);
    append(&source.db, &event).unwrap();
    source.flush();
    let original = envelope(&event).unwrap();
    let hash = hey_boss_sync::digest(original.as_bytes());
    retire(&source.db, &[event.id.clone()]).unwrap();
    source.flush();
    let retirement = source.engine.pending(1).unwrap().remove(0).2;
    receive(&target.db, &retirement).unwrap();
    receive(&target.db, &original).unwrap();
    assert!(target.page(None).events.is_empty());
    collect(&source.db, &mut source.engine).unwrap();
    assert_eq!(source.engine.payload(&hash).unwrap(), None);
    assert!(!append(&source.db, &event).unwrap());
}

#[test]
fn a_read_does_not_take_the_database_writer_while_another_writer_is_open() {
    let f = Fixture::new();
    append(&f.db, &f.event("one", 100)).unwrap();
    let other = Store::open(&f.root.join("issues.db"))
        .unwrap()
        .into_database();
    let tx = crate::database::Transaction::new_unchecked(
        &f.db,
        crate::database::TransactionBehavior::Immediate,
    )
    .unwrap();
    let page = read(&other, &Filter::default(), None, 10).unwrap();
    assert_eq!(page.events.len(), 1);
    tx.rollback().unwrap();
}

#[test]
fn disk_pressure_has_hysteresis_survives_restart_and_commits_with_its_observation() {
    let f = Fixture::new();
    let gib = 1024 * 1024 * 1024;
    for free in [10 * gib, gib, gib, 2 * gib, 4 * gib, 10 * gib] {
        super::observe::disk(&f.db, "machine", "/data", free, 50 * gib).unwrap();
    }
    let page = f.page(None);
    assert_eq!(
        page.events
            .iter()
            .map(|r| r.event.kind.as_str())
            .collect::<Vec<_>>(),
        ["machine.disk_low", "machine.disk_recovered"]
    );
    assert_eq!(page.events[0].event.data["available_bytes"], gib);
    let reopened = open(&f.root.join("issues.db")).unwrap();
    super::observe::disk(&reopened, "machine", "/data", 10 * gib, 50 * gib).unwrap();
    assert_eq!(f.page(None).events.len(), 2);
}

#[test]
fn journal_publication_replays_after_a_crash_without_acknowledging_twice() {
    let mut f = Fixture::new();
    let event = f.event("crash", 100);
    append(&f.db, &event).unwrap();
    f.engine.put(&envelope(&event).unwrap()).unwrap();
    // Simulate loss of the application transaction after the journal commit.
    assert_eq!(export(&f.db, &mut f.engine).unwrap(), 1);
    assert_eq!(f.engine.head().unwrap(), 1);
    assert_eq!(export(&f.db, &mut f.engine).unwrap(), 0);
    assert_eq!(f.page(None).events.len(), 1);
}

#[test]
fn connectivity_is_an_observation_by_a_reporter_and_records_only_transitions() {
    let f = Fixture::new();
    for connected in [false, false, true, true, false] {
        super::observe::peer(&f.db, "reporter", "devbox", connected).unwrap();
    }
    let page = f.page(None);
    assert_eq!(page.events.len(), 3);
    assert_eq!(page.events[1].event.kind, "peer.connected");
    assert_eq!(page.events[1].event.origin, "reporter");
    assert_eq!(page.events[1].event.subject, "devbox");
}
