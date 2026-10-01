use super::*;

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
    let writer = Mutex::new(db);
    let ready = Condvar::new();
    {
        let lease = Lease(Some(writer.lock().unwrap()), &ready);
        lease.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA synchronous=OFF; BEGIN; INSERT INTO changes VALUES(1)").unwrap();
    }
    let db = writer.lock().unwrap();
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
    let writer = Mutex::new(db);
    let ready = Condvar::new();
    for value in 0..128 {
        let lease = Lease(Some(writer.lock().unwrap()), &ready);
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
            .lock()
            .unwrap()
            .query_row("SELECT value FROM counters WHERE id=1", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        127
    );
}
