use super::*;
use std::{sync::mpsc, time::Duration};

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
        let _ = std::fs::remove_dir_all(&self.directory);
    }
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
        scope.spawn(move || owner.stop());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(owner::socket_path(&path)).is_ok() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        tx.commit().unwrap();
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
