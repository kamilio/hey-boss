//! Exercise the installed command path with an existing companion service.
use hey_boss::database::Connection;
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn database_inspection_reads_finish_while_another_session_holds_the_writer() {
    let root = std::env::temp_dir().join(format!("hb-maintenance-reader-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let fixture = Fixture {
        root,
        service: None,
        replacement: None,
    };
    let path = fixture.root.join("issues.db");
    let mut owner = hey_boss::database::Owner::start(&path).unwrap().unwrap();
    let mut connection = fixture.connection();
    connection
        .execute_batch("CREATE TABLE inspection(value INTEGER); INSERT INTO inspection VALUES(1)")
        .unwrap();
    let transaction = connection
        .transaction_with_behavior(hey_boss::database::TransactionBehavior::Immediate)
        .unwrap();
    transaction
        .execute("UPDATE inspection SET value=2", [])
        .unwrap();
    let mut child = fixture
        .command(&["fleet", "database", "--path", path.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"sql\":\"SELECT value FROM inspection\",\"args\":[]}\n")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let finished = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    transaction.commit().unwrap();
    let output = child.wait_with_output().unwrap();
    owner.stop();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        finished,
        "Read-only inspection queued behind an unrelated writer"
    );
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["rows"], serde_json::json!([[1]]));
}

