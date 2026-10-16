use super::super::context::Context;
use super::*;
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{BufReader, Read},
    os::fd::AsRawFd,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn engine(ctx: &Context) -> Result<hey_boss_sync::Engine> {
    let path = ctx.path.with_extension("sync.db");
    ctx.protect_file(&path)?;
    hey_boss_sync::Engine::open(&path)
}

fn prepare(
    ctx: &Context,
    db: &Connection,
    engine: &mut hey_boss_sync::Engine,
    controller: bool,
) -> Result<()> {
    install(db, &ctx.node)?;
    archives::prepare(db, engine)?;
    for _ in 0..8 {
        if export(db, engine)? == 0 {
            break;
        }
    }
    if baseline(db, engine, controller)?
        && controller
        && db.query_row(
            "SELECT EXISTS(SELECT 1 FROM distributed_members WHERE node=?1 AND ready=0)",
            [&ctx.node],
            |r| r.get::<_, bool>(0),
        )?
    {
        db.execute(
            "UPDATE distributed_members SET ready=1 WHERE node=?1 AND ready=0",
            [&ctx.node],
        )?;
    }
    for _ in 0..8 {
        if import(db, engine)? == 0 {
            break;
        }
    }
    // The immutable journal is durable before exported advances. Keep only a
    // bounded application outbox rather than retaining a second full history.
    if db.query_row("SELECT EXISTS(SELECT 1 FROM distributed_outbox WHERE clock<=(SELECT exported FROM distributed_meta WHERE id=1))", [], |r| r.get::<_, bool>(0))? {
        db.execute("DELETE FROM distributed_outbox WHERE clock IN (SELECT clock FROM distributed_outbox WHERE clock<=(SELECT exported FROM distributed_meta WHERE id=1) ORDER BY clock LIMIT 1000)", [])?;
    }
    Ok(())
}

// Application preparation is serialized locally. Network I/O never holds this
// lock: each peer has its own WAL connection and independent transport loop.
fn prepared(
    ctx: &Context,
    db: &Connection,
    journal: &mut hey_boss_sync::Engine,
    controller: bool,
) -> Result<()> {
    let Some(_lock) = ctx.lock("distributed-sync.lock", false)? else {
        return Ok(());
    };
    crate::issues::allocations::maintain(db)?;
    let result =
        prepare(ctx, db, journal, controller).and_then(|()| health::prepared(ctx, db, journal));
    if let Err(error) = &result {
        health::failed(ctx, &error.to_string());
    }
    result
}

struct PresenceSnapshot {
    pulses: Vec<hey_boss_sync::session::Presence>,
    captured: Instant,
    age: Duration,
}
impl PresenceSnapshot {
    fn current(&self) -> Vec<hey_boss_sync::session::Presence> {
        let age = self
            .age
            .saturating_add(self.captured.elapsed())
            .as_millis()
            .min(i64::MAX as u128) as i64;
        self.pulses
            .iter()
            .filter_map(|pulse| {
                let mut pulse = pulse.clone();
                pulse.remaining_ms = pulse.remaining_ms.saturating_sub(age);
                (pulse.remaining_ms > 0).then_some(pulse)
            })
            .collect()
    }
}

#[derive(Default)]
struct ApplicationState {
    outgoing: Option<PresenceSnapshot>,
    incoming: Option<PresenceSnapshot>,
}

