#[test]
fn opens_persistent_replication_storage() {
    let directory = tempfile::tempdir().unwrap();
    let engine = super::Engine::open(&directory.path().join("replica.db")).unwrap();
    assert_eq!(engine.head().unwrap(), 0);
}

#[test]
fn batch_clock_lookup_is_bounded_by_page_size() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    unsafe extern "C" fn budget(context: *mut std::ffi::c_void) -> i32 {
        let steps = unsafe { &*context.cast::<AtomicUsize>() };
        i32::from(steps.fetch_add(1000, Ordering::Relaxed) >= 200_000)
    }
    let root = tempfile::tempdir().unwrap();
    let mut engine = super::Engine::open(&root.path().join("a.db")).unwrap();
    let tx = engine.db.transaction().unwrap();
    for n in 0..2048 {
        let payload = format!("retained fact {n}");
        tx.execute(
            "INSERT INTO records VALUES(?1,?2)",
            rusqlite::params![super::digest(payload.as_bytes()), payload],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let steps = AtomicUsize::new(0);
    // Count SQLite VM work, including the virtual table's nested statements;
    // a wall-clock assertion would depend on the machine running the test.
    unsafe {
        rusqlite::ffi::sqlite3_progress_handler(
            engine.db.handle(),
            1000,
            Some(budget),
            (&steps as *const AtomicUsize).cast_mut().cast(),
        );
    }
    let batch = engine.batch(1920);
    unsafe {
        rusqlite::ffi::sqlite3_progress_handler(engine.db.handle(), 0, None, std::ptr::null_mut());
    }
    let batch = batch.expect("a page must not scan all retained clocks for each record");
    assert_eq!(batch.records.len(), 128);
    assert_eq!(batch.through, 2048);
}

#[test]
fn batch_clocks_match_public_changes_after_local_remote_and_duplicate_writes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("a.db");
    let mut a = super::Engine::open(&path).unwrap();
    let mut b = super::Engine::open(&root.path().join("b.db")).unwrap();
    a.put("local").unwrap();
    a.put("shared").unwrap();
    b.put("remote").unwrap();
    b.put("shared").unwrap();
    a.ingest(&b.batch(0).unwrap()).unwrap();
    a.ingest(&b.batch(0).unwrap()).unwrap();
    drop(a);
    let a = super::Engine::open(&path).unwrap();
    let batch = a.batch(0).unwrap();
    assert_eq!(batch.records.len(), 3);
    for record in batch.records {
        let expected: (Vec<u8>, i64, i64, String) = a.db.query_row(
            "SELECT site_id,db_version,seq,ts FROM crsql_changes WHERE \"table\"='records' AND pk=crsql_pack_columns(?1) AND cid='payload'",
            [&record.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert_eq!(
            (
                record.site,
                record.version,
                record.sequence,
                record.timestamp
            ),
            expected
        );
    }
}

#[test]
fn disconnected_attempts_survive_restart_and_relay_without_main() {
    let root = tempfile::tempdir().unwrap();
    let a_path = root.path().join("a.db");
    let mut a = super::Engine::open(&a_path).unwrap();
    let mut b = super::Engine::open(&root.path().join("b.db")).unwrap();
    let mut relay = super::Engine::open(&root.path().join("relay.db")).unwrap();
    let a_id = a.put(r#"{"attempt":"a","result":"completed"}"#).unwrap();
    let b_id = b.put(r#"{"attempt":"b","result":"completed"}"#).unwrap();
    drop(a);
    let mut a = super::Engine::open(&a_path).unwrap();
    relay.ingest(&a.batch(0).unwrap()).unwrap();
    relay.ingest(&b.batch(0).unwrap()).unwrap();
    let batch = relay.batch(0).unwrap();
    a.ingest(&batch).unwrap();
    b.ingest(&batch).unwrap();
    a.ingest(&batch).unwrap();
    assert_eq!(a.head().unwrap(), 2);
    assert_eq!(b.head().unwrap(), 2);
    assert_eq!(a.payload(&a_id).unwrap(), b.payload(&a_id).unwrap());
    assert_eq!(a.payload(&b_id).unwrap(), b.payload(&b_id).unwrap());
}

#[test]
fn corrupt_batch_commits_neither_records_nor_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let mut a = super::Engine::open(&root.path().join("a.db")).unwrap();
    let mut b = super::Engine::open(&root.path().join("b.db")).unwrap();
    a.put("first").unwrap();
    a.put("second").unwrap();
    let mut batch = a.batch(0).unwrap();
    batch.records[1].payload = "corrupt".into();
    assert!(b.ingest(&batch).is_err());
    assert_eq!(b.head().unwrap(), 0);
    assert_eq!(b.received(&batch.replica).unwrap(), 0);
}

#[test]
fn gaps_are_rejected_and_replayed_prefixes_cannot_rewind_receipts() {
    let root = tempfile::tempdir().unwrap();
    let mut a = super::Engine::open(&root.path().join("a.db")).unwrap();
    let mut b = super::Engine::open(&root.path().join("b.db")).unwrap();
    a.put("first").unwrap();
    let first = a.batch(0).unwrap();
    a.put("second").unwrap();
    let second = a.batch(1).unwrap();
    assert!(b.ingest(&second).is_err());
    b.ingest(&first).unwrap();
    b.ingest(&second).unwrap();
    b.ingest(&first).unwrap();
    assert_eq!(b.received(&first.replica).unwrap(), 2);
    assert!(a.batch(3).is_err());
}

#[test]
fn bounded_pages_forward_remote_origins_and_resume_after_restarts() {
    let root = tempfile::tempdir().unwrap();
    let mut a = super::Engine::open(&root.path().join("a.db")).unwrap();
    let mut relay = super::Engine::open(&root.path().join("relay.db")).unwrap();
    let mut b = super::Engine::open(&root.path().join("b.db")).unwrap();
    for n in 0..300 {
        a.put(&format!("record {n}")).unwrap();
    }
    for _ in 0..3 {
        let batch = a.batch(relay.received(a.id()).unwrap()).unwrap();
        relay.ingest(&batch).unwrap();
        let batch = relay.batch(b.received(relay.id()).unwrap()).unwrap();
        b.ingest(&batch).unwrap();
    }
    assert_eq!(b.head().unwrap(), 300);
    for n in 0..300 {
        assert_eq!(
            b.payload(&super::digest(format!("record {n}").as_bytes()))
                .unwrap(),
            Some(format!("record {n}"))
        );
    }
    drop(relay);
    let relay = super::Engine::open(&root.path().join("relay.db")).unwrap();
    assert_eq!(relay.received(a.id()).unwrap(), 300);
}

#[test]
fn wire_rejects_oversized_truncated_and_invalid_payloads() {
    let header = (super::wire::MAX_FRAME as u32 + 1).to_be_bytes();
    assert!(super::wire::receive::<super::Batch>(&mut &header[..]).is_err());
    assert!(super::wire::receive::<super::Batch>(&mut &[0, 0][..]).is_err());
    assert!(super::wire::receive::<super::Batch>(&mut &[0, 0, 0, 2, b'{'][..]).is_err());
    assert!(
        super::wire::receive::<super::Batch>(&mut &[][..])
            .unwrap()
            .is_none()
    );
}