#[test]
fn standalone_database_driver_keeps_constraint_and_durability_defaults() {
    let root = std::env::temp_dir().join(format!("hb-maintenance-defaults-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let fixture = Fixture {
        root,
        service: None,
        replacement: None,
    };
    let path = fixture.root.join("private.db");
    let mut child = fixture
        .command(&["fleet", "database", "--path", path.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for sql in [
        "PRAGMA foreign_keys",
        "PRAGMA synchronous",
        "CREATE TABLE parent(id INTEGER PRIMARY KEY)",
        "CREATE TABLE child(parent INTEGER REFERENCES parent(id))",
        "INSERT INTO child VALUES(99)",
        "SELECT count(*) FROM sqlite_master WHERE name='issues'",
    ] {
        writeln!(input, "{}", serde_json::json!({"sql":sql,"args":[]})).unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replies: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies[0]["rows"], serde_json::json!([[1]]));
    assert_eq!(replies[1]["rows"], serde_json::json!([[2]]));
    assert_eq!(replies[4]["constraint"], true);
    assert_eq!(replies[5]["rows"], serde_json::json!([[0]]));
}

struct Fixture {
    root: PathBuf,
    service: Option<Child>,
    replacement: Option<u32>,
}

#[test]
fn writer_wait_reports_the_holder_without_sql_values_and_recovers() {
    let root = std::env::temp_dir().join(format!("hb-writer-evidence-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut fixture = Fixture {
        root,
        service: None,
        replacement: None,
    };
    use sha2::{Digest, Sha256};
    let database = fixture.root.canonicalize().unwrap().join("issues.db");
    let identity = format!(
        "{:x}",
        Sha256::digest(database.as_os_str().as_encoded_bytes())
    );
    let log = PathBuf::from("/tmp")
        .join(format!("hey-boss-db-{}", unsafe { libc::getuid() }))
        .join(format!("{}.diagnostics.jsonl", &identity[..24]));
    fixture.service = Some(
        fixture
            .command(&["fleet", "companion"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Service journals may be inaccessible to the logged-in user.
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let holder = fixture.connection();
    holder
        .execute_batch("CREATE TABLE contention(value TEXT)")
        .unwrap();
    let owner = holder.owner_pid().unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    holder
        .execute("INSERT INTO contention VALUES('private literal')", [])
        .unwrap();
    let waiting = fixture.connection();
    let write = std::thread::spawn(move || {
        waiting.execute("INSERT INTO contention VALUES(?1)", ["private binding"])
    });
    let deadline = Instant::now() + Duration::from_secs(12);
    let evidence = loop {
        let log = fs::read_to_string(&log).unwrap_or_default();
        if let Some(record) = log
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|record| {
                record["event"] == "database_writer_wait"
                    && record["sql"]
                        .as_str()
                        .is_some_and(|sql| sql.contains("contention"))
            })
        {
            break Some(record);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    holder.execute_batch("COMMIT").unwrap();
    assert_eq!(write.join().unwrap().unwrap(), 1);
    assert_eq!(holder.owner_pid().unwrap(), owner);
    assert_eq!(
        holder
            .query_row("SELECT count(*) FROM contention", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    let _ = fs::remove_file(&log);
    let evidence = evidence.expect("No user-readable writer contention evidence was recorded");
    assert!(evidence["at_ms"].as_u64().unwrap() > 0);
    assert_eq!(evidence["pid"], owner);
    assert_eq!(evidence["holder"]["operation"], "execute");
    assert_eq!(evidence["holder"]["executing"], false);
    assert!(evidence["holder"]["held_ms"].as_u64().unwrap() >= 5000);
    assert!(!evidence.to_string().contains("private"));
}

#[test]
fn queued_transaction_gets_its_idle_budget_after_the_writer_replies() {
    let root = std::env::temp_dir().join(format!("hb-writer-idle-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let fixture = Fixture {
        root,
        service: None,
        replacement: None,
    };
    drop(hey_boss::issues::Store::open(&fixture.root.join("issues.db")).unwrap());
    {
        let db = rusqlite::Connection::open(fixture.root.join("issues.db")).unwrap();
        db.execute_batch("CREATE TABLE contention(value INTEGER)")
            .unwrap();
    }
    let mut owner = hey_boss::database::Owner::start(&fixture.root.join("issues.db"))
        .unwrap()
        .unwrap();
    let holder = fixture.connection();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let waiting = fixture.connection();
    let (started, received) = std::sync::mpsc::channel();
    let transaction = std::thread::spawn(move || -> rusqlite::Result<()> {
        started.send(()).unwrap();
        waiting.execute_batch("BEGIN IMMEDIATE")?;
        waiting.execute("INSERT INTO contention VALUES(42)", [])?;
        waiting.execute_batch("COMMIT")
    });
    received.recv().unwrap();
    // Cross the 30-second idle limit while retaining an actively used writer.
    // The queued BEGIN is still within the separate 45-second acquisition limit.
    let deadline = Instant::now() + Duration::from_secs(32);
    while Instant::now() < deadline {
        holder
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .unwrap();
        std::thread::sleep(Duration::from_millis(250));
    }
    holder.execute_batch("COMMIT").unwrap();
    let result = transaction.join().unwrap();
    eprintln!("Queued transaction result: {result:?}");
    let values = holder
        .query_collect("SELECT value FROM contention", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    drop(holder);
    owner.stop();
    result.expect("The owner expired a transaction while it was waiting for the writer");
    assert_eq!(
        values,
        vec![42],
        "The queued transaction must commit exactly once"
    );
}
impl Fixture {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        command
            .args(args)
            .current_dir(&self.root)
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", self.root.join("fleet"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_FLEET_SUPERVISED");
        command
    }
    fn connection(&self) -> Connection {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(connection) = Connection::connect(&self.root.join("issues.db")) {
                return connection;
            }
            assert!(Instant::now() < deadline, "database owner did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn initialize_project(&self) {
        let output = self
            .command(&[
                "project",
                "init",
                "--project",
                "Database owner",
                "--prs",
                "false",
                "--worktree",
                "false",
                "--yes",
                "--json",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn create(&self, title: &str) {
        let output = self
            .command(&[
                "issue",
                "--project",
                "Database owner",
                "--agent",
                "test:owner",
                "--json",
                "create",
                "--title",
                title,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["issue"]["title"], title);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut service) = self.service.take() {
            unsafe {
                libc::kill(service.id() as i32, libc::SIGTERM);
            }
            let _ = service.wait();
        }
        if let Some(pid) = self.replacement {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
#[cfg(target_os = "macos")]
fn sandbox_denial_does_not_bootstrap_another_service() {
    use sha2::{Digest, Sha256};
    let root = std::env::temp_dir().join(format!("hb-denied-process-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut fixture = Fixture {
        root,
        service: None,
        replacement: None,
    };
    fixture.service = Some(
        fixture
            .command(&["fleet", "companion"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let owner_pid = fixture.connection().owner_pid().unwrap();
    fixture.initialize_project();
    fixture.create("Sandbox read test");
    let path = fixture.root.join("issues.db").canonicalize().unwrap();
    let identity = format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()));
    let startup = PathBuf::from(format!(
        "/tmp/hey-boss-db-{}/{}.startup",
        unsafe { libc::getuid() },
        &identity[..24]
    ));
    for args in [
        vec!["issue", "list", "--project", "Database owner", "--all"],
        vec![
            "mm",
            "show",
            "--project",
            "Database owner",
            "--bodies",
            "none",
        ],
    ] {
        for json in [false, true] {
            let mut args = args.clone();
            if json {
                args.push("--json");
            }
            let command = fixture.command(&args);
            for transport in ["direct", "pipe", "redirect"] {
                for denied in [false, true] {
                    let mut invocation = if denied {
                        let mut sandbox = Command::new("/usr/bin/sandbox-exec");
                        sandbox.args(["-p", "(version 1)(allow default)(deny network-outbound)"]);
                        sandbox
                    } else {
                        Command::new("/usr/bin/env")
                    };
                    if transport == "pipe" {
                        // Positional arguments preserve quoting; pipefail checks
                        // the CLI's status rather than the successful consumer.
                        invocation.args([
                            "/bin/bash",
                            "-o",
                            "pipefail",
                            "-c",
                            "\"$@\" | cat",
                            "--",
                        ]);
                    } else if transport == "redirect" {
                        invocation.args([
                            "/bin/bash",
                            "-c",
                            "\"$@\" > read-output; status=$?; cat read-output; exit \"$status\"",
                            "--",
                        ]);
                    }
                    invocation
                        .arg(command.get_program())
                        .args(command.get_args())
                        .current_dir(&fixture.root);
                    for (name, value) in command.get_envs() {
                        if let Some(value) = value {
                            invocation.env(name, value);
                        } else {
                            invocation.env_remove(name);
                        }
                    }
                    let output = invocation.output().unwrap();
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    assert_eq!(
                        output.status.code(),
                        Some(if denied { 1 } else { 0 }),
                        "{args:?}, transport={transport}, denied={denied}: {stdout}{stderr}"
                    );
                    if denied {
                        let message = if json {
                            assert!(stderr.is_empty(), "{stderr}");
                            let error: Value = serde_json::from_slice(&output.stdout).unwrap();
                            assert_eq!(error["ok"], false);
                            assert_eq!(error["error"]["code"], "database_error");
                            error["error"]["message"].as_str().unwrap().to_owned()
                        } else {
                            assert!(stdout.is_empty(), "{stdout}");
                            stderr.into_owned()
                        };
                        assert!(
                            message.contains("Database service access denied"),
                            "{message}"
                        );
                        assert!(
                            message.contains("Operation not permitted")
                                || message.contains("Permission denied"),
                            "{message}"
                        );
                        assert!(
                            message.contains("without pipes or redirection"),
                            "{message}"
                        );
                        assert!(message.contains("approval"), "{message}");
                        assert!(message.contains("mm show --bodies none"), "{message}");
                    } else if json {
                        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
                        assert_eq!(value["ok"], true, "{value}");
                    } else {
                        assert!(stdout.contains("Database owner"), "{stdout}");
                    }
                }
            }
        }
    }
    assert!(
        !startup.exists(),
        "Denied clients must not try to start another service"
    );
    assert_eq!(fixture.connection().owner_pid().unwrap(), owner_pid);
}

#[test]
fn standalone_databases_in_one_directory_have_independent_services() {
    let root = std::env::temp_dir().join(format!("hb-multiple-process-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut fixture = Fixture {
        root,
        service: None,
        replacement: None,
    };
    fixture.initialize_project();
    fixture.create("First database");
    fixture.replacement = Some(fixture.connection().owner_pid().unwrap());
    let second = fixture.root.join("second.db");
    let output = fixture
        .command(&["issue", "--json", "projects"])
        .env("HEY_BOSS_ISSUE_DB", &second)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let connection = Connection::connect(&second).unwrap();
    let pid = connection.owner_pid().unwrap();
    assert_ne!(Some(pid), fixture.replacement);
    drop(connection);
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

#[test]
fn staged_migration_does_not_leave_a_temporary_service_running() {
    let root = std::env::temp_dir().join(format!("hb-migrate-process-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut fixture = Fixture {
        root,
        service: None,
        replacement: None,
    };
    let output = fixture
        .command(&[
            "issue",
            "migrate",
            "--installation",
            env!("CARGO_BIN_EXE_hey-boss"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if let Ok(connection) = Connection::connect(&fixture.root.join("issues.db")) {
        fixture.replacement = Some(connection.owner_pid().unwrap());
    }
    assert!(
        fixture.replacement.is_none(),
        "Staged installer left a companion running"
    );
    let db = rusqlite::Connection::open(fixture.root.join("issues.db")).unwrap();
    assert!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap()
            > 0
    );
}

#[test]
fn existing_service_owns_cli_writes_and_missing_service_recovers_automatically() {
    let root = std::env::temp_dir().join(format!("hb-owner-process-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut fixture = Fixture {
        root,
        service: None,
        replacement: None,
    };
    fixture.service = Some(
        fixture
            .command(&["fleet", "companion"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let connection = fixture.connection();
    let pid = fixture.service.as_ref().unwrap().id();
    assert_eq!(connection.owner_pid().unwrap(), pid);
    fixture.initialize_project();
    fixture.create("Before restart");
    assert_eq!(
        connection.owner_pid().unwrap(),
        pid,
        "CLI started another owner despite the existing service"
    );
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    assert!(fixture.service.take().unwrap().wait().unwrap().success());
    fixture.create("After restart");
    let next = fixture.connection();
    let replacement = next.owner_pid().unwrap();
    fixture.replacement = Some(replacement);
    assert_ne!(replacement, pid);
    let count = next
        .query_row("SELECT count(*) FROM issues", [], |r| r.get::<_, i64>(0))
        .unwrap();
    assert_eq!(count, 2);
}
