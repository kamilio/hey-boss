//! Compact queue rotations, independent of issue content and ownership.

use super::*;
use rusqlite::{OptionalExtension, params};
use serde_json::json;

pub(super) fn install(db: &Connection) -> Result<()> {
    let installed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='distributed_order_capture')",
        [],
        |r| r.get(0),
    )?;
    if installed {
        return Ok(());
    }
    db.execute_batch("CREATE TABLE distributed_order_capture(active INTEGER NOT NULL); INSERT INTO distributed_order_capture VALUES(0);
        CREATE TABLE distributed_order_rotations(project_id TEXT NOT NULL,clock INTEGER NOT NULL,origin TEXT NOT NULL,first INTEGER NOT NULL,last INTEGER NOT NULL,PRIMARY KEY(project_id,clock,origin)) WITHOUT ROWID;
        CREATE TABLE distributed_order_positions(project_id TEXT NOT NULL,number INTEGER NOT NULL,clock INTEGER NOT NULL,origin TEXT NOT NULL,position INTEGER NOT NULL,PRIMARY KEY(project_id,number)) WITHOUT ROWID;
        CREATE INDEX distributed_order_range ON distributed_order_positions(project_id,position);
        CREATE INDEX IF NOT EXISTS distributed_issue_order_base ON distributed_fields(json_extract(entity,'$.project_id'),entity) WHERE table_name='issues' AND field='sort_order';
        CREATE TABLE distributed_order_dirty(project_id TEXT PRIMARY KEY) WITHOUT ROWID;
        DROP TRIGGER IF EXISTS distributed_capture_issues_UPDATE;")?;
    db.execute("INSERT INTO distributed_order_positions SELECT json_extract(entity,'$.project_id'),json_extract(entity,'$.number'),clock,origin,CAST(value AS INTEGER) FROM distributed_fields WHERE table_name='issues' AND field='sort_order'", [])?;
    Ok(())
}

fn rotate(position: i64, from: i64, to: i64) -> i64 {
    if position == from {
        to
    } else if to <= position && position < from {
        position + 1
    } else if from < position && position <= to {
        position - 1
    } else {
        position
    }
}

// Legacy snapshots remain the coordinate base. New operations compose in a
// total (Lamport clock, origin) order, even when relays deliver them backwards.
pub(super) fn seed(db: &Connection, key: &Value) -> Result<()> {
    let project = key["project_id"].as_str().unwrap();
    let number = key["number"].as_i64().unwrap();
    let Some((clock,origin,value)) = db.query_row("SELECT clock,origin,value FROM distributed_fields WHERE table_name='issues' AND entity=?1 AND field='sort_order'", [key.to_string()], |r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional()? else {return Ok(())};
    let mut position: i64 = serde_json::from_str(&value)?;
    let mut query = db.prepare("SELECT first,last FROM distributed_order_rotations WHERE project_id=?1 AND (clock,origin)>(?2,?3) ORDER BY clock,origin")?;
    for movement in query.query_map(params![project, clock, origin], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })? {
        let (from, to) = movement?;
        position = rotate(position, from, to);
    }
    db.execute("INSERT INTO distributed_order_positions VALUES(?1,?2,?3,?4,?5) ON CONFLICT(project_id,number) DO UPDATE SET clock=excluded.clock,origin=excluded.origin,position=excluded.position", params![project,number,clock,origin,position])?;
    db.execute(
        "INSERT OR IGNORE INTO distributed_order_dirty VALUES(?1)",
        [project],
    )?;
    Ok(())
}

