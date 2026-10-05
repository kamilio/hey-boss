//! Advisory evidence from the fixed discovery query; never a complete roster.
use crate::{Error, Response, Result};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(crate) const CACHE_TAG: &str = "partial-discovery-v1";

pub(crate) struct Page {
    pub response: Response,
    pub source_bytes: usize,
}

pub(crate) fn capture(body: Option<&Value>, data: Value) -> Option<Value> {
    if body?["query"].as_str()? != super::MY_PRS.trim() {
        return None;
    }
    let source_bytes = data.to_string().len();
    let payload = retain(body, data)?;
    Some(json!({"payload":payload,"source_bytes":source_bytes}))
}

pub(crate) fn decode(mut response: Response, after: Value) -> Result<Page> {
    let invalid = || Error::Invalid("invalid retained partial discovery evidence".into());
    let source_bytes = response.data["source_bytes"]
        .as_u64()
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(invalid)?;
    let payload = response.data.get_mut("payload").ok_or_else(invalid)?.take();
    if source_bytes < payload.to_string().len() {
        return Err(invalid());
    }
    response.data = retain(
        Some(&json!({"query":super::MY_PRS.trim(),"variables":{"after":after}})),
        payload,
    )
    .ok_or_else(invalid)?;
    Ok(Page {
        response,
        source_bytes,
    })
}

