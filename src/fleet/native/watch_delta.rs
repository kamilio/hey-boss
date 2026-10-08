//! Durable watch deltas use SQLite built-ins so existing writers can keep
//! using capture triggers throughout a rolling upgrade.
use serde_json::Value;

/// Watch objects are status -> PR map -> PR -> evidence. Deeper values and
/// arrays are replaced atomically; unchanged siblings never enter the journal.
fn expression(before: &str, after: &str, depth: usize) -> String {
    // Pair both versions in one grouped scan. Correlated joins rematerialize
    // large JSON objects per member and can hold the writer for hundreds of ms.
    let entries = format!("entries{depth}");
    let changes = format!("changes{depth}");
    let d = format!("d{depth}");
    let replacement = format!(
        "json_object('value',CASE {d}.new_type
        WHEN 'object' THEN json({d}.new_value) WHEN 'array' THEN json({d}.new_value)
        WHEN 'true' THEN json('true') WHEN 'false' THEN json('false') ELSE {d}.new_value END)"
    );
    let field = if depth < 3 {
        format!(
            "CASE WHEN {d}.old_type='object' AND {d}.new_type='object' THEN {} ELSE {replacement} END",
            expression(
                &format!("{d}.old_value"),
                &format!("{d}.new_value"),
                depth + 1
            )
        )
    } else {
        replacement
    };
    format!(
        "CASE WHEN json_type({before})='object' AND json_type({after})='object' THEN
        (WITH {entries} AS MATERIALIZED (
            SELECT 0 version,key,type,value FROM json_each({before})
            UNION ALL SELECT 1,key,type,value FROM json_each({after})),
        {changes} AS MATERIALIZED (
            SELECT key,
                max(CASE WHEN version=0 THEN type END) old_type,
                max(CASE WHEN version=0 THEN value END) old_value,
                max(CASE WHEN version=1 THEN type END) new_type,
                max(CASE WHEN version=1 THEN value END) new_value
            FROM {entries} GROUP BY key
            HAVING old_type IS NOT new_type OR old_value IS NOT new_value)
        SELECT json_object(
            'fields',json_group_object({d}.key,json({field})) FILTER (WHERE {d}.new_type IS NOT NULL),
            'remove',json_group_array({d}.key) FILTER (WHERE {d}.new_type IS NULL))
        FROM {changes} {d})
        ELSE json_object('value',json({after})) END"
    )
}

pub(super) fn capture_after(full: &str) -> String {
    format!("CASE WHEN OLD.project_id IS NEW.project_id AND OLD.issue_number IS NEW.issue_number THEN
        json_object('project_id',NEW.project_id,'issue_number',NEW.issue_number,'status_delta',json({}))
        ELSE {full} END", expression("OLD.status", "NEW.status", 0))
}

pub(super) fn apply(value: &mut Value, delta: &Value) -> Result<(), &'static str> {
    let object = delta.as_object().ok_or("Invalid JSON delta")?;
    if let Some(replacement) = object.get("value") {
        if object.len() != 1 {
            return Err("Invalid replacement delta");
        }
        *value = replacement.clone();
        return Ok(());
    }
    if object.is_empty() {
        return Ok(());
    }
    if object.len() != 2 {
        return Err("Invalid object delta");
    }
    let fields = delta["fields"].as_object().ok_or("Missing delta fields")?;
    let removed = delta["remove"].as_array().ok_or("Missing delta removals")?;
    let value = value
        .as_object_mut()
        .ok_or("JSON delta requires an object base")?;
    for key in removed {
        value.remove(key.as_str().ok_or("Invalid removed key")?);
    }
    for (key, patch) in fields {
        apply(value.entry(key.clone()).or_insert(Value::Null), patch)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn exact_delta_round_trip_and_retry() {
        let values = [
            Value::Null,
            json!(false),
            json!(1),
            json!("text"),
            json!([]),
            json!([1,null,{"x":true}]),
            json!({}),
            json!({"a":null}),
            json!({"quote\"slash\\.[0]":null,"":true}),
            json!({"a":{"b":null,"c":[1,2]},"https://a/b~c":false}),
            json!({"a":{"b":true},"https://a/b~c":null}),
        ];
        for before in &values {
            for after in &values {
                let db = crate::database::Connection::open_in_memory().unwrap();
                let encoded: String = db
                    .query_row(
                        &format!("SELECT {}", expression("?1", "?2", 0)),
                        [before.to_string(), after.to_string()],
                        |r| r.get(0),
                    )
                    .unwrap();
                let delta: Value = serde_json::from_str(&encoded).unwrap();
                let mut result = before.clone();
                apply(&mut result, &delta).unwrap();
                assert_eq!(&result, after);
                apply(&mut result, &delta).unwrap();
                assert_eq!(&result, after);
            }
        }
    }
}