// Only replaceable presence is queued in memory. Facts are acknowledged by the
// independent durable journal. A slow application writer cannot stall SSH.
struct Application {
    state: Arc<std::sync::Mutex<ApplicationState>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Application {
    fn start(ctx: &Context, controller: bool, peer: &str) -> Self {
        let state = Arc::new(std::sync::Mutex::new(ApplicationState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (shared, stopping, ctx, peer) =
            (state.clone(), stop.clone(), ctx.clone(), peer.to_owned());
        let thread = std::thread::spawn(move || {
            let run = || -> Result<()> {
                let db = ctx.db()?;
                let mut journal = engine(&ctx)?;
                while !cancelled(&ctx, &stopping) {
                    let received = shared.lock().unwrap().incoming.take();
                    if let Some(received) = received {
                        crate::issues::allocations::receive_presence(
                            &db,
                            &received.pulses,
                            received.age.saturating_add(received.captured.elapsed()),
                        )?;
                        crate::issues::allocations::record_exchange(&db, &peer, received.age)?;
                    }
                    // This connection's presence must progress even when a
                    // different connection is preparing the same application.
                    let captured = Instant::now();
                    let pulses = crate::issues::allocations::presence(&db)?;
                    shared.lock().unwrap().outgoing = Some(PresenceSnapshot {
                        pulses,
                        captured,
                        age: Duration::ZERO,
                    });
                    if let Some(_lock) = ctx.lock("distributed-sync.lock", false)? {
                        crate::issues::allocations::maintain(&db)?;
                        prepare(&ctx, &db, &mut journal, controller)?;
                        health::prepared(&ctx, &db, &journal)?;
                        // Timestamp before the query so query latency never
                        // grants extra lifetime to a cached pulse.
                        let captured = Instant::now();
                        let pulses = crate::issues::allocations::presence(&db)?;
                        shared.lock().unwrap().outgoing = Some(PresenceSnapshot {
                            pulses,
                            captured,
                            age: Duration::ZERO,
                        });
                    }
                    wait(&ctx, &stopping, SYNC_INTERVAL);
                }
                Ok(())
            };
            if let Err(error) = run() {
                health::failed(&ctx, &error.to_string());
            }
        });
        Self {
            state,
            stop,
            thread: Some(thread),
        }
    }
    fn presence(&self) -> Vec<hey_boss_sync::session::Presence> {
        self.state
            .lock()
            .unwrap()
            .outgoing
            .as_ref()
            .map(PresenceSnapshot::current)
            .unwrap_or_default()
    }
    fn check(&self) -> Result<()> {
        if self
            .thread
            .as_ref()
            .is_none_or(|thread| thread.is_finished())
        {
            return Err(replica::invalid(
                "Sync application processing stopped; reconnecting",
            ));
        }
        Ok(())
    }
    fn received(&self, pulses: Vec<hey_boss_sync::session::Presence>, age: Duration) {
        self.state.lock().unwrap().incoming = Some(PresenceSnapshot {
            pulses,
            captured: Instant::now(),
            age,
        });
    }
}
impl Drop for Application {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// A silent client must not leave a server process and database connection alive
// forever. This also bounds incomplete frames; EOF still shuts down cleanly.
struct BoundedReader<R>(R);
impl<R: Read + AsRawFd> Read for BoundedReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let timeout = Timing::from_exchange(Duration::ZERO).lease * 2;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let mut fd = libc::pollfd {
                fd: self.0.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe {
                libc::poll(
                    &mut fd,
                    1,
                    remaining.as_millis().min(i32::MAX as u128) as i32,
                )
            };
            if ready > 0 {
                return self.0.read(bytes);
            }
            if ready == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Sync peer stopped responding",
                ));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

pub(in crate::fleet::native) fn serve(
    ctx: Context,
    controller: &str,
    continuous: bool,
) -> Result<()> {
    if controller.len() != 32 || !controller.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(replica::invalid("Invalid controller identity"));
    }
    let db = ctx.db()?;
    install(&db, &ctx.node)?;
    if db.query_row(
        "SELECT controller=?1 FROM distributed_meta WHERE id=1",
        [controller],
        |r| r.get::<_, Option<bool>>(0),
    )? != Some(true)
    {
        let _lock = ctx.lock("distributed-sync.lock", true)?;
        install(&db, &ctx.node)?;
        db.execute("UPDATE distributed_meta SET controller=?1 WHERE id=1 AND (controller IS NULL OR active=0)",[controller])?;
    }
    let mut journal = engine(&ctx)?;
    let input = unsafe { libc::dup(libc::STDIN_FILENO) };
    if input < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    use std::os::fd::FromRawFd;
    let mut reader = BoundedReader(unsafe { std::fs::File::from_raw_fd(input) });
    let mut writer = std::io::stdout().lock();
    if !continuous {
        prepared(&ctx, &db, &mut journal, false)?;
        for _ in 0..8 {
            let result =
                hey_boss_sync::session::serve_next(&mut journal, &mut reader, &mut writer, |_| {
                    Ok(Vec::new())
                })?
                .ok_or_else(|| replica::invalid("SSH synchronization ended before handshake"))?;
            health::peer_exchanged(&ctx, &result)?;
        }
        prepared(&ctx, &db, &mut journal, false)?;
        return Ok(());
    }
    let application = Application::start(&ctx, false, controller);
    while !ctx.stopped() {
        let mut started = Instant::now();
        let result =
            hey_boss_sync::session::serve_next(&mut journal, &mut reader, &mut writer, |_| {
                started = Instant::now();
                application.check()?;
                Ok(application.presence())
            })?;
        let Some(result) = result else {
            break;
        };
        health::peer_exchanged(&ctx, &result)?;
        application.received(result.presence, result.presence_age.max(started.elapsed()));
    }
    Ok(())
}

use crate::issues::allocations::{SYNC_INTERVAL, Timing};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, PartialEq, Eq)]
struct Target {
    host: String,
    peer: Option<(super::super::peer_sync::Peer, String)>,
    diagnostic: std::path::PathBuf,
}
fn cancelled(ctx: &Context, stop: &AtomicBool) -> bool {
    ctx.stopped() || stop.load(Ordering::Acquire)
}
fn wait(ctx: &Context, stop: &AtomicBool, duration: Duration) {
    let deadline = Instant::now() + duration;
    while !cancelled(ctx, stop) && Instant::now() < deadline {
        std::thread::sleep(
            (SYNC_INTERVAL / 8).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}
fn connected(ctx: &Context, target: &Target, stop: &AtomicBool) -> Result<()> {
    let mut journal = engine(ctx)?;
    let controller = target
        .peer
        .as_ref()
        .map(|(_, controller)| controller.as_str())
        .unwrap_or(&ctx.node);
    if controller.len() != 32
        || !controller.bytes().all(|b| b.is_ascii_hexdigit())
        || !crate::health::remote::valid_host(&target.host)
    {
        return Err(replica::invalid("Invalid sync destination"));
    }
    let mut command = if let Some((peer, _)) = &target.peer {
        super::super::peer_sync::ssh(ctx, peer)
    } else {
        let mut command = Command::new("ssh");
        crate::ssh::reuse_connection(&mut command, &ctx.state, &target.host);
        command
    };
    let script = format!(
        "export PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH\"; exec hey-boss fleet sync --continuous --controller {controller}"
    );
    let mut child = command
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            &format!("ConnectTimeout={}", (SYNC_INTERVAL * 4).as_secs()),
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            &format!("ServerAliveInterval={}", SYNC_INTERVAL.as_secs()),
            "-o",
            "ServerAliveCountMax=2",
            &target.host,
            &script,
        ])
        .env("SFT_NO_BROWSER", "1")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut writer = child.stdin.take().unwrap();
    let progress = std::sync::Mutex::new(Some(Instant::now()));
    let finished = AtomicBool::new(false);
    let application = Application::start(ctx, target.peer.is_none(), &target.host);
    let result = std::thread::scope(|scope| {
        let pid = child.id();
        let progress = &progress;
        let finished = &finished;
        scope.spawn(move || {
            while !finished.load(Ordering::Acquire) {
                let timed_out = progress.lock().unwrap().is_some_and(|start| {
                    start.elapsed() >= Timing::from_exchange(Duration::ZERO).lease * 2
                });
                if cancelled(ctx, stop) || timed_out {
                    unsafe {
                        libc::kill(pid as i32, libc::SIGKILL);
                    }
                    break;
                }
                std::thread::sleep(SYNC_INTERVAL / 8);
            }
        });
        let result = (|| -> Result<()> {
            while !cancelled(ctx, stop) {
                application.check()?;
                let start = Instant::now();
                *progress.lock().unwrap() = Some(start);
                let presence = application.presence();
                let result = hey_boss_sync::session::connect_present(
                    &mut journal,
                    &mut reader,
                    &mut writer,
                    presence,
                )?;
                health::peer_exchanged(ctx, &result)?;
                application.received(result.presence, result.presence_age.max(start.elapsed()));
                ctx.atomic_json(&target.diagnostic,&json!({"host":target.host,"state":"connected","at":super::super::context::now(),"exchange_ms":start.elapsed().as_millis()}))?;
                *progress.lock().unwrap() = None;
                // Drain actual changes immediately. Idle peers send one tiny
                // exchange per interval over the existing authenticated pipe.
                if result.records == 0 {
                    wait(ctx, stop, SYNC_INTERVAL.saturating_sub(start.elapsed()));
                }
            }
            Ok(())
        })();
        finished.store(true, Ordering::Release);
        result
    });
    drop(writer);
    drop(reader);
    let _ = child.kill();
    let _ = child.wait();
    result
}

