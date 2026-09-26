//! Real CLI startup against an unavailable private database service.
use hey_boss::issues::Store;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::UnixListener,
    },
    path::PathBuf,
    process::{Child, Command},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    root: hey_boss::admin::Temporary,
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
    worker: Option<Child>,
}
impl Fixture {
    fn new() -> Self {
        let root = hey_boss::admin::Temporary::new().unwrap();
        fs::write(
            root.0.join("ssh"),
            "#!/bin/sh\ntouch ssh-attempted\nexit 255\n",
        )
        .unwrap();
        fs::set_permissions(root.0.join("ssh"), fs::Permissions::from_mode(0o755)).unwrap();
        let db = root.0.join("issues.db");
        drop(Store::open(&db).unwrap());
        let digest = format!(
            "{:x}",
            Sha256::digest(db.canonicalize().unwrap().as_os_str().as_encoded_bytes())
        );
        let directory = PathBuf::from(format!("/tmp/hey-boss-db-{}", unsafe { libc::getuid() }));
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let socket = directory.join(format!("{}.sock", &digest[..24]));
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let server = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = [0u8; 4];
                if stream.read_exact(&mut header).is_err() {
                    continue;
                }
                let mut body = vec![0; u32::from_be_bytes(header) as usize];
                stream.read_exact(&mut body).unwrap();
                // The service accepts connections, then drops the schema read.
                // This models a restart between connecting and opening Store.
                assert_eq!(body, b"\"Hello\"");
                let reply = serde_json::to_vec(&json!({
                    "version":1,"pid":std::process::id(),"transaction":false,
                    "last_id":0,"changes":0,"parameters":0,"columns":[],"rows":[],
                    "steps":0,"error":null,"more":false,"application":0,"schema":0
                }))
                .unwrap();
                stream
                    .write_all(&(reply.len() as u32).to_be_bytes())
                    .unwrap();
                stream.write_all(&reply).unwrap();
                let _ = stream.read_exact(&mut header);
            }
        });
        Self {
            root,
            socket,
            stop,
            server: Some(server),
            worker: None,
        }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(
            std::env::var_os("HEY_BOSS_TEST_WORKER_BINARY")
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_hey-boss").into()),
        );
        c.current_dir(&self.root.0)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.0.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("HEY_BOSS_ISSUE_DB", self.root.0.join("issues.db"))
            .env("HEY_BOSS_FLEET_STATE", self.root.0.join("fleet"))
            .env("HEY_BOSS_INBOX_SOCKET", self.root.0.join("no-inbox.sock"))
            .env("HEY_BOSS_CODEX", "/usr/bin/false")
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args(["worker", "run", "--json"])
            .stdout(fs::File::create(self.root.0.join("stdout")).unwrap())
            .stderr(fs::File::create(self.root.0.join("stderr")).unwrap());
        c
    }
    fn wait_idle(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let error = fs::read_to_string(self.root.0.join("stderr")).unwrap();
            assert!(
                self.worker.as_mut().unwrap().try_wait().unwrap().is_none(),
                "Worker crashed: {error}{}",
                fs::read_to_string(self.root.0.join("stdout")).unwrap()
            );
            if error.contains("idle") {
                break;
            }
            assert!(Instant::now() < deadline, "No idle status: {error}");
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn recover(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.server.take().unwrap().join().unwrap();
        fs::remove_file(&self.socket).unwrap();
    }
    fn cancel(&mut self) {
        let child = self.worker.as_mut().unwrap();
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "Cancellation failed: {status}");
                break;
            }
            assert!(Instant::now() < deadline, "Cancellation hung");
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(worker) = &mut self.worker {
            let _ = worker.kill();
            let _ = worker.wait();
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
        let _ = fs::remove_file(&self.socket);
    }
}

#[test]
fn startup_connection_outage_waits_idle_and_is_cancellable() {
    let mut f = Fixture::new();
    f.worker = Some(f.command().spawn().unwrap());
    f.wait_idle();
    thread::sleep(Duration::from_millis(1300));
    assert!(f.worker.as_mut().unwrap().try_wait().unwrap().is_none());
    assert!(
        fs::read_to_string(f.root.0.join("stderr"))
            .unwrap()
            .matches("Worker idle:")
            .count()
            >= 2
    );
    f.cancel();
}

#[test]
fn startup_connection_recovery_registers_one_worker_without_launching_unallocated_work() {
    let mut f = Fixture::new();
    let db = rusqlite::Connection::open(f.root.0.join("issues.db")).unwrap();
    db.execute_batch("UPDATE fleet_meta SET role='agent',node='offline-companion';
        INSERT INTO projects(id,name,next_number) VALUES('named:Offline','Offline',2);
        INSERT INTO agents(id,metadata,last_seen) VALUES('test:connection','{}',0);
        INSERT INTO issues(project_id,number,title,body,state,labels,version,created_by,created_at,updated_at,sort_order)
        VALUES('named:Offline',1,'Unallocated issue','','open','[]',1,'test:connection',0,0,1);").unwrap();
    f.worker = Some(
        f.command()
            .args(["--project", "named:Offline", "--cwd"])
            .arg(&f.root.0)
            .spawn()
            .unwrap(),
    );
    f.wait_idle();
    f.recover();
    let _owner = hey_boss::database::Owner::start(&f.root.0.join("issues.db")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = fs::read_to_string(f.root.0.join("stdout")).unwrap();
        if let Some(status) = output
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|s| s["worker_id"].is_string())
        {
            assert_eq!(status["active"], 0);
            assert_ne!(
                status["fleet"]["supervisor_connection"]["state"],
                "connected"
            );
            break;
        }
        assert!(
            f.worker.as_mut().unwrap().try_wait().unwrap().is_none(),
            "Worker exited: {output}"
        );
        assert!(
            Instant::now() < deadline,
            "Worker never recovered: {output}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        db.query_row("SELECT count(*) FROM issue_workers", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM worker_runs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(
        db.query_row("SELECT assignee FROM issues", [], |r| r
            .get::<_, Option<String>>(0))
            .unwrap()
            .is_none()
    );
    f.cancel();
    assert!(!f.root.0.join("ssh-attempted").exists());
}
