use super::super::context::Context;
use super::*;
use serde_json::json;
use std::{
    io::BufReader,
    process::{Command, Stdio},
    time::Duration,
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
    if baseline(db, engine, controller)? && controller {
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
    db.execute("DELETE FROM distributed_outbox WHERE clock IN (SELECT clock FROM distributed_outbox WHERE clock<=(SELECT exported FROM distributed_meta WHERE id=1) ORDER BY clock LIMIT 1000)", [])?;
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

fn exchange(ctx: &Context, host: &str) -> Result<()> {
    if !crate::health::remote::valid_host(host) {
        return Err(replica::invalid("Invalid sync host"));
    }
    let Some(_lock) = ctx.lock("distributed-sync.lock", false)? else {
        return Ok(());
    };
    let db = ctx.db()?;
    let mut engine = engine(ctx)?;
    prepare(ctx, &db, &mut engine, true)?;
    let mut command = Command::new("ssh");
    crate::ssh::reuse_connection(&mut command, &ctx.state, host);
    // The machine identity is validated by Context; it is a fixed hex string.
    if ctx.node.len() != 32 || !ctx.node.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(replica::invalid("Invalid local machine identity"));
    }
    let script = format!(
        "export PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH\"; exec hey-boss fleet sync --controller {}",
        ctx.node
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
    Ok(())
}

/// Shadow capture and exchange leave legacy scheduling intact until cutover.
pub(in crate::fleet::native) fn start(ctx: Context, controller: bool) {
    std::thread::spawn(move || {
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
                            if let Err(error) = exchange(&ctx, host) {
                                ctx.atomic_json(&ctx.state.join(format!("sync-{}.json",hey_boss_sync::digest(host.as_bytes()))),&json!({"host":host,"state":"disconnected","error":error.to_string()}))?;
                            } else {
                                ctx.atomic_json(&ctx.state.join(format!("sync-{}.json",hey_boss_sync::digest(host.as_bytes()))),&json!({"host":host,"state":"connected","at":super::super::context::now()}))?;
                            }
                        }
                    }
                } else if let Some(_lock) = ctx.lock("distributed-sync.lock", false)? {
                    let db = ctx.db()?;
                    let mut engine = engine(&ctx)?;
                    prepare(&ctx, &db, &mut engine, false)?;
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