pub(in crate::fleet::native) fn start(ctx: Context, controller: bool) {
    std::thread::spawn(move || {
        let mut workers =
            BTreeMap::<String, (Target, Arc<AtomicBool>, std::thread::JoinHandle<()>)>::new();
        while !ctx.stopped() {
            let result = (|| -> Result<()> {
                let mut targets = BTreeMap::new();
                if controller {
                    for machine in ctx.inventory()? {
                        if let Some(host) = machine["host"].as_str().filter(|host| *host != "local")
                        {
                            targets.insert(
                                host.to_owned(),
                                Target {
                                    host: host.into(),
                                    peer: None,
                                    diagnostic: ctx.state.join(format!(
                                        "sync-{}.json",
                                        hey_boss_sync::digest(host.as_bytes())
                                    )),
                                },
                            );
                        }
                    }
                } else if let Some(settings) = super::super::peer_sync::cached(&ctx)? {
                    for peer in settings
                        .peers
                        .iter()
                        .filter(|peer| settings.enabled && settings.machine < peer.machine)
                    {
                        targets.insert(
                            peer.machine.clone(),
                            Target {
                                host: peer.address.clone(),
                                peer: Some((peer.clone(), settings.controller.clone())),
                                diagnostic: super::super::peer_sync::diagnostic_path(
                                    &ctx,
                                    &peer.machine,
                                ),
                            },
                        );
                    }
                }
                // Stop changed routes first, and reap them before replacing a connection.
                // A slow peer's shutdown never blocks discovery of healthy peers.
                let mut finished = Vec::new();
                for (key, (target, stop, handle)) in &workers {
                    if targets.get(key) != Some(target) {
                        stop.store(true, Ordering::Release);
                    }
                    if handle.is_finished() {
                        finished.push(key.clone());
                    }
                }
                for key in finished {
                    if let Some((_, _, handle)) = workers.remove(&key) {
                        let _ = handle.join();
                    }
                }
                for (key, target) in targets {
                    if workers.contains_key(&key) {
                        continue;
                    }
                    let saved_target = target.clone();
                    let stop = Arc::new(AtomicBool::new(false));
                    let stopping = stop.clone();
                    let ctx = ctx.clone();
                    let handle = std::thread::spawn(move || {
                        let mut backoff = SYNC_INTERVAL;
                        while !cancelled(&ctx, &stopping) {
                            let connected_at = Instant::now();
                            if let Err(error) = connected(&ctx, &target, &stopping) {
                                if connected_at.elapsed()
                                    >= Timing::from_exchange(Duration::ZERO).lease
                                {
                                    backoff = SYNC_INTERVAL;
                                }
                                let _=ctx.atomic_json(&target.diagnostic,&json!({"host":target.host,"state":"disconnected","error":error.to_string(),"at":super::super::context::now()}));
                                wait(&ctx, &stopping, backoff);
                                backoff =
                                    (backoff * 2).min(Timing::from_exchange(Duration::ZERO).lease);
                            }
                        }
                    });
                    workers.insert(key, (saved_target, stop, handle));
                }
                // Capture and recovery continue with no reachable transport.
                let db = ctx.db()?;
                let mut journal = engine(&ctx)?;
                prepared(&ctx, &db, &mut journal, controller)?;
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!("Distributed sync: {error}");
            }
            ctx.wait(SYNC_INTERVAL);
        }
        for (_, (_, stop, handle)) in workers {
            stop.store(true, Ordering::Release);
            let _ = handle.join();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn application_contention_cannot_stop_durable_network_catchup() {
        let root = crate::admin::Temporary::new().unwrap();
        let ctx = Context {
            home: root.0.clone(),
            state: root.0.clone(),
            desired: root.0.join("fleet.json"),
            path: root.0.join("issues.db"),
            binary: std::env::current_exe().unwrap(),
            node: "a".repeat(32),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let db = crate::issues::Store::open(&ctx.path)
            .unwrap()
            .into_database();
        install(&db, &ctx.node).unwrap();
        drop(db);
        let lock = ctx.lock("distributed-sync.lock", true).unwrap().unwrap();
        let application = Application::start(&ctx, false, "peer");
        let mut local = engine(&ctx).unwrap();
        let mut peer = hey_boss_sync::Engine::open(&root.0.join("peer.db")).unwrap();
        for n in 0..400 {
            peer.put(&format!("offline-{n}")).unwrap();
        }
        let (mut a, mut b) = std::os::unix::net::UnixStream::pair().unwrap();
        for socket in [&a, &b] {
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
        }
        std::thread::scope(|scope| {
            let server = scope.spawn(|| {
                for _ in 0..5 {
                    hey_boss_sync::session::serve(&mut peer, &mut b.try_clone().unwrap(), &mut b)
                        .unwrap();
                }
            });
            for _ in 0..5 {
                let result = hey_boss_sync::session::connect_present(
                    &mut local,
                    &mut a.try_clone().unwrap(),
                    &mut a,
                    application.presence(),
                )
                .unwrap();
                application.received(result.presence, result.presence_age);
            }
            server.join().unwrap();
        });
        assert_eq!(local.head().unwrap(), 400);
        drop(application);
        drop(lock);
    }

    #[test]
    fn queued_presence_keeps_aging_without_database_access() {
        let pulse = hey_boss_sync::session::Presence {
            machine: "peer".into(),
            sequence: 1,
            attempts: vec!["attempt".into()],
            remaining_ms: 100,
        };
        let snapshot = PresenceSnapshot {
            pulses: vec![pulse],
            captured: Instant::now() - Duration::from_millis(60),
            age: Duration::from_millis(50),
        };
        assert!(
            snapshot.current().is_empty(),
            "Queued presence must not renew expired claims"
        );
    }
    #[test]
    fn idle_preparation_does_not_wait_for_the_application_writer() {
        let root = crate::admin::Temporary::new().unwrap();
        let ctx = Context {
            home: root.0.clone(),
            state: root.0.clone(),
            desired: root.0.join("fleet.json"),
            path: root.0.join("issues.db"),
            binary: std::env::current_exe().unwrap(),
            node: "a".repeat(32),
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let db = crate::issues::Store::open(&ctx.path)
            .unwrap()
            .into_database();
        install(&db, &ctx.node).unwrap();
        db.execute(
            "INSERT INTO distributed_members(node,slot,ready) VALUES(?1,0,1)",
            [&ctx.node],
        )
        .unwrap();
        let mut journal = engine(&ctx).unwrap();
        for _ in 0..32 {
            prepare(&ctx, &db, &mut journal, true).unwrap();
        }
        db.execute("UPDATE distributed_meta SET active=1", [])
            .unwrap();
        prepare(&ctx, &db, &mut journal, true).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM distributed_dirty", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(db);

        let mut owner = crate::database::Owner::start(&ctx.path).unwrap().unwrap();
        let writer = Connection::connect(&ctx.path).unwrap();
        let idle = Connection::connect(&ctx.path).unwrap();
        let transaction = writer.unchecked_transaction().unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let sync = std::thread::spawn(move || {
            sent.send(prepare(&ctx, &idle, &mut journal, true).map_err(|e| e.to_string()))
                .unwrap();
        });
        let completed = received.recv_timeout(Duration::from_secs(2));
        // Always release before asserting so a failed regression can join cleanly.
        transaction.rollback().unwrap();
        sync.join().unwrap();
        owner.stop();
        assert!(
            completed.is_ok(),
            "Idle sync queued behind an unrelated application edit"
        );
        completed.unwrap().unwrap();
    }

    #[test]
    fn peer_preparation_keeps_controller_and_baseline_ownership() {
        let root = crate::admin::Temporary::new().unwrap();
        let ctx = Context {
            home: root.0.clone(),
            state: root.0.clone(),
            desired: root.0.join("fleet.json"),
            path: root.0.join("issues.db"),
            binary: std::env::current_exe().unwrap(),
            node: "b".repeat(32),
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let db = crate::issues::Store::open(&ctx.path)
            .unwrap()
            .into_database();
        db.execute("UPDATE fleet_meta SET role='agent',node=?1", [&ctx.node])
            .unwrap();
        install(&db, &ctx.node).unwrap();
        db.execute(
            "UPDATE distributed_meta SET controller=?1",
            ["a".repeat(32)],
        )
        .unwrap();
        db.execute(
            "INSERT INTO distributed_members(node,slot,ready) VALUES(?1,0,0)",
            ["a".repeat(32)],
        )
        .unwrap();
        let mut journal = engine(&ctx).unwrap();
        for _ in 0..5 {
            prepare(&ctx, &db, &mut journal, false).unwrap();
        }
        assert_eq!(
            db.query_row("SELECT controller FROM distributed_meta", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "a".repeat(32)
        );
        assert_eq!(db.query_row("SELECT count(*) FROM distributed_baseline WHERE table_name NOT IN ('worker_runs','scheduled_job_runs','worker_allocations')",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(
            db.query_row("SELECT ready FROM distributed_members", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(journal);
    }
}
