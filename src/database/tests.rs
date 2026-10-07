use super::*;
use std::{sync::mpsc, time::Duration};

#[test]
fn socket_access_denials_keep_their_kind_and_recovery_guidance() {
    for errno in [libc::EPERM, libc::EACCES] {
        let source = std::io::Error::from_raw_os_error(errno);
        let original = source.to_string();
        let error = connection_error(source);
        assert!(permission_denied(&error));
        let domain = crate::issues::Error::from(error);
        assert_eq!(domain.code, "database_error");
        assert_eq!(domain.exit_code(), 1);
        assert!(domain.message.contains(&original));
        assert!(domain.message.contains("Database service access denied"));
        assert!(domain.message.contains("without pipes or redirection"));
        assert!(domain.message.contains("\nRetry hey-boss"));
        assert!(
            domain
                .message
                .contains("\nCompact reads: hey-boss issue list --limit 20")
        );
        assert!(domain.message.contains("hey-boss mm show --bodies none"));
    }
    for errno in [libc::ENOENT, libc::ECONNREFUSED] {
        let source = std::io::Error::from_raw_os_error(errno);
        let expected = format!("Database service unavailable: {source}");
        let error = connection_error(source);
        assert!(!permission_denied(&error));
        assert_eq!(error.to_string(), expected);
    }
}

/// A private transport that disconnects immediately before or after COMMIT.
/// All other requests go through the real database service protocol.
pub(crate) fn lose_commit_response(
    path: &Path,
    commit: bool,
) -> (Connection, std::thread::JoinHandle<()>) {
    let Backend::Remote(remote) = Connection::connect(path).unwrap().backend else {
        unreachable!()
    };
    let mut upstream = remote.stream.into_inner().unwrap();
    let (client, server) = UnixStream::pair().unwrap();
    let thread = std::thread::spawn(move || {
        let mut server = BufReader::new(server);
        while let Some(command) = wire::read::<Command>(&mut server).unwrap() {
            let finishing = matches!(&command, Command::Batch { sql } if sql == "COMMIT");
            if finishing && !commit {
                break;
            }
            wire::write(upstream.get_mut(), &command).unwrap();
            loop {
                let reply = wire::read::<Reply>(&mut upstream).unwrap().unwrap();
                if finishing {
                    assert!(reply.error.is_none());
                    return;
                }
                wire::write(server.get_mut(), &reply).unwrap();
                if !reply.more {
                    break;
                }
            }
        }
    });
    (
        Connection {
            backend: Backend::Remote(Remote {
                path: path.to_owned(),
                stream: RefCell::new(Some(BufReader::new(client))),
                transaction: Cell::new(false),
                last_id: Cell::new(0),
            }),
        },
        thread,
    )
}

