use super::*;
use crate::issues::Store;

struct Fixture {
    root: std::path::PathBuf,
    db: Connection,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-retry-readiness-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        let db = Store::open(&root.join("issues.db"))
            .unwrap()
            .into_database();
        db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('p','Project',3),('other','Other',2);
            INSERT INTO agents VALUES('human:test','{}',0);
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels)
            VALUES('p',1,'Task','','open','human:test',0,0,1,'[]'),('p',2,'Other task','','open','human:test',0,0,1,'[]'),
                  ('other',1,'Historical task','','closed','human:test',0,0,1,'[]');
            CREATE TABLE distributed_attempts(id TEXT PRIMARY KEY,machine TEXT NOT NULL,project_id TEXT NOT NULL,
                issue_number INTEGER NOT NULL,state TEXT NOT NULL,started_at INTEGER NOT NULL,finished_at INTEGER,
                summary TEXT NOT NULL,payload TEXT NOT NULL);").unwrap();
        migrate(&db).unwrap();
        Self { root, db }
    }

    fn ready(&self, number: i64) -> bool {
        self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM issue_pickup_ready WHERE project_id='p' AND number=?1)",
            [number], |r| r.get(0),
        ).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn pickup_readiness_does_not_scan_unrelated_attempt_history() {
    let f = Fixture::new();
    let db = &f.db;
    db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000)
        INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at)
        SELECT 'local-'||x,'other',1,'{}','human:test','completed',1,'synthetic','local',x,x,x FROM n;
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000)
        INSERT INTO distributed_attempts SELECT 'remote-'||x,'remote','other',1,'completed',x,x,'','{}' FROM n;").unwrap();
    // Use a correlated lookup, as worker status and fleet admission do. A
    // standalone query with literal issue keys hides the UNION planner problem.
    let mut query = db.prepare("SELECT i.number FROM issues i WHERE i.project_id='p'
        AND EXISTS(SELECT 1 FROM issue_pickup_ready ready WHERE ready.project_id=i.project_id AND ready.number=i.number)
        ORDER BY i.number").unwrap();
    let rows = query
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(rows, [1, 2]);
    let steps = query.get_status(rusqlite::StatementStatus::VmStep);
    assert!(
        steps < 5_000,
        "readiness scanned unrelated history: {steps} VM steps"
    );
}

#[test]
fn pickup_readiness_keeps_latest_retry_and_reopen_rules() {
    let f = Fixture::new();
    let db = &f.db;
    assert!(f.ready(1));
    db.execute_batch("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at,summary)
        VALUES('local','p',1,'{}','human:test','blocked',1,'synthetic','local',10,10,10,'Codex needs input or approval: answer');
        INSERT INTO distributed_attempts VALUES('local','remote','p',1,'completed',100,100,'','{}');").unwrap();
    assert!(
        !f.ready(1),
        "local evidence wins over its replicated duplicate"
    );
    db.execute_batch("INSERT INTO distributed_attempts VALUES('remote','remote','p',1,'completed',20,20,'','{}');").unwrap();
    assert!(
        f.ready(1),
        "a newer completed attempt supersedes the old hold"
    );
    db.execute_batch("UPDATE distributed_attempts SET state='blocked',payload='{\"retry_at\":9000000000000}' WHERE id='remote';").unwrap();
    assert!(!f.ready(1), "remote cooldown survives moving the work");
    db.execute_batch(
        "UPDATE distributed_attempts SET payload='{\"retry_at\":0}' WHERE id='remote';",
    )
    .unwrap();
    assert!(f.ready(1), "an expired cooldown releases pickup");
    db.execute_batch("UPDATE distributed_attempts SET summary='Codex needs input or approval: answer' WHERE id='remote';").unwrap();
    assert!(
        !f.ready(1),
        "explicit input stays held after a deadline expires"
    );
    db.execute_batch(
        "INSERT INTO events(project_id,issue_number,actor,action,created_at,data)
        VALUES('p',1,'human:test','reopened',21,'{}');",
    )
    .unwrap();
    assert!(f.ready(1), "explicit reopen releases the latest hold");
    assert!(f.ready(2), "another issue's history cannot hold this issue");
}

#[test]
fn pickup_readiness_upgrade_is_idempotent_and_repairs_old_views() {
    let f = Fixture::new();
    let version = || {
        f.db.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    let before = version();
    migrate(&f.db).unwrap();
    assert_eq!(
        version(),
        before,
        "healthy opens must not rewrite the schema"
    );
    f.db.execute_batch(
        "DROP VIEW issue_pickup_ready;
        CREATE VIEW issue_pickup_ready AS SELECT project_id,number FROM issues;",
    )
    .unwrap();
    migrate(&f.db).unwrap();
    let repaired = version();
    assert!(repaired > before);
    migrate(&f.db).unwrap();
    assert_eq!(version(), repaired);
    let reopened = Store::open(&f.root.join("issues.db")).unwrap();
    assert_eq!(
        version(),
        repaired,
        "store startup must retain the optimized view"
    );
    drop(reopened);
}
