use super::*;
use crate::issues::Store;

struct Fixture {
    root: std::path::PathBuf,
    db: Connection,
}
impl Fixture {
    fn new(node: &str, role: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-archive-sync-{}",
            issues::worker::random_id().unwrap()
        ));
        let db = Store::open(&root.join("issues.db"))
            .unwrap()
            .into_database();
        db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('named:Archive','Archive',3);
            INSERT INTO agents VALUES('human:boss','{}',0);
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,closed_at,version,labels,blockers) VALUES
            ('named:Archive',1,'Archived task','Saved body','closed','human:boss',1,100,100,1,'[]','[]'),
            ('named:Archive',2,'Reserved task','','open','human:boss',1,100,NULL,1,'[]','[]');").unwrap();
        replica::install_capture(&db, role, node).unwrap();
        Self { root, db }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn archive_collision_pull_drains_pending_changes_without_changing_allocation_ownership() {
    let main = Fixture::new("main", "controller");
    main.db.execute_batch("INSERT INTO comments(id,project_id,issue_number,author,body,created_at) VALUES(45,'named:Archive',1,'human:boss','Saved comment',10);
        INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Archive',1,'human:boss','commented',10,'{\"comment_id\":45}');
        INSERT INTO fleet_allocations VALUES('named:Archive',2,'other-machine');").unwrap();
    let agent = Fixture::new("peer", "agent");
    replica::apply_pull(
        &agent.db,
        "peer",
        &replica::snapshot(&main.db, "peer").unwrap(),
        &[],
    )
    .unwrap();
    // Reproduce a legacy event retaining the sender's numeric reference.
    agent.db.execute_batch("UPDATE fleet_meta SET syncing=2;
        UPDATE events SET data=json_set(data,'$.comment_id',45);
        UPDATE fleet_meta SET syncing=0;
        INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES('named:Archive',1,'human:boss','Offline comment',200);").unwrap();
    assert!(cold::archive_issue(&main.db, "named:Archive", 1, cold::GRACE_MS + 100).unwrap());
    let pending = replica::journal(&agent.db, 0).unwrap();
    assert!(!pending.is_empty());
    let payload = replica::snapshot(&main.db, "peer").unwrap();
    let prepared = prepare_pull(&agent.db, &payload, |key, project, number, cursor| {
        cold::transfer::export_page(&main.db, key, project, number, cursor)
    })
    .unwrap();
    replica::apply_pull(&agent.db, "peer", &prepared, &[]).unwrap();
    assert_eq!(agent.db.query_row("SELECT count(*) FROM events e JOIN comments c ON c.id=json_extract(e.data,'$.comment_id') WHERE c.body='Saved comment'", [], |r|r.get::<_,i64>(0)).unwrap(), 1);
    assert_eq!(
        replica::journal(&agent.db, 0).unwrap(),
        pending,
        "physical repair must not create outgoing mutations"
    );
    let receipts = replica::accept_changes(&main.db, "peer", &pending).unwrap();
    assert!(receipts.iter().all(|r| r["state"] == "applied"));
    assert_eq!(
        replica::accept_changes(&main.db, "peer", &pending).unwrap(),
        receipts
    );
    for _ in 0..3 {
        let payload = replica::snapshot(&main.db, "peer").unwrap();
        let prepared = prepare_pull(&agent.db, &payload, |_, _, _, _| {
            panic!("restored history needs no transfer")
        })
        .unwrap();
        replica::apply_pull(&agent.db, "peer", &prepared, &receipts).unwrap();
        assert!(replica::journal(&agent.db, 0).unwrap().is_empty());
        assert_eq!(
            agent
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
            main.db
                .query_row(
                    "SELECT node FROM fleet_allocations WHERE issue_number=2",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "other-machine"
        );
    }
}
