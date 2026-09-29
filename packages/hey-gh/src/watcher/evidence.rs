//! Display limits never participate in event identity.
use serde_json::{Value, json};

fn priority(value: &Value) -> u8 {
    if matches!(
        value["state"].as_str(),
        Some("failure" | "error" | "CHANGES_REQUESTED")
    ) || matches!(
        value["conclusion"].as_str(),
        Some(
            "failure" | "cancelled" | "timed_out" | "action_required" | "stale" | "startup_failure"
        )
    ) {
        0
    } else if matches!(
        value["state"].as_str(),
        Some("pending" | "missing" | "unknown")
    ) || value["status"].as_str().is_some_and(|s| s != "completed")
    {
        1
    } else {
        2
    }
}

fn trim(value: &mut Value, depth: usize, text_limit: usize, truncated: &mut bool) {
    match value {
        Value::String(text) => {
            if let Some((boundary, _)) = text.char_indices().nth(text_limit) {
                text.truncate(boundary);
                *truncated = true;
            }
        }
        Value::Array(values) => {
            if depth >= 4 {
                values.clear();
                *truncated = true;
            } else {
                if values.len() > 8 {
                    values.truncate(8);
                    *truncated = true;
                }
                for value in values {
                    trim(value, depth + 1, text_limit, truncated);
                }
            }
        }
        Value::Object(fields) => {
            if depth >= 4 {
                fields.clear();
                *truncated = true;
            } else {
                for value in fields.values_mut() {
                    trim(value, depth + 1, text_limit, truncated);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn bounded(mut evidence: Value) -> Value {
    let fields = evidence
        .as_object_mut()
        .expect("Watcher evidence is an object");
    if !fields.contains_key("required_counts")
        && let Some(required) = fields.get("required").and_then(Value::as_array)
    {
        let mut counts = serde_json::Map::new();
        counts.insert("total".into(), json!(required.len()));
        for check in required {
            let state = check["state"].as_str().unwrap_or("unknown");
            let state = if matches!(
                state,
                "failure" | "satisfied" | "pending" | "missing" | "unknown"
            ) {
                state
            } else {
                "unknown"
            };
            let count = counts.get(state).and_then(Value::as_u64).unwrap_or(0);
            counts.insert(state.into(), json!(count + 1));
        }
        fields.insert("required_counts".into(), json!(counts));
    }
    let mut omitted = fields
        .remove("omitted")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let mut truncated = fields.get("truncated") == Some(&Value::Bool(true));
    let mut remaining = 56 * 1024;
    for (name, value) in fields.iter_mut() {
        let Value::Array(rows) = value else {
            trim(value, 0, 512, &mut truncated);
            continue;
        };
        let original = rows.len();
        let budget = remaining.min(if name == "required" {
            8192
        } else if name.ends_with("errors") {
            2048
        } else {
            4096
        });
        let mut candidates = std::mem::take(rows);
        // Keep failures ahead of the successful checks when a large matrix is
        // condensed. Stable ordering preserves the latest-first review order.
        candidates.sort_by_key(priority);
        let mut used = 2;
        for mut row in candidates {
            if rows.len() >= 64 {
                break;
            }
            trim(&mut row, 0, 512, &mut truncated);
            let mut bytes = row.to_string().len() + 1;
            if bytes + 2 > budget {
                trim(&mut row, 0, 128, &mut truncated);
                bytes = row.to_string().len() + 1;
            }
            if used + bytes > budget {
                continue;
            }
            used += bytes;
            rows.push(row);
        }
        remaining = remaining.saturating_sub(used);
        if original > rows.len() {
            let previous = omitted.get(name).and_then(Value::as_u64).unwrap_or(0);
            omitted.insert(
                name.clone(),
                json!(previous + (original - rows.len()) as u64),
            );
            truncated = true;
        }
    }
    if !omitted.is_empty() {
        fields.insert("omitted".into(), json!(omitted));
    }
    if truncated {
        fields.insert("truncated".into(), json!(true));
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_include_omitted_required_failures_and_survive_a_second_pass() {
        let evidence = bounded(json!({"required": (0..300).map(|n| json!({
            "context":format!("check-{n}"), "state": if n < 200 {"failure"} else {"satisfied"}
        })).collect::<Vec<_>>() }));
        assert_eq!(
            evidence["required_counts"],
            json!({"total":300,"failure":200,"satisfied":100})
        );
        assert_eq!(evidence["required"].as_array().unwrap().len(), 64);
        assert_eq!(bounded(evidence.clone()), evidence);
    }
}
