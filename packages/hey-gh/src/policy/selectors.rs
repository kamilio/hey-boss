//! Confirm standalone policy selectors without refreshing unrelated REST fields.
use crate::{Client, Error, Freshness, Response, Result, now_ms};
use serde_json::{Value, json};
use std::time::Duration;

const SELECTORS: &str = r#"query RequiredPolicySelectors($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    id databaseId nameWithOwner
    pullRequest(number: $number) {
      id number state merged mergeable headRefOid baseRefName baseRefOid
      baseRepository { id databaseId nameWithOwner }
      potentialMergeCommit { oid parents(first: 2) { totalCount nodes { oid } } }
      stack { id } stackEntry { id }
    }
  }
}"#;

fn recent(response: &Response, age: Duration) -> bool {
    now_ms()
        .checked_sub(response.validated_at_ms)
        .is_some_and(|elapsed| (elapsed as u128) < age.as_millis())
}

fn eligible(pr: &Value, repository: &str, number: u64) -> bool {
    let nonempty = |v: &Value| v.as_str().is_some_and(|s| !s.is_empty());
    let sha = |v: &Value| v.as_str().is_some_and(crate::repository::valid_sha);
    pr["number"] == number
        && nonempty(&pr["node_id"])
        && pr["state"] == "open"
        && pr["merged"] == false
        && pr["mergeable"] == true
        && pr["stack"].is_null()
        && sha(&pr["head"]["sha"])
        && sha(&pr["base"]["sha"])
        && sha(&pr["merge_commit_sha"])
        && pr["base"]["ref"]
            .as_str()
            .is_some_and(|s| crate::repository::validate_branch(s).is_ok())
        && pr["base"]["repo"]["id"].as_u64().is_some_and(|id| id > 0)
        && nonempty(&pr["base"]["repo"]["node_id"])
        && pr["base"]["repo"]["full_name"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(repository))
}

fn matches(pr: &Value, response: &Value) -> bool {
    let repository = &response["data"]["repository"];
    let node = &repository["pullRequest"];
    let base = &pr["base"]["repo"];
    let same_repository = |repo: &Value| {
        repo["id"] == base["node_id"]
            && repo["databaseId"] == base["id"]
            && repo["nameWithOwner"] == base["full_name"]
    };
    let merge = &node["potentialMergeCommit"];
    same_repository(repository) && same_repository(&node["baseRepository"])
        && node["id"] == pr["node_id"] && node["number"] == pr["number"]
        && node["state"] == "OPEN" && node["merged"] == false && node["mergeable"] == "MERGEABLE"
        && node["headRefOid"] == pr["head"]["sha"]
        && node["baseRefName"] == pr["base"]["ref"] && node["baseRefOid"] == pr["base"]["sha"]
        && merge["oid"] == pr["merge_commit_sha"] && merge["parents"]["totalCount"] == 2
        && merge["parents"]["nodes"].as_array().is_some_and(|parents| {
            parents.len() == 2
                && parents.iter().all(|parent| parent["oid"].as_str().is_some_and(crate::repository::valid_sha))
                && parents.iter().any(|parent| parent["oid"] == node["headRefOid"])
        })
        // Missing fields are unknown, not evidence of absent native membership.
        && node.get("stack") == Some(&Value::Null)
        && node.get("stackEntry") == Some(&Value::Null)
}

impl Client {
    /// None confirms only the seeded selectors; it is never a fresh REST body.
    pub(super) async fn confirm_policy_pr(
        &self,
        repository: &str,
        number: u64,
        seed: &Response,
        freshness: Freshness,
    ) -> Result<Option<Response>> {
        if let Freshness::MaxAge(age) = freshness
            && !age.is_zero()
            && eligible(&seed.data, repository, number)
        {
            // Prefer a REST observation another reader already refreshed.
            let stale = match self
                .peek_get(&format!("repos/{repository}/pulls/{number}"))
                .await
            {
                Ok(latest) => !recent(&latest, age),
                Err(Error::CacheMiss) => false,
                Err(error) => return Err(error),
            };
            if stale {
                let (owner, repo) = repository.split_once('/').expect("validated repository");
                // This optional route must leave time for REST when GraphQL is
                // stalled or paced. It shares the existing main-account scope.
                let result = tokio::time::timeout(
                    Duration::from_secs(2),
                    self.graphql(
                        SELECTORS,
                        json!({"owner":owner,"repo":repo,"number":number}),
                        freshness,
                    ),
                )
                .await;
                match result {
                    Ok(Ok(response))
                        if response.validated_at_ms >= seed.validated_at_ms
                            && recent(&response, age)
                            && matches(&seed.data, &response.data) =>
                    {
                        return Ok(None);
                    }
                    Ok(Err(
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
                    )) => return Err(error),
                    _ => {}
                }
                // A changed, unavailable or ambiguous selector cannot lend its
                // freshness to the seed, even if a REST cache entry is recent.
                return self
                    .pull_request(repository, number, Freshness::Revalidate)
                    .await
                    .map(Some);
            }
        }
        self.pull_request(repository, number, freshness)
            .await
            .map(Some)
    }
}
