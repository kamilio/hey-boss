//! Recovery belongs to the owning daemon, independently of HTTP and heartbeats.
use super::context::Context;
use std::time::Duration;

pub(super) fn start(ctx: Context) {
    std::thread::spawn(move || {
        let home = std::env::var_os("CODEX_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| ctx.home.join(".codex"));
        let mut recovery = crate::issues::model_recovery::Recovery::default();
        while !ctx.stopped() {
            let result = ctx
                .db()
                .and_then(|db| Ok(recovery.step(&db, &ctx.node, &home)?));
            let delay = match result {
                Ok(true) => {
                    recovery = Default::default();
                    Duration::from_secs(3600)
                }
                Ok(false) => Duration::from_millis(250),
                Err(error) => {
                    eprintln!("Agent model recovery: {error}");
                    // Restart the page after transient DB/filesystem errors.
                    recovery = Default::default();
                    Duration::from_secs(60)
                }
            };
            ctx.wait(delay);
        }
    });
}
