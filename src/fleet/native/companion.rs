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
    time::Duration,
};
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

// Verification and atomic application can outlast a heartbeat interval. This
// reports liveness only: the durable cursor acknowledgment still follows commit.
struct PullProgress {
    done: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl PullProgress {
    fn start<W: Write + Send + 'static>(output: Arc<Mutex<W>>) -> Self {
        let (done, wait) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            while matches!(
                wait.recv_timeout(Duration::from_secs(5)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                if send(
                    &mut *output.lock().unwrap(),
                    json!({"kind":"ack","progress":"pull"}),
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
impl Drop for PullProgress {
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
struct Interruptible<R> {
    input: R,
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
                return self.input.read(bytes);
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
    ) -> Self {
        let (messages, received) = mpsc::sync_channel(4);
        let ping = Arc::new(AtomicBool::new(false));
        let pending_ping = ping.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let reader_cancel = cancel.clone();
        let thread = std::thread::spawn(move || {
            let mut input = BufReader::new(Interruptible {
                input,
                cancel: reader_cancel,
                stop,
            });
            loop {
                let frame = read_frame(&mut input).and_then(|message| {
                    if message.as_ref().is_some_and(|m| m["version"] != 1) {
                        Err(invalid("Unsupported fleet protocol version"))
                    } else {
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
pub(super) fn stdio(ctx: Context) -> Result<()> {
    // Register fleet identity before journaling and exporting the first hello.
    ctx.rpc(json!({"action":"whoami"}))?;
    let db = ctx.db()?;
    let role: String = db.query_row("SELECT role FROM fleet_meta WHERE id=1", [], |r| r.get(0))?;
    if role == "standalone" {
        db.backup(
            "main",
            ctx.state
                .join(format!("fleet-bootstrap-{}.db", now() as i64)),
            None,
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
    replica::install_capture(&db, "agent", &ctx.node)?;
    let output = Arc::new(Mutex::new(std::io::stdout()));
    let relay = authority::Relay::start(&ctx, output.clone())?;
    let chief_ownership = crate::chief_ownership::read(&db)?;
    reply(
        &output,
        json!({"kind":"hello","capabilities":{"pull_gzip_chunks":true},"node":ctx.node,"hostname":crate::issues::identity::host(),"build":Context::running_build(),"projects":replica::rows(&db,"SELECT * FROM projects",&[])?,"local_config":local_config(&ctx)?,"chief_ownership":chief_ownership,"workers":ctx.workers()?,"cursor":replica::state_get(&db,"cursor",Value::Null)?,"revision":replica::state_get(&db,"revision",Value::Null)?,"pending":count(&db,"fleet_outbox")?}),
    )?;
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
    let mut input = Incoming::start(input, relay.replies(), ctx.stop.clone());
    let mut pulls = pull::PullReader::default();
    while !ctx.stopped() {
        let Some(message) = input.next()? else {
            break;
        };
        let _progress = matches!(message["kind"].as_str(), Some("pull" | "pull_end"))
            .then(|| PullProgress::start(output.clone()));
        let Some(message) = pulls.receive(message)? else {
            continue;
        };
        match message["kind"].as_str() {
            Some("configure") => {
                relay.configure(&message);
                reply(&output, control::configure_companion(&ctx, &message)?)?;
            }
            Some("pull") => {
                replica::apply_pull(
                    &db,
                    &ctx.node,
                    &message["payload"],
                    message["receipts"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                )?;
                crate::chief_ownership::stop_unassigned(&db)?;
                ctx.atomic_json(
                    &ctx.state.join("fleet-agent-status.json"),
                    &json!({"connected_at":now(),"last_sync":now()}),
                )?;
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
                ctx.atomic_json(
                    &ctx.state.join("fleet-agent-status.json"),
                    &json!({"connected_at":now(),"last_sync":replica::state_get(&db,"last_sync",Value::Null)?}),
                )?;
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
    fn input_delivers_authority_replies_ahead_of_work_and_coalesces_pings() {
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let (replies, received) = mpsc::sync_channel(2);
        let mut input = Incoming::start(reader, replies, Arc::new(AtomicBool::new(false)));
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
            let input = Incoming::start(reader, replies, Arc::new(AtomicBool::new(false)));
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
        let mut input = Incoming::start(reader, replies, Arc::new(AtomicBool::new(false)));
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
        let progress = PullProgress::start(output.clone());
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
