//! Validated transport projections for PR status rows.
use crate::{Error, Result};
use serde_json::Value;

pub const PR_STATUS_FIELDS: &[&str] = &[
    "id",
    "number",
    "title",
    "url",
    "state",
    "isDraft",
    "createdAt",
    "updatedAt",
    "mergedAt",
    "closedAt",
    "author",
    "repository",
    "headRefName",
    "headRefOid",
    "baseRefName",
    "baseRefOid",
    "mergeable",
    "mergeStateStatus",
    "reviewDecision",
    "additions",
    "deletions",
    "statusCheckRollup",
    "statusCheckRollupComplete",
    "headCiState",
    "ci",
    "conflicts",
    "comments",
    "reviewComments",
    "reviews",
    "reviewThreads",
    "reviewStatus",
    "requiredChecks",
    "complete",
    "sourceErrors",
    "removed",
    "body",
    "labels",
    "assignees",
];

pub(crate) fn validate(fields: &[&str]) -> Result<()> {
    if fields.is_empty() {
        return Err(Error::Invalid(
            "PR fields must be a nonempty selection of supported fields".into(),
        ));
    }
    for field in fields {
        if !PR_STATUS_FIELDS.contains(field) {
            return Err(Error::Invalid(format!(
                "unknown PR JSON field: {field}; supported fields: {}",
                PR_STATUS_FIELDS.join(",")
            )));
        }
    }
    Ok(())
}

pub(crate) fn project(row: &mut Value, fields: &[&str]) {
    // Health is mandatory on the wire even when callers select only identity.
    // CLI stdout retains its existing exact --json selection after validation.
    let mut original = row.as_object_mut().map(std::mem::take).unwrap_or_default();
    let mut projected = serde_json::Map::new();
    for field in fields.iter().copied().chain(["complete", "sourceErrors"]) {
        if !projected.contains_key(field) {
            projected.insert(
                field.to_owned(),
                original.remove(field).unwrap_or(Value::Null),
            );
        }
    }
    *row = Value::Object(projected);
}

/// Read raw JSON spans first so omitted row collections never become Value trees.
/// Keep the original stored byte budget and selection/health fields; the final
/// transport projection removes internal selection fields from the output.
pub(crate) fn decode_stored(data: &str, fields: Option<&[String]>) -> serde_json::Result<Value> {
    let Some(fields) = fields else {
        return serde_json::from_str(data);
    };
    if !data.trim_start().starts_with('{') {
        return serde_json::from_str(data);
    }
    let root: std::collections::BTreeMap<String, &serde_json::value::RawValue> =
        serde_json::from_str(data)?;
    let mut decoded = serde_json::Map::new();
    for (key, raw) in root {
        let value = if key == "pullRequest" && raw.get().trim_start().starts_with('{') {
            let row: std::collections::BTreeMap<String, &serde_json::value::RawValue> =
                serde_json::from_str(raw.get())?;
            let mut selected = serde_json::Map::new();
            for (key, raw) in row {
                if fields.contains(&key)
                    || ["complete", "sourceErrors", "repository", "state", "removed"]
                        .contains(&key.as_str())
                {
                    selected.insert(key, serde_json::from_str(raw.get())?);
                }
            }
            Value::Object(selected)
        } else {
            serde_json::from_str(raw.get())?
        };
        decoded.insert(key, value);
    }
    Ok(Value::Object(decoded))
}
