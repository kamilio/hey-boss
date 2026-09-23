use hey_boss::issues::Store;
use rusqlite::{Connection, ErrorCode};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::{
    fs,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[test]
fn journal_mode_probe() {
    let Some(path) = std::env::var_os("HEY_BOSS_LOCK_PROBE_DB") else {
        return;
    };
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) };
    assert_eq!(
        result, -1,
        "SQLite's database lock was released while the Store is still open"
    );
    assert!(matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EACCES | libc::EAGAIN)
    ));
    drop(file);
    let db = Connection::open(path).unwrap();
    db.busy_timeout(Duration::from_millis(100)).unwrap();
    let result = db.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get::<_, String>(0));
    assert!(
        matches!(result, Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == ErrorCode::DatabaseBusy),
        "A separate process must not change journaling while a Store is open: {result:?}"
    );
}

#[test]
fn opening_another_store_preserves_database_os_locks() {
    let root = temporary_directory();
    let path = root.join("issues.db");
    let store = Store::open(&path).unwrap();
    drop(Store::open(&path).unwrap());
    let probe = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "journal_mode_probe", "--nocapture"])
        .env("HEY_BOSS_LOCK_PROBE_DB", &path)
        .output()
        .unwrap();
    assert!(
        probe.status.success(),
        "{}{}",
        String::from_utf8_lossy(&probe.stdout),
        String::from_utf8_lossy(&probe.stderr)
    );
    drop(store);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn opening_hard_linked_database_names_cannot_split_wal_state() {
    let root = temporary_directory();
    let path = root.join("issues.db");
    let store = Store::open(&path).unwrap();
    let alias = root.join("second.db");
    fs::hard_link(&path, &alias).unwrap();
    let opened = Store::open(&alias);
    assert!(
        opened.is_err(),
        "A hard-linked database name was accepted with a distinct WAL filename"
    );
    let error = opened.err().unwrap();
    assert!(error.to_string().contains("hard link"), "{error}");
    let error = Store::open(&path)
        .err()
        .expect("Original name must also reject multiple links");
    assert!(error.to_string().contains("hard link"), "{error}");
    assert!(!root.join("second.db-wal").exists());
    assert!(!root.join("second.db-shm").exists());
    assert_database_locked(&path);
    fs::remove_file(alias).unwrap();
    drop(store);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn opening_a_hard_linked_sidecar_cannot_resize_an_unrelated_file() {
    let root = temporary_directory();
    let path = root.join("issues.db");
    drop(Store::open(&path).unwrap());
    let unrelated = root.join("unrelated.data");
    let contents = vec![b'x'; 4096];
    fs::write(&unrelated, &contents).unwrap();
    for suffix in ["-shm", "-wal", "-journal"] {
        for symbolic in [false, true] {
            let sidecar = root.join(format!("issues.db{suffix}"));
            assert!(!sidecar.exists());
            if symbolic {
                symlink(&unrelated, &sidecar).unwrap();
            } else {
                fs::hard_link(&unrelated, &sidecar).unwrap();
            }
            let opened = Store::open(&path);
            assert_eq!(
                fs::metadata(&unrelated).unwrap().len(),
                contents.len() as u64,
                "SQLite resized an unrelated file through its {suffix} alias"
            );
            assert!(opened.is_err(), "SQLite accepted a {suffix} alias");
            let error = opened.err().unwrap();
            assert!(error.to_string().contains("sidecar"), "{error}");
            assert_eq!(fs::read(&unrelated).unwrap(), contents);
            fs::remove_file(sidecar).unwrap();
        }
    }
    let store = Store::open(&path).unwrap();
    assert_database_locked(&path);
    drop(store);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bundled_database_driver_rejects_database_name_aliases() {
    let root = temporary_directory();
    let path = root.join("issues.db");
    let store = Store::open(&path).unwrap();
    let alias = root.join("second.db");
    fs::hard_link(&path, &alias).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args(["fleet", "database", "--path"])
        .arg(&alias)
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "Database driver accepted main alias"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("hard link"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!root.join("second.db-wal").exists());
    assert!(!root.join("second.db-shm").exists());
    assert_database_locked(&path);
    fs::remove_file(alias).unwrap();
    drop(store);
    let unrelated = root.join("unrelated.data");
    let contents = vec![b'x'; 4096];
    fs::write(&unrelated, &contents).unwrap();
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = root.join(format!("issues.db{suffix}"));
        fs::hard_link(&unrelated, &sidecar).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(["fleet", "database", "--path"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(!result.status.success(), "Driver accepted {suffix} alias");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("sidecar"),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(fs::read(&unrelated).unwrap(), contents);
        fs::remove_file(sidecar).unwrap();
    }
    let empty = root.join("empty.db");
    let result = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args(["fleet", "database", "--path"])
        .arg(&empty)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let metadata = fs::metadata(empty).unwrap();
    assert_eq!(metadata.nlink(), 1);
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reading_a_plan_alias_of_the_live_database_preserves_its_locks() {
    let root = temporary_directory();
    let path = root.join("issues.db");
    let mut store = Store::open(&path).unwrap();
    let request = serde_json::from_value(serde_json::json!({
        "version":1,"project":{"id":"named:Locktest","name":"Locktest"},
        "operation":{"action":"read_plan","plan":{
            "path":"plan.md","checkout":root,
            "machine":hey_boss::issues::identity::machine().unwrap(),"host":"local"
        }}
    }))
    .unwrap();
    for database_suffix in ["", "-wal", "-shm"] {
        let database_file = root.join(format!("issues.db{database_suffix}"));
        for plan_extension in ["md", "hey-boss-sync-lock", "hey-boss-sync-paused"] {
            let alias = root.join(format!("plan.{plan_extension}"));
            for symbolic in [false, true] {
                fs::write(root.join("plan.md"), "# Safe plan\n\nBody\n").unwrap();
                if alias.exists() {
                    fs::remove_file(&alias).unwrap();
                }
                if symbolic {
                    symlink(&database_file, &alias).unwrap();
                } else {
                    fs::hard_link(&database_file, &alias).unwrap();
                }
                let error = store.execute(&request).unwrap_err();
                assert!(error.to_string().contains("must not alias"), "{error}");
                assert_database_locked(&path);
                fs::remove_file(alias.clone()).unwrap();
            }
        }
    }
    fs::write(root.join("plan.md"), "# Safe plan\n\nBody\n").unwrap();
    let response = store.execute(&request).unwrap();
    assert_eq!(response["title"], "Safe plan");
    assert_eq!(response["body"], "Body");
    assert_database_locked(&path);
    drop(store);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

fn assert_database_locked(path: &std::path::Path) {
    let probe = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "journal_mode_probe", "--nocapture"])
        .env("HEY_BOSS_LOCK_PROBE_DB", path)
        .output()
        .unwrap();
    assert!(
        probe.status.success(),
        "{}{}",
        String::from_utf8_lossy(&probe.stdout),
        String::from_utf8_lossy(&probe.stderr)
    );
}

#[test]
fn downloading_an_attachment_alias_preserves_database_locks() {
    let root = temporary_directory();
    let path = root.join("issues.db");
    let mut store = Store::open(&path).unwrap();
    let request = |operation| {
        serde_json::from_value(serde_json::json!({
        "version":1,"project":{"id":"named:Locktest","name":"Locktest"},
        "actor":{"id":"human:boss","kind":"human","machine":"test","host":"test","cwd":"/tmp","source":"test"},
        "operation":operation
    })).unwrap()
    };
    store
        .execute(&request(
            serde_json::json!({"action":"create","title":"Files","body":"","labels":[]}),
        ))
        .unwrap();
    let uploaded = store
        .execute(&request(serde_json::json!({
            "action":"attachment","operation":{"command":"upload",
            "target":{"kind":"issue","id":"1"},"name":"file.txt","data":"aGVsbG8="}
        })))
        .unwrap();
    let id = uploaded["attachment"]["id"].as_str().unwrap();
    let attachment = root.join("issues.attachments").join(id);
    fs::remove_file(&attachment).unwrap();
    let download = request(
        serde_json::json!({"action":"attachment","operation":{"command":"download","id":id}}),
    );
    for suffix in ["", "-wal", "-shm"] {
        let target = root.join(format!("issues.db{suffix}"));
        for symbolic in [false, true] {
            if symbolic {
                symlink(&target, &attachment).unwrap();
            } else {
                fs::hard_link(&target, &attachment).unwrap();
            }
            let error = store.execute(&download).unwrap_err();
            assert_database_locked(&path);
            assert!(error.to_string().contains("must not alias"), "{error}");
            fs::remove_file(&attachment).unwrap();
        }
    }
    fs::write(&attachment, "hello").unwrap();
    assert_eq!(store.execute(&download).unwrap()["data"], "aGVsbG8=");
    assert_database_locked(&path);
    drop(store);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

fn temporary_directory() -> std::path::PathBuf {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "hb-database-locks-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    root
}

#[test]
fn concurrent_creation_is_private_and_preserves_existing_files() {
    let root = temporary_directory();
    let path = root.join("issues.db");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                Store::open(&path)
            })
        })
        .collect();
    let stores: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().unwrap().unwrap())
        .collect();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::metadata(&path).unwrap().nlink(), 1);
    for suffix in ["-wal", "-shm"] {
        assert_eq!(
            fs::metadata(root.join(format!("issues.db{suffix}")))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(
        !fs::read_dir(&root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("create-"))
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    drop(Store::open(&path).unwrap());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let alias = root.join("linked.db");
    symlink(&path, &alias).unwrap();
    assert!(Store::open(&alias).is_err());
    assert!(Store::open(&root).is_err());
    drop(stores);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
