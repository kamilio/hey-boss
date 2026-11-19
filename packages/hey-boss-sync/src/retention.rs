//! The retained facts are a replayable snapshot plus a bounded recent log.
//! Only the application can prove that a fact is superseded. Unknown facts,
//! resources and retirement tombstones stay available to their own consumers.
use super::*;

const RECENT_RECORDS: i64 = 10_000;
const RECENT_BYTES: i64 = 64 * 1024 * 1024;

impl Engine {
    /// Inspect at most one ordinary page, and never retire unapplied facts.
    /// `obsolete` must prove that replaying the remaining facts reconstructs
    /// every effect of this record, including conflict and deletion metadata.
    pub fn compact(
        &mut self,
        applied: i64,
        mut obsolete: impl FnMut(i64, &str, &str) -> Result<bool>,
    ) -> Result<usize> {
        let (mut cursor, mut cutoff, swept, recent_bytes): (i64, i64, i64, i64) =
            self.db.query_row(
                "SELECT cursor,cutoff,head,recent_bytes FROM retention WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
        let head = self.head()?;
        if cursor >= cutoff {
            // Avoid rescanning the entire snapshot on every idle SSH exchange.
            // The next sweep also revisits formerly current facts after edits.
            if head - swept < 1000 && recent_bytes <= RECENT_BYTES {
                return Ok(0);
            }
            // Serialize the byte-counter reset with arrivals. Otherwise a
            // concurrent large append could escape the next size check.
            let tx = self
                .db
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (boundary, bytes): (i64, i64) = tx.query_row(
                "SELECT coalesce(max(CASE WHEN n>?2 OR bytes>?3 THEN position END),0),
                    coalesce(sum(CASE WHEN n<=?2 AND bytes<=?3 THEN size ELSE 0 END),0) FROM (
                    SELECT position,row_number() OVER (ORDER BY position DESC) AS n,
                        size,sum(size) OVER (ORDER BY position DESC) AS bytes FROM (
                            SELECT d.position,CASE typeof(r.payload) WHEN 'blob' THEN length(r.payload)
                                ELSE length(CAST(r.payload AS BLOB)) END AS size
                            FROM delivery d JOIN records r USING(id)
                            ORDER BY d.position DESC LIMIT ?1
                        )
                )",
                params![RECENT_RECORDS + 1, RECENT_RECORDS, RECENT_BYTES],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            cutoff = boundary;
            tx.execute(
                "UPDATE retention SET cursor=0,cutoff=?1,head=coalesce((SELECT seq FROM sqlite_sequence WHERE name='delivery'),0),recent_bytes=?2 WHERE id=1",
                params![cutoff, bytes],
            )?;
            tx.commit()?;
            cursor = 0;
        }
        let through = cutoff.min(applied);
        if cursor >= through {
            return Ok(0);
        }
        let mut removed = Vec::new();
        let mut bytes = 0;
        let mut scanned = 0;
        let mut next = through;
        {
            let mut query = self.db.prepare("SELECT d.position,r.id,r.payload FROM delivery d JOIN records r USING(id) WHERE d.position>?1 AND d.position<=?2 ORDER BY d.position LIMIT 128")?;
            for row in query.query_map(params![cursor, through], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    compression::payload(r, 2)?,
                ))
            })? {
                let (position, id, payload) = row?;
                if scanned > 0 && bytes + payload.len() > PAGE_BYTES {
                    break;
                }
                bytes += payload.len();
                scanned += 1;
                next = position;
                // Empty records prevent stale peers from resurrecting retired
                // resources. They are not ordinary superseded field revisions.
                if !payload.is_empty() && obsolete(position, &id, &payload)? {
                    removed.push(id);
                }
            }
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = serde_json::to_string(&removed)?;
        tx.execute(
            "DELETE FROM records WHERE id IN (SELECT value FROM json_each(?1)) AND payload<>''",
            [&ids],
        )?;
        if !removed.is_empty() {
            // Our protocol replicates immutable facts, not CRR deletions. The
            // application proved these identities unnecessary for replay. Drop
            // their lookasides too, rather than growing one tombstone per edit.
            // Preserve the pinned extension's version high-water mark first.
            tx.execute("INSERT INTO crsql_master(key,value) VALUES('pre_compact_dbversion',crsql_db_version()) ON CONFLICT(key) DO UPDATE SET value=max(value,excluded.value)",[])?;
            tx.execute("DELETE FROM records__crsql_clock WHERE key IN (SELECT __crsql_key FROM records__crsql_pks WHERE id IN (SELECT value FROM json_each(?1)) AND NOT EXISTS(SELECT 1 FROM records WHERE records.id=records__crsql_pks.id))",[&ids])?;
            tx.execute("DELETE FROM records__crsql_pks WHERE id IN (SELECT value FROM json_each(?1)) AND NOT EXISTS(SELECT 1 FROM records WHERE records.id=records__crsql_pks.id)",[&ids])?;
            tx.execute("UPDATE retention SET compacted=max(compacted,coalesce((SELECT max(position) FROM delivery WHERE id IN (SELECT value FROM json_each(?1)) AND NOT EXISTS(SELECT 1 FROM records WHERE records.id=delivery.id)),0)) WHERE id=1",[&ids])?;
            tx.execute("DELETE FROM delivery WHERE id IN (SELECT value FROM json_each(?1)) AND NOT EXISTS(SELECT 1 FROM records WHERE records.id=delivery.id)",[&ids])?;
        }
        tx.execute("UPDATE retention SET cursor=?1 WHERE id=1", [next])?;
        tx.commit()?;
        if !removed.is_empty() {
            self.db.execute_batch("PRAGMA incremental_vacuum(128)")?;
        }
        Ok(scanned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn append(engine: &mut Engine, start: usize, count: usize, padding: usize) {
        let tx = engine.db.transaction().unwrap();
        let mut insert = tx.prepare("INSERT INTO records VALUES(?1,?2)").unwrap();
        for n in start..start + count {
            let payload = format!("record-{n}:{}", "x".repeat(padding));
            insert
                .execute(params![digest(payload.as_bytes()), payload])
                .unwrap();
        }
        drop(insert);
        tx.commit().unwrap();
    }

    fn count(engine: &Engine, table: &str) -> i64 {
        engine
            .db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn rotation_replays_current_state_and_recent_history_after_restart() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("a.db");
        let mut a = Engine::open(&path).unwrap();
        let current = a.put("current state").unwrap();
        let retired = a.put("retired resource").unwrap();
        a.prune(std::slice::from_ref(&retired)).unwrap();
        append(&mut a, 0, 11_000, 0);
        let head = a.head().unwrap();
        let stale = a.batch(0).unwrap();
        let identity = a.id().to_owned();
        // Reopen a pre-retention store. Its replica identity and existing
        // delivery positions must survive the additive schema upgrade.
        a.db.execute_batch("DROP TRIGGER record_retention_bytes; DROP TABLE retention")
            .unwrap();
        drop(a);
        let mut a = Engine::open(&path).unwrap();
        assert_eq!(a.id(), identity);
        while a.compact(head, |_, id, _| Ok(id != current)).unwrap() > 0 {}
        assert_eq!(count(&a, "records"), RECENT_RECORDS + 2);
        assert_eq!(count(&a, "delivery"), RECENT_RECORDS + 2);
        assert_eq!(count(&a, "records__crsql_pks"), RECENT_RECORDS + 2);
        assert_eq!(a.head().unwrap(), head);
        let version: i64 =
            a.db.query_row("SELECT crsql_db_version()", [], |r| r.get(0))
                .unwrap();
        drop(a);
        let mut a = Engine::open(&path).unwrap();
        assert!(
            a.db.query_row("SELECT crsql_db_version()", [], |r| r.get::<_, i64>(0))
                .unwrap()
                >= version
        );
        let mut fresh = Engine::open(&root.path().join("fresh.db")).unwrap();
        loop {
            let batch = a.batch(fresh.received(a.id()).unwrap()).unwrap();
            let done = batch.records.is_empty();
            fresh.ingest(&batch).unwrap();
            if done {
                break;
            }
        }
        assert_eq!(fresh.head().unwrap(), RECENT_RECORDS + 2);
        assert_eq!(
            fresh.payload(&current).unwrap().as_deref(),
            Some("current state")
        );
        assert!(fresh.payload(&retired).unwrap().is_none());
        // A stale relay can send obsolete facts again. The application still
        // reduces their original clocks, and another sweep reclaims them.
        fresh.ingest(&stale).unwrap();
        assert!(fresh.payload(&retired).unwrap().is_none());
        a.put("after restart").unwrap();
        assert_eq!(a.head().unwrap(), head + 1);
        let batch = a.batch(head).unwrap();
        assert!(batch.records[0].version > version);
        fresh.ingest(&batch).unwrap();
        assert_eq!(fresh.received(a.id()).unwrap(), head + 1);
    }

    #[test]
    fn unapplied_records_and_failed_retirement_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let mut a = Engine::open(&root.path().join("a.db")).unwrap();
        append(&mut a, 0, 11_000, 0);
        assert_eq!(a.compact(0, |_, _, _| panic!("unapplied")).unwrap(), 0);
        a.db.execute_batch("CREATE TRIGGER fail_retirement BEFORE UPDATE OF cursor ON retention WHEN NEW.cursor>0 BEGIN SELECT RAISE(ABORT,'interrupted'); END;").unwrap();
        assert!(a.compact(11_000, |_, _, _| Ok(true)).is_err());
        assert_eq!(count(&a, "records"), 11_000);
        assert_eq!(count(&a, "delivery"), 11_000);
        assert_eq!(count(&a, "records__crsql_pks"), 11_000);
        assert_eq!(a.batch(0).unwrap().compacted, 0);
        a.db.execute_batch("DROP TRIGGER fail_retirement").unwrap();
        assert_eq!(a.compact(64, |_, _, _| Ok(true)).unwrap(), 64);
        assert_eq!(
            a.compact(64, |_, _, _| panic!("past applied cursor"))
                .unwrap(),
            0
        );
        assert_eq!(count(&a, "records"), 11_000 - 64);
    }

    #[test]
    fn byte_limit_bounds_large_recent_payloads_and_idle_sweeps_do_no_work() {
        let root = tempfile::tempdir().unwrap();
        let mut a = Engine::open(&root.path().join("a.db")).unwrap();
        append(&mut a, 0, 34, 2 * 1024 * 1024);
        let head = a.head().unwrap();
        while a.compact(head, |_, _, _| Ok(true)).unwrap() > 0 {}
        let bytes: i64 =
            a.db.query_row(
                "SELECT sum(length(CAST(payload AS BLOB))) FROM records",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(bytes <= RECENT_BYTES);
        assert!(count(&a, "records") < 34);
        let changes = a.db.total_changes();
        assert_eq!(a.compact(head, |_, _, _| panic!("idle sweep")).unwrap(), 0);
        assert_eq!(a.db.total_changes(), changes);
    }

    #[test]
    fn sparse_replay_rejects_gaps_outside_the_declared_compacted_prefix() {
        let root = tempfile::tempdir().unwrap();
        let mut a = Engine::open(&root.path().join("a.db")).unwrap();
        let mut b = Engine::open(&root.path().join("b.db")).unwrap();
        for payload in ["one", "two", "three"] {
            a.put(payload).unwrap();
        }
        let mut batch = a.batch(0).unwrap();
        batch.records.remove(1);
        batch.compacted = 1;
        assert!(b.ingest(&batch).is_err());
        batch.compacted = 2;
        b.ingest(&batch).unwrap();
        assert_eq!(b.received(a.id()).unwrap(), 3);
        let mut bad = batch.clone();
        bad.records.swap(0, 1);
        assert!(b.ingest(&bad).is_err());
        bad = batch.clone();
        bad.through = 4;
        assert!(b.ingest(&bad).is_err());
        bad = batch;
        bad.records[1].payload = "corrupt".into();
        assert!(b.ingest(&bad).is_err());
    }
}
