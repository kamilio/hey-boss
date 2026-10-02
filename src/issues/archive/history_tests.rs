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
fn maintenance_archives_eligible_issues_and_recovers_interrupted_restores() {
    let mut f = Fixture::new();
    let _owner = crate::database::Owner::start(&f.root.join("issues.db"))
        .unwrap()
        .unwrap();
    f.db = crate::database::Connection::connect(&f.root.join("issues.db")).unwrap();
    let before = f.read(json!({"action":"view","number":1}));
    let mut maintenance = Maintenance::default();
    assert_eq!(maintenance.run(&f.db, GRACE_MS + 99).unwrap(), 0);
    assert!(maintenance.run(&f.db, GRACE_MS + 100).unwrap() > 0);
    for _ in 0..10 {
        maintenance.run(&f.db, GRACE_MS + 100).unwrap();
    }
    assert_eq!(f.read(json!({"action":"view","number":1})), before);
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    f.db.execute("UPDATE issues SET archive_restoring=1 WHERE number=1", [])
        .unwrap();
    assert!(maintenance.run(&f.db, GRACE_MS + 101).unwrap() > 0);
    assert!(
        f.db.query_row(
            "SELECT archive_key IS NULL AND archive_restoring=0 FROM issues WHERE number=1",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert_eq!(f.read(json!({"action":"view","number":1})), before);
    assert_eq!(maintenance.run(&f.db, GRACE_MS + 102).unwrap(), 0);
}

#[test]
fn maintenance_preserves_grace_for_late_history_and_skips_failed_items() {
    let mut f = Fixture::new();
    f.db.execute_batch("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Archive',1,'human:boss','late_event',101,'{}');
        INSERT INTO requests(project_id,actor,request_id,payload,response,created_at) VALUES('named:Archive','human:boss','bad','{}','invalid json',1),('named:Archive','human:boss','good','{}','{}',2);").unwrap();
    let _owner = crate::database::Owner::start(&f.root.join("issues.db"))
        .unwrap()
        .unwrap();
    f.db = crate::database::Connection::connect(&f.root.join("issues.db")).unwrap();
    let mut maintenance = Maintenance::default();
    maintenance.run(&f.db, GRACE_MS + 100).unwrap();
    assert!(
        f.db.query_row(
            "SELECT archive_key IS NULL FROM issues WHERE number=1",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    maintenance.run(&f.db, GRACE_MS + 101).unwrap();
    assert!(
        f.db.query_row(
            "SELECT archive_key IS NOT NULL FROM requests WHERE request_id='good'",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert!(f.db.query_row("SELECT archive_key IS NULL AND response='invalid json' FROM requests WHERE request_id='bad'", [], |r|r.get::<_,bool>(0)).unwrap());
    assert!(
        f.db.query_row(
            "SELECT archive_key IS NOT NULL FROM issues WHERE number=1",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
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

#[test]
fn many_maximum_size_comments_archive_through_the_database_service() {
    let f = Fixture::new();
    f.db.execute("WITH RECURSIVE n(x) AS (VALUES(3) UNION ALL SELECT x+1 FROM n WHERE x<18)
        INSERT INTO comments(id,project_id,issue_number,author,body,created_at) SELECT x,'named:Archive',1,'human:boss',?1,50 FROM n", ["x".repeat(crate::issues::BODY_LIMIT)]).unwrap();
    let _owner = crate::database::Owner::start(&f.root.join("issues.db")).unwrap().unwrap();
    let db = crate::database::Connection::connect(&f.root.join("issues.db")).unwrap();
    assert!(archive_issue(&db, "named:Archive", 1, GRACE_MS + 100).unwrap());
    while cleanup_history(&db).unwrap() > 0 {}
    let cold = history_connection(&db, "named:Archive", 1).unwrap().unwrap();
    assert_eq!(cold.query_row("SELECT count(*) FROM comments WHERE length(body)=?1", [crate::issues::BODY_LIMIT], |r|r.get::<_,i64>(0)).unwrap(), 16);
    restore_issue(&db, "named:Archive", 1, GRACE_MS + 101).unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM comments WHERE length(body)=?1", [crate::issues::BODY_LIMIT], |r|r.get::<_,i64>(0)).unwrap(), 16);
}

#[test]
fn abandoned_download_cleanup_preserves_verified_copies_and_recent_transfers() {
    let source = Fixture::new();
    source.archive();
    let key: String = source
        .db
        .query_row("SELECT archive_key FROM issues WHERE number=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let target = Fixture::new();
    let mut stale = transfer::Download::new(&target.db, &key, "named:Archive", 1).unwrap();
    let archive = Archive::open(&archive_path(&target.db).unwrap()).unwrap();
    archive
        .db
        .execute("UPDATE archive_downloads SET updated_at=0", [])
        .unwrap();
    let _fresh = transfer::Download::new(&target.db, &key, "named:Archive", 1).unwrap();
    assert!(transfer::cleanup_downloads(&target.db, GRACE_MS).unwrap() > 0);
    assert_eq!(
        archive
            .db
            .query_row("SELECT count(*) FROM archive_downloads", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let page = transfer::export_page(&source.db, &key, "named:Archive", 1, &Value::Null).unwrap();
    assert!(stale.receive(&page).is_err());
    assert!(
        !transfer::Catalog::new(&target.db)
            .unwrap()
            .contains(&key, "named:Archive", 1)
            .unwrap()
    );

    let original = Archive::open(&archive_path(&source.db).unwrap()).unwrap();
    original
        .db
        .execute(
            "INSERT INTO archive_downloads VALUES('download-abandoned',0)",
            [],
        )
        .unwrap();
    original.db.execute("INSERT INTO issue_copies SELECT 'download-abandoned',project_id,number,record,comments,record_hash FROM issue_copies WHERE key=?1", [&key]).unwrap();
    original.db.execute("INSERT INTO issue_history SELECT 'download-abandoned',kind,id,text_id,created_at,author,action,record,record_hash FROM issue_history WHERE archive_key=?1", [&key]).unwrap();
    for _ in 0..10 {
        if transfer::cleanup_downloads(&source.db, GRACE_MS).unwrap() == 0 {
            break;
        }
    }
    assert_eq!(
        original
            .db
            .query_row(
                "SELECT count(*) FROM issue_copies WHERE key='download-abandoned'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        original
            .db
            .query_row(
                "SELECT count(*) FROM issue_history WHERE archive_key='download-abandoned'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        issue_body(&source.db, "named:Archive", 1, &key).unwrap(),
        "Searchable archive body 🦀"
    );
}

#[test]
fn archive_transfer_is_bounded_verified_and_resumes_at_record_boundaries() {
    let source = Fixture::new();
    source
        .db
        .execute(
            "UPDATE issues SET body=?1 WHERE number=1",
            ["日本語🦀\\\"\n".repeat(250_000)],
        )
        .unwrap();
    source.archive();
    let key: String = source
        .db
        .query_row("SELECT archive_key FROM issues WHERE number=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let target = Fixture::new();
    let mut incoming = transfer::Download::new(&target.db, &key, "named:Archive", 1).unwrap();
    let mut cursor = Value::Null;
    let mut pages = 0;
    loop {
        let page = transfer::export_page(&source.db, &key, "named:Archive", 1, &cursor).unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() < 2 * 1024 * 1024);
        pages += 1;
        cursor = page["next"].clone();
        if incoming.receive(&page).unwrap() {
            break;
        }
    }
    assert!(pages > 1);
    let saved = Archive::read(&archive_path(&target.db).unwrap()).unwrap();
    assert_eq!(
        history::verify_copy(&saved, &key, "named:Archive", 1).unwrap()["body"],
        "日本語🦀\\\"\n".repeat(250_000)
    );
    assert!(history::verify_copy(&saved, &key, "wrong-project", 1).is_err());
}

#[test]
fn broken_archive_transfers_never_publish_a_copy_or_change_hot_history() {
    let source = Fixture::new();
    source.archive();
    let key: String = source
        .db
        .query_row("SELECT archive_key FROM issues WHERE number=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let page = transfer::export_page(&source.db, &key, "named:Archive", 1, &Value::Null).unwrap();
    assert_eq!(page["done"], true);
    for change in 0..7 {
        let target = Fixture::new();
        let mut damaged = page.clone();
        match change {
            0 => damaged["project"] = json!("another-project"),
            1 => damaged["number"] = json!(2),
            2 => damaged["chunks"][0]["data"] = json!("bm90LXRoZS1zYXZlZC1yb290"),
            3 => damaged["chunks"].as_array_mut().unwrap().truncate(1),
            4 => damaged["chunks"][0]["total"] = json!(MAX_OBJECT_BYTES + 1),
            5 => damaged["chunks"][0]["offset"] = json!(1),
            6 => damaged["cursor"] = json!({"kind":"events","id":"2","offset":0}),
            _ => unreachable!(),
        }
        {
            let mut incoming =
                transfer::Download::new(&target.db, &key, "named:Archive", 1).unwrap();
            assert!(
                incoming.receive(&damaged).is_err(),
                "accepted damage {change}"
            );
        }
        let archive = Archive::read(&archive_path(&target.db).unwrap()).unwrap();
        assert_eq!(
            archive
                .db
                .query_row("SELECT count(*) FROM issue_copies", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            archive
                .db
                .query_row("SELECT count(*) FROM issue_history", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            target
                .db
                .query_row("SELECT count(*) FROM comments", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(
            target
                .db
                .query_row(
                    "SELECT archive_key IS NULL FROM issues WHERE number=1",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
        );
    }
    let target = Fixture::new();
    for _ in 0..2 {
        let mut incoming = transfer::Download::new(&target.db, &key, "named:Archive", 1).unwrap();
        assert!(incoming.receive(&page).unwrap());
        assert!(incoming.receive(&page).unwrap());
    }
    let archive = Archive::read(&archive_path(&target.db).unwrap()).unwrap();
    assert_eq!(
        archive
            .db
            .query_row("SELECT count(*) FROM issue_copies", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn imported_archives_preserve_local_history_ids_and_resolved_comment_links() {
    let source = Fixture::new();
    source
        .db
        .execute("UPDATE fleet_meta SET node='controller' WHERE id=1", [])
        .unwrap();
    source.archive();
    let key: String = source
        .db
        .query_row("SELECT archive_key FROM issues WHERE number=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut target = Fixture::new();
    target.db.execute_batch("UPDATE fleet_meta SET node='companion',role='agent' WHERE id=1;
        UPDATE comments SET id=id+100;
        UPDATE events SET id=id+200,data=CASE WHEN json_type(data,'$.comment_id')='integer' THEN json_set(data,'$.comment_id',json_extract(data,'$.comment_id')+100) ELSE data END;
        INSERT INTO fleet_row_ids SELECT 'controller','comments',id-100,id FROM comments;
        INSERT INTO fleet_row_ids SELECT 'controller','events',id-200,id FROM events;").unwrap();
    let operations = [
        json!({"action":"view","number":1}),
        json!({"action":"history","number":1,"limit":100,"offset":0}),
        json!({"action":"timeline","number":1,"limit":100}),
    ];
    let before: Vec<_> = operations
        .iter()
        .map(|op| target.read(op.clone()))
        .collect();
    let mut incoming = transfer::Download::new(&target.db, &key, "named:Archive", 1).unwrap();
    let page = transfer::export_page(&source.db, &key, "named:Archive", 1, &Value::Null).unwrap();
    assert!(incoming.receive(&page).unwrap());
    drop(incoming);
    transfer::map_local_history(&target.db, &key, "named:Archive", 1).unwrap();
    transfer::map_local_history(&target.db, &key, "named:Archive", 1).unwrap();
    target.db.execute("UPDATE issues SET archive_key=?1,archived_comments=2,archive_cleanup=1,body='' WHERE number=1",[&key]).unwrap();
    while cleanup_history(&target.db).unwrap() != 0 {}
    assert_eq!(
        operations
            .iter()
            .map(|op| target.read(op.clone()))
            .collect::<Vec<_>>(),
        before
    );
    restore_issue(&target.db, "named:Archive", 1, GRACE_MS + 101).unwrap();
    assert_eq!(
        operations
            .iter()
            .map(|op| target.read(op.clone()))
            .collect::<Vec<_>>(),
        before
    );
    assert_eq!(
        target
            .db
            .query_row(
                "SELECT group_concat(id) FROM (SELECT id FROM comments ORDER BY id)",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "101,102"
    );
}

#[test]
fn imported_history_reserves_unused_ids_before_new_local_comments() {
    let source = Fixture::new();
    source
        .db
        .execute("UPDATE fleet_meta SET node='controller' WHERE id=1", [])
        .unwrap();
    source.archive();
    let key: String = source
        .db
        .query_row("SELECT archive_key FROM issues WHERE number=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut target = Fixture::new();
    target.db.execute_batch("UPDATE fleet_meta SET node='companion',role='agent' WHERE id=1; UPDATE comments SET issue_number=2; UPDATE events SET issue_number=2;").unwrap();
    let mut incoming = transfer::Download::new(&target.db, &key, "named:Archive", 1).unwrap();
    assert!(
        incoming
            .receive(
                &transfer::export_page(&source.db, &key, "named:Archive", 1, &Value::Null).unwrap()
            )
            .unwrap()
    );
    drop(incoming);
    transfer::map_local_history(&target.db, &key, "named:Archive", 1).unwrap();
    target.db.execute("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Archive',2,'human:boss','New local comment',103)",[]).unwrap();
    let new_id = target.db.last_insert_rowid();
    assert!(new_id > 4);
    transfer::map_local_history(&target.db, &key, "named:Archive", 1).unwrap();
    target.db.execute("UPDATE issues SET archive_key=?1,archived_comments=2,archive_cleanup=1,body='' WHERE number=1",[&key]).unwrap();
    let archived = target.read(json!({"action":"view","number":1}));
    assert_eq!(archived["comments"][0]["id"], 3);
    assert_eq!(archived["comments"][0]["resolved"], true);
    restore_issue(&target.db, "named:Archive", 1, GRACE_MS + 101).unwrap();
    assert_eq!(
        target
            .db
            .query_row("SELECT body FROM comments WHERE id=?1", [new_id], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "New local comment"
    );
    assert_eq!(
        target
            .db
            .query_row(
                "SELECT count(*) FROM comments WHERE issue_number=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        target
            .db
            .query_row(
                "SELECT count(*) FROM comments WHERE issue_number=2",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        3
    );
}

#[test]
fn restoring_history_does_not_reapply_commit_attachment_side_effects() {
    let f = Fixture::new();
    f.db.execute_batch("INSERT INTO events(id,project_id,issue_number,actor,action,created_at,data) VALUES
        (9,'named:Archive',1,'human:boss','commit_attached',20,'{\"sha\":\"1234567890abcdef\",\"url\":\"https://github.com/example/repo/commit/1234567890abcdef\"}'),
        (10,'named:Archive',1,'human:boss','commit_removed',30,'{\"sha\":\"1234567890abcdef\"}');").unwrap();
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM issue_commits", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    f.archive();
    restore_issue(&f.db, "named:Archive", 1, GRACE_MS + 101).unwrap();
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM issue_commits", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn historical_events_survive_policy_changes_that_reject_new_matching_events() {
    let f = Fixture::new();
    f.db.execute_batch("DROP TRIGGER dependency_notice_event;
        INSERT INTO events(id,project_id,issue_number,actor,action,created_at,data) VALUES(9,'named:Archive',1,'human:boss','dependency_rework',20,'{\"dependencies\":[[999]]}');").unwrap();
    crate::issues::dependency_notices::migrate(&f.db).unwrap();
    assert_eq!(f.db.execute("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Archive',1,'human:boss','dependency_rework',20,'{\"dependencies\":[[999]]}')",[]).unwrap(),0);
    f.archive();
    restore_issue(&f.db, "named:Archive", 1, GRACE_MS + 101).unwrap();
    assert_eq!(
        f.db.query_row(
            "SELECT count(*) FROM events WHERE action='dependency_rework'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        f.db.query_row("SELECT syncing FROM fleet_meta WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn complete_backup_opens_with_archived_body_and_history_at_a_new_path() {
    let mut f = Fixture::new();
    let before = f.read(json!({"action":"view","number":1}));
    f.archive();
    let destination = f.root.join("backup/copy.sqlite");
    backup_store(&f.db, &destination).unwrap();
    let mut backup = Store::open(&destination).unwrap();
    let request:Request=serde_json::from_value(json!({"version":1,"project":{"id":"named:Archive","name":"Archive"},"operation":{"action":"view","number":1}})).unwrap();
    assert_eq!(backup.execute(&request).unwrap(), before);
    let archive = f.root.join("backup/copy.sqlite.archive.db");
    assert!(archive.exists());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(archive).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn archive_files_and_sidecars_cannot_be_overwritten_by_auxiliary_outputs() {
    let f = Fixture::new();
    f.archive();
    let hot = f.root.join("issues.db");
    let archive = archive_path(&f.db).unwrap();
    let connection = Archive::open(&archive).unwrap();
    connection
        .db
        .execute("INSERT INTO objects VALUES('test','test',1,1,x'00')", [])
        .unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let mut path = archive.as_os_str().to_owned();
        path.push(suffix);
        let path = std::path::PathBuf::from(path);
        assert!(path.exists());
        assert!(crate::issues::planning::protect_database_paths(&hot, [&path]).is_err());
    }
    assert!(backup_store(&f.db, &archive).is_err());
}

#[test]
fn a_backup_never_reports_success_when_referenced_cold_storage_is_missing() {
    let f = Fixture::new();
    f.archive();
    std::fs::rename(
        archive_path(&f.db).unwrap(),
        f.root.join("unavailable.archive.db"),
    )
    .unwrap();
    assert!(backup_store(&f.db, &f.root.join("backup/incomplete.db")).is_err());
}

#[test]
fn a_reserved_archive_path_is_protected_before_its_first_publication() {
    let f = Fixture::new();
    let archive = archive_path(&f.db).unwrap();
    assert!(!archive.exists());
    assert!(backup_store(&f.db, &archive).is_err());
    assert!(!archive.exists());
}
