//! Report applied data separately from an open SSH connection.
use super::super::context::{Context, now};
use super::*;
use crate::issues::allocations::Clock;
use serde_json::json;

const FRESH_MS: i64 = 30_000;

fn classify(sample: &Value, exchange: &Value, boot: &str, tick: i64) -> Value {
    let age = |v: &Value| {
        v["tick"]
            .as_i64()
            .filter(|t| v["boot"] == boot && *t <= tick)
            .map(|t| tick - t)
    };
    let mut result = sample.clone();
    if !result.is_object() {
        result = json!({});
    }
    let complete = [
        "pending_local",
        "pending_projection",
        "conflicts",
        "baseline_remaining",
        "pending_archives",
        "projected",
        "journal_head",
        "pending_reservations",
        "conflicting_attempts",
    ]
    .iter()
    .all(|key| sample[*key].as_u64().is_some());
    let state = if age(sample).is_none_or(|age| age > FRESH_MS) {
        "unknown"
    } else if sample["error"].is_string() {
        if sample["retrying"] == true {
            "waiting"
        } else {
            "error"
        }
    } else if !complete {
        "unknown"
    } else if sample["active"] != 1 {
        "shadow"
    } else if sample["conflicts"].as_u64().unwrap_or(0) > 0 {
        "rejected"
    } else if sample["pending_local"].as_u64().unwrap_or(0) > 0
        || sample["pending_projection"].as_u64().unwrap_or(0) > 0
        || sample["baseline_remaining"].as_u64().unwrap_or(0) > 0
        || sample["pending_archives"].as_u64().unwrap_or(0) > 0
        || sample["pending_reservations"].as_u64().unwrap_or(0) > 0
        || sample["conflicting_attempts"].as_u64().unwrap_or(0) > 0
        || sample["projected"] != sample["journal_head"]
        || exchange["incoming_drained"] == false
        || exchange["outgoing_pending"].as_u64().unwrap_or(0) > 0
        || exchange["journal_head"]
            .as_u64()
            .is_some_and(|head| head > sample["projected"].as_u64().unwrap_or(0))
    {
        "catching_up"
    } else if age(exchange).is_none_or(|age| age > FRESH_MS) {
        "disconnected"
    } else if exchange["incoming_drained"] != true {
        "unknown"
    } else {
        "healthy"
    };
    result["state"] = json!(state);
    result["age_ms"] = json!(age(sample));
    result["last_exchange"] = exchange.get("at").cloned().unwrap_or(Value::Null);
    result["exchange_age_ms"] = json!(age(exchange));
    result["pending_peers"] = exchange.get("pending_peers").cloned().unwrap_or(json!([]));
    result["outgoing_pending"] = exchange
        .get("outgoing_pending")
        .cloned()
        .unwrap_or(Value::Null);
    result["message"] = json!(match state {
        "healthy" => "Changes are applied and sync is responding",
        "shadow" => "Sync migration is not active; received changes are not visible",
        "rejected" => "Some received changes could not be applied",
        "catching_up" if sample["pending_reservations"].as_u64().unwrap_or(0) > 0 =>
            "Worker reservations are still synchronizing; new agents are waiting",
        "catching_up" if sample["conflicting_attempts"].as_u64().unwrap_or(0) > 0 =>
            "Workers are stopping attempts owned by another machine",
        "catching_up" => "Changes are waiting to be applied or sent",
        "disconnected" => "No recent completed sync exchange; local work is retained",
        "error" => "Sync could not prepare or apply changes",
        "waiting" => "Database busy; data sync will retry",
        _ => "Sync progress has not been verified recently",
    });
    result
}

pub(in crate::fleet::native) fn observed(machine: &Value, at: f64) -> Value {
    let mut value = machine
        .get("sync")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if machine["host"] != "local"
        && machine["sync_observed_at"]
            .as_f64()
            .is_none_or(|seen| seen > at || at - seen > 30.0)
    {
        value["state"] = json!("unknown");
        value["message"] = json!("This machine has not reported recent data sync progress");
    }
    value
}

