use super::super::context::Context;
use super::*;
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::BufReader,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Retries(BTreeMap<String, Instant>);

impl Retries {
    fn run(&mut self, host: &str, exchange: impl FnOnce() -> Result<bool>) -> Result<bool> {
        let now = Instant::now();
        self.0.retain(|_, deadline| *deadline > now);
        if self.0.contains_key(host) {
            return Ok(false);
        }
        let result = exchange();
        if result.is_err() {
            // An offline peer must not repeatedly hold the shared journal lock
            // for the entire SSH timeout. A busy local lock is not a failure.
            self.0
                .insert(host.into(), Instant::now() + Duration::from_secs(60));
        }
        result
    }
}

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

pub(in crate::fleet::native) fn serve(ctx: Context, controller: &str) -> Result<()> {
    if controller.len() != 32 || !controller.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(replica::invalid("Invalid controller identity"));
    }
    let Some(_lock) = ctx.lock("distributed-sync.lock", true)? else {
        return Err(replica::invalid("Sync is busy"));
    };
    let db = ctx.db()?;
    install(&db, &ctx.node)?;
    db.execute(
        "UPDATE distributed_meta SET controller=?1 WHERE id=1 AND (controller IS NULL OR active=0)",
        [controller],
    )?;
    let mut engine = engine(&ctx)?;
    prepare(&ctx, &db, &mut engine, false)?;
    let mut reader = std::io::stdin().lock();
    let mut writer = std::io::stdout().lock();
    for _ in 0..8 {
        hey_boss_sync::session::serve(&mut engine, &mut reader, &mut writer)?;
    }
    for _ in 0..8 {
        if import(&db, &engine)? == 0 {
            break;
        }
    }
    Ok(())
}

fn exchange(
    ctx: &Context,
    host: &str,
    peer: Option<(&super::super::peer_sync::Peer, &str)>,
) -> Result<bool> {
    if !crate::health::remote::valid_host(host) {
        return Err(replica::invalid("Invalid sync host"));
    }
    let Some(_lock) = ctx.lock("distributed-sync.lock", false)? else {
        return Ok(false);
    };
    let db = ctx.db()?;
    let mut engine = engine(ctx)?;
    prepare(ctx, &db, &mut engine, peer.is_none())?;
    let mut command = if let Some((peer, _)) = peer {
        super::super::peer_sync::ssh(ctx, peer)
    } else {
        let mut command = Command::new("ssh");
        crate::ssh::reuse_connection(&mut command, &ctx.state, host);
        command
    };
    let controller = peer.map(|(_, controller)| controller).unwrap_or(&ctx.node);
    // The machine identity is validated by Context; it is a fixed hex string.
    if controller.len() != 32 || !controller.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(replica::invalid("Invalid local machine identity"));
    }
    let script = format!(
        "export PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH\"; exec hey-boss fleet sync --controller {}",
        controller
    );
    let mut child = command
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            host,
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
    let result = std::thread::scope(|scope| {
        let (done, finished) = std::sync::mpsc::channel();
        let pid = child.id();
        scope.spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(40);
            loop {
                match finished.recv_timeout(Duration::from_millis(100)) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(_) => {}
                }
                if ctx.stopped() || std::time::Instant::now() >= deadline {
                    // Child remains unreaped until this watchdog joins, so its
                    // PID cannot have been recycled for an unrelated process.
                    unsafe {
                        libc::kill(pid as i32, libc::SIGKILL);
                    }
                    break;
                }
            }
        });
        let result = (|| -> Result<()> {
            for _ in 0..8 {
                hey_boss_sync::session::connect(&mut engine, &mut reader, &mut writer)?;
            }
            Ok(())
        })();
        let _ = done.send(());
        result
    });
    drop(writer);
    drop(reader);
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    result?;
    if !status.success() {
        return Err(replica::invalid("SSH sync peer exited unsuccessfully"));
    }
    for _ in 0..8 {
        if import(&db, &engine)? == 0 {
            break;
        }
    }
    Ok(true)
}

