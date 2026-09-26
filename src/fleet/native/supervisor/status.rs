//! Read-only CLI projections. Never construct the historical snapshot to summarize it.
use super::*;

fn page(offset: usize, returned: usize, total: usize) -> Value {
    json!({"offset":offset,"returned":returned,"total":total,
        "next_offset":(offset.saturating_add(returned) < total).then_some(offset + returned)})
}

// Unknown is null, not a healthy default. Truncation is declared in coverage.
fn fields(value: &Value, names: &[&str]) -> Value {
    Value::Object(
        names
            .iter()
            .map(|name| {
                let value = match &value[*name] {
                    Value::String(s) => json!(s.chars().take(240).collect::<String>()),
                    Value::Bool(_) | Value::Number(_) | Value::Null => value[*name].clone(),
                    _ => Value::Null,
                };
                ((*name).to_owned(), value)
            })
            .collect(),
    )
}

fn machine(value: &Value, workers: Option<&Vec<Value>>) -> Value {
    let mut result = fields(
        value,
        &[
            "host",
            "hostname",
            "node",
            "role",
            "state",
            "heartbeat",
            "last_sync",
            "pending",
            "conflicts",
            "build",
            "deployment",
            "error",
            "deployment_error",
            "configuration_error",
            "desired_revision",
            "applied_revision",
            "retry_deploy_at",
        ],
    );
    if result["role"] == "agent" {
        result["role"] = json!("companion");
    }
    result["worker_count"] = json!(workers.map(Vec::len));
    result["workers_omitted"] = json!(workers.map(|w| w.len().saturating_sub(20)));
    result["workers"] = workers
        .map(|workers| {
            json!(
                workers
                    .iter()
                    .take(20)
                    .map(|w| {
                        let mut row = fields(
                            w,
                            &[
                                "id",
                                "kind",
                                "intent",
                                "state",
                                "pid",
                                "build",
                                "active",
                                "free",
                                "eligible",
                                "upgrading",
                                "updated_at",
                                "error",
                            ],
                        );
                        row["enabled"] = w["config"]["enabled"]
                            .as_bool()
                            .map(Value::Bool)
                            .unwrap_or(Value::Null);
                        row["concurrency"] = w["config"]["concurrency"]
                            .as_u64()
                            .map(|v| json!(v))
                            .unwrap_or(Value::Null);
                        row
                    })
                    .collect::<Vec<_>>()
            )
        })
        .unwrap_or(Value::Null);
    result
}

