use super::*;
use crate::issues::Request;
use serde_json::json;

struct Fixture {
    root: std::path::PathBuf,
    store: Store,
    db: crate::database::Connection,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-issue-archive-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        let store = Store::open(&root.join("issues.db")).unwrap();
        let db = Store::open(&root.join("issues.db"))
            .unwrap()
            .into_database();
        db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('named:Archive','Archive',3);
            INSERT INTO agents VALUES('human:boss','{\"model\":\"test-model\"}',0);
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,closed_at,closed_by,version,labels,blockers) VALUES
              ('named:Archive',1,'Old issue','Searchable archive body 🦀','closed','human:boss',1,100,100,'human:boss',5,'[\"saved\"]','[]'),
              ('named:Archive',2,'Dependent','Active content','open','human:boss',1,1,NULL,NULL,1,'[]','[1]');
            INSERT INTO comments(id,project_id,issue_number,author,body,created_at) VALUES
              (1,'named:Archive',1,'human:boss','First comment',20),(2,'named:Archive',1,'human:boss','Second comment',40);
            INSERT INTO events(id,project_id,issue_number,actor,action,created_at,data) VALUES
              (1,'named:Archive',1,'human:boss','created',1,'{\"after\":{\"body\":\"Original body\"}}'),
              (2,'named:Archive',1,'human:boss','commented',20,'{\"comment_id\":1,\"actor_model\":\"test-model\"}'),
              (3,'named:Archive',1,'human:boss','commented',40,'{\"comment_id\":2}'),
              (4,'named:Archive',1,'human:boss','comment_resolved',50,'{\"comment_id\":1}'),
              (5,'named:Archive',1,'human:boss','closed',100,'{}');
            INSERT INTO issue_status_updates VALUES('s1','named:Archive',1,'human:boss','green','First progress',10),('s2','named:Archive',1,'human:boss','green','Final progress',90);").unwrap();
        Self { root, store, db }
    }
    fn read(&mut self, operation: Value) -> Value {
        let request: Request = serde_json::from_value(json!({"version":1,"project":{"id":"named:Archive","name":"Archive"},"operation":operation})).unwrap();
        self.store.execute(&request).unwrap()
    }
    fn archive(&self) {
        assert!(archive_issue(&self.db, "named:Archive", 1, GRACE_MS + 100).unwrap());
        for _ in 0..10 {
            if cleanup_history(&self.db).unwrap() == 0 {
                break;
            }
        }
    }
    fn write(&mut self, operation: Value) -> Value {
        let request: Request = serde_json::from_value(json!({"version":1,"project":{"id":"named:Archive","name":"Archive"},"actor":{"id":"human:boss","kind":"human","session_id":null,"machine":"test","host":"test","pid":null,"process_start":null,"cwd":self.root,"source":"test","model":"test-model"},"operation":operation})).unwrap();
        self.store.execute(&request).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn issue_archival_preserves_existing_read_responses_and_dependency_readiness() {
    let mut f = Fixture::new();
    let operations = vec![
        json!({"action":"view","number":1}),
        json!({"action":"comments","number":1,"limit":1,"offset":0,"sort":"newest"}),
        json!({"action":"comments","number":1,"limit":1,"offset":1,"sort":"oldest"}),
        json!({"action":"history","number":1,"limit":3,"offset":1}),
        json!({"action":"timeline","number":1,"limit":3}),
        json!({"action":"status_history","number":1,"limit":1,"offset":1}),
        json!({"action":"list","state":"closed","mine":false,"unassigned":false,"labels":[],"limit":20,"offset":0,"all":true}),
        json!({"action":"list","state":"all","mine":false,"unassigned":false,"labels":[],"search":"archive body","limit":20,"offset":0,"all":true}),
    ];
    let before: Vec<_> = operations.iter().map(|op| f.read(op.clone())).collect();
    assert!(!archive_issue(&f.db, "named:Archive", 1, GRACE_MS + 99).unwrap());
    f.archive();
    let after: Vec<_> = operations.iter().map(|op| f.read(op.clone())).collect();
    for ((operation, before), after) in operations.iter().zip(before).zip(after) {
        assert_eq!(before, after, "Changed read behavior for {operation}");
    }
    assert_eq!(
        f.db.query_row("SELECT body FROM issues WHERE number=1", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        ""
    );
    assert_eq!(
        f.db.query_row(
            "SELECT count(*) FROM comments WHERE issue_number=1",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert!(
        f.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE number=2)",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert!(!archive_issue(&f.db, "named:Archive", 2, GRACE_MS + 100).unwrap());
}

#[test]
fn restoring_a_copy_preserves_ids_and_history_without_changing_logical_revision() {
    let mut f = Fixture::new();
    let before = f.read(json!({"action":"view","number":1}));
    f.archive();
    restore_issue(&f.db, "named:Archive", 1, GRACE_MS + 100).unwrap();
    assert_eq!(f.read(json!({"action":"view","number":1})), before);
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM comments WHERE id IN (1,2)", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(
        f.db.query_row(
            "SELECT archive_key IS NULL FROM issues WHERE number=1",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert!(!archive_issue(&f.db, "named:Archive", 1, GRACE_MS + 101).unwrap());
    restore_issue(&f.db, "named:Archive", 1, GRACE_MS + 102).unwrap();
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn reopening_and_commenting_restore_history_before_mutating_the_issue() {
    let mut f = Fixture::new();
    f.archive();
    let reopened = f.write(json!({"action":"reopen","number":1,"if_version":5}));
    assert_eq!(reopened["issue"]["state"], "open");
    assert_eq!(reopened["issue"]["body"], "Searchable archive body 🦀");
    assert!(
        f.db.query_row(
            "SELECT archive_key IS NULL FROM issues WHERE number=1",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(
        !f.db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE number=2)",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
    let history = f.read(json!({"action":"history","number":1,"limit":100,"offset":0}));
    assert_eq!(history["events"].as_array().unwrap().len(), 6);
    assert_eq!(history["events"][5]["action"], "reopened");

    let mut f = Fixture::new();
    f.archive();
    f.write(json!({"action":"comment","number":1,"body":"New after archiving"}));
    let view = f.read(json!({"action":"view","number":1}));
    assert_eq!(view["comment_count"], 3);
    assert_eq!(view["issue"]["state"], "closed");
    assert_eq!(view["comments"][0]["resolved"], true);
}

#[test]
fn missing_archive_never_allows_hot_cleanup_or_silent_empty_history() {
    let mut f = Fixture::new();
    assert!(archive_issue(&f.db, "named:Archive", 1, GRACE_MS + 100).unwrap());
    let path = f.root.join("issues.db.archive.db");
    std::fs::rename(&path, f.root.join("saved.archive.db")).unwrap();
    assert!(cleanup_history(&f.db).is_err());
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let request: Request = serde_json::from_value(json!({"version":1,"project":{"id":"named:Archive","name":"Archive"},"operation":{"action":"view","number":1}})).unwrap();
    assert_eq!(
        f.store.execute(&request).unwrap_err().code,
        "archive_unavailable"
    );
    std::fs::rename(f.root.join("saved.archive.db"), &path).unwrap();
    restore_issue(&f.db, "named:Archive", 1, GRACE_MS + 200).unwrap();
    assert_eq!(
        f.read(json!({"action":"view","number":1}))["comment_count"],
        2
    );
}

#[test]
fn conflicting_history_ids_do_not_silently_discard_archived_content() {
    let f = Fixture::new();
    f.archive();
    f.db.execute("INSERT INTO comments(id,project_id,issue_number,author,body,created_at) VALUES(1,'named:Archive',2,'human:boss','Unrelated data',20)",[]).unwrap();
    assert!(restore_issue(&f.db, "named:Archive", 1, GRACE_MS + 101).is_err());
    assert_eq!(
        f.db.query_row("SELECT body FROM comments WHERE id=1", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Unrelated data"
    );
    assert!(
        f.db.query_row(
            "SELECT archive_key IS NOT NULL FROM issues WHERE number=1",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert_eq!(cleanup_history(&f.db).unwrap(), 0);
}

#[test]
fn altered_archived_payloads_fail_integrity_checks_on_read_and_restore() {
    let mut f = Fixture::new();
    f.archive();
    let archive = Archive::open(&f.root.join("issues.db.archive.db")).unwrap();
    archive
        .db
        .execute(
            "UPDATE issue_copies SET record=json_set(record,'$.body','Unexpected replacement')",
            [],
        )
        .unwrap();
    let request: Request = serde_json::from_value(json!({"version":1,"project":{"id":"named:Archive","name":"Archive"},"operation":{"action":"view","number":1}})).unwrap();
    assert!(f.store.execute(&request).is_err());
    assert!(restore_issue(&f.db, "named:Archive", 1, GRACE_MS + 101).is_err());

    let mut f = Fixture::new();
    f.archive();
    let archive = Archive::open(&f.root.join("issues.db.archive.db")).unwrap();
    archive.db.execute("UPDATE issue_history SET record=json_set(record,'$.body','Unexpected replacement') WHERE kind='comments' AND id=1",[]).unwrap();
    assert!(f.store.execute(&request).is_err());
    assert!(restore_issue(&f.db, "named:Archive", 1, GRACE_MS + 101).is_err());
}

#[test]
fn interrupted_restore_resumes_without_duplicate_history_or_cleanup_races() {
    let mut f = Fixture::new();
    f.db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(3) UNION ALL SELECT x+1 FROM n WHERE x<70) INSERT INTO comments(id,project_id,issue_number,author,body,created_at) SELECT x,'named:Archive',1,'human:boss','Comment '||x,x FROM n;").unwrap();
    let before =
        f.read(json!({"action":"comments","number":1,"limit":100,"offset":0,"sort":"oldest"}));
    f.archive();
    while cleanup_history(&f.db).unwrap() != 0 {}
    f.db.execute_batch("CREATE TRIGGER interrupt_restore BEFORE INSERT ON comments WHEN NEW.id=40 BEGIN SELECT RAISE(ABORT,'simulated interruption'); END;").unwrap();
    assert!(restore_issue(&f.db, "named:Archive", 1, GRACE_MS + 101).is_err());
    let restored: i64 =
        f.db.query_row("SELECT count(*) FROM comments", [], |r| r.get(0))
            .unwrap();
    assert!((1..70).contains(&restored));
    assert_eq!(cleanup_history(&f.db).unwrap(), 0);
    assert_eq!(
        f.read(json!({"action":"comments","number":1,"limit":100,"offset":0,"sort":"oldest"})),
        before
    );
    f.db.execute_batch("DROP TRIGGER interrupt_restore")
        .unwrap();
    restore_issue(&f.db, "named:Archive", 1, GRACE_MS + 102).unwrap();
    assert_eq!(
        f.read(json!({"action":"comments","number":1,"limit":100,"offset":0,"sort":"oldest"})),
        before
    );
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        70
    );
    assert_eq!(
        f.db.query_row(
            "SELECT archive_restoring FROM issues WHERE number=1",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn concurrent_edits_and_reservations_abort_archive_publication() {
    use std::{sync::mpsc, time::Duration};
    for change in [
        "UPDATE issues SET body='Concurrent content',version=version+1 WHERE number=1",
        "INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Archive',1,'human:boss','Concurrent comment',101)",
        "INSERT INTO fleet_allocations(node,project_id,issue_number) VALUES('other','named:Archive',1)",
    ] {
        let f = Fixture::new();
        let path = f.root.join("issues.db");
        let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
        let (entered, waiting) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (db, transport) = crate::database::tests::pause_before_writer(&path, entered, released);
        let task =
            std::thread::spawn(move || archive_issue(&db, "named:Archive", 1, GRACE_MS + 100));
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        f.db.execute_batch(change).unwrap();
        release.send(()).unwrap();
        assert!(!task.join().unwrap().unwrap());
        transport.join().unwrap();
        assert!(
            f.db.query_row(
                "SELECT archive_key IS NULL FROM issues WHERE number=1",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
        assert_eq!(
            f.db.query_row("SELECT count(*) FROM comments WHERE id IN (1,2)", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        owner.stop();
    }
}

#[test]
fn an_archive_winning_the_writer_race_retries_mutation_without_duplicate_effects() {
    use std::{sync::mpsc, time::Duration};
    let mut f = Fixture::new();
    let path = f.root.join("issues.db");
    let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (db, transport) = crate::database::tests::pause_before_writer(&path, entered, released);
    let mut store = Store::open(&path).unwrap();
    store.replace_connection_for_test(db);
    let request: Request = serde_json::from_value(json!({"version":1,"project":{"id":"named:Archive","name":"Archive"},"actor":{"id":"human:boss","kind":"human","machine":"test","host":"test","cwd":f.root,"source":"test"},"request_id":"archive-race-once","operation":{"action":"comment","number":1,"body":"Concurrent mutation"}})).unwrap();
    let task = std::thread::spawn(move || store.execute(&request));
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    f.archive();
    release.send(()).unwrap();
    assert_eq!(task.join().unwrap().unwrap()["ok"], true);
    transport.join().unwrap();
    assert_eq!(
        f.read(json!({"action":"view","number":1}))["comment_count"],
        3
    );
    assert_eq!(
        f.db.query_row(
            "SELECT count(*) FROM requests WHERE request_id='archive-race-once'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    owner.stop();
}

#[test]
fn recent_history_extends_the_grace_period_and_deleted_issues_remain_readable() {
    for insert in [
        "INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Archive',1,'human:boss','Recent comment',101)",
        "INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Archive',1,'human:boss','observed',101,'{}')",
        "INSERT INTO issue_status_updates VALUES('late','named:Archive',1,'human:boss','green','Late status',101)",
    ] {
        let f = Fixture::new();
        f.db.execute_batch(insert).unwrap();
        assert!(!archive_issue(&f.db, "named:Archive", 1, GRACE_MS + 100).unwrap());
        assert!(archive_issue(&f.db, "named:Archive", 1, GRACE_MS + 101).unwrap());
    }
    let mut f = Fixture::new();
    f.db.execute("UPDATE issues SET state='open',closed_at=NULL,closed_by=NULL,deleted_at=100 WHERE number=1",[]).unwrap();
    let operations = [
        json!({"action":"view","number":1}),
        json!({"action":"list","state":"deleted","mine":false,"unassigned":false,"labels":[],"search":"archive body","limit":20,"offset":0,"all":true}),
    ];
    let before: Vec<_> = operations.iter().map(|op| f.read(op.clone())).collect();
    f.archive();
    assert_eq!(
        operations
            .iter()
            .map(|op| f.read(op.clone()))
            .collect::<Vec<_>>(),
        before
    );
    f.write(json!({"action":"restore","number":1}));
    assert_eq!(
        f.read(json!({"action":"view","number":1}))["issue"]["body"],
        "Searchable archive body 🦀"
    );
}
