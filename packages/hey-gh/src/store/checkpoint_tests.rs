//! Stall actual checkpoint I/O, not the caller's report future.
use super::*;
use rusqlite::ffi;
use std::{
    collections::HashMap,
    ffi::{CString, c_char, c_int},
    sync::{
        Condvar, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

struct Gate {
    armed: AtomicBool,
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    released: Mutex<bool>,
    wake: Condvar,
}

impl Gate {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

struct Release(Arc<Gate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct HookedFile {
    original: usize,
    _methods: Box<ffi::sqlite3_io_methods>,
    main: bool,
}

static PARENT: AtomicUsize = AtomicUsize::new(0);
static FILES: OnceLock<Mutex<HashMap<usize, HookedFile>>> = OnceLock::new();
static GATE: OnceLock<Arc<Gate>> = OnceLock::new();

unsafe extern "C" fn sync(file: *mut ffi::sqlite3_file, flags: c_int) -> c_int {
    let (original, main) = {
        let files = FILES.get().unwrap().lock().unwrap();
        let entry = files.get(&(file as usize)).unwrap();
        // The original method table belongs to SQLite's built-in VFS.
        let original = unsafe { &*(entry.original as *const ffi::sqlite3_io_methods) };
        (original.xSync.unwrap(), entry.main)
    };
    let gate = GATE.get().unwrap();
    if main && gate.armed.swap(false, Ordering::SeqCst) {
        if let Some(entered) = gate.entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
        // Release on failure as well: an assertion must not strand a SQLite
        // worker. WAL-file sync never enters this gate.
        let _ = gate
            .wake
            .wait_timeout_while(
                gate.released.lock().unwrap(),
                Duration::from_secs(10),
                |released| !*released,
            )
            .unwrap();
    }
    unsafe { original(file, flags) }
}

unsafe extern "C" fn close(file: *mut ffi::sqlite3_file) -> c_int {
    let entry = FILES
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .remove(&(file as usize))
        .unwrap();
    let original = entry.original as *const ffi::sqlite3_io_methods;
    // Restore the underlying table before calling its close implementation.
    // The owned replacement table remains alive through that call.
    unsafe {
        (*file).pMethods = original;
        ((*original).xClose.unwrap())(file)
    }
}

unsafe extern "C" fn open(
    _vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    out: *mut c_int,
) -> c_int {
    let parent = PARENT.load(Ordering::SeqCst) as *mut ffi::sqlite3_vfs;
    let result = unsafe { ((*parent).xOpen.unwrap())(parent, name, file, flags, out) };
    if result != ffi::SQLITE_OK {
        return result;
    }
    // Retain the real file layout and every other original method. Only sync
    // and close are wrapped, so WAL locking/recovery still uses the real VFS.
    let original = unsafe { (*file).pMethods };
    let mut methods = Box::new(unsafe { std::ptr::read(original) });
    methods.xSync = Some(sync);
    methods.xClose = Some(close);
    let pointer = &*methods as *const _;
    FILES.get().unwrap().lock().unwrap().insert(
        file as usize,
        HookedFile {
            original: original as usize,
            _methods: methods,
            main: flags & ffi::SQLITE_OPEN_MAIN_DB != 0,
        },
    );
    unsafe {
        (*file).pMethods = pointer;
    }
    result
}

fn install_sync_gate(gate: Arc<Gate>) {
    FILES.set(Mutex::new(HashMap::new())).ok().unwrap();
    GATE.set(gate).ok().unwrap();
    // This fixture runs in an isolated child process. Its VFS registration
    // deliberately lives until that process exits, including background closes.
    unsafe {
        let parent = ffi::sqlite3_vfs_find(std::ptr::null());
        assert!(!parent.is_null());
        PARENT.store(parent as usize, Ordering::SeqCst);
        let mut vfs = Box::new(std::ptr::read(parent));
        vfs.zName = CString::new("hey-gh-checkpoint-test").unwrap().into_raw();
        vfs.pNext = std::ptr::null_mut();
        vfs.xOpen = Some(open);
        assert_eq!(
            ffi::sqlite3_vfs_register(Box::into_raw(vfs), 1),
            ffi::SQLITE_OK
        );
    }
}

#[test]
fn stalled_checkpoint_sync_does_not_hold_the_cache_writer() {
    const CHILD: &str = "HEY_GH_CHECKPOINT_IO_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "store::checkpoint_tests::stalled_checkpoint_sync_does_not_hold_the_cache_writer",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(2)
        .build()
        .unwrap()
        .block_on(stalled_checkpoint());
}

async fn stalled_checkpoint() {
    let (entered, ready) = tokio::sync::oneshot::channel();
    let gate = Arc::new(Gate {
        armed: AtomicBool::new(false),
        entered: Mutex::new(Some(entered)),
        released: Mutex::new(false),
        wake: Condvar::new(),
    });
    let release = Release(gate.clone());
    install_sync_gate(gate.clone());
    let dir = tempfile::tempdir().unwrap();
    let recovery_path = std::env::var_os("HEY_GH_CHECKPOINT_RECOVERY_PATH");
    let path = recovery_path
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dir.path().join("cache.sqlite"));
    let store = Store::open(&path, Duration::from_secs(3600), 100, 16 * 1024 * 1024).unwrap();
    let response = |data| Response {
        data,
        fetched_at_ms: 1,
        validated_at_ms: 1,
        source: Source::Network,
        etag: None,
        last_modified: None,
        link: None,
    };
    store
        .put("scope", "warm", &response(serde_json::json!("warm")))
        .await
        .unwrap();
    store
        .run(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .map_err(storage)
        })
        .await
        .unwrap();
    gate.armed.store(true, Ordering::SeqCst);
    let large = response(serde_json::json!({"body":"x".repeat(5 * 1024 * 1024)}));
    let mut first = tokio::spawn({
        let store = store.clone();
        async move { store.put("scope", "large", &large).await }
    });
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .expect("write never reached a real checkpoint sync")
        .unwrap();
    let completed = tokio::time::timeout(Duration::from_millis(500), &mut first).await;
    let peer = tokio::time::timeout(
        Duration::from_millis(500),
        store.put("scope", "peer", &response(serde_json::json!("durable"))),
    )
    .await;
    let read = tokio::time::timeout(Duration::from_millis(500), store.get("scope", "warm")).await;
    // One blocking thread is occupied by actual checkpoint sync. Repeated
    // requests must coalesce rather than consume the thread needed by reads
    // and the FIFO writer.
    let peers = tokio::time::timeout(Duration::from_secs(2), async {
        for index in 0..32 {
            store
                .put(
                    "scope",
                    &format!("peer-{index}"),
                    &response(serde_json::json!(index)),
                )
                .await?;
            assert_eq!(
                store.get("scope", "warm").await?.unwrap().data,
                serde_json::json!("warm")
            );
        }
        Ok::<_, crate::Error>(())
    })
    .await;
    if recovery_path.is_some()
        && matches!(completed, Ok(Ok(Ok(()))))
        && matches!(peer, Ok(Ok(())))
        && matches!(peers, Ok(Ok(())))
    {
        let synchronous: i64 = store
            .run(|conn| {
                conn.query_row("PRAGMA synchronous", [], |row| row.get(0))
                    .map_err(storage)
            })
            .await
            .unwrap();
        assert_eq!(synchronous, 2, "WAL commits must retain FULL durability");
        assert!(!*gate.released.lock().unwrap());
        // Exit without closing connections or releasing checkpoint sync. The
        // parent must recover acknowledged commits from the real WAL.
        std::process::exit(0);
    }
    drop(release);
    let completed_before_checkpoint = completed.is_ok();
    match completed {
        Ok(result) => result.unwrap().unwrap(),
        Err(_) => first.await.unwrap().unwrap(),
    }
    store.run(|_| Ok(())).await.unwrap();
    assert!(
        completed_before_checkpoint,
        "checkpoint I/O held the initiating write after its WAL commit"
    );
    peer.expect("checkpoint I/O held the next cache writer")
        .unwrap();
    peers
        .expect("checkpoint requests exhausted the blocking pool")
        .unwrap();
    assert_eq!(
        read.expect("checkpoint blocked an independent cached read")
            .unwrap()
            .unwrap()
            .data,
        serde_json::json!("warm")
    );
    assert_eq!(
        store.get("scope", "peer").await.unwrap().unwrap().data,
        serde_json::json!("durable")
    );
    let synchronous: i64 = store
        .run(|conn| {
            conn.query_row("PRAGMA synchronous", [], |row| row.get(0))
                .map_err(storage)
        })
        .await
        .unwrap();
    assert_eq!(synchronous, 2, "WAL commits must retain FULL durability");
    assert!(
        recovery_path.is_none(),
        "recovery fixture did not exit while checkpoint was stalled"
    );
}

#[tokio::test]
async fn committed_writes_recover_after_exit_during_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "store::checkpoint_tests::stalled_checkpoint_sync_does_not_hold_the_cache_writer",
            "--test-threads=1",
        ])
        .env("HEY_GH_CHECKPOINT_IO_CHILD", "1")
        .env("HEY_GH_CHECKPOINT_RECOVERY_PATH", &path)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dir.path()
            .join("cache.sqlite-wal")
            .metadata()
            .unwrap()
            .len()
            > 0
    );
    let store = Store::open(&path, Duration::from_secs(3600), 100, 16 * 1024 * 1024).unwrap();
    assert_eq!(
        store.get("scope", "large").await.unwrap().unwrap().data["body"]
            .as_str()
            .unwrap()
            .len(),
        5 * 1024 * 1024
    );
    for index in 0..32 {
        assert_eq!(
            store
                .get("scope", &format!("peer-{index}"))
                .await
                .unwrap()
                .unwrap()
                .data,
            serde_json::json!(index)
        );
    }
    let integrity: String = store
        .run(|conn| {
            conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .map_err(storage)
        })
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
}

