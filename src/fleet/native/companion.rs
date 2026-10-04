use super::{
    Result, authority,
    context::{Context, now, read_frame, send},
    control, conversation, pull,
    replica::{self, invalid},
    takeover,
};
use serde_json::{Value, json};
use std::{
    io::{BufReader, Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
// A receipt is published only after installation succeeds. Check the actual
// binary too, so an incomplete swap or rollback never forces a reconnect.
fn completed_upgrade(ctx: &Context) -> bool {
    let Ok(receipt) = ctx.read_json(&ctx.state.join("upgrade-receipt.json"), Value::Null) else {
        return false;
    };
    let Some(build) = receipt["source"]["build"].as_str() else {
        return false;
    };
    if build.is_empty() || build == env!("HEY_BOSS_BUILD_ID") {
        return false;
    }
    ctx.build()
        .is_ok_and(|installed| installed.ends_with(&format!("(build {build})")))
}

fn local_config(ctx: &Context) -> Result<Vec<Value>> {
    Ok(
        ctx.read_json(&ctx.state.join("fleet-agent.json"), json!({}))?["workers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|w| w.get("local_revision").is_some())
            .cloned()
            .collect(),
    )
}
fn reply(output: &Arc<Mutex<std::io::Stdout>>, value: Value) -> Result<()> {
    send(&mut *output.lock().unwrap(), value)
}

// Transport observation must not wait for database or worker collection. Keep
// last_sync separate: receiving a ping is not evidence that a pull was applied.
struct ConnectionStatus {
    ctx: Context,
    value: Value,
    written: Option<Instant>,
}
impl ConnectionStatus {
    fn new(ctx: Context, last_sync: Value) -> Self {
        Self {
            ctx,
            value: json!({"connected_at":0,"last_sync":last_sync}),
            written: None,
        }
    }
    fn observe(&mut self, connected: bool) -> Result<()> {
        self.value["connected_at"] = json!(if connected { now() } else { 0.0 });
        if !connected
            || self
                .written
                .is_none_or(|time| time.elapsed() >= Duration::from_secs(1))
        {
            self.write()?;
        }
        Ok(())
    }
    fn synced(&mut self) -> Result<()> {
        self.value["last_sync"] = json!(now());
        self.write()
    }
    fn write(&mut self) -> Result<()> {
        self.ctx
            .atomic_json(&self.ctx.state.join("fleet-agent-status.json"), &self.value)?;
        self.written = Some(Instant::now());
        Ok(())
    }
}

// Database work can outlast a heartbeat interval. Report transport liveness
// independently; revision/cursor acknowledgments still follow application.
struct OperationProgress {
    done: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl OperationProgress {
    fn start<W: Write + Send + 'static>(output: Arc<Mutex<W>>, operation: &'static str) -> Self {
        let (done, wait) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            while matches!(
                wait.recv_timeout(Duration::from_secs(5)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                if send(
                    &mut *output.lock().unwrap(),
                    json!({"kind":"ack","progress":operation}),
                )
                .is_err()
                {
                    break;
                }
            }
        });
        Self {
            done: Some(done),
            thread: Some(thread),
        }
    }
}
impl Drop for OperationProgress {
    fn drop(&mut self) {
        drop(self.done.take());
        let _ = self.thread.take().unwrap().join();
    }
}

// Receive replies independently of database work. Four wire-bounded frames
// retain backpressure for bulk transfers; repeated pings occupy only one slot
// until their heartbeat has finished. A slow pull must not hide an RPC reply.
struct Incoming {
    messages: Option<mpsc::Receiver<Result<Option<Value>>>>,
    ping: Arc<AtomicBool>,
    handling_ping: bool,
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
const SUPERVISOR_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

struct Interruptible<R> {
    input: R,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}
impl<R: Read + AsRawFd> Read for Interruptible<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        while !self.cancel.load(Ordering::Acquire) && !self.stop.load(Ordering::Acquire) {
            let mut fd = libc::pollfd {
                fd: self.input.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut fd, 1, 100) };
            if ready > 0 {
                let length = self.input.read(bytes)?;
                if length > 0 {
                    self.deadline = Instant::now() + SUPERVISOR_IDLE_TIMEOUT;
                }
                return Ok(length);
            }
            if ready == 0 && Instant::now() >= self.deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Supervisor sent no data for 60 seconds; reconnecting",
                ));
            }
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
        Ok(0)
    }
}
impl Incoming {
    fn start<R: Read + AsRawFd + Send + 'static>(
        input: R,
        replies: mpsc::SyncSender<Value>,
        stop: Arc<AtomicBool>,
        mut observe: impl FnMut(bool) -> Result<()> + Send + 'static,
    ) -> Self {
        let (messages, received) = mpsc::sync_channel(4);
        let ping = Arc::new(AtomicBool::new(false));
        let pending_ping = ping.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let reader_cancel = cancel.clone();
        let thread = std::thread::spawn(move || {
            let mut input = BufReader::new(Interruptible {
                input,
                deadline: Instant::now() + SUPERVISOR_IDLE_TIMEOUT,
                cancel: reader_cancel,
                stop,
            });
            loop {
                let frame = read_frame(&mut input).and_then(|message| {
                    if message.as_ref().is_some_and(|m| m["version"] != 1) {
                        Err(invalid("Unsupported fleet protocol version"))
                    } else {
                        if message.is_some() {
                            observe(true)?;
                        }
                        Ok(message)
                    }
                });
                if let Ok(Some(message)) = &frame {
                    if message["kind"] == "authority_reply" {
                        let _ = replies.try_send(frame.unwrap().unwrap());
                        continue;
                    }
                    if message["kind"] == "ping" && pending_ping.swap(true, Ordering::AcqRel) {
                        continue;
                    }
                }
                let done = !matches!(frame, Ok(Some(_)));
                if messages.send(frame).is_err() || done {
                    break;
                }
            }
            let _ = observe(false);
        });
        Self {
            messages: Some(received),
            ping,
            handling_ping: false,
            cancel,
            thread: Some(thread),
        }
    }
    fn next(&mut self) -> Result<Option<Value>> {
        if self.handling_ping {
            self.ping.store(false, Ordering::Release);
        }
        let message = self
            .messages
            .as_ref()
            .unwrap()
            .recv()
            .map_err(|_| invalid("Fleet input reader exited"))??;
        self.handling_ping = message.as_ref().is_some_and(|m| m["kind"] == "ping");
        Ok(message)
    }
}
impl Drop for Incoming {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        // Wake both a blocked sender and a reader waiting for a partial frame.
        drop(self.messages.take());
        let _ = self.thread.take().unwrap().join();
    }
}
pub(super) fn stdio(ctx: Context, startup: super::handshake::Progress) -> Result<()> {
    // Register fleet identity before journaling and exporting the first hello.
    startup.phase("identity");
    ctx.rpc(json!({"action":"whoami"}))?;
    startup.phase("capture");
    let db = ctx.db()?;
    let role: String = db.query_row("SELECT role FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    if role == "standalone" {
        startup.phase("bootstrap");
        // The initial contribution is a logical journal. A standalone archive
        // has no remote source yet, so materialize its history before joining.
        for row in replica::rows(
            &db,
            "SELECT project_id,number FROM issues WHERE archive_key IS NOT NULL",
            &[],
        )? {
            crate::issues::archive::restore_issue(
                &db,
                row["project_id"].as_str().unwrap(),
                row["number"].as_i64().unwrap(),
                crate::issues::worker::now(),
            )?;
        }
        crate::issues::archive::backup_store(
            &db,
            &ctx.state
                .join(format!("fleet-bootstrap-{}.db", now() as i64)),
        )?;
        replica::install_capture(&db, "agent", &ctx.node)?;
        let tx = db.unchecked_transaction()?;
        for table in [
            "agents",
            "issues",
            "issue_status_updates",
            "issue_subtasks",
            "comments",
            "events",
            "issue_pull_requests",
        ] {
            for row in replica::rows(&db, &format!("SELECT * FROM {table}"), &[])? {
                replica::execute(
                    &db,
                    "INSERT INTO fleet_outbox(table_name,after_json,created_at) VALUES(?,?,?)",
                    &[
                        json!(table),
                        json!(row.to_string()),
                        json!(crate::issues::worker::now()),
                    ],
                )?;
            }
        }
        let max: i64 = db.query_row("SELECT coalesce(max(seq),0) FROM fleet_outbox", [], |r| {
            r.get(0)
        })?;
        replica::state_set(&db, "bootstrap_last_seq", &json!(max))?;
        tx.commit()?;
    }
    startup.phase("capture");
    replica::install_capture(&db, "agent", &ctx.node)?;
    let output = Arc::new(Mutex::new(std::io::stdout()));
    startup.phase("relay");
    let relay = authority::Relay::start(&ctx, output.clone())?;
    startup.phase("workers");
    let workers = ctx.workers()?;
    startup.phase("snapshot");
    let chief_ownership = crate::chief_ownership::read(&db)?;
    let hello = json!({"kind":"hello","capabilities":{"pull_gzip_chunks":true,"issue_archives":true},"node":ctx.node,"hostname":crate::issues::identity::host(),"build":Context::running_build(),"projects":replica::rows(&db,"SELECT * FROM projects",&[])?,"local_config":local_config(&ctx)?,"chief_ownership":chief_ownership,"workers":workers,"cursor":replica::state_get(&db,"cursor",Value::Null)?,"revision":replica::state_get(&db,"revision",Value::Null)?,"pending":count(&db,"fleet_outbox")?});
    let status = Arc::new(Mutex::new(ConnectionStatus::new(
        ctx.clone(),
        replica::state_get(&db, "last_sync", Value::Null)?,
    )));
    // Join the reporter before hello so startup frames cannot leak into the session.
    drop(startup);
    reply(&output, hello)?;
    let (tx, rx) = mpsc::sync_channel::<Value>(100);
    let signals = ctx.clone();
    let signal_output = output.clone();
    std::thread::spawn(move || {
        while let Ok(message) = rx.recv() {
            let result = control::apply_signal(&signals, &message).unwrap_or_else(
                |e| json!({"id":message["id"],"state":"pending","error":e.to_string()}),
            );
            if reply(&signal_output, json!({"kind":"ack","signal":result})).is_err() {
                break;
            }
        }
    });
    let fd = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let input = unsafe { std::fs::File::from_raw_fd(fd) };
    let observed = status.clone();
    let mut input = Incoming::start(input, relay.replies(), ctx.stop.clone(), move |connected| {
        observed.lock().unwrap().observe(connected)
    });
    let mut pulls = pull::PullReader::default();
    while !ctx.stopped() {
        let Some(message) = input.next()? else {
            break;
        };
        // End at a message boundary, before starting another database task.
        // The supervisor reconnects; detached workers keep their execution.
        if message["kind"] == "ping" && completed_upgrade(&ctx) {
            break;
        }
        let operation = match message["kind"].as_str() {
            Some("pull" | "pull_end") => Some("pull"),
            Some("configure") => Some("configure"),
            Some("ping") => Some("heartbeat"),
            Some("conversation" | "takeover" | "steer") => Some("request"),
            _ => None,
        };
        let _progress =
            operation.map(|operation| OperationProgress::start(output.clone(), operation));
        let Some(message) = pulls.receive(message)? else {
            continue;
        };
        match message["kind"].as_str() {
            Some("configure") => {
                relay.configure(&message);
                reply(&output, control::configure_companion(&ctx, &message)?)?;
            }
            Some("pull") => {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let payload = super::archive::prepare_pull(
                        &db,
                        &message["payload"],
                        |key, project, number, cursor| {
                            authority::call(
                                &ctx.state,
                                &ctx.path,
                                json!({"kind":"issue_archive","key":key,"project":project,"number":number,"cursor":cursor}),
                            )
                        },
                    )?;
                    match replica::apply_pull(
                        &db,
                        &ctx.node,
                        &payload,
                        message["receipts"]
                            .as_array()
                            .map(Vec::as_slice)
                            .unwrap_or(&[]),
                    ) {
                        Ok(()) => break,
                        Err(error)
                            if error
                                .downcast_ref::<crate::issues::Error>()
                                .is_some_and(|e| e.code == "archive_retry")
                                && Instant::now() < deadline =>
                        {
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
                crate::chief_ownership::stop_unassigned(&db)?;
                status.lock().unwrap().synced()?;
                control::reconcile(
                    &ctx,
                    &ctx.read_json(&ctx.state.join("fleet-agent.json"), json!({}))?,
                )?;
                reply(
                    &output,
                    json!({"kind":"ack","cursor":replica::state_get(&db,"cursor",Value::Null)?,"pending":count(&db,"fleet_outbox")?}),
                )?;
            }
            Some("signal") => {
                if tx.try_send(message.clone()).is_err() {
                    reply(
                        &output,
                        json!({"kind":"ack","signal":{"id":message["id"],"state":"pending","error":"Worker control queue is busy"}}),
                    )?;
                }
            }
            Some("conversation" | "takeover" | "steer") => {
                let cursor = message.get("cursor").cloned().unwrap_or(json!(0));
                let result = if message["kind"] == "steer" {
                    takeover::steer(&ctx, &message)
                } else if message["kind"] == "takeover" {
                    takeover::apply(&ctx, message["run"].as_str().unwrap_or(""))
                } else {
                    conversation::page(&ctx, message["run"].as_str().unwrap_or(""), &cursor)
                }
                .unwrap_or_else(|e| json!({"ok":false,"error":e.to_string()}));
                reply(
                    &output,
                    json!({"kind":message["kind"],"id":message["id"],"result":result}),
                )?;
            }
            Some("ping") => {
                // Acknowledgment precedes observation of the stopped Chief.
                let chief_ownership = crate::chief_ownership::read(&db)?;
                reply(
                    &output,
                    json!({"kind":"heartbeat","at":now(),"chief_ownership":chief_ownership,"workers":ctx.workers()?,"changes":replica::journal(&db,0)?,"cursor":replica::state_get(&db,"cursor",Value::Null)?,"local_config":local_config(&ctx)?,"pending":count(&db,"fleet_outbox")?,"conflicts":replica::rows(&db,"SELECT count(*) count FROM fleet_conflicts WHERE resolved=0",&[])?[0]["count"],"revision":replica::state_get(&db,"revision",Value::Null)?}),
                )?;
            }
            _ => return Err(invalid("Unknown fleet message kind")),
        }
    }
    // Closing the transport leaves detached workers and active agents alive.
    Ok(())
}
fn count(db: &crate::database::Connection, table: &str) -> Result<i64> {
    Ok(db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?)
}
pub(super) fn daemon(ctx: Context) -> Result<()> {
    let Some(_lock) = ctx.lock("fleet-agent.lock", false)? else {
        return Err("Fleet companion is already running".into());
    };
    super::model_recovery::start(ctx.clone());
    while !ctx.stopped() {
        let interrupted = {
            let _lock = ctx.lock("fleet-worker-control.lock", false)?;
            if _lock.is_some() {
                replica::rows(
                    &ctx.db()?,
                    "SELECT * FROM fleet_signals WHERE host='local' AND state IN ('stopping','starting') ORDER BY created_at",
                    &[],
                )?
            } else {
                vec![]
            }
        };
        for pending in interrupted {
            if let Err(e) = control::apply_signal(&ctx, &pending) {
                ctx.atomic_json(
                    &ctx.state.join("fleet-agent-error.json"),
                    &json!({"worker":pending["worker"],"error":e.to_string(),"at":now()}),
                )?;
            }
        }
        let config = ctx.read_json(&ctx.state.join("fleet-agent.json"), json!({}))?;
        if config.as_object().is_some_and(|m| !m.is_empty())
            && let Err(e) = control::reconcile(&ctx, &config)
        {
            ctx.atomic_json(
                &ctx.state.join("fleet-agent-error.json"),
                &json!({"error":e.to_string(),"at":now()}),
            )?;
        }
        ctx.wait(Duration::from_secs(5));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_transport_expires_and_releases_its_relay_without_peer_eof() {
        let (root, mut ctx, store) = super::super::context::tests::test_context();
        // macOS temporary directories can exceed the Unix socket path limit.
        let relay_state = std::path::PathBuf::from("/tmp")
            .join(format!("hb-idle-{}", super::super::context::id().unwrap()));
        std::fs::create_dir(&relay_state).unwrap();
        ctx.state = relay_state.clone();
        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        let relay = authority::Relay::start(&ctx, output.clone()).unwrap();
        let (reader, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let (sent, received) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut input = Interruptible {
                input: reader,
                deadline: Instant::now(),
                cancel: cancelled,
                stop: Arc::new(AtomicBool::new(false)),
            };
            let result = input.read(&mut [0_u8; 1]);
            drop(relay);
            let _ = sent.send(result);
        });
        let result = received.recv_timeout(Duration::from_secs(1));
        cancel.store(true, Ordering::Release);
        thread.join().unwrap();
        let replacement = authority::Relay::start(&ctx, output);
        let rebound = replacement.is_ok();
        drop(replacement);
        drop(store);
        std::fs::remove_dir_all(relay_state).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(
            result.unwrap().unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(rebound, "Expired transport kept the relay lock");
    }

    #[test]
    fn incoming_bytes_renew_the_silent_transport_deadline() {
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let old = Instant::now() + Duration::from_secs(1);
        let mut input = Interruptible {
            input: reader,
            deadline: old,
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        writer.write_all(b"ping").unwrap();
        assert_eq!(input.read(&mut [0_u8; 4]).unwrap(), 4);
        assert!(input.deadline > old + Duration::from_secs(50));
        input.cancel.store(true, Ordering::Release);
        assert_eq!(input.read(&mut [0_u8; 1]).unwrap(), 0);
    }

    #[test]
    fn received_pings_refresh_status_while_work_is_blocked_without_advancing_sync() {
        let (_root, ctx, _store) = super::super::context::tests::test_context();
        let status = Arc::new(Mutex::new(ConnectionStatus::new(ctx.clone(), json!(123))));
        let writer_db = ctx.db().unwrap();
        writer_db.execute_batch("BEGIN IMMEDIATE").unwrap();
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let (replies, _) = mpsc::sync_channel(2);
        let (observations, received) = mpsc::channel();
        let observed = status.clone();
        let input = Incoming::start(reader, replies, ctx.stop.clone(), move |connected| {
            observed.lock().unwrap().observe(connected)?;
            let _ = observations.send(connected);
            Ok(())
        });
        // The main consumer is deliberately idle, as during a slow pull.
        send(&mut writer, json!({"kind":"ping"})).unwrap();
        assert!(received.recv_timeout(Duration::from_secs(2)).unwrap());
        let saved = ctx
            .read_json(&ctx.state.join("fleet-agent-status.json"), Value::Null)
            .unwrap();
        assert!(now() - saved["connected_at"].as_f64().unwrap() < 2.0);
        assert_eq!(saved["last_sync"], 123);
        // Even coalesced pings remain observations of the live supervisor.
        send(&mut writer, json!({"kind":"ping"})).unwrap();
        assert!(received.recv_timeout(Duration::from_secs(2)).unwrap());
        drop(writer);
        assert!(!received.recv_timeout(Duration::from_secs(2)).unwrap());
        drop(input);
        let saved = ctx
            .read_json(&ctx.state.join("fleet-agent-status.json"), Value::Null)
            .unwrap();
        assert_eq!(saved["connected_at"], 0.0);
        assert_eq!(saved["last_sync"], 123);
        writer_db.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn input_delivers_authority_replies_ahead_of_work_and_coalesces_pings() {
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let (replies, received) = mpsc::sync_channel(2);
        let mut input =
            Incoming::start(
                reader,
                replies,
                Arc::new(AtomicBool::new(false)),
                |_| Ok(()),
            );
        send(&mut writer, json!({"kind":"pull"})).unwrap();
        for _ in 0..100 {
            send(&mut writer, json!({"kind":"ping"})).unwrap();
        }
        send(
            &mut writer,
            json!({"kind":"authority_reply","id":"waiting"}),
        )
        .unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap()["id"],
            "waiting"
        );
        assert_eq!(input.next().unwrap().unwrap()["kind"], "pull");
        assert_eq!(input.next().unwrap().unwrap()["kind"], "ping");
        drop(writer);
        assert!(input.next().unwrap().is_none());
    }

    #[test]
    fn input_shutdown_interrupts_partial_frames_and_full_queues() {
        for bytes in [
            b"{\"kind\":".to_vec(),
            b"{\"version\":1,\"kind\":\"configure\"}\n".repeat(8),
        ] {
            let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
            let (replies, _) = mpsc::sync_channel(2);
            let input =
                Incoming::start(
                    reader,
                    replies,
                    Arc::new(AtomicBool::new(false)),
                    |_| Ok(()),
                );
            writer.write_all(&bytes).unwrap();
            let started = std::time::Instant::now();
            drop(input);
            assert!(started.elapsed() < Duration::from_secs(1));
        }
    }

    #[test]
    fn input_rejects_incompatible_replies_before_routing_them() {
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let (replies, received) = mpsc::sync_channel(2);
        let mut input =
            Incoming::start(
                reader,
                replies,
                Arc::new(AtomicBool::new(false)),
                |_| Ok(()),
            );
        writeln!(
            writer,
            "{}",
            json!({"version":2,"kind":"authority_reply","id":"invalid"})
        )
        .unwrap();
        assert!(
            input
                .next()
                .unwrap_err()
                .to_string()
                .contains("protocol version")
        );
        assert!(received.try_recv().is_err());
    }

    struct Recording {
        bytes: Vec<u8>,
        frames: mpsc::Sender<Value>,
    }
    impl Write for Recording {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            let frame = serde_json::from_slice(&self.bytes).unwrap();
            self.bytes.clear();
            self.frames.send(frame).unwrap();
            Ok(())
        }
    }

    #[test]
    fn slow_pull_reports_liveness_without_acknowledging_a_cursor_and_stops_on_drop() {
        let (frames, received) = mpsc::channel();
        let output = Arc::new(Mutex::new(Recording {
            bytes: Vec::new(),
            frames,
        }));
        let progress = OperationProgress::start(output.clone(), "pull");
        let frame = received.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(frame, json!({"version":1,"kind":"ack","progress":"pull"}));
        assert!(frame.get("cursor").is_none());
        let stopped = std::time::Instant::now();
        drop(progress);
        assert!(stopped.elapsed() < Duration::from_secs(1));
        drop(output);
        assert!(matches!(
            received.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}