/// Shadow capture and exchange leave legacy scheduling intact until cutover.
pub(in crate::fleet::native) fn start(ctx: Context, controller: bool) {
    std::thread::spawn(move || {
        let mut retries = Retries::default();
        while !ctx.stopped() {
            let result = (|| -> Result<()> {
                if controller {
                    if let Some(_lock) = ctx.lock("distributed-sync.lock", false)? {
                        let db = ctx.db()?;
                        let mut engine = engine(&ctx)?;
                        prepare(&ctx, &db, &mut engine, true)?;
                    }
                    let hosts = ctx.inventory()?;
                    for machine in hosts {
                        if let Some(host) = machine["host"].as_str().filter(|h| *h != "local") {
                            match retries.run(host, || exchange(&ctx, host, None)) {
                                Err(error) => {
                                    ctx.atomic_json(&ctx.state.join(format!("sync-{}.json",hey_boss_sync::digest(host.as_bytes()))),&json!({"host":host,"state":"disconnected","error":error.to_string()}))?;
                                }
                                Ok(true) => {
                                    ctx.atomic_json(&ctx.state.join(format!("sync-{}.json",hey_boss_sync::digest(host.as_bytes()))),&json!({"host":host,"state":"connected","at":super::super::context::now()}))?;
                                }
                                Ok(false) => {}
                            }
                        }
                    }
                } else if let Some(_lock) = ctx.lock("distributed-sync.lock", false)? {
                    let db = ctx.db()?;
                    let mut engine = engine(&ctx)?;
                    prepare(&ctx, &db, &mut engine, false)?;
                }
                // One initiator per pair keeps the graph acyclic and avoids two
                // peers each holding their journal lock while dialing the other.
                // The cached plan survives loss of the controller connection.
                if !controller && let Some(settings) = super::super::peer_sync::cached(&ctx)? {
                    for peer in settings
                        .peers
                        .iter()
                        .filter(|peer| settings.enabled && settings.machine < peer.machine)
                    {
                        let state = match retries.run(&peer.machine, || {
                            exchange(&ctx, &peer.address, Some((peer, &settings.controller)))
                        }) {
                            Ok(false) => continue,
                            Ok(true) => {
                                json!({"state":"connected","at":super::super::context::now()})
                            }
                            Err(error) => {
                                json!({"state":"disconnected","error":error.to_string(),"at":super::super::context::now()})
                            }
                        };
                        ctx.atomic_json(
                            &super::super::peer_sync::diagnostic_path(&ctx, &peer.machine),
                            &state,
                        )?;
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!("Distributed sync: {error}");
            }
            ctx.wait(Duration::from_secs(2));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn unavailable_peer_waits_while_healthy_and_busy_peers_keep_syncing() {
        let mut retries = Retries::default();
        assert!(
            retries
                .run("offline", || Err(replica::invalid("Disconnected")))
                .is_err()
        );
        assert!(
            !retries
                .run("offline", || panic!(
                    "An unavailable peer must not consume another timeout immediately"
                ))
                .unwrap()
        );
        assert!(retries.run("healthy", || Ok(true)).unwrap());
        assert!(!retries.run("busy", || Ok(false)).unwrap());
        assert!(
            retries.run("busy", || Ok(true)).unwrap(),
            "A held sync lock is not a connection failure"
        );
        let deadline = *retries.0.get("offline").unwrap();
        assert!(deadline > Instant::now());
        assert!(deadline <= Instant::now() + Duration::from_secs(60));
        retries
            .0
            .insert("offline".into(), Instant::now() - Duration::from_secs(1));
        assert!(retries.run("offline", || Ok(true)).unwrap());
        assert!(
            retries.run("offline", || Ok(true)).unwrap(),
            "Recovered peers resume normal synchronization"
        );
        assert!(!retries.0.contains_key("offline"));
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
        assert_eq!(db.query_row("SELECT count(*) FROM distributed_baseline WHERE table_name NOT IN ('worker_runs','scheduled_job_runs')",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(
            db.query_row("SELECT ready FROM distributed_members", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(journal);
        let _held = ctx.lock("distributed-sync.lock", false).unwrap().unwrap();
        assert!(!exchange(&ctx, "unused.invalid", None).unwrap());
    }
}