#[tokio::test]
async fn coalesced_checkpoint_catches_up_after_reader_releases_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let store = Store::open(&path, Duration::from_secs(3600), 100, 16 * 1024 * 1024).unwrap();
    let response = |data| Response {
        data,
        fetched_at_ms: 1,
        validated_at_ms: 1,
        source: Source::Network,
        etag: None,
        last_modified: None,
        link: None,
    };
    store
        .put("scope", "warm", &response(serde_json::json!("warm")))
        .await
        .unwrap();
    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM sqlite_schema;")
        .unwrap();
    store
        .put(
            "scope",
            "large",
            &response(serde_json::json!("x".repeat(5 * 1024 * 1024))),
        )
        .await
        .unwrap();
    let observer = Connection::open(&path).unwrap();
    observer.busy_timeout(Duration::ZERO).unwrap();
    let counters = || {
        observer
            .query_row("PRAGMA wal_checkpoint(NOOP)", [], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (busy, frames, copied) = counters();
            if busy == 0 && frames >= 1000 && copied > 0 && copied < frames {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("reader did not pin a partial checkpoint");
    tokio::time::timeout(
        Duration::from_millis(500),
        store.put("scope", "follow-up", &response(serde_json::json!("next"))),
    )
    .await
    .expect("pinned reader held the cache writer")
    .unwrap();
    reader.execute_batch("ROLLBACK").unwrap();
    // Observe through a separate connection: Store::run would itself request
    // maintenance and conceal a lost coalesced follow-up.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (busy, frames, copied) = counters();
            if busy == 0 && frames >= 1000 && copied == frames {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("queued checkpoint did not catch up after the reader finished");
}