pub(in crate::fleet::native) fn cached(ctx: &Context) -> Result<Value> {
    let clock = Clock::read()?;
    let sample = ctx
        .read_json(&ctx.state.join("distributed-health.json"), Value::Null)
        .unwrap_or(Value::Null);
    let mut peers = Vec::new();
    let directory = ctx.state.join("distributed-peers");
    if directory.exists() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                peers.push(
                    ctx.read_json(&entry.path(), Value::Null)
                        .unwrap_or(Value::Null),
                );
            }
        }
    }
    let exchange = aggregate(
        &peers,
        &clock.boot,
        clock.tick,
        sample["journal_head"].as_i64().unwrap_or(0),
    );
    let mut result = classify(&sample, &exchange, &clock.boot, clock.tick);
    let mut failures = Vec::new();
    // A failed route stays failed during retry backoff. Only a successful
    // exchange clears it; the transport removes diagnostics for retired routes.
    // This cached read must not depend on the database or configuration locks.
    for entry in std::fs::read_dir(&ctx.state)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| {
            (name.starts_with("sync-") || name.starts_with("peer-sync-")) && name.ends_with(".json")
        }) else {
            continue;
        };
        let route = ctx
            .read_json(&ctx.state.join(name), Value::Null)
            .unwrap_or(Value::Null);
        if route["boot"] == clock.boot
            && route["tick"]
                .as_i64()
                .is_some_and(|tick| tick <= clock.tick)
            && route["state"] == "disconnected"
            && let Some(error) = route["error"].as_str()
        {
            failures.push(format!(
                "{}: {error}",
                route["host"].as_str().unwrap_or("Peer")
            ));
        }
    }
    if !failures.is_empty() {
        failures.sort();
        if let Some(error) = result["error"].as_str() {
            failures.insert(0, error.to_owned());
        }
        result["state"] = json!("error");
        result["message"] = json!(failures.join("\n"));
    }
    Ok(result)
}

fn aggregate(peers: &[Value], boot: &str, tick: i64, local_head: i64) -> Value {
    let peers = peers
        .iter()
        .filter(|p| p["boot"] == boot && p["tick"].as_i64().is_some_and(|t| t <= tick))
        .collect::<Vec<_>>();
    // A responding peer cannot refresh the freshness of a different route.
    let Some(oldest) = peers.iter().min_by_key(|p| p["tick"].as_i64()) else {
        return Value::Null;
    };
    let mut result = (*oldest).clone();
    let mut pending = Vec::new();
    let mut outgoing = 0_i64;
    let mut head = local_head;
    let mut drained = true;
    for peer in peers {
        let peer_head = peer["journal_head"].as_i64().unwrap_or(0);
        let sent = peer_head.saturating_sub(peer["outgoing_pending"].as_i64().unwrap_or(0));
        let remaining = local_head.max(peer_head).saturating_sub(sent);
        outgoing = outgoing.saturating_add(remaining);
        head = head.max(peer_head);
        drained &= peer["incoming_drained"] == true;
        if remaining > 0 || peer["incoming_drained"] != true {
            pending.push(peer["peer"].clone());
        }
    }
    result["incoming_drained"] = json!(drained);
    result["outgoing_pending"] = json!(outgoing);
    result["journal_head"] = json!(head);
    result["pending_peers"] = json!(pending);
    result
}

pub(super) fn prepared(
    ctx: &Context,
    db: &Connection,
    engine: &hey_boss_sync::Engine,
) -> Result<()> {
    let mut sample=replica::rows(db,"SELECT active,exported,projected,(SELECT count(*) FROM distributed_outbox WHERE clock>m.exported) AS pending_local,(SELECT count(*) FROM distributed_dirty) AS pending_projection,(SELECT count(*) FROM distributed_rejections) AS conflicts,(SELECT count(*) FROM distributed_baseline WHERE complete=0) AS baseline_remaining,(SELECT count(*) FROM distributed_archives WHERE complete=0) AS pending_archives FROM distributed_meta m WHERE id=1",&[])?.remove(0);
    sample["journal_head"] = json!(engine.head()?);
    sample["pending_reservations"] = json!(crate::issues::allocations::pending_reservations(db)?);
    sample["conflicting_attempts"] = json!(crate::issues::allocations::conflicting_attempts(db)?);
    save(ctx, "distributed-health.json", sample)
}

