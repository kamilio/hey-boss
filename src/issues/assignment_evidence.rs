//! Bound claim/steering payloads independently of the durable watcher state.
use serde_json::{Map, Value, json};

fn short(value: &Value, limit: usize) -> Value {
    value
        .as_str()
        .map(|text| json!(text.chars().take(limit).collect::<String>()))
        .unwrap_or_else(|| value.clone())
}

fn condensed(snapshot: &Value) -> Value {
    let source = &snapshot["evidence"];
    let mut evidence = Map::new();
    for field in [
        "repository",
        "number",
        "head",
        "complete",
        "ci_complete",
        "ci_settled",
        "has_checks",
        "required_state",
        "conflicts",
        "sources_match",
        "source_heads",
        "source_merges",
        "source_bases",
    ] {
        if let Some(value) = source.get(field) {
            evidence.insert(field.into(), short(value, 256));
        }
    }
    let required = source["required"].as_array().cloned().unwrap_or_default();
    evidence.insert("required_counts".into(), source.get("required_counts").cloned().unwrap_or_else(|| json!({"total":required.len(),"failure":required.iter().filter(|c|c["state"]=="failure").count()})));
    let mut required = required;
    required.sort_by_key(|check| check["state"] != "failure");
    let shown:Vec<_> = required.iter().take(4).map(|check| json!({"context":short(&check["context"],128),"state":check["state"],"url":short(&check["url"],512)})).collect();
    evidence.insert("required".into(), json!(shown));
    let mut omitted = source["omitted"].as_object().cloned().unwrap_or_default();
    for (name, value) in source.as_object().into_iter().flatten() {
        if let Some(rows) = value.as_array() {
            let count = rows
                .len()
                .saturating_sub(if name == "required" { 4 } else { 0 });
            if count > 0 {
                let previous = omitted.get(name).and_then(Value::as_u64).unwrap_or(0);
                omitted.insert(name.clone(), json!(previous.saturating_add(count as u64)));
            }
        }
    }
    evidence.insert("omitted".into(), json!(omitted));
    evidence.insert("truncated".into(), json!(true));
    let mut result = json!({"evidence":evidence});
    for field in ["head", "checked_at", "lifecycle", "error"] {
        if let Some(value) = snapshot.get(field) {
            result[field] = short(value, 256);
        }
    }
    if let Some(errors) = snapshot["errors"].as_object() {
        result["errors"] = json!(
            errors
                .iter()
                .map(|(source, error)| (source.clone(), short(error, 256)))
                .collect::<Map<_, _>>()
        );
    }
    result
}

pub(super) fn bounded(mut status: Value) -> Value {
    let prs = std::mem::take(status["prs"].as_object_mut().unwrap());
    // Account for the root, separators and omission marker before adding rows.
    let mut remaining = (128 * 1024_usize).saturating_sub(status.to_string().len() + 128);
    let mut candidates: Vec<_> = prs.into_iter().collect();
    let trigger = status["trigger"]["url"].as_str().unwrap_or("");
    candidates.sort_by_key(|(url, pr)| {
        if url == trigger {
            0
        } else if pr["evidence"]["required_counts"]["failure"]
            .as_u64()
            .unwrap_or(0)
            > 0
        {
            1
        } else if pr["error"].is_string() {
            2
        } else {
            3
        }
    });
    let total = candidates.len();
    let mut result = Map::new();
    for (index, (url, mut snapshot)) in candidates.into_iter().take(64).enumerate() {
        let overhead = json!(url).to_string().len() + 2;
        let available = remaining.saturating_sub((total.min(64) - index - 1) * 512);
        if snapshot.to_string().len() + overhead > available {
            snapshot = condensed(&snapshot);
        }
        let bytes = snapshot.to_string().len() + overhead;
        if bytes > remaining {
            continue;
        }
        remaining -= bytes;
        result.insert(url, snapshot);
    }
    if total > result.len() {
        status["omitted_prs"] = json!(total - result.len());
    }
    status["prs"] = json!(result);
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_status_keeps_conflicts_when_large_evidence_is_condensed() {
        let url = "https://github.com/o/r/pull/1";
        let status = bounded(json!({"prs":{url:{"evidence":{
            "conflicts":"conflicting", "sources_match":true,
            "reviews":[{"body":"x".repeat(128 * 1024)}]
        }}}}));
        assert_eq!(status["prs"][url]["evidence"]["conflicts"], "conflicting");
        assert_eq!(status["prs"][url]["evidence"]["sources_match"], true);
        assert_eq!(status["prs"][url]["evidence"]["truncated"], true);
        assert!(status.to_string().len() <= 128 * 1024);
    }

    #[test]
    fn condensed_review_only_status_keeps_absent_checks_explicit() {
        let snapshot = condensed(
            &json!({"evidence":{"complete":true,"ci_complete":false,"ci_settled":true,"has_checks":false,"reviews":[{"body":"finding"}]}}),
        );
        assert_eq!(snapshot["evidence"]["ci_complete"], false);
        assert_eq!(snapshot["evidence"]["ci_settled"], true);
        assert_eq!(snapshot["evidence"]["has_checks"], false);
        assert_eq!(snapshot["evidence"]["omitted"]["reviews"], 1);
    }

    #[test]
    fn omitted_pr_count_and_trigger_survive_a_large_roster() {
        let prs = (1..=70)
            .map(|number| {
                (
                    format!("https://github.com/o/r/pull/{number}"),
                    json!({"evidence":{"number":number,"complete":false}}),
                )
            })
            .collect::<Map<_, _>>();
        let status = bounded(json!({"trigger":{"url":"https://github.com/o/r/pull/70"},"prs":prs}));
        assert_eq!(status["prs"].as_object().unwrap().len(), 64);
        assert_eq!(status["omitted_prs"], 6);
        assert!(
            status["prs"]
                .get("https://github.com/o/r/pull/70")
                .is_some()
        );
        assert!(status.to_string().len() <= 128 * 1024);
    }
}
