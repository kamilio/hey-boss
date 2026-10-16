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
    ]
    .iter()
    .all(|key| sample[*key].as_u64().is_some());
    let state = if age(sample).is_none_or(|age| age > FRESH_MS) {
        "unknown"
    } else if sample["error"].is_string() {
        "error"
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
        || sample["projected"] != sample["journal_head"]
    {
        "catching_up"
    } else if age(exchange).is_none_or(|age| age > FRESH_MS) {
        "disconnected"
    } else {
        "healthy"
    };
    result["state"] = json!(state);
    result["age_ms"] = json!(age(sample));
    result["last_exchange"] = exchange.get("at").cloned().unwrap_or(Value::Null);
    result["exchange_age_ms"] = json!(age(exchange));
    result["message"] = json!(match state {
        "healthy" => "Changes are applied and sync is responding",
        "shadow" => "Sync migration is not active; received changes are not visible",
        "rejected" => "Some received changes could not be applied",
        "catching_up" => "Changes are waiting to be applied or sent",
        "disconnected" => "No recent completed sync exchange; local work is retained",
        "error" => "Sync could not prepare or apply changes",
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
    Ok(classify(
        &ctx.read_json(&ctx.state.join("distributed-health.json"), Value::Null)
            .unwrap_or(Value::Null),
        &ctx.read_json(&ctx.state.join("distributed-exchange.json"), Value::Null)
            .unwrap_or(Value::Null),
        &clock.boot,
        clock.tick,
    ))
}

pub(super) fn prepared(
    ctx: &Context,
    db: &Connection,
    engine: &hey_boss_sync::Engine,
) -> Result<()> {
    let mut sample=replica::rows(db,"SELECT active,exported,projected,(SELECT count(*) FROM distributed_outbox WHERE clock>m.exported) AS pending_local,(SELECT count(*) FROM distributed_dirty) AS pending_projection,(SELECT count(*) FROM distributed_rejections) AS conflicts,(SELECT count(*) FROM distributed_baseline WHERE complete=0) AS baseline_remaining,(SELECT count(*) FROM distributed_archives WHERE complete=0) AS pending_archives FROM distributed_meta m WHERE id=1",&[])?.remove(0);
    sample["journal_head"] = json!(engine.head()?);
    save(ctx, "distributed-health.json", sample)
}

pub(super) fn failed(ctx: &Context, error: &str) {
    let _ = save(ctx, "distributed-health.json", json!({"error":error}));
}

pub(super) fn exchanged(ctx: &Context) -> Result<()> {
    save(ctx, "distributed-exchange.json", json!({}))
}

fn save(ctx: &Context, name: &str, mut value: Value) -> Result<()> {
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
        json!({"boot":"boot","tick":100,"active":1,"pending_local":0,"pending_projection":0,"conflicts":0,"baseline_remaining":0,"pending_archives":0,"projected":7,"journal_head":7})
    }
    #[test]
    fn connection_alone_cannot_hide_shadow_mode_or_unapplied_data() {
        let exchange = json!({"boot":"boot","tick":100,"at":1});
        let mut s = sample();
        assert_eq!(classify(&s, &exchange, "boot", 101)["state"], "healthy");
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
        exchanged(&ctx).unwrap();
        assert_eq!(cached(&ctx).unwrap()["state"], "healthy");
    }
}