pub(super) fn failed(ctx: &Context, error: &str) {
    let _ = save(ctx, "distributed-health.json", json!({"error":error}));
}

pub(super) fn waiting(ctx: &Context, error: &str) {
    let _ = save(
        ctx,
        "distributed-health.json",
        json!({"error":error,"retrying":true}),
    );
}

pub(super) fn peer_exchanged(
    ctx: &Context,
    exchange: &hey_boss_sync::session::Exchange,
) -> Result<()> {
    save(
        ctx,
        &format!("distributed-peers/{}.json", exchange.peer),
        json!({"peer":exchange.peer,"incoming_drained":exchange.incoming_drained,"outgoing_pending":exchange.outgoing_pending,"journal_head":exchange.journal_head}),
    )
}

pub(super) fn save(
    ctx: &Context,
    name: impl AsRef<std::path::Path>,
    mut value: Value,
) -> Result<()> {
    let clock = Clock::read()?;
    value["boot"] = json!(clock.boot);
    value["tick"] = json!(clock.tick);
    value["at"] = json!(now());
    ctx.atomic_json(&ctx.state.join(name), &value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Value {
        json!({"boot":"boot","tick":100,"active":1,"pending_local":0,"pending_projection":0,"conflicts":0,"baseline_remaining":0,"pending_archives":0,"projected":7,"journal_head":7,"pending_reservations":0,"conflicting_attempts":0})
    }
    #[test]
    fn transport_backlog_and_new_receipts_prevent_false_health() {
        let s = sample();
        let mut exchange = json!({"boot":"boot","tick":100,"at":1,"incoming_drained":false,"outgoing_pending":0,"journal_head":7});
        assert_eq!(classify(&s, &exchange, "boot", 101)["state"], "catching_up");
        exchange["incoming_drained"] = json!(true);
        exchange["outgoing_pending"] = json!(2);
        assert_eq!(classify(&s, &exchange, "boot", 101)["state"], "catching_up");
        exchange["outgoing_pending"] = json!(0);
        exchange["journal_head"] = json!(8);
        assert_eq!(classify(&s, &exchange, "boot", 101)["state"], "catching_up");
        exchange["journal_head"] = json!(7);
        assert_eq!(classify(&s, &exchange, "boot", 101)["state"], "healthy");
    }
    #[test]
    fn a_healthy_peer_does_not_hide_another_peers_backlog() {
        let mut peers = vec![
            json!({"boot":"boot","tick":100,"peer":"behind","incoming_drained":false,"outgoing_pending":0,"journal_head":7}),
            json!({"boot":"boot","tick":101,"peer":"ready","incoming_drained":true,"outgoing_pending":0,"journal_head":7}),
        ];
        let exchange = aggregate(&peers, "boot", 102, 7);
        assert_eq!(
            classify(&sample(), &exchange, "boot", 102)["state"],
            "catching_up"
        );
        assert_eq!(exchange["pending_peers"], json!(["behind"]));
        peers[0]["incoming_drained"] = json!(true);
        assert_eq!(
            classify(&sample(), &aggregate(&peers, "boot", 102, 7), "boot", 102)["state"],
            "healthy"
        );
        assert_eq!(aggregate(&peers, "boot", 102, 8)["outgoing_pending"], 2);
        assert!(aggregate(&peers, "next-boot", 102, 7).is_null());
        peers[0]["tick"] = json!(0);
        peers[1]["tick"] = json!(31_000);
        let mut fresh = sample();
        fresh["tick"] = json!(31_000);
        assert_eq!(
            classify(
                &fresh,
                &aggregate(&peers, "boot", 31_001, 7),
                "boot",
                31_001
            )["state"],
            "disconnected"
        );
    }
    #[test]
    fn connection_alone_cannot_hide_shadow_mode_or_unapplied_data() {
        let exchange = json!({"boot":"boot","tick":100,"at":1,"incoming_drained":true});
        let mut s = sample();
        assert_eq!(classify(&s, &exchange, "boot", 101)["state"], "healthy");
        for key in ["pending_reservations", "conflicting_attempts"] {
            s[key] = json!(1);
            assert_eq!(classify(&s, &exchange, "boot", 101)["state"], "catching_up");
            s[key] = json!(0);
        }
        s["active"] = json!(0);
        assert_eq!(classify(&s, &exchange, "boot", 101)["state"], "shadow");
        s["active"] = json!(1);
        s["pending_projection"] = json!(2);
        assert_eq!(classify(&s, &exchange, "boot", 101)["state"], "catching_up");
        s["pending_projection"] = json!(0);
        s["pending_archives"] = json!(1);
        assert_eq!(classify(&s, &exchange, "boot", 101)["state"], "catching_up");
        s["conflicts"] = json!(1);
        assert_eq!(classify(&s, &exchange, "boot", 101)["state"], "rejected");
        assert_eq!(
            classify(&Value::Null, &exchange, "boot", 101)["state"],
            "unknown"
        );
    }
    #[test]
    fn a_dead_sync_loop_and_a_reboot_cannot_keep_a_green_status() {
        let s = sample();
        assert_eq!(classify(&s, &s, "boot", 31_000)["state"], "unknown");
        assert_eq!(classify(&s, &s, "new-boot", 101)["state"], "unknown");
        assert_eq!(classify(&s, &s, "boot", 99)["state"], "unknown");
        assert_eq!(
            classify(&s, &Value::Null, "boot", 101)["state"],
            "disconnected"
        );
        assert_eq!(
            classify(&s, &json!({"boot":"boot","tick":0}), "boot", 30_050)["state"],
            "disconnected"
        );
    }
    #[test]
    fn a_received_journal_is_not_the_same_as_applied_changes() {
        let mut s = sample();
        s["journal_head"] = json!(8);
        assert_eq!(classify(&s, &s, "boot", 101)["state"], "catching_up");
        s["error"] = json!("disk full");
        assert_eq!(classify(&s, &s, "boot", 101)["state"], "error");
    }
    #[test]
    fn retryable_contention_is_waiting_until_fresh_application_progress() {
        let mut sample = sample();
        let exchange = json!({"boot":"boot","tick":100,"incoming_drained":true,"journal_head":7});
        sample["error"] = json!("Database writer queue timed out before execution");
        sample["retrying"] = json!(true);
        assert_eq!(
            classify(&sample, &exchange, "boot", 101)["state"],
            "waiting"
        );
        assert_eq!(
            classify(&sample, &exchange, "boot", 101)["message"],
            "Database busy; data sync will retry"
        );
        assert_eq!(
            classify(&sample, &exchange, "boot", 31_000)["state"],
            "unknown"
        );
        sample.as_object_mut().unwrap().remove("error");
        sample.as_object_mut().unwrap().remove("retrying");
        assert_eq!(
            classify(&sample, &exchange, "boot", 101)["state"],
            "healthy"
        );
    }
    #[test]
    fn corrupt_diagnostics_do_not_break_the_fleet_dashboard() {
        let root = crate::admin::Temporary::new().unwrap();
        let ctx = Context {
            home: root.0.clone(),
            state: root.0.clone(),
            desired: root.0.join("fleet.yaml"),
            path: root.0.join("issues.db"),
            binary: std::env::current_exe().unwrap(),
            node: "a".repeat(32),
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        std::fs::write(ctx.state.join("distributed-health.json"), "{").unwrap();
        assert_eq!(cached(&ctx).unwrap()["state"], "unknown");
    }
    #[test]
    fn transport_rejection_is_visible_until_recovery_even_with_a_healthy_peer() {
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
        ctx.atomic_json(
            &ctx.home.join(".hey-boss/config.json"),
            &json!({"ssh_hosts":["old-mac", "ready"]}),
        )
        .unwrap();
        let failed_path = ctx
            .state
            .join(format!("sync-{}.json", hey_boss_sync::digest(b"old-mac")));
        let ready_path = ctx
            .state
            .join(format!("sync-{}.json", hey_boss_sync::digest(b"ready")));
        save(&ctx, "distributed-health.json", sample()).unwrap();
        save(
            &ctx,
            "distributed-peers/ready.json",
            json!({"peer":"ready","incoming_drained":true,"outgoing_pending":0,"journal_head":7}),
        )
        .unwrap();
        save(
            &ctx,
            &failed_path,
            json!({"host":"old-mac","state":"disconnected","error":"Incompatible sync protocol"}),
        )
        .unwrap();
        save(
            &ctx,
            &ready_path,
            json!({"host":"ready","state":"connected"}),
        )
        .unwrap();
        let failed = cached(&ctx).unwrap();
        assert_eq!(failed["state"], "error");
        assert!(
            failed["message"]
                .as_str()
                .unwrap()
                .contains("old-mac: Incompatible sync protocol")
        );
        let mut retrying = ctx.read_json(&failed_path, Value::Null).unwrap();
        retrying["tick"] = json!(0);
        ctx.atomic_json(&failed_path, &retrying).unwrap();
        assert_eq!(
            cached(&ctx).unwrap()["state"],
            "error",
            "Retry backoff cannot erase the rejection"
        );
        save(
            &ctx,
            &failed_path,
            json!({"host":"old-mac","state":"connected"}),
        )
        .unwrap();
        assert_eq!(cached(&ctx).unwrap()["state"], "healthy");
        ctx.atomic_json(&failed_path, &json!({"boot":"previous-boot","tick":0,"host":"old-mac","state":"disconnected","error":"old failure"})).unwrap();
        assert_eq!(cached(&ctx).unwrap()["state"], "healthy");
    }
    #[test]
    fn heartbeat_age_and_incomplete_samples_fail_closed() {
        assert_eq!(
            observed(
                &json!({"host":"peer","sync":{"state":"healthy"},"sync_observed_at":1}),
                32.0
            )["state"],
            "unknown"
        );
        let mut s = sample();
        s.as_object_mut().unwrap().remove("pending_local");
        assert_eq!(classify(&s, &s, "boot", 101)["state"], "unknown");
    }
    #[test]
    fn retained_cold_history_does_not_mean_archive_transfer_is_pending() {
        let root = crate::admin::Temporary::new().unwrap();
        let ctx = Context {
            home: root.0.clone(),
            state: root.0.clone(),
            desired: root.0.join("fleet.yaml"),
            path: root.0.join("issues.db"),
            binary: std::env::current_exe().unwrap(),
            node: "a".repeat(32),
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let db = crate::issues::Store::open(&ctx.path)
            .unwrap()
            .into_database();
        install(&db, &ctx.node).unwrap();
        db.execute_batch("UPDATE distributed_meta SET active=1; UPDATE distributed_baseline SET complete=1; INSERT INTO distributed_archive_history(key,project_id,number) VALUES('cold','p',1);").unwrap();
        let engine = hey_boss_sync::Engine::open(&ctx.path.with_extension("sync.db")).unwrap();
        prepared(&ctx, &db, &engine).unwrap();
        save(
            &ctx,
            "distributed-peers/test.json",
            json!({"incoming_drained":true,"outgoing_pending":0,"journal_head":0}),
        )
        .unwrap();
        assert_eq!(cached(&ctx).unwrap()["state"], "healthy");
    }
}
