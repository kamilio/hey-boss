//! Native fleet implementation. Protocol-v1 and durable filenames stay stable.
mod companion;
mod context;
mod control;
mod conversation;
mod mobile;
mod pull;
mod replica;
mod service;
mod supervisor;
mod takeover;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
use context::Context;
pub(super) use context::read_control_body;
use serde_json::{Value, json};
use std::{io::Write, os::unix::net::UnixStream, time::Duration};

pub(super) fn run(action: &super::Action) -> std::io::Result<()> {
    run_inner(action).map_err(std::io::Error::other)
}
fn save_upgrade_source(ctx: &Context, source: &std::path::Path) -> Result<()> {
    let source = source.canonicalize()?;
    if !source.join("Cargo.toml").is_file() {
        return Err(replica::invalid("Source must be a hey-boss checkout"));
    }
    let path = ctx.state.join("upgrade-source");
    ctx.protect_file(&path)?;
    std::fs::write(path, format!("{}\n", source.display()))?;
    Ok(())
}
fn run_inner(action: &super::Action) -> Result<()> {
    let ctx = Context::new()?;
    if matches!(
        action,
        super::Action::Supervisor | super::Action::Companion { install: false, .. }
    ) {
        let stop = ctx.stop.clone();
        ctrlc::set_handler(move || stop.store(true, std::sync::atomic::Ordering::Release))?;
    }
    match action {
        super::Action::Setup { source } => {
            if let Some(source) = source {
                save_upgrade_source(&ctx, source)?;
            }
            service::install(&ctx, "controller")?;
            println!(
                "Automatic fleet supervisor started. Workers view: http://127.0.0.1:4781/workers"
            );
            Ok(())
        }
        super::Action::Supervisor => supervisor::run(ctx),
        super::Action::Companion { stdio, install } => {
            if *install {
                service::ensure_companion(&ctx)
            } else if *stdio {
                companion::stdio(ctx)
            } else {
                companion::daemon(ctx)
            }
        }
        super::Action::Status => {
            println!(
                "{}",
                serde_json::to_string_pretty(&local_request(&ctx, json!({"kind":"status"}))?)?
            );
            Ok(())
        }
        super::Action::Signal {
            host,
            worker,
            signal,
        } => {
            println!(
                "{}",
                local_request(
                    &ctx,
                    json!({"kind":"signal","host":host,"worker":worker,"signal":signal})
                )?
            );
            Ok(())
        }
        #[allow(unreachable_patterns)]
        _ => unreachable!("Internal fleet command is handled before native dispatch"),
    }
}

fn local_request(ctx: &Context, value: Value) -> Result<Value> {
    let mut connection = UnixStream::connect(ctx.state.join("fleet.sock"))?;
    connection.set_read_timeout(Some(Duration::from_secs(15)))?;
    connection.set_write_timeout(Some(Duration::from_secs(15)))?;
    connection.write_all(value.to_string().as_bytes())?;
    connection.write_all(b"\n")?;
    connection.shutdown(std::net::Shutdown::Write)?;
    let bytes = read_control_body(&mut connection)?
        .ok_or_else(|| replica::invalid("Supervisor response exceeds limit"))?;
    let result: Value = serde_json::from_slice(&bytes)?;
    if result["ok"] == false {
        return Err(replica::invalid(
            result["error"].as_str().unwrap_or("Fleet request failed"),
        ));
    }
    Ok(result)
}

// The owner-private database transport also supports replica operations. This
// lets rolling-upgrade regression fixtures exercise the native implementation.
pub(super) fn replica_request(db: &rusqlite::Connection, request: &Value) -> Result<Value> {
    let node = request["node"].as_str().unwrap_or("");
    match request["replica"].as_str() {
        Some("capture") => {
            replica::install_capture(
                db,
                request["role"]
                    .as_str()
                    .ok_or_else(|| replica::invalid("Missing fleet role"))?,
                node,
            )?;
            Ok(Value::Null)
        }
        Some("snapshot") => replica::snapshot(db, node),
        Some("incremental") => replica::incremental(
            db,
            node,
            request["cursor"]
                .as_i64()
                .ok_or_else(|| replica::invalid("Invalid replica cursor"))?,
        ),
        Some("allocate") => {
            replica::allocate(
                db,
                node,
                request["workers"]
                    .as_array()
                    .ok_or_else(|| replica::invalid("Missing workers"))?,
            )?;
            Ok(Value::Null)
        }
        Some("accept") => Ok(json!(replica::accept_changes(
            db,
            node,
            request["changes"]
                .as_array()
                .ok_or_else(|| replica::invalid("Missing replica changes"))?
        )?)),
        Some("pull") => {
            replica::apply_pull(
                db,
                node,
                &request["payload"],
                request["receipts"]
                    .as_array()
                    .ok_or_else(|| replica::invalid("Missing receipts"))?,
            )?;
            Ok(Value::Null)
        }
        _ => Err(replica::invalid("Unknown replica operation")),
    }
}

#[cfg(test)]
mod tests {
    use super::context::tests::{assert_sqlite_locked, test_context};
    use super::*;
    use std::fs;

    #[test]
    fn setup_source_marker_aliases_cannot_truncate_sqlite_files() {
        let (root, ctx, store) = test_context();
        let source = root.join("source 🌍");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Cargo.toml"), "[package]\nname='test'\n").unwrap();
        let marker = ctx.state.join("upgrade-source");
        for suffix in ["", "-wal", "-shm"] {
            for symbolic in [false, true] {
                let target = root.join(format!("issues.db{suffix}"));
                if symbolic {
                    std::os::unix::fs::symlink(&target, &marker).unwrap();
                } else {
                    fs::hard_link(&target, &marker).unwrap();
                }
                let before = fs::metadata(&target).unwrap().len();
                let result = save_upgrade_source(&ctx, &source);
                assert_eq!(
                    fs::metadata(&target).unwrap().len(),
                    before,
                    "Source marker truncated SQLite file {suffix}"
                );
                assert!(result.unwrap_err().to_string().contains("must not alias"));
                assert_sqlite_locked(&ctx.path);
                fs::remove_file(&marker).unwrap();
            }
        }
        save_upgrade_source(&ctx, &source).unwrap();
        assert_eq!(
            fs::read_to_string(marker).unwrap(),
            format!("{}\n", source.canonicalize().unwrap().display())
        );
        assert_sqlite_locked(&ctx.path);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
