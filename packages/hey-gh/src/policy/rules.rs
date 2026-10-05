//! Revalidate a complete, unchanged REST rule roster using the branch proof.
use super::branch::{BRANCH, recent, reference};
use crate::{Client, Error, Freshness, Response, Result, repository::segment};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn nonempty(value: &Value) -> bool {
    value.as_str().is_some_and(|s| !s.is_empty())
}

fn parameterless(kind: &str) -> bool {
    matches!(
        kind,
        "creation"
            | "deletion"
            | "non_fast_forward"
            | "required_linear_history"
            | "required_signatures"
    )
}

fn supported(data: &Value) -> bool {
    data.as_array().is_some_and(|rules| {
        rules.len() <= 100
            && rules.iter().all(|rule| {
                let Some(fields) = rule.as_object() else {
                    return false;
                };
                let kind = rule["type"].as_str().unwrap_or_default();
                let common = nonempty(&rule["ruleset_source"])
                    && matches!(
                        rule["ruleset_source_type"].as_str(),
                        Some("Repository" | "Organization")
                    )
                    && rule["ruleset_id"].as_u64().is_some_and(|id| id > 0);
                common
                    && if parameterless(kind) {
                        fields.len() == 4
                    } else if kind == "required_status_checks" && fields.len() == 5 {
                        rule["parameters"]
                            .as_object()
                            .is_some_and(|params| params.len() == 3)
                            && rule["parameters"]["strict_required_status_checks_policy"]
                                .is_boolean()
                            && rule["parameters"]["do_not_enforce_on_create"].is_boolean()
                            && rule["parameters"]["required_status_checks"]
                                .as_array()
                                .is_some_and(|checks| {
                                    checks.iter().all(|check| {
                                        check.as_object().is_some_and(|fields| fields.len() == 2)
                                            && nonempty(&check["context"])
                                            && check.get("integration_id").is_some_and(|id| {
                                                id.is_null() || id.as_i64().is_some()
                                            })
                                    })
                                })
                    } else {
                        false
                    }
            })
    })
}

fn matches(data: &Value, seed: &Value, repository: &str, branch: &str) -> bool {
    let Some(reference) = reference(data, repository, branch) else {
        return false;
    };
    let rules = &reference["rules"];
    let Some(nodes) = rules["nodes"].as_array() else {
        return false;
    };
    if rules["pageInfo"]["hasNextPage"] != false
        || nodes.len() > 100
        || rules["totalCount"].as_u64() != Some(nodes.len() as u64)
    {
        return false;
    }
    let mut ids = BTreeSet::new();
    let mut projected = Vec::new();
    for node in nodes {
        if !nonempty(&node["id"]) || !ids.insert(node["id"].as_str().unwrap()) {
            return false;
        }
        let Some(kind) = node["type"].as_str() else {
            return false;
        };
        let kind = kind.to_ascii_lowercase();
        let set = &node["repositoryRuleset"];
        if !nonempty(&set["id"]) {
            return false;
        }
        let origin = &set["source"];
        let name = match origin["__typename"].as_str() {
            Some("Repository") => &origin["nameWithOwner"],
            Some("Organization") => &origin["login"],
            _ => return false,
        };
        let mut rule = json!({"type":kind,"ruleset_id":set["databaseId"],"ruleset_source_type":origin["__typename"],"ruleset_source":name});
        if kind == "required_status_checks" {
            let params = &node["parameters"];
            if params["__typename"] != "RequiredStatusChecksParameters" {
                return false;
            }
            let Some(checks) = params["requiredStatusChecks"].as_array() else {
                return false;
            };
            if checks
                .iter()
                .any(|check| check.get("integrationId").is_none())
            {
                return false;
            }
            rule["parameters"] = json!({"strict_required_status_checks_policy":params["strictRequiredStatusChecksPolicy"],
                "do_not_enforce_on_create":params["doNotEnforceOnCreate"],
                "required_status_checks":checks.iter().map(|check| json!({"context":check["context"],"integration_id":check["integrationId"]})).collect::<Vec<_>>()});
        } else if !parameterless(&kind) || node.get("parameters") != Some(&Value::Null) {
            return false;
        }
        projected.push(rule);
    }
    let projected = Value::Array(projected);
    if !supported(&projected) {
        return false;
    }
    // API rule order differs; preserve duplicates and every supported field.
    let ordered = |value: &Value| {
        let mut rows: Vec<_> = value
            .as_array()
            .unwrap()
            .iter()
            .map(Value::to_string)
            .collect();
        rows.sort();
        rows
    };
    ordered(&projected) == ordered(seed)
}

impl Client {
    pub(super) async fn policy_rules(
        &self,
        repository: &str,
        branch: &str,
        freshness: Freshness,
    ) -> Result<Response> {
        let path = format!("repos/{repository}/rules/branches/{}", segment(branch));
        if let Freshness::MaxAge(age) = freshness
            && !age.is_zero()
        {
            let seed = match self.peek_get(&path).await {
                Ok(seed) => Some(seed),
                Err(Error::CacheMiss) => None,
                Err(error) => return Err(error),
            };
            if let Some(seed) = seed
                && !recent(&seed, age)
                && seed.link.is_none()
                && supported(&seed.data)
            {
                // Do not turn an explicitly denied REST policy into success via
                // a different endpoint. The ordinary policy reader owns recovery.
                let error_key = format!("policy-error://{}/{path}", self.hostname());
                if self
                    .peek_derived(&error_key)
                    .await?
                    .is_some_and(|error| error.data["status"].is_number())
                {
                    return self.policy_get(&path, freshness).await;
                }
                let (owner, repo) = repository.split_once('/').expect("validated repository");
                let result = crate::client::optional_selector_read(self.graphql(
                    BRANCH,
                    json!({"owner":owner,"repo":repo,"ref":format!("refs/heads/{branch}")}),
                    freshness,
                ))
                .await;
                match result {
                    Ok(mut proof)
                        if proof.validated_at_ms >= seed.validated_at_ms
                            && recent(&proof, age)
                            && matches(&proof.data, &seed.data, repository, branch) =>
                    {
                        // A peer can observe a REST denial while this shared
                        // query is in flight. Preserve that explicit failure.
                        if self
                            .peek_derived(&error_key)
                            .await?
                            .is_some_and(|error| error.data["status"].is_number())
                        {
                            return self.policy_get(&path, freshness).await;
                        }
                        proof.data = seed.data;
                        proof.etag = None;
                        proof.last_modified = None;
                        proof.link = None;
                        return Ok(proof);
                    }
                    Err(
                        error @ (Error::Auth(_)
                        | Error::LocalAuth(_)
                        | Error::Storage(_)
                        | Error::Stopped
                        | Error::GraphQL {
                            access_denied: true,
                            ..
                        }
                        | Error::GitHub {
                            status: 401 | 403, ..
                        }),
                    ) => return Err(error),
                    _ => {}
                }
                // A peer may have cached an old roster during GraphQL. Changed
                // or ambiguous evidence requires a real REST revalidation.
                return self.policy_get(&path, Freshness::Revalidate).await;
            }
        }
        self.policy_get(&path, freshness).await
    }
}