pub(super) fn reduce(db: &Connection, fact: &Revision) -> Result<()> {
    let row = fact
        .after
        .as_ref()
        .ok_or_else(|| replica::invalid("Missing queue rotation"))?;
    let project = row["project_id"]
        .as_str()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| replica::invalid("Missing queue project"))?;
    let from = row["from"]
        .as_i64()
        .ok_or_else(|| replica::invalid("Invalid queue position"))?;
    let to = row["to"]
        .as_i64()
        .ok_or_else(|| replica::invalid("Invalid queue position"))?;
    if fact.before.is_some()
        || from == to
        || row.as_object().is_none_or(|r| r.len() != 3)
        || fact.clock == 0
    {
        return Err(replica::invalid("Invalid queue rotation"));
    }
    if let Some(saved) = db.query_row("SELECT first,last FROM distributed_order_rotations WHERE project_id=?1 AND clock=?2 AND origin=?3", params![project,fact.clock,fact.origin], |r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).optional()? {
        return if saved==(from,to) {Ok(())} else {Err(replica::invalid("A replica reused a queue revision"))};
    }
    let late: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM distributed_order_rotations WHERE project_id=?1 AND (clock,origin)>(?2,?3))", params![project,fact.clock,fact.origin], |r|r.get(0))?;
    db.execute(
        "INSERT INTO distributed_order_rotations VALUES(?1,?2,?3,?4,?5)",
        params![project, fact.clock, fact.origin, from, to],
    )?;
    if late {
        // Only a genuinely out-of-order operation rebuilds coordinates. The
        // normal FIFO path rotates the indexed interval once, without bodies.
        let keys = replica::rows(
            db,
            "SELECT entity FROM distributed_fields WHERE table_name='issues' AND field='sort_order' AND json_extract(entity,'$.project_id')=?1",
            &[json!(project)],
        )?;
        for key in keys {
            seed(db, &serde_json::from_str(key["entity"].as_str().unwrap())?)?;
        }
    } else {
        db.execute("UPDATE distributed_order_positions SET position=CASE WHEN position=?4 THEN ?5 WHEN ?5<?4 THEN position+1 ELSE position-1 END WHERE project_id=?1 AND (clock,origin)<(?2,?3) AND position BETWEEN min(?4,?5) AND max(?4,?5)", params![project,fact.clock,fact.origin,from,to])?;
    }
    db.execute(
        "INSERT OR IGNORE INTO distributed_order_dirty VALUES(?1)",
        [project],
    )?;
    db.execute(
        "UPDATE distributed_meta SET clock=max(clock,?1) WHERE id=1",
        [fact.clock],
    )?;
    Ok(())
}

pub(super) fn position(db: &Connection, key: &Value) -> Result<Option<i64>> {
    Ok(db
        .query_row(
            "SELECT position FROM distributed_order_positions WHERE project_id=?1 AND number=?2",
            params![key["project_id"].as_str(), key["number"].as_i64()],
            |r| r.get(0),
        )
        .optional()?)
}

