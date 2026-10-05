//! A narrow branch projection for repositories protected only by rulesets.
use super::classic_checks_disabled;
use crate::{Client, Error, Freshness, Response, Result, now_ms, repository::segment};
use serde_json::{Value, json};
use std::time::Duration;

pub(super) const BRANCH: &str = r#"query RequiredPolicyBranch($owner: String!, $repo: String!, $ref: String!) {
  repository(owner: $owner, name: $repo) {
    id nameWithOwner
    ref(qualifiedName: $ref) {
      id name prefix target { __typename oid }
      branchProtectionRule { id }
      refUpdateRule { pattern }
      rules(first: 100) {
        totalCount pageInfo { hasNextPage }
        nodes {
          id type
          repositoryRuleset {
            id databaseId
            source { __typename ... on Repository { nameWithOwner } ... on Organization { login } }
          }
          parameters {
            __typename
            ... on RequiredStatusChecksParameters {
              doNotEnforceOnCreate strictRequiredStatusChecksPolicy
              requiredStatusChecks { context integrationId }
            }
          }
        }
      }
    }
  }
}"#;

pub(super) fn recent(response: &Response, age: Duration) -> bool {
    now_ms()
        .checked_sub(response.validated_at_ms)
        .is_some_and(|elapsed| (elapsed as u128) < age.as_millis())
}

pub(super) fn reference<'a>(data: &'a Value, repository: &str, branch: &str) -> Option<&'a Value> {
    let repo = &data["data"]["repository"];
    let reference = &repo["ref"];
    let nonempty = |value: &Value| value.as_str().is_some_and(|s| !s.is_empty());
    if !nonempty(&repo["id"])
        || !repo["nameWithOwner"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(repository))
        || !nonempty(&reference["id"])
        || reference["prefix"] != "refs/heads/"
        || reference["name"] != branch
        || reference["target"]["__typename"] != "Commit"
        || !reference["target"]["oid"]
            .as_str()
            .is_some_and(crate::repository::valid_sha)
    {
        return None;
    }
    Some(reference)
}

fn projection(data: &Value, repository: &str, branch: &str) -> Option<Value> {
    let reference = reference(data, repository, branch)?;
    // Check both the classic object and the view available to non-admins.
    // Missing/redacted fields are not proof that classic protection is off.
    if reference.get("branchProtectionRule") != Some(&Value::Null)
        || reference.get("refUpdateRule") != Some(&Value::Null)
        // Ref.rules contains active repository/organization rules that apply
        // to this ref. One actual rule proves protected=true; zero is ambiguous.
        // This existence proof alone does not validate rule parameters.
        || !reference["rules"]["totalCount"].as_u64().is_some_and(|n| n > 0)
        || !reference["rules"]["nodes"].as_array().is_some_and(|nodes| !nodes.is_empty() && nodes.len() <= 100 && nodes.iter().all(|node| node["id"].as_str().is_some_and(|id| !id.is_empty())))
    {
        return None;
    }
    Some(
        json!({"commit":{"sha":reference["target"]["oid"]},"protected":true,
        "protection":{"enabled":false,"required_status_checks":{"enforcement_level":"off","contexts":[],"checks":[]}}}),
    )
}

impl Client {
    pub(super) async fn policy_branch(
        &self,
        repository: &str,
        branch: &str,
        freshness: Freshness,
    ) -> Result<Response> {
        let path = format!("repos/{repository}/branches/{}", segment(branch));
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
                && seed.data["protected"] == true
                && classic_checks_disabled(&seed.data)
            {
                let (owner, repo) = repository.split_once('/').expect("validated repository");
                let result = crate::client::optional_selector_read(self.graphql(
                    BRANCH,
                    json!({"owner":owner,"repo":repo,"ref":format!("refs/heads/{branch}")}),
                    freshness,
                ))
                .await;
                match result {
                    Ok(mut response)
                        if response.validated_at_ms >= seed.validated_at_ms
                            && recent(&response, age) =>
                    {
                        if let Some(data) = projection(&response.data, repository, branch) {
                            // The GraphQL validation proves only these policy
                            // fields. Never update or validate the full REST body.
                            response.data = data;
                            response.etag = None;
                            response.last_modified = None;
                            response.link = None;
                            return Ok(response);
                        }
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
                // An ambiguous/changed proof must not borrow freshness from a
                // REST response a peer cached before that proof was observed.
                return self.get(&path, Freshness::Revalidate).await;
            }
        }
        self.get(&path, freshness).await
    }
}
