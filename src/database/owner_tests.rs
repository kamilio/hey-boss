use super::*;

fn writer(db: rusqlite::Connection) -> Writer {
    Writer {
        db: Mutex::new(db),
        waiters: Mutex::new(VecDeque::new()),
        ready: Condvar::new(),
        next: AtomicUsize::new(0),
    }
}

#[test]
fn writer_release_cannot_notify_between_availability_check_and_wait() {
    let writer = writer(rusqlite::Connection::open_in_memory().unwrap());
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (released_tx, released_rx) = std::sync::mpsc::channel();
        let writer_ref = &writer;
        let stop_ref = &stop;
        let owner = scope.spawn(move || {
            let (stream, _peer) = UnixStream::pair().unwrap();
            let lease = acquire(writer_ref, &stream, stop_ref).unwrap();
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(lease);
            released_tx.send(()).unwrap();
        });
        held_rx.recv().unwrap();
        // Reproduce acquire's check-to-wait gap while holding its queue lock.
        let waiters = writer.waiters.lock().unwrap();
        assert!(matches!(
            writer.db.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        release_tx.send(()).unwrap();
        let released_before_wait = released_rx.recv_timeout(Duration::from_millis(100)).is_ok();
        let started = Instant::now();
        let (waiters, result) = writer
            .ready
            .wait_timeout(waiters, Duration::from_secs(1))
            .unwrap();
        drop(waiters);
        owner.join().unwrap();
        eprintln!(
            "Writer release finished before wait: {released_before_wait}; wake timed out: {}; wait: {:?}",
            result.timed_out(),
            started.elapsed()
        );
        assert!(
            !released_before_wait,
            "Writer release escaped the check-to-wait synchronization"
        );
        assert!(!result.timed_out(), "Writer release notification was lost");
        assert!(writer.db.try_lock().is_ok());
    });
}

#[test]
fn writer_handoff_keeps_fifo_order_after_a_queued_client_disconnects() {
    let writer = writer(rusqlite::Connection::open_in_memory().unwrap());
    let stop = AtomicBool::new(false);
    let (stream, _peer) = UnixStream::pair().unwrap();
    let lease = acquire(&writer, &stream, &stop).unwrap();
    std::thread::scope(|scope| {
        let (sent, received) = std::sync::mpsc::channel();
        let mut peers = Vec::new();
        for index in 0..4 {
            let (stream, peer) = UnixStream::pair().unwrap();
            peers.push(peer);
            let sent = sent.clone();
            let writer = &writer;
            let stop = &stop;
            scope.spawn(move || {
                let lease = acquire(writer, &stream, stop);
                sent.send((index, lease.is_ok())).unwrap();
                drop(lease);
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while writer.waiters.lock().unwrap().len() != index + 1 {
                assert!(
                    Instant::now() < deadline,
                    "Client did not join the writer queue"
                );
                std::thread::yield_now();
            }
        }
        // The first client leaves while the original transaction still owns
        // the writer; its cancellation must not let later arrivals overtake.
        drop(peers.remove(0));
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            (0, false)
        );
        assert!(received.try_recv().is_err());
        drop(lease);
        for index in 1..4 {
            assert_eq!(
                received.recv_timeout(Duration::from_secs(5)).unwrap(),
                (index, true)
            );
        }
    });
    assert!(writer.waiters.lock().unwrap().is_empty());
    assert!(writer.db.try_lock().is_ok());
}

fn run(db: &rusqlite::Connection, command: Command) -> Result<Reply> {
    let (mut output, _input) = UnixStream::pair().unwrap();
    let mut reply = Reply::default();
    execute(db, command, &mut reply, &mut output)?;
    Ok(reply)
}

#[test]
fn pragma_queries_and_setters_always_observe_current_settings() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    let read = || Command::Query {
        sql: "PRAGMA synchronous".into(),
        values: vec![],
    };
    let off = || Command::Execute {
        sql: "PRAGMA synchronous=OFF".into(),
        values: vec![],
    };
    let reply = run(&db, read()).unwrap();
    assert!(matches!(reply.rows[0][0], wire::SqlValue::Integer(2)));
    run(&db, off()).unwrap();
    let reply = run(&db, read()).unwrap();
    assert!(matches!(reply.rows[0][0], wire::SqlValue::Integer(0)));
    db.execute_batch("PRAGMA synchronous=FULL").unwrap();
    run(&db, off()).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn writer_release_rolls_back_and_restores_foreign_keys_and_full_sync() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE changes(value INTEGER)")
        .unwrap();
    let writer = writer(db);
    {
        let lease = Lease(Some(writer.db.lock().unwrap()), &writer);
        lease.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA synchronous=OFF; BEGIN; INSERT INTO changes VALUES(1)").unwrap();
    }
    let db = writer.db.lock().unwrap();
    assert!(db.is_autocommit());
    assert!(
        db.db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY)
            .unwrap()
    );
    assert_eq!(
        db.pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM changes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn reused_queries_refresh_columns_after_schema_changes_even_without_rows() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE shape(a INTEGER); INSERT INTO shape VALUES(1)")
        .unwrap();
    let query = || Command::Query {
        sql: "SELECT * FROM shape WHERE a>?1".into(),
        values: vec![wire::SqlValue::Integer(0)],
    };
    let empty = || Command::Query {
        sql: "SELECT * FROM shape WHERE 0".into(),
        values: vec![],
    };
    assert_eq!(run(&db, query()).unwrap().columns, ["a"]);
    assert_eq!(run(&db, empty()).unwrap().columns, ["a"]);
    db.execute_batch("ALTER TABLE shape ADD COLUMN b INTEGER DEFAULT 2")
        .unwrap();
    let reply = run(&db, query()).unwrap();
    assert_eq!(reply.columns, ["a", "b"]);
    assert_eq!(reply.rows[0].len(), 2);
    assert!(matches!(reply.rows[0][1], wire::SqlValue::Integer(2)));
    let reply = run(&db, empty()).unwrap();
    assert_eq!(reply.columns, ["a", "b"]);
    assert!(reply.rows.is_empty());
    db.execute_batch(
        "DROP TABLE shape; CREATE TABLE shape(a INTEGER,c TEXT); INSERT INTO shape VALUES(3,'new')",
    )
    .unwrap();
    let reply = run(&db, query()).unwrap();
    assert_eq!(reply.columns, ["a", "c"]);
    assert!(matches!(&reply.rows[0][1], wire::SqlValue::Text(value) if value == "new"));
    let reply = run(
        &db,
        Command::Prepare {
            sql: "SELECT * FROM shape".into(),
        },
    )
    .unwrap();
    assert_eq!(reply.columns, ["a", "c"]);
    assert!(
        run(
            &db,
            Command::Query {
                sql: "SELECT * FROM shape WHERE a>?1".into(),
                values: vec![]
            }
        )
        .is_err()
    );
    assert_eq!(run(&db, query()).unwrap().rows.len(), 1);
}