pub(super) fn apply(db: &Connection) -> Result<()> {
    // This is a derived ordering-only write under the projector's capture
    // guard. It neither revises issue content nor imports worker ownership.
    db.execute("UPDATE issues SET sort_order=(SELECT position FROM distributed_order_positions p WHERE p.project_id=issues.project_id AND p.number=issues.number) WHERE project_id IN (SELECT project_id FROM distributed_order_dirty) AND EXISTS(SELECT 1 FROM distributed_order_positions p WHERE p.project_id=issues.project_id AND p.number=issues.number AND p.position<>issues.sort_order)", [])?;
    db.execute("DELETE FROM distributed_order_dirty", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use rusqlite::params;
    use serde_json::json;

    struct Queue {
        root: std::path::PathBuf,
        db: Connection,
        engine: hey_boss_sync::Engine,
    }
    impl Queue {
        fn new(size: i64) -> Self {
            Self::with_origin(size, "source")
        }
        fn with_origin(size: i64, origin: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "hb-order-{}",
                crate::issues::worker::random_id().unwrap()
            ));
            std::fs::create_dir(&root).unwrap();
            let db = crate::issues::Store::open(&root.join("issues.db"))
                .unwrap()
                .into_database();
            install(&db, origin).unwrap();
            db.execute_batch("UPDATE distributed_meta SET active=1,slot=0; UPDATE fleet_meta SET role='peer'; INSERT INTO projects(id,name,next_number) VALUES('p','Project',5000); INSERT INTO agents VALUES('human:test','{}',0);").unwrap();
            let tx = db.unchecked_transaction().unwrap();
            for number in 1..=size {
                tx.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) VALUES('p',?1,'Unchanged title',?2,'open','human:test',1,1,1,'[]',?1)", params![number, "x".repeat(4096)]).unwrap();
            }
            tx.commit().unwrap();
            let engine = hey_boss_sync::Engine::open(&root.join("sync.db")).unwrap();
            let mut queue = Self { root, db, engine };
            queue.flush();
            queue
        }
        fn flush(&mut self) {
            while export(&self.db, &mut self.engine).unwrap() > 0 {}
            while import(&self.db, &self.engine).unwrap() > 0 {}
        }
        fn request(&self, operation: Value, id: &str) -> crate::issues::Request {
            serde_json::from_value(json!({"version":1,"project":{"id":"p","name":"Project"},"actor":{"id":"human:test","kind":"human","machine":"source","host":"test","cwd":"/tmp","source":"test"},"request_id":id,"operation":operation})).unwrap()
        }
        fn execute(&self, operation: Value, id: &str) -> crate::issues::Result<Value> {
            crate::issues::Store::open(&self.root.join("issues.db"))
                .unwrap()
                .execute(&self.request(operation, id))
        }
        fn send_to(&self, peer: &mut Self) -> usize {
            let mut pages = 0;
            loop {
                let after = peer.engine.received(self.engine.id()).unwrap();
                let batch = self.engine.batch(after).unwrap();
                if batch.records.is_empty() {
                    break;
                }
                peer.engine.ingest(&batch).unwrap();
                // Duplicate/reconnect delivery is idempotent.
                peer.engine.ingest(&batch).unwrap();
                peer.flush();
                pages += 1;
            }
            pages
        }
        fn numbers(&self) -> Vec<i64> {
            self.db
                .prepare("SELECT number FROM issues ORDER BY sort_order,number")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        }
        fn preserved(&self) -> Vec<Value> {
            replica::rows(&self.db,"SELECT number,title,body,state,assignee,closed_by,closed_at,deleted_at,draft,manual_blocked,blockers FROM issues ORDER BY number", &[]).unwrap()
        }
    }
    impl Drop for Queue {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn single_moves_have_bounded_records_and_bytes() {
        for size in [8, 4096] {
            let mut q = Queue::new(size);
            for step in 0..6 {
                let start = q.engine.head().unwrap();
                let op = if step % 2 == 0 {
                    json!({"action":"move","number":size,"before":1})
                } else {
                    json!({"action":"move","number":size})
                };
                q.execute(op, &format!("move-{step}")).unwrap();
                q.flush();
                let batch = q.engine.batch(start).unwrap();
                let count = q.engine.head().unwrap() - start;
                let bytes = serde_json::to_vec(&batch).unwrap().len();
                eprintln!("queue={size} move={step}: records={count} wire_bytes={bytes}");
                assert!(count <= 6, "single move emitted {count} records");
                assert!(bytes < 8192, "single move transmitted {bytes} bytes");
                assert!(
                    batch
                        .records
                        .iter()
                        .all(|r| !r.payload.contains("Unchanged title")
                            && !r.payload.contains(&"x".repeat(4096)))
                );
                let numbers =
                    q.db.prepare("SELECT number FROM issues ORDER BY sort_order,number")
                        .unwrap()
                        .query_map([], |r| r.get::<_, i64>(0))
                        .unwrap()
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .unwrap();
                let mut expected: Vec<_> = (1..size).collect();
                if step % 2 == 0 {
                    expected.insert(0, size)
                } else {
                    expected.push(size)
                };
                assert_eq!(numbers, expected);
            }
        }
    }

    #[test]
    fn moves_preserve_lifecycle_guards_receipts_and_deliver_following_work_in_one_page() {
        let mut a = Queue::new(4096);
        let mut b = Queue::with_origin(0, "peer");
        a.db.execute_batch("UPDATE issues SET assignee='human:test' WHERE number=2; UPDATE issues SET draft=1 WHERE number=3;
            UPDATE issues SET state='blocked',manual_blocked=1 WHERE number=4;
            UPDATE issues SET state='closed',closed_at=2,closed_by='human:test' WHERE number=5;
            UPDATE issues SET blockers='[1]',state='blocked' WHERE number=6;").unwrap();
        a.flush();
        a.send_to(&mut b);
        let before = a.preserved();
        let version: i64 =
            a.db.query_row(
                "SELECT issue_order_version FROM projects WHERE id='p'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let movement = json!({"action":"move","number":4096,"before":1,"if_order_version":version});
        let saved = a.execute(movement.clone(), "once").unwrap();
        assert_eq!(a.execute(movement, "once").unwrap(), saved);
        assert_eq!(
            a.execute(
                json!({"action":"move","number":1,"after":2,"if_order_version":version}),
                "stale"
            )
            .unwrap_err()
            .code,
            "conflict"
        );
        assert_eq!(a.preserved(), before);
        // Concurrent peer metadata must survive the queue rotation.
        b.execute(
            json!({"action":"edit","number":4096,"title":"Concurrent metadata","add_labels":[],"remove_labels":[]}),
            "metadata",
        )
        .unwrap();
        b.flush();
        a.execute(json!({"action":"reopen","number":5}), "reopen")
            .unwrap();
        a.execute(
            json!({"action":"comment","number":5,"body":"Following comment"}),
            "comment",
        )
        .unwrap();
        a.db.execute("UPDATE fleet_meta SET node='source'", [])
            .unwrap();
        a.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at) VALUES('following-reservation','p',5,'{}','human:test','reserved',100,'synthetic','source',2,2)", []).unwrap();
        crate::issues::allocations::receive_presence(
            &b.db,
            &[hey_boss_sync::session::Presence {
                machine: "source".into(),
                sequence: 1,
                attempts: vec!["following-reservation".into()],
                remaining_ms: 24_000,
            }],
            std::time::Duration::ZERO,
        )
        .unwrap();
        assert_eq!(
            crate::issues::allocations::pending_reservations(&b.db).unwrap(),
            1
        );
        a.flush();
        assert_eq!(
            a.send_to(&mut b),
            1,
            "reopen/comment/reservation must fit in the next wire page"
        );
        assert_eq!(
            crate::issues::allocations::pending_reservations(&b.db).unwrap(),
            0,
            "a queue move must not hold new agents behind thousands of issue snapshots"
        );
        assert_eq!(
            b.db.query_row(
                "SELECT node FROM fleet_allocations WHERE project_id='p' AND issue_number=5",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "source"
        );
        b.send_to(&mut a);
        assert_eq!(a.numbers(), b.numbers());
        assert_eq!(a.preserved(), b.preserved());
        let row = a.preserved().pop().unwrap();
        assert_eq!(row["title"], "Concurrent metadata");
        assert_eq!(b.preserved()[4]["state"], "open");
        assert_eq!(
            b.db.query_row(
                "SELECT count(*) FROM comments WHERE body='Following comment'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            b.db.query_row("SELECT count(*) FROM worker_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        // Reopen the stores and replay neither journal nor application history.
        a.send_to(&mut b);
        let head = b.engine.head().unwrap();
        let db = crate::issues::Store::open(&b.root.join("issues.db"))
            .unwrap()
            .into_database();
        install(&db, "peer").unwrap();
        assert_eq!(a.send_to(&mut b), 0);
        assert_eq!(b.engine.head().unwrap(), head);
        assert_eq!(a.numbers(), b.numbers());
    }

    #[test]
    fn capture_upgrade_preserves_old_history_and_pending_births_can_move() {
        let mut q = Queue::new(8);
        let head = q.engine.head().unwrap();
        let checkpoints: (i64, i64) =
            q.db.query_row("SELECT exported,projected FROM distributed_meta", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        q.db.execute_batch("DROP TABLE distributed_order_positions; DROP TABLE distributed_order_dirty; DROP TABLE distributed_order_rotations; DROP TABLE distributed_order_capture; DROP TRIGGER distributed_capture_issues_UPDATE;").unwrap();
        install(&q.db, "source").unwrap();
        assert_eq!(q.engine.head().unwrap(), head);
        assert_eq!(
            q.db.query_row("SELECT exported,projected FROM distributed_meta", [], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })
            .unwrap(),
            checkpoints
        );
        q.execute(json!({"action":"move","number":8,"before":1}), "upgraded")
            .unwrap();
        q.flush();
        assert_eq!(q.numbers(), vec![8, 1, 2, 3, 4, 5, 6, 7]);
        // New births already have durable coordinates in the outbox; moving
        // immediately after create does not wait for a background sync tick.
        q.db.execute("INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order) VALUES('p',9,'New task','','open','human:test',1,1,1,'[]',9)", []).unwrap();
        q.execute(json!({"action":"move","number":9,"before":8}), "new-birth")
            .unwrap();
        q.flush();
        while !baseline(&q.db, &mut q.engine, true).unwrap() {}
        q.flush();
        assert_eq!(q.numbers(), vec![9, 8, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn concurrent_and_reverse_delivery_rotations_converge_without_duplicate_positions() {
        let mut a = Queue::new(8);
        let mut b = Queue::with_origin(0, "peer");
        a.send_to(&mut b);
        a.execute(json!({"action":"move","number":8,"before":1}), "a")
            .unwrap();
        b.execute(json!({"action":"move","number":1,"after":8}), "b")
            .unwrap();
        a.flush();
        b.flush();
        // Each peer has already reduced its own move before receiving the other.
        a.send_to(&mut b);
        b.send_to(&mut a);
        assert_eq!(a.numbers(), b.numbers());
        for q in [&a, &b] {
            assert_eq!(
                q.db.query_row("SELECT count(DISTINCT sort_order) FROM issues", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
                8
            );
        }
        let c = Queue::with_origin(0, "late");
        let mut records = Vec::new();
        let mut cursor = 0;
        loop {
            let batch = a.engine.batch(cursor).unwrap();
            if batch.records.is_empty() {
                break;
            }
            cursor = batch.through;
            records.extend(batch.records);
        }
        let tx = c.db.unchecked_transaction().unwrap();
        // Includes old full snapshots, compact metadata and rotations. Applying
        // the entire history in reverse must still yield the same queue.
        for record in records.into_iter().rev() {
            if let Some(fact) = archives::revision(&record.payload).unwrap() {
                projection::reduce(&tx, &fact).unwrap();
            }
        }
        materialize::drain(&tx).unwrap();
        tx.commit().unwrap();
        assert_eq!(a.numbers(), c.numbers());
    }
}
