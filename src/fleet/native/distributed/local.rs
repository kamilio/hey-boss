//! Owning-node reads and controls remain available while the relay is offline.
use super::super::{authority, context::Context, control, conversation, takeover};
use super::*;
use serde_json::json;

pub(in crate::fleet::native) fn request(ctx: &Context, request: &Value) -> Result<Value> {
    match request["kind"].as_str() {
        Some("capabilities") => Ok(authority::capability_report(
            "local",
            authority::capabilities(),
            json!(Context::running_build()),
        )),
        Some("status" | "overview") => status(ctx, request),
        Some("signal" | "worker_signal" | "conversation" | "takeover" | "steer") => {
            let host = request["host"].as_str().unwrap_or("local");
            if !["local", &ctx.node, &crate::issues::identity::host()].contains(&host) {
                return Err(replica::invalid(
                    "This action belongs to another machine; connect to its owning node through SSH",
                ));
            }
            match request["kind"].as_str().unwrap() {
                "signal" | "worker_signal" => {
                    let mut result = control::apply_signal(ctx, request)?;
                    result["ok"] = json!(true);
                    Ok(result)
                }
                "takeover" => takeover::apply(
                    ctx,
                    request["run"]
                        .as_str()
                        .ok_or_else(|| replica::invalid("Missing run"))?,
                ),
                "steer" => takeover::steer(ctx, request),
                _ => {
                    let window: crate::agent_conversations::Window =
                        serde_json::from_value(request.clone())?;
                    conversation::page(
                        ctx,
                        request["run"]
                            .as_str()
                            .ok_or_else(|| replica::invalid("Missing run"))?,
                        &serde_json::to_value(window)?,
                    )
                }
            }
        }
        _ => Err(replica::invalid(
            "Use local worker commands for this operation on a distributed peer",
        )),
    }
}

fn status(ctx: &Context, request: &Value) -> Result<Value> {
    let view = request["view"].as_str().unwrap_or("summary");
    let limit = request["limit"].as_u64().unwrap_or(20) as usize;
    let offset = request["offset"].as_u64().unwrap_or(0) as usize;
    if !(1..=100).contains(&limit)
        || !matches!(
            view,
            "summary" | "machines" | "signals" | "conflicts" | "events"
        )
    {
        return Err(replica::invalid("Invalid status page"));
    }
    let sync = activation::status(ctx)?;
    let db = ctx.db()?;
    let machine = json!({"host":"local","hostname":crate::issues::identity::host(),"node":ctx.node,"role":"peer","state":"connected","build":Context::running_build(),"pending":sync["pending_local"],"conflicts":sync["conflicts"],"workers":ctx.workers()?});
    let records = match view {
        "signals" => replica::rows(
            &db,
            "SELECT * FROM fleet_signals ORDER BY created_at DESC,id LIMIT ?1 OFFSET ?2",
            &[json!(limit), json!(offset)],
        )?,
        "conflicts" => replica::rows(
            &db,
            "SELECT table_name,entity,reason FROM distributed_rejections ORDER BY table_name,entity LIMIT ?1 OFFSET ?2",
            &[json!(limit), json!(offset)],
        )?,
        "events" => vec![],
        _ => {
            if offset == 0 {
                vec![machine]
            } else {
                vec![]
            }
        }
    };
    let total: i64 = match view {
        "signals" => db.query_row("SELECT count(*) FROM fleet_signals", [], |r| r.get(0))?,
        "conflicts" => sync["conflicts"].as_i64().unwrap_or(0),
        "events" => 0,
        _ => 1,
    };
    let mut result = json!({"ok":true,"view":view,"authoritative":false,"node":ctx.node,"sync":sync,"coverage":{"machines":"local machine only; remote observations are available through the relay"},"counts":{"machines":1,"unresolved_conflicts":sync["conflicts"]},"page":{"offset":offset,"returned":records.len(),"total":total,"next_offset":(offset+records.len() < total as usize).then_some(offset+records.len())}});
    result[if view == "summary" {
        "machines"
    } else {
        "records"
    }] = json!(records);
    Ok(result)
}