struct Fixture {
    directory: std::path::PathBuf,
    owner: Owner,
}
impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "hb-owner-{}-{}",
            std::process::id(),
            crate::issues::worker::random_id().unwrap()
        ));
        std::fs::create_dir(&directory).unwrap();
        let owner = Owner::start(&directory.join("issues.db")).unwrap().unwrap();
        Self { directory, owner }
    }
    fn connect(&self) -> Connection {
        Connection::connect(&self.directory.join("issues.db")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.owner.stop();
        let socket = owner::socket_path(&self.directory.join("issues.db"));
        let _ = std::fs::remove_file(socket.with_extension("lock"));
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Pause the first writer acquisition after real read requests have completed.
pub(crate) fn pause_before_writer(
    path: &Path,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
) -> (Connection, std::thread::JoinHandle<()>) {
    pause_before_command(
        path,
        entered,
        release,
        |command| matches!(command, Command::Batch { sql } if sql == "BEGIN IMMEDIATE"),
    )
}

/// Hold a real service read at a deterministic point inside its transaction.
pub(crate) fn pause_before_query(
    path: &Path,
    sql: &'static str,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
) -> (Connection, std::thread::JoinHandle<()>) {
    pause_before_command(
        path,
        entered,
        release,
        move |command| matches!(command, Command::Query { sql: query, .. } if query == sql),
    )
}

fn pause_before_command(
    path: &Path,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
    should_pause: impl Fn(&Command) -> bool + Send + 'static,
) -> (Connection, std::thread::JoinHandle<()>) {
    let Backend::Remote(remote) = Connection::connect(path).unwrap().backend else {
        unreachable!()
    };
    let mut upstream = remote.stream.into_inner().unwrap();
    let (client, server) = UnixStream::pair().unwrap();
    let transport = std::thread::spawn(move || {
        let mut server = BufReader::new(server);
        let mut paused = false;
        while let Some(command) = wire::read::<Command>(&mut server).unwrap() {
            if !paused && should_pause(&command) {
                entered.send(()).unwrap();
                release.recv().unwrap();
                paused = true;
            }
            wire::write(upstream.get_mut(), &command).unwrap();
            loop {
                let reply = wire::read::<Reply>(&mut upstream).unwrap().unwrap();
                wire::write(server.get_mut(), &reply).unwrap();
                if !reply.more {
                    break;
                }
            }
        }
    });
    (
        Connection {
            backend: Backend::Remote(Remote {
                path: path.to_owned(),
                stream: RefCell::new(Some(BufReader::new(client))),
                transaction: Cell::new(false),
                last_id: Cell::new(0),
            }),
        },
        transport,
    )
}

/// Count real service round trips and SQL work, excluding the connection handshake.
pub(crate) fn measured_connection(
    path: &Path,
) -> (Connection, std::thread::JoinHandle<(usize, i64)>) {
    let Backend::Remote(remote) = Connection::connect(path).unwrap().backend else {
        unreachable!()
    };
    let mut upstream = remote.stream.into_inner().unwrap();
    let (client, server) = UnixStream::pair().unwrap();
    let transport = std::thread::spawn(move || {
        let mut server = BufReader::new(server);
        let mut commands = 0;
        let mut steps = 0;
        while let Some(command) = wire::read::<Command>(&mut server).unwrap() {
            commands += 1;
            wire::write(upstream.get_mut(), &command).unwrap();
            loop {
                let reply = wire::read::<Reply>(&mut upstream).unwrap().unwrap();
                wire::write(server.get_mut(), &reply).unwrap();
                if !reply.more {
                    steps += i64::from(reply.steps);
                    break;
                }
            }
        }
        (commands, steps)
    });
    let connection = Connection {
        backend: Backend::Remote(Remote {
            path: path.to_owned(),
            stream: RefCell::new(Some(BufReader::new(client))),
            transaction: Cell::new(false),
            last_id: Cell::new(0),
        }),
    };
    (connection, transport)
}

#[test]
fn foreign_key_settings_remain_isolated_between_writer_sessions() {
    let fixture = Fixture::new();
    let relaxed = fixture.connect();
    let strict = fixture.connect();
    relaxed.execute_batch("CREATE TABLE parents(id INTEGER PRIMARY KEY); CREATE TABLE children(id INTEGER PRIMARY KEY,parent INTEGER REFERENCES parents(id)); PRAGMA foreign_keys=OFF").unwrap();
    let insert = "INSERT INTO children(id,parent) VALUES(?1,99)";
    assert_eq!(relaxed.execute(insert, [1]).unwrap(), 1);
    assert!(strict.execute(insert, [2]).is_err());
    assert_eq!(relaxed.execute(insert, [3]).unwrap(), 1);
    relaxed.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    assert!(relaxed.execute(insert, [4]).is_err());
    strict
        .execute("INSERT INTO parents VALUES(99)", [])
        .unwrap();
    assert_eq!(strict.execute(insert, [2]).unwrap(), 1);
}

#[test]
fn reused_service_statements_follow_other_sessions_schema_changes_and_recover_from_errors() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    connection.execute_batch("CREATE TABLE changing(id INTEGER PRIMARY KEY,value TEXT UNIQUE); INSERT INTO changing VALUES(1,'first')").unwrap();
    let sql = "SELECT * FROM changing WHERE id=?1";
    assert_eq!(
        connection
            .query_row(sql, [1], |row| row.get::<_, String>("value"))
            .unwrap(),
        "first"
    );
    fixture
        .connect()
        .execute_batch("ALTER TABLE changing ADD COLUMN added INTEGER DEFAULT 42")
        .unwrap();
    assert_eq!(
        connection
            .query_row(sql, [1], |row| row.get::<_, i64>("added"))
            .unwrap(),
        42
    );
    fixture
        .connect()
        .execute_batch("ALTER TABLE changing RENAME COLUMN value TO renamed")
        .unwrap();
    assert_eq!(
        connection
            .query_row(sql, [1], |row| row.get::<_, String>("renamed"))
            .unwrap(),
        "first"
    );
    assert!(connection.query_row(sql, [], |_| Ok(())).is_err());
    let insert = "INSERT INTO changing(id,renamed) VALUES(?1,?2)";
    assert!(
        connection
            .execute(insert, rusqlite::params![1, "duplicate"])
            .is_err()
    );
    assert_eq!(
        connection
            .execute(insert, rusqlite::params![2, "second"])
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(sql, [2], |row| row.get::<_, String>("renamed"))
            .unwrap(),
        "second"
    );
}

#[test]
fn scalar_reads_in_a_mutation_use_one_round_trip_each() {
    let fixture = Fixture::new();
    fixture
        .connect()
        .execute_batch(
            "CREATE TABLE rpc_counter(id INTEGER PRIMARY KEY,value INTEGER);
         WITH RECURSIVE ids(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM ids WHERE id<128)
         INSERT INTO rpc_counter SELECT id,id*2 FROM ids;",
        )
        .unwrap();
    let (connection, transport) = measured_connection(&fixture.directory.join("issues.db"));
    let started = std::time::Instant::now();
    let transaction =
        Transaction::new_unchecked(&connection, TransactionBehavior::Immediate).unwrap();
    let mut total = 0i64;
    for id in 1..=128 {
        total += transaction
            .query_row("SELECT value FROM rpc_counter WHERE id=?1", [id], |row| {
                row.get::<_, i64>("value")
            })
            .unwrap();
    }
    transaction
        .execute("UPDATE rpc_counter SET value=?1 WHERE id=1", [total])
        .unwrap();
    transaction.commit().unwrap();
    let elapsed = started.elapsed();
    drop(connection);
    let (commands, _) = transport.join().unwrap();
    assert_eq!(
        fixture
            .connect()
            .query_row("SELECT value FROM rpc_counter WHERE id=1", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        128 * 129
    );
    eprintln!("128 transactional scalar reads: {commands} RPCs, {elapsed:?} writer transaction");
    assert_eq!(
        commands,
        128 + 3,
        "Scalar reads must not add metadata round trips while holding the writer"
    );
}

#[test]
fn dependent_issue_requests_recover_on_the_same_service_connection() {
    use crate::issues::{Request, Store};
    use serde_json::json;
    let fixture = Fixture::new();
    let path = fixture.directory.join("issues.db");
    let mut store = Store::open(&path).unwrap();
    crate::database::Connection::open(&path).unwrap().execute("INSERT OR IGNORE INTO projects(id,name,next_number) VALUES('named:Recovery','Recovery',1)", []).unwrap();
    store.replace_connection_for_test(fixture.connect());
    let request = |operation, key: &str| -> Request {
        serde_json::from_value(json!({"version":1,
            "project":{"id":"named:Recovery","name":"Recovery"},
            "actor":{"id":"test:recovery","kind":"test","session_id":"recovery","machine":"test","host":"test","cwd":"/tmp","source":"test"},
            "operation":operation,"request_id":key})).unwrap()
    };
    let parent = request(
        json!({"action":"create","title":"Prerequisite","body":"","labels":[]}),
        "parent",
    );
    store.execute(&parent).unwrap();
    let affected = fixture.connect();
    assert!(affected.execute_batch("BEGIN IMMEDIATE; UPDATE projects SET next_number=20; SELECT * FROM missing_recovery_table").is_err());
    store.replace_connection_for_test(affected);
    let child = request(
        json!({"action":"create","title":"Dependent","body":"","labels":[],"blockers":[1]}),
        "dependent",
    );
    let created = store.execute(&child).unwrap();
    assert_eq!(created["issue"]["number"], 2);
    assert_eq!(created["issue"]["state"], "blocked");
    assert_eq!(store.execute(&child).unwrap(), created);
    let invalid = request(
        json!({"action":"create","title":"Invalid","body":"","labels":[],"blockers":[999]}),
        "invalid",
    );
    assert!(store.execute(&invalid).is_err());
    let mut view = request(json!({"action":"view","number":2}), "view");
    view.request_id = None;
    assert_eq!(store.execute(&view).unwrap()["issue"]["title"], "Dependent");
    let later = request(
        json!({"action":"create","title":"Later","body":"","labels":[]}),
        "later",
    );
    assert_eq!(store.execute(&later).unwrap()["issue"]["number"], 3);
}

#[test]
fn creation_receipts_reconcile_lost_commits_and_simultaneous_retries() {
    use crate::issues::{Request, Store};
    use serde_json::json;
    for committed in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.directory.join("issues.db");
        let mut store = Store::open(&path).unwrap();
        crate::database::Connection::open(&path).unwrap().execute("INSERT OR IGNORE INTO projects(id,name,next_number) VALUES('named:Recovery','Recovery',1)", []).unwrap();
        store.replace_connection_for_test(fixture.connect());
        let request = |operation, key: Option<&str>| -> Request {
            serde_json::from_value(json!({"version":1,
                "project":{"id":"named:Recovery","name":"Recovery"},
                "actor":{"id":"test:recovery","kind":"test","session_id":"recovery","machine":"test","host":"test","cwd":"/tmp","source":"test"},
                "operation":operation,"request_id":key})).unwrap()
        };
        let parent = request(
            json!({"action":"create","title":"Prerequisite","body":"","labels":[]}),
            Some("parent"),
        );
        store.execute(&parent).unwrap();
        let child = request(
            json!({"action":"create","title":"Dependent","body":"Original specification","labels":[],"blockers":[1]}),
            Some("ambiguous"),
        );
        let status = request(json!({"action":"request_status","id":"ambiguous"}), None);
        let (connection, transport) = lose_commit_response(&path, committed);
        store.replace_connection_for_test(connection);
        assert!(store.execute(&child).is_err());
        transport.join().unwrap();
        let receipt = store.execute(&status).unwrap();
        assert_eq!(
            receipt["request"]["state"],
            if committed {
                "recorded"
            } else {
                "not_recorded"
            }
        );
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let mut concurrent = Store::open(&path).unwrap();
                concurrent.replace_connection_for_test(fixture.connect());
                let child = child.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    concurrent.execute(&child).unwrap()
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(results.iter().all(|r| r == &results[0]));
        assert_eq!(results[0]["issue"]["number"], 2);
        assert_eq!(results[0]["issue"]["blocker_numbers"], json!([1]));
        assert_eq!(
            store.execute(&status).unwrap()["request"]["response"],
            results[0]
        );
        let mut different = child.clone();
        different.operation = serde_json::from_value(
            json!({"action":"create","title":"Different","body":"","labels":[]}),
        )
        .unwrap();
        assert_eq!(store.execute(&different).unwrap_err().code, "conflict");
        let db = fixture.connect();
        for (table, expected) in [("issues", 2), ("requests", 2)] {
            assert_eq!(
                db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                expected
            );
        }
        let mut other_actor = status.clone();
        other_actor.actor.as_mut().unwrap().id = "another-actor".into();
        assert_eq!(
            store.execute(&other_actor).unwrap()["request"]["state"],
            "not_recorded"
        );
        let mut other_project = status.clone();
        other_project.project.id = "named:Other".into();
        other_project.project.name = "Other".into();
        assert_eq!(
            store.execute(&other_project).unwrap_err().code,
            "project_not_initialized"
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn failed_batch_does_not_leave_an_unowned_transaction() {
    let fixture = Fixture::new();
    for connection in [Connection::open_in_memory().unwrap(), fixture.connect()] {
        connection
            .execute_batch("CREATE TABLE recovery(value INTEGER)")
            .unwrap();
        let error = connection.execute_batch("BEGIN IMMEDIATE; INSERT INTO recovery VALUES(1); SELECT * FROM missing_recovery_table").unwrap_err();
        assert!(error.to_string().contains("missing_recovery_table"));
        // Before the fix, this BEGIN returned SQLite 1: cannot start a
        // transaction within a transaction, masking the original failure.
        let tx = connection.unchecked_transaction().unwrap();
        assert_eq!(
            tx.query_row("SELECT count(*) FROM recovery", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        tx.execute("INSERT INTO recovery VALUES(2)", []).unwrap();
        tx.commit().unwrap();
    }
}

#[test]
fn nested_begin_failure_preserves_the_callers_transaction() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    connection
        .execute_batch("CREATE TABLE recovery(value INTEGER)")
        .unwrap();
    let tx = connection.unchecked_transaction().unwrap();
    tx.execute("INSERT INTO recovery VALUES(1)", []).unwrap();
    assert!(connection.unchecked_transaction().is_err());
    assert!(!connection.is_autocommit());
    assert_eq!(
        tx.query_row("SELECT count(*) FROM recovery", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(tx);
    assert!(connection.is_autocommit());
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM recovery", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn lost_rollback_reply_does_not_poison_the_client_connection() {
    let fixture = Fixture::new();
    let forwarding = fixture.connect();
    forwarding
        .execute_batch("CREATE TABLE recovery(value INTEGER)")
        .unwrap();
    let Backend::Remote(remote) = forwarding.backend else {
        unreachable!()
    };
    let mut upstream = remote.stream.into_inner().unwrap();
    let (client, server) = UnixStream::pair().unwrap();
    let connection = Connection {
        backend: Backend::Remote(Remote {
            path: fixture.directory.join("issues.db"),
            stream: RefCell::new(Some(BufReader::new(client))),
            transaction: Cell::new(false),
            last_id: Cell::new(0),
        }),
    };
    let transport = std::thread::spawn(move || {
        let mut server = BufReader::new(server);
        while let Some(command) = wire::read::<Command>(&mut server).unwrap() {
            let rollback = matches!(&command, Command::Batch { sql } if sql == "ROLLBACK");
            wire::write(upstream.get_mut(), &command).unwrap();
            let reply = wire::read::<Reply>(&mut upstream).unwrap().unwrap();
            if rollback {
                break;
            }
            wire::write(server.get_mut(), &reply).unwrap();
        }
    });
    let tx = connection.unchecked_transaction().unwrap();
    tx.execute("INSERT INTO recovery VALUES(1)", []).unwrap();
    drop(tx);
    transport.join().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM recovery", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let tx = connection.unchecked_transaction().unwrap();
    tx.execute("INSERT INTO recovery VALUES(2)", []).unwrap();
    tx.commit().unwrap();
}

#[test]
fn concurrent_sessions_serialize_transactions_and_keep_reads_responsive() {
    let fixture = Fixture::new();
    let first = fixture.connect();
    first
        .execute_batch("CREATE TABLE counter(value INTEGER); INSERT INTO counter VALUES(0)")
        .unwrap();
    let transaction = first.unchecked_transaction().unwrap();
    transaction
        .execute("UPDATE counter SET value=1", [])
        .unwrap();
    let reader = fixture.connect();
    assert_eq!(
        reader
            .query_row("SELECT value FROM counter", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let second = fixture.connect();
    let (sent, received) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let tx = second.unchecked_transaction().unwrap();
        tx.execute("UPDATE counter SET value=value+1", []).unwrap();
        tx.commit().unwrap();
        sent.send(()).unwrap();
    });
    assert!(received.recv_timeout(Duration::from_millis(100)).is_err());
    transaction.commit().unwrap();
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    writer.join().unwrap();
    assert_eq!(
        reader
            .query_row("SELECT value FROM counter", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn disconnected_session_rolls_back_and_releases_writer() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    connection.execute_batch("CREATE TABLE counter(value INTEGER); INSERT INTO counter VALUES(0); BEGIN IMMEDIATE; UPDATE counter SET value=9").unwrap();
    drop(connection);
    let next = fixture.connect();
    next.execute("UPDATE counter SET value=value+1", [])
        .unwrap();
    assert_eq!(
        next.query_row("SELECT value FROM counter", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn existing_service_takes_ownership_when_the_elected_peer_exits() {
    let mut fixture = Fixture::new();
    let path = fixture.directory.join("issues.db");
    let mut service = Owner::host(&path).unwrap();
    fixture.owner.stop();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(db) = Connection::connect(&path) {
            assert_eq!(
                db.query_row("SELECT 42", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                42
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Existing service did not take ownership"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    service.stop();
    assert!(Connection::connect(&path).is_err());
}

#[test]
fn owner_election_does_not_replace_a_live_socket() {
    let fixture = Fixture::new();
    assert!(
        Owner::start(&fixture.directory.join("issues.db"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .connect()
            .query_row("SELECT 42", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        42
    );
}

#[test]
fn replaced_owner_lock_does_not_split_live_transactions_between_services() {
    let mut fixture = Fixture::new();
    let path = fixture.directory.join("issues.db");
    let socket = owner::socket_path(&path);
    let connection = fixture.connect();
    connection.execute_batch("CREATE TABLE counter(value INTEGER); INSERT INTO counter VALUES(0); BEGIN IMMEDIATE; UPDATE counter SET value=7").unwrap();
    // Model temporary-file cleanup unlinking an old but still locked inode.
    std::fs::remove_file(socket.with_extension("lock")).unwrap();
    let replacement = Owner::start(&path).unwrap();
    assert!(
        replacement.is_none(),
        "A live service must retain its transactions even after its lock pathname is removed"
    );
    connection.execute_batch("COMMIT").unwrap();
    drop(connection);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut next = loop {
        if let Some(owner) = Owner::start(&path).unwrap() {
            break owner;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Displaced owner did not drain"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    fixture.owner.stop();
    assert_eq!(
        Connection::connect(&path)
            .unwrap()
            .query_row("SELECT value FROM counter", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        7
    );
    next.stop();
}

#[test]
fn stopping_displaced_owner_preserves_replacement_socket() {
    let mut fixture = Fixture::new();
    let socket = owner::socket_path(&fixture.directory.join("issues.db"));
    std::fs::remove_file(&socket).unwrap();
    let replacement = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    fixture.owner.stop();
    assert!(
        socket.exists(),
        "Old owner removed a replacement service's socket"
    );
    drop(replacement);
    std::fs::remove_file(socket).unwrap();
}

#[test]
fn first_use_below_a_symlinked_directory_elects_one_canonical_owner() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.directory.join("real")).unwrap();
    std::os::unix::fs::symlink("real", fixture.directory.join("link")).unwrap();
    let path = fixture.directory.join("link/new/deep/issues.db");
    let mut owner = Owner::start(&path).unwrap().unwrap();
    let canonical = path.canonicalize().unwrap();
    assert!(Owner::start(&canonical).unwrap().is_none());
    assert_eq!(
        Connection::connect(&path).unwrap().owner_pid().unwrap(),
        Connection::connect(&canonical)
            .unwrap()
            .owner_pid()
            .unwrap()
    );
    owner.stop();
}

#[test]
fn sqlite_types_returning_and_extended_errors_survive_transport() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    connection
        .execute_batch(
            "CREATE TABLE values_test(id INTEGER PRIMARY KEY, text TEXT UNIQUE, bytes BLOB)",
        )
        .unwrap();
    let id = connection
        .query_row(
            "INSERT INTO values_test(text,bytes) VALUES(?1,?2) RETURNING id",
            rusqlite::params!["héllo", vec![0u8, 255]],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(id, 1);
    assert_eq!(connection.last_insert_rowid(), 1);
    let values = connection
        .query_row("SELECT text,bytes,NULL,3.5 FROM values_test", [], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, f64>(3)?,
            ))
        })
        .unwrap();
    assert_eq!(values, ("héllo".into(), vec![0, 255], None, 3.5));
    assert!(matches!(
        connection.query_row("SELECT id FROM values_test WHERE 0", [], |_| Ok(())),
        Err(rusqlite::Error::QueryReturnedNoRows)
    ));
    assert!(matches!(
        connection.query_row("SELECT id FROM values_test", [], |r| r.get::<_, i64>("missing")),
        Err(rusqlite::Error::InvalidColumnName(name)) if name == "missing"
    ));
    assert!(
        connection
            .query_row("SELECT ?1", [], |r| r.get::<_, i64>(0))
            .is_err()
    );
    let error = connection
        .execute("INSERT INTO values_test(text) VALUES(?1)", ["héllo"])
        .unwrap_err();
    assert!(
        matches!(error, rusqlite::Error::SqliteFailure(code, _) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
    );
}

#[test]
fn prepared_transaction_commands_keep_the_same_session() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    connection
        .execute_batch("CREATE TABLE counter(value INTEGER)")
        .unwrap();
    connection
        .prepare("/* transport */ BEGIN")
        .unwrap()
        .query([])
        .unwrap();
    connection
        .execute("INSERT INTO counter VALUES(7)", [])
        .unwrap();
    connection.prepare("ROLLBACK").unwrap().query([]).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM counter", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn idle_sessions_reconnect_automatically_after_a_service_restart() {
    let mut fixture = Fixture::new();
    let connection = fixture.connect();
    connection
        .execute_batch("CREATE TABLE counter(value INTEGER); INSERT INTO counter VALUES(0)")
        .unwrap();
    fixture.owner.stop();
    fixture.owner = Owner::start(&fixture.directory.join("issues.db"))
        .unwrap()
        .unwrap();
    connection
        .execute("UPDATE counter SET value=value+1", [])
        .unwrap();
    assert_eq!(
        connection
            .query_row("SELECT value FROM counter", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    connection
        .execute("UPDATE counter SET value=value+1", [])
        .unwrap();
    assert_eq!(
        connection
            .query_row("SELECT value FROM counter", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn collected_queries_stop_mapping_on_error_and_retain_column_metadata() {
    let fixture = Fixture::new();
    for db in [Connection::open_in_memory().unwrap(), fixture.connect()] {
        let mut calls = 0;
        let result: rusqlite::Result<Vec<i64>> = db.query_collect(
            "SELECT 7 value UNION ALL SELECT 'invalid' UNION ALL SELECT 9",
            [],
            |row| {
                calls += 1;
                assert_eq!(row.column_count(), 1);
                assert_eq!(row.column_name(0)?, "value");
                assert!(matches!(
                    row.column_name(1),
                    Err(rusqlite::Error::InvalidColumnIndex(1))
                ));
                row.get(0)
            },
        );
        assert_eq!(calls, 2);
        assert!(
            matches!(result, Err(rusqlite::Error::InvalidColumnType(0, name, _)) if name == "value")
        );
        let rows: Vec<i64> = db
            .query_collect("SELECT ?1 AS value", [42], |row| row.get("value"))
            .unwrap();
        assert_eq!(rows, [42]);
    }
}

#[test]
fn read_snapshots_do_not_reserve_the_writer_and_large_results_stream() {
    let fixture = Fixture::new();
    let reader = fixture.connect();
    reader
        .execute_batch(
            "CREATE TABLE documents(body TEXT); INSERT INTO documents VALUES('original')",
        )
        .unwrap();
    let snapshot = reader.read_transaction().unwrap();
    assert_eq!(
        snapshot
            .query_row("SELECT count(*) FROM documents", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let writer = fixture.connect();
    let body = "x".repeat(512 * 1024);
    let tx = writer.unchecked_transaction().unwrap();
    for _ in 0..34 {
        tx.execute("INSERT INTO documents VALUES(?1)", [&body])
            .unwrap();
    }
    tx.commit().unwrap();
    assert_eq!(
        snapshot
            .query_row("SELECT count(*) FROM documents", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(snapshot.execute("DELETE FROM documents", []).is_err());
    snapshot.commit().unwrap();
    let bodies: Vec<String> = reader
        .prepare("SELECT body FROM documents")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(bodies.len(), 35);
    assert!(bodies[1..].iter().all(|v| v == &body));
}

#[test]
fn committed_write_with_a_lost_response_is_not_replayed() {
    let fixture = Fixture::new();
    let inspector = fixture.connect();
    inspector
        .execute_batch("CREATE TABLE counter(value INTEGER); INSERT INTO counter VALUES(0)")
        .unwrap();
    let forwarding = fixture.connect();
    let (client, server) = UnixStream::pair().unwrap();
    let connection = Connection {
        backend: Backend::Remote(Remote {
            path: fixture.directory.join("issues.db"),
            stream: RefCell::new(Some(BufReader::new(client))),
            transaction: Cell::new(false),
            last_id: Cell::new(0),
        }),
    };
    let server = std::thread::spawn(move || {
        let mut server = BufReader::new(server);
        let Some(Command::Execute { sql, values }) = wire::read(&mut server).unwrap() else {
            panic!("Expected one mutation")
        };
        forwarding.execute(&sql, params_from_iter(values)).unwrap();
        // The mutation commits, but its response never reaches the caller.
    });
    assert!(
        connection
            .execute("UPDATE counter SET value=value+1", [])
            .is_err()
    );
    server.join().unwrap();
    assert_eq!(
        inspector
            .query_row("SELECT value FROM counter", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn graceful_service_shutdown_finishes_an_active_transaction() {
    let mut fixture = Fixture::new();
    let connection = fixture.connect();
    connection
        .execute_batch("CREATE TABLE counter(value INTEGER)")
        .unwrap();
    let tx = connection.unchecked_transaction().unwrap();
    tx.execute("INSERT INTO counter VALUES(7)", []).unwrap();
    let path = fixture.directory.join("issues.db");
    std::thread::scope(|scope| {
        let owner = &mut fixture.owner;
        let (finished, wait) = mpsc::channel();
        scope.spawn(move || {
            owner.stop();
            finished.send(()).unwrap();
        });
        // Shutdown retains election ownership until the transaction commits.
        assert!(wait.recv_timeout(Duration::from_millis(100)).is_err());
        assert!(Owner::start(&path).unwrap().is_none());
        tx.commit().unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    fixture.owner = Owner::start(&path).unwrap().unwrap();
    assert_eq!(
        fixture
            .connect()
            .query_row("SELECT value FROM counter", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        7
    );
}