#[test]
fn repeated_service_statements_compile_once_and_keep_per_query_work_counts() {
    // The authorizer runs when SQLite compiles SQL, so this counts compilation
    // directly rather than relying on noisy wall-clock timing.
    unsafe extern "C" fn count_compiles(
        context: *mut std::ffi::c_void,
        action: std::ffi::c_int,
        _: *const std::ffi::c_char,
        _: *const std::ffi::c_char,
        _: *const std::ffi::c_char,
        _: *const std::ffi::c_char,
    ) -> std::ffi::c_int {
        if action == rusqlite::ffi::SQLITE_SELECT || action == rusqlite::ffi::SQLITE_UPDATE {
            // The boxed counter outlives this connection and its callbacks.
            let count = unsafe { &*context.cast::<AtomicUsize>() };
            count.fetch_add(1, Ordering::Relaxed);
        }
        rusqlite::ffi::SQLITE_OK
    }

    let count = Box::new(AtomicUsize::new(0));
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE counters(id INTEGER PRIMARY KEY,value INTEGER); INSERT INTO counters VALUES(1,10),(2,20)").unwrap();
    unsafe {
        assert_eq!(
            rusqlite::ffi::sqlite3_set_authorizer(
                db.handle(),
                Some(count_compiles),
                (&*count as *const AtomicUsize).cast_mut().cast(),
            ),
            rusqlite::ffi::SQLITE_OK
        );
    }
    let (mut output, _input) = UnixStream::pair().unwrap();
    let mut first_steps = None;
    for n in 0..128 {
        let id = n % 2 + 1;
        let mut reply = Reply::default();
        execute(
            &db,
            Command::Query {
                sql: "SELECT value FROM counters WHERE id=?1".into(),
                values: vec![wire::SqlValue::Integer(id)],
            },
            &mut reply,
            &mut output,
        )
        .unwrap();
        assert_eq!(reply.columns, ["value"]);
        assert_eq!(reply.rows.len(), 1);
        assert!(matches!(reply.rows[0][0], wire::SqlValue::Integer(value) if value == id*10));
        assert_eq!(reply.steps, *first_steps.get_or_insert(reply.steps));
    }
    let writer = writer(db);
    for value in 0..128 {
        let lease = Lease(Some(writer.db.lock().unwrap()), &writer);
        let mut reply = Reply::default();
        execute(
            &lease,
            Command::Execute {
                sql: "UPDATE counters SET value=?1 WHERE id=?2".into(),
                values: vec![wire::SqlValue::Integer(value), wire::SqlValue::Integer(1)],
            },
            &mut reply,
            &mut output,
        )
        .unwrap();
        assert_eq!(reply.changes, 1);
    }
    let compiles = count.load(Ordering::Relaxed);
    eprintln!("128 repeated reads + 128 writes: {compiles} SQL compilations");
    assert_eq!(
        compiles, 2,
        "Repeated parameterized SQL must reuse its compiled statement"
    );
    assert_eq!(
        writer
            .db
            .lock()
            .unwrap()
            .query_row("SELECT value FROM counters WHERE id=1", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        127
    );
}
