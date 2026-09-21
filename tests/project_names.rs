use hey_boss::issues::{Operation, Project, Request, Store};
use rusqlite::Connection;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "hey-boss-names-{}-{}.db",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn store(&self) -> Store {
        Store::open(&self.0).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn project(id: &str, name: &str) -> Project {
    Project {
        id: id.into(),
        name: name.into(),
    }
}
fn registry(store: &mut Store, p: &Project) -> serde_json::Value {
    store
        .execute(&Request {
            version: 1,
            project: p.clone(),
            project_override: None,
            actor: None,
            operation: Operation::Projects {
                include_hidden: true,
            },
            request_id: None,
        })
        .unwrap()
}

#[test]
fn names_reuse_the_first_project_across_discovery_notifications_and_overrides() {
    let f = Fixture::new();
    let mut store = f.store();
    let first = project("github.com/poe-internal/poe2", "poe2");
    let other = project("github.com/another/poe2", "poe2");
    store.notification_project(&first, None).unwrap();
    store.discover_projects(&[(other.clone(), 10)]).unwrap();
    assert_eq!(store.notification_project(&other, None).unwrap(), first);
    assert_eq!(
        store.notification_project(&other, Some("POE2")).unwrap(),
        first
    );
    assert_eq!(
        store
            .notification_project(&other, Some("named:poe2"))
            .unwrap(),
        first
    );
    let value = registry(&mut store, &other);
    assert_eq!(value["project"]["id"], first.id);
    assert_eq!(value["projects"].as_array().unwrap().len(), 1);
    assert!(value["project_warnings"].as_array().unwrap().is_empty());
    // Normal name reuse must not create a warning queue.
    assert_eq!(
        registry(&mut store, &other)["project_warnings"],
        value["project_warnings"]
    );
    let db = Connection::open(&f.0).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM project_name_collisions", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(
        db.execute(
            "INSERT INTO projects(id,name,next_number) VALUES('rogue','PoE2',1)",
            []
        )
        .is_err()
    );
}

#[test]
fn upgrade_discards_reuse_warnings_without_touching_saved_history() {
    let f = Fixture::new();
    let mut store = f.store();
    let first = project("github.com/first/poe2", "poe2");
    store.notification_project(&first, None).unwrap();
    drop(store);
    let db = Connection::open(&f.0).unwrap();
    db.execute(
        "INSERT INTO project_name_collisions VALUES('github.com/other/poe2','poe2',?1,0)",
        [&first.id],
    )
    .unwrap();
    drop(db);
    let value = registry(&mut f.store(), &first);
    assert!(value["project_warnings"].as_array().unwrap().is_empty());
    assert_eq!(value["projects"].as_array().unwrap().len(), 1);
}

#[test]
fn legacy_duplicate_names_keep_history_and_have_one_stable_destination() {
    let f = Fixture::new();
    drop(f.store());
    let db = Connection::open(&f.0).unwrap();
    // Simulate a pre-upgrade registry without the new uniqueness guard.
    db.execute_batch("DROP TRIGGER IF EXISTS project_name_guard; DROP TRIGGER IF EXISTS project_name_register; DROP TABLE IF EXISTS project_name_keys; DROP TABLE IF EXISTS project_name_collisions;
        INSERT INTO projects(id,name,next_number,created_at,activity_at) VALUES('named:poe2','poe2',2,1,10),('github.com/poe-internal/poe2','poe2',3,5,5);
        INSERT INTO agents VALUES('human:test','{}',0);
        INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES
            ('named:poe2',1,'Legacy work','','open','human:test',0,0,1,'[]'),
            ('github.com/poe-internal/poe2',1,'Canonical work','','open','human:test',0,0,1,'[]'),
            ('github.com/poe-internal/poe2',2,'More work','','open','human:test',0,0,1,'[]');").unwrap();
    drop(db);
    let mut store = f.store();
    let first = project("github.com/poe-internal/poe2", "poe2");
    let value = registry(&mut store, &first);
    assert_eq!(value["projects"].as_array().unwrap().len(), 1);
    assert_eq!(value["projects"][0]["id"], first.id);
    assert_eq!(value["project_warnings"][0]["legacy"], true);
    let history = store
        .execute(&Request {
            version: 1,
            project: first.clone(),
            project_override: Some("named:poe2".into()),
            actor: None,
            operation: Operation::View { number: 1 },
            request_id: None,
        })
        .unwrap();
    assert_eq!(history["issue"]["title"], "Legacy work");
    // Compatibility IDs remain readable instead of deleting saved content.
    assert_eq!(
        store
            .notification_project(&first, Some("named:poe2"))
            .unwrap()
            .id,
        "named:poe2"
    );
    assert_eq!(
        store.notification_project(&first, Some("poe2")).unwrap(),
        first
    );
    drop(store);
    assert_eq!(
        registry(&mut f.store(), &first)["projects"][0]["id"],
        value["projects"][0]["id"]
    );
}
