use hey_boss::issues::Store;
use rusqlite::{Connection, ErrorCode};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{PermissionsExt, symlink};
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