impl Supervisor {
    pub(super) fn status_request(&self, request: &Value) -> Result<Value> {
        let Some(view) = request.get("view") else {
            return self.status();
        };
        let view = view
            .as_str()
            .ok_or_else(|| invalid("Invalid status view"))?;
        if !matches!(
            view,
            "summary" | "machines" | "conflicts" | "signals" | "events"
        ) {
            return Err(invalid("Unknown status view"));
        }
        let number = |key: &str, default: u64| -> Result<usize> {
            let n = request
                .get(key)
                .map(|v| v.as_u64().ok_or_else(|| invalid("Invalid status page")))
                .transpose()?
                .unwrap_or(default);
            Ok(u32::try_from(n)? as usize)
        };
        let limit = number("limit", 20)?;
        let offset = number("offset", 0)?;
        if !(1..=100).contains(&limit) {
            return Err(invalid("Status limit must be 1–100"));
        }
        let db = self.ctx.db()?;
        // One read transaction keeps database counts and their page consistent.
        let tx = db.read_transaction()?;
        let count = |sql: &str| -> Result<usize> {
            Ok(db.query_row(sql, [], |r| r.get::<_, i64>(0))? as usize)
        };
        let conflicts = count("SELECT count(*) FROM fleet_conflicts WHERE resolved=0")?;
        let signals = count("SELECT count(*) FROM fleet_signals")?;
        let pending_signals = count(
            "SELECT count(*) FROM fleet_signals WHERE state IN ('pending','stopping','starting')",
        )?;
        let mut result = json!({"ok":true,"view":view,"authoritative":true,"supervisor":self.ctx.node,
            "coverage":{"machines":"last reported state; disconnected machines may be stale","conflicts":"all unresolved supervisor conflicts; companion counts are reported separately","events":"retained supervisor memory only; not complete history","signals":"all supervisor records","pages":"live offset pages may shift between reads","summary_text_limit":240,"workers_per_machine":20}});
        // Only diagnostic selection reads saved changes; no substring truncation.
        if matches!(view, "conflicts" | "signals") {
            let sql = if view == "conflicts" {
                "SELECT id,node,seq,table_name,data AS saved_change,reason,created_at,resolved FROM fleet_conflicts WHERE resolved=0 ORDER BY created_at DESC,id DESC LIMIT ? OFFSET ?"
            } else {
                "SELECT * FROM fleet_signals ORDER BY created_at DESC,id DESC LIMIT ? OFFSET ?"
            };
            let records = replica::rows(&db, sql, &[json!(limit), json!(offset)])?;
            result["page"] = page(
                offset,
                records.len(),
                if view == "conflicts" {
                    conflicts
                } else {
                    signals
                },
            );
            result["records"] = json!(records);
        }
        tx.commit()?;
        let state = self.state.lock().unwrap();
        let total = 1 + state
            .machines
            .keys()
            .filter(|h| h.as_str() != "local")
            .count();
        result["epoch"] = json!(state.epoch);
        result["capabilities"] = authority::capabilities();
        result["sequence"] = json!(state.sequence);
        result["desired_build"] =
            fields(&json!({"build":state.desired_build}), &["build"])["build"].clone();
        result["counts"] = json!({"machines":total,"unresolved_conflicts":conflicts,"signals":signals,"pending_signals":pending_signals,"retained_events":state.events.len()});
        // Omitted/disconnected machines must not hide pending changes or conflicts.
        // Report known sums with unknown-machine counts; never silently add null as zero.
        for (source, target) in [
            ("pending", "reported_pending_changes"),
            ("conflicts", "reported_companion_conflicts"),
        ] {
            let mut known = 0u64;
            let mut unknown = 0usize;
            for (_, m) in state.machines.iter().filter(|(h, _)| h.as_str() != "local") {
                if let Some(n) = m[source].as_u64() {
                    known = known.saturating_add(n);
                } else {
                    unknown += 1;
                }
            }
            result["counts"][target] = json!({"known":known,"unknown_machines":unknown});
        }
        if view == "events" {
            let records: Vec<_> = state
                .events
                .iter()
                .rev()
                .skip(offset)
                .take(limit)
                .cloned()
                .collect();
            result["page"] = page(offset, records.len(), state.events.len());
            result["records"] = json!(records);
        }
        if matches!(view, "summary" | "machines") {
            // Supervisor changes are already authoritative. Its outbox is a
            // replication journal, not a queue awaiting supervisor acceptance.
            let local = json!({"host":"local","hostname":crate::issues::identity::host(),"node":self.ctx.node,"role":"supervisor","state":"connected","heartbeat":state.local_updated,"pending":0,"conflicts":conflicts,"build":state.build});
            let machines: Vec<_> = std::iter::once((&local, Some(&state.local)))
                .chain(
                    state
                        .machines
                        .iter()
                        .filter(|(h, _)| h.as_str() != "local")
                        .map(|(_, m)| (m, m["workers"].as_array())),
                )
                .skip(offset)
                .take(limit)
                .map(|(m, w)| {
                    if view == "summary" {
                        machine(m, w)
                    } else {
                        let mut m = m.clone();
                        if m["host"] == "local" {
                            m["workers"] = json!(state.local);
                        }
                        m
                    }
                })
                .collect();
            result["page"] = page(offset, machines.len(), total);
            result[if view == "summary" {
                "machines"
            } else {
                "records"
            }] = json!(machines);
        }
        Ok(result)
    }
}