pub(crate) fn retain(body: Option<&Value>, mut data: Value) -> Option<Value> {
    let body = body?;
    if body["query"].as_str()? != super::MY_PRS.trim()
        || !body["variables"].as_object().is_some_and(|v| {
            v.len() == 1
                && v.get("after")
                    .is_some_and(|a| a.is_null() || a.as_str().is_some_and(|s| !s.is_empty()))
        })
    {
        return None;
    }
    let errors = data["errors"].as_array().filter(|e| !e.is_empty())?;
    let conn = &data["data"]["viewer"]["pullRequests"];
    let nodes = conn["nodes"].as_array()?;
    // The fixed query requests at most 25 nodes. Pagination fields must be
    // independent of every reported error before a scan can continue.
    if nodes.is_empty() || nodes.len() > 25 || conn["totalCount"].as_u64()? < nodes.len() as u64 {
        return None;
    }
    let has_next = conn["pageInfo"]["hasNextPage"].as_bool()?;
    if has_next
        && !conn["pageInfo"]["endCursor"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    {
        return None;
    }
    let mut denied = BTreeSet::new();
    for error in errors {
        let path = error["path"].as_array()?;
        if error["type"] != "FORBIDDEN"
            || path.len() < 4
            || path[0] != "viewer"
            || path[1] != "pullRequests"
            || path[2] != "nodes"
            || !path[4..]
                .iter()
                .all(|p| p.as_u64().is_some() || p.as_str().is_some_and(|s| !s.is_empty()))
        {
            return None;
        }
        let index = usize::try_from(path[3].as_u64()?).ok()?;
        if index >= nodes.len() {
            return None;
        }
        denied.insert(index);
    }
    let mut identities = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for (index, node) in nodes.iter().enumerate() {
        if denied.contains(&index) {
            continue;
        }
        if !identities.insert(super::key(node).ok()?)
            || !ids.insert(node["id"].as_str().filter(|s| !s.is_empty())?)
        {
            return None;
        }
    }
    // Null rollups can prove empty checks. Never let a field-level denial
    // manufacture that proof: exclude its whole node, including other fields.
    let nodes = data["data"]["viewer"]["pullRequests"]["nodes"].as_array_mut()?;
    for index in denied {
        nodes[index] = Value::Null;
    }
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body() -> Value {
        json!({"query":crate::dashboard::MY_PRS.trim(),"variables":{"after":null}})
    }

    fn page() -> Value {
        json!({"data":{"viewer":{"pullRequests":{
            "totalCount":2,"nodes":[{"id":"PR_1","number":1,"repository":{"nameWithOwner":"acme/demo"},"untouched":{"a":1}},null],
            "pageInfo":{"hasNextPage":false,"endCursor":null}
        }}},"errors":[{"type":"FORBIDDEN","path":["viewer","pullRequests","nodes",1]}]})
    }

    #[test]
    fn permitted_nodes_are_unchanged_and_field_denials_exclude_the_entire_node() {
        let original = page();
        assert_eq!(
            retain(Some(&body()), original.clone()),
            Some(original.clone())
        );
        let mut fields = original;
        fields["errors"].as_array_mut().unwrap().push(json!({"type":"FORBIDDEN","path":["viewer","pullRequests","nodes",0,"commits","nodes",0,"commit","statusCheckRollup"]}));
        let retained = retain(Some(&body()), fields.clone()).unwrap();
        assert_eq!(
            retained["data"]["viewer"]["pullRequests"]["nodes"],
            json!([null, null])
        );
        assert_eq!(retained["errors"], fields["errors"]);
        assert_eq!(retain(Some(&body()), retained.clone()), Some(retained));
    }

    #[test]
    fn retained_frames_preserve_clocks_and_reject_invalid_byte_accounting() {
        let original = page();
        let frame = capture(Some(&body()), original.clone()).unwrap();
        let response = |data| Response {
            data,
            fetched_at_ms: 41,
            validated_at_ms: 42,
            source: crate::Source::Cache,
            etag: None,
            last_modified: None,
            link: None,
        };
        let decoded = decode(response(frame.clone()), Value::Null).unwrap();
        assert_eq!(decoded.response.data, original);
        assert_eq!(decoded.source_bytes, original.to_string().len());
        assert_eq!(decoded.response.fetched_at_ms, 41);
        assert_eq!(decoded.response.validated_at_ms, 42);
        for bytes in [Value::Null, json!(-1), json!(0), json!("999999")] {
            let mut invalid = frame.clone();
            invalid["source_bytes"] = bytes;
            assert!(decode(response(invalid), Value::Null).is_err());
        }
        let mut invalid = frame;
        invalid["payload"] = Value::Null;
        assert!(decode(response(invalid), Value::Null).is_err());
        assert!(decode(response(original), Value::Null).is_err());
    }

    #[test]
    fn unscoped_errors_invalid_pagination_and_ambiguous_nodes_are_not_retained() {
        for case in [
            "missing-path",
            "root",
            "page-info",
            "negative",
            "outside",
            "string-index",
            "rate-limited",
            "internal",
            "missing-error",
            "empty-errors",
            "unknown-null",
            "duplicate-id",
            "duplicate-selector",
            "missing-id",
            "missing-number",
            "count",
            "cursor",
            "page-flag",
        ] {
            let mut p = page();
            match case {
                "missing-path" => {
                    p["errors"][0].as_object_mut().unwrap().remove("path");
                }
                "root" => p["errors"][0]["path"] = json!(["viewer"]),
                "page-info" => {
                    p["errors"][0]["path"] = json!(["viewer", "pullRequests", "pageInfo"])
                }
                "negative" => p["errors"][0]["path"][3] = json!(-1),
                "outside" => p["errors"][0]["path"][3] = json!(2),
                "string-index" => p["errors"][0]["path"][3] = json!("1"),
                "rate-limited" => p["errors"][0]["type"] = json!("RATE_LIMITED"),
                "internal" => p["errors"][0]["type"] = json!("INTERNAL"),
                "missing-error" => {
                    p.as_object_mut().unwrap().remove("errors");
                }
                "empty-errors" => p["errors"] = json!([]),
                "unknown-null" => p["data"]["viewer"]["pullRequests"]["nodes"][0] = Value::Null,
                "duplicate-id" | "duplicate-selector" => {
                    let mut node = p["data"]["viewer"]["pullRequests"]["nodes"][0].clone();
                    if case == "duplicate-id" {
                        node["number"] = json!(2);
                    } else {
                        node["id"] = json!("PR_2");
                    }
                    p["data"]["viewer"]["pullRequests"]["nodes"]
                        .as_array_mut()
                        .unwrap()
                        .push(node);
                    p["data"]["viewer"]["pullRequests"]["totalCount"] = json!(3);
                }
                "missing-id" => {
                    p["data"]["viewer"]["pullRequests"]["nodes"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("id");
                }
                "missing-number" => {
                    p["data"]["viewer"]["pullRequests"]["nodes"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("number");
                }
                "count" => p["data"]["viewer"]["pullRequests"]["totalCount"] = json!(1),
                "cursor" => {
                    p["data"]["viewer"]["pullRequests"]["pageInfo"]["hasNextPage"] = json!(true)
                }
                "page-flag" => {
                    p["data"]["viewer"]["pullRequests"]["pageInfo"]["hasNextPage"] = json!("false")
                }
                _ => unreachable!(),
            }
            assert!(retain(Some(&body()), p).is_none(), "{case}");
        }
        for b in [
            None,
            Some(json!({"query":"query Other { viewer { login } }","variables":{"after":null}})),
            Some(json!({"query":crate::dashboard::MY_PRS,"variables":{"after":3}})),
        ] {
            assert!(retain(b.as_ref(), page()).is_none());
        }
    }

    #[test]
    fn partial_cache_keys_preserve_repository_generation_and_completed_job_layout() {
        assert_eq!(
            crate::client::tagged_cache_key("graphql#hash#repository-generation=3", CACHE_TAG),
            "graphql#hash#partial-discovery-v1#repository-generation=3"
        );
        assert_eq!(
            crate::client::tagged_cache_key("jobs?per_page=100", "completed-jobs-version=abc"),
            "jobs?per_page=100#completed-jobs-version=abc"
        );
    }
}
