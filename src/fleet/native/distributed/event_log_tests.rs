use super::*;
use crate::event_log::{self, Event, Filter};
use serde_json::json;

#[test]
fn operational_events_share_the_application_journal_without_entering_issue_projection() {
    let a = Fixture::new();
    let b = Fixture::new();
    let source = a.db();
    let target = b.db();
    install(&source, "source").unwrap();
    install(&target, "target").unwrap();
    let mut outgoing = hey_boss_sync::Engine::open(&a.0.join("sync.db")).unwrap();
    let mut incoming = hey_boss_sync::Engine::open(&b.0.join("sync.db")).unwrap();
    let event = Event {
        id: "disk-full".into(),
        kind: "machine.disk_low".into(),
        version: 1,
        origin: "source".into(),
        time: 100,
        project: None,
        subject: "/".into(),
        data: json!({"available_bytes":0}),
    };
    event_log::append(&source, &event).unwrap();
    assert_eq!(export(&source, &mut outgoing).unwrap(), 1);
    incoming.ingest(&outgoing.batch(0).unwrap()).unwrap();
    assert_eq!(import(&target, &incoming).unwrap(), 1);
    let page = event_log::read(&target, &Filter::default(), None, 10).unwrap();
    assert_eq!(page.events[0].event, event);
    assert_eq!(
        target
            .query_row("SELECT count(*) FROM distributed_fields", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        export(&target, &mut incoming).unwrap(),
        0,
        "received facts do not echo through the application outbox"
    );
    event_log::retire(&source, &["disk-full".into()]).unwrap();
    export(&source, &mut outgoing).unwrap();
    let received = incoming.received(outgoing.id()).unwrap();
    incoming.ingest(&outgoing.batch(received).unwrap()).unwrap();
    import(&target, &incoming).unwrap();
    assert!(event_log::get(&target, "disk-full").is_err());
}
