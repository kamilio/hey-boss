//! Confirm standalone policy selectors without refreshing unrelated REST fields.
use crate::{Client, Error, Freshness, Response, Result, now_ms};
use serde_json::{Value, json};
use std::time::Duration;

pub(super) struct Confirmation {
    pub rest: Option<Response>,
    pub selectors: super::PullRequestConfirmation,
}

fn project(pr: &Value, validated_at_ms: u64) -> super::PullRequestConfirmation {
    let stack = &pr["stack"];
    let stack = if stack.is_null() {
        Value::Null
    } else {
        json!({"id":stack["id"],"number":stack["number"],"position":stack["position"],"size":stack["size"],
            "base":{"ref":stack["base"]["ref"],"sha":stack["base"]["sha"]}})
    };
    super::PullRequestConfirmation {
        selectors: json!({
            "node_id":pr["node_id"],"number":pr["number"],
            "state":pr["state"],"merged":pr["merged"],"mergeable":pr["mergeable"],
            "head":{"sha":pr["head"]["sha"]},
            "base":{"ref":pr["base"]["ref"],"sha":pr["base"]["sha"],
                "repo":{"id":pr["base"]["repo"]["id"],"node_id":pr["base"]["repo"]["node_id"],"full_name":pr["base"]["repo"]["full_name"]}},
            "merge_commit_sha":pr["merge_commit_sha"],"stack":stack
        }),
        validated_at_ms,
    }
}

impl Confirmation {
    fn rest(response: Response) -> Self {
        Self {
            selectors: project(&response.data, response.validated_at_ms),
            rest: Some(response),
        }
    }
}

const SELECTORS: &str = r#"query RequiredPolicySelectors($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    id databaseId nameWithOwner
    pullRequest(number: $number) {
      id number state merged mergeable headRefOid baseRefName baseRefOid
      baseRepository { id databaseId nameWithOwner }
      potentialMergeCommit { oid parents(first: 2) { totalCount nodes { oid } } }
      mergeCommit { oid }
      stack { id } stackEntry { id }
    }
  }
}"#;

fn recent(response: &Response, age: Duration) -> bool {
    now_ms()
        .checked_sub(response.validated_at_ms)
        .is_some_and(|elapsed| (elapsed as u128) < age.as_millis())
}

fn lifecycle(pr: &Value) -> Option<&'static str> {
    match (pr["state"].as_str(), pr["merged"].as_bool()) {
        (Some("open"), Some(false)) => Some("OPEN"),
        (Some("closed"), Some(false)) => Some("CLOSED"),
        (Some("closed"), Some(true)) => Some("MERGED"),
        _ => None,
    }
}

pub(super) fn merged_seed(pr: &Value, repository: &str, number: u64) -> bool {
    lifecycle(pr) == Some("MERGED") && eligible(pr, repository, number)
}

fn conflicting_seed(pr: &Value) -> bool {
    lifecycle(pr) == Some("OPEN")
        && pr["mergeable"] == false
        && pr.get("merge_commit_sha") == Some(&Value::Null)
}

fn eligible(pr: &Value, repository: &str, number: u64) -> bool {
    let nonempty = |v: &Value| v.as_str().is_some_and(|s| !s.is_empty());
    let sha = |v: &Value| v.as_str().is_some_and(crate::repository::valid_sha);
    pr["number"] == number
        && nonempty(&pr["node_id"])
        && lifecycle(pr).is_some()
        // A merged PR's merge SHA is its actual merge/squash commit, not an
        // uncertain test merge. Its current mergeability is no longer relevant.
        && (((pr["merged"] == true || pr["mergeable"] == true)
            && sha(&pr["merge_commit_sha"]))
            || conflicting_seed(pr))
        && pr["stack"].is_null()
        && sha(&pr["head"]["sha"])
        && sha(&pr["base"]["sha"])
        && pr["base"]["ref"]
            .as_str()
            .is_some_and(|s| crate::repository::validate_branch(s).is_ok())
        && pr["base"]["repo"]["id"].as_u64().is_some_and(|id| id > 0)
        && nonempty(&pr["base"]["repo"]["node_id"])
        && pr["base"]["repo"]["full_name"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(repository))
}

fn identity_matches(pr: &Value, response: &Value) -> bool {
    let repository = &response["data"]["repository"];
    let node = &repository["pullRequest"];
    let base = &pr["base"]["repo"];
    let same_repository = |repo: &Value| {
        repo["id"] == base["node_id"]
            && repo["databaseId"] == base["id"]
            && repo["nameWithOwner"] == base["full_name"]
    };
    same_repository(repository) && same_repository(&node["baseRepository"])
        && node["id"] == pr["node_id"] && node["number"] == pr["number"]
        && node["state"].as_str() == lifecycle(pr) && node["merged"] == pr["merged"]
        && node["headRefOid"] == pr["head"]["sha"]
        && node["baseRefName"] == pr["base"]["ref"] && node["baseRefOid"] == pr["base"]["sha"]
        // Missing fields are unknown, not evidence of absent native membership.
        && node.get("stack") == Some(&Value::Null)
        && node.get("stackEntry") == Some(&Value::Null)
}

fn matches(pr: &Value, response: &Value) -> bool {
    let node = &response["data"]["repository"]["pullRequest"];
    if conflicting_seed(pr) {
        // Explicit current conflicts can confirm a head-only collection. A
        // missing/unknown test merge cannot confirm this seed, and a retained
        // merge ref is never evidence that the conflict remains unchanged.
        return identity_matches(pr, response)
            && node["mergeable"] == "CONFLICTING"
            && node.get("potentialMergeCommit") == Some(&Value::Null);
    }
    let merge_matches = if pr["merged"] == true {
        node["mergeCommit"]["oid"] == pr["merge_commit_sha"]
    } else {
        let merge = &node["potentialMergeCommit"];
        merge["oid"] == pr["merge_commit_sha"]
            && merge["parents"]["totalCount"] == 2
            && merge["parents"]["nodes"].as_array().is_some_and(|parents| {
                parents.len() == 2
                    && parents.iter().all(|parent| {
                        parent["oid"]
                            .as_str()
                            .is_some_and(crate::repository::valid_sha)
                    })
                    && parents
                        .iter()
                        .any(|parent| parent["oid"] == node["headRefOid"])
            })
    };
    identity_matches(pr, response)
        && (pr["merged"] == true || node["mergeable"] == "MERGEABLE")
        && merge_matches
}

impl Client {
    /// Selector confirmation never refreshes the seeded REST body.
    pub(super) async fn confirm_policy_pr(
        &self,
        repository: &str,
        number: u64,
        seed: &Response,
        freshness: Freshness,
    ) -> Result<Confirmation> {
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
                // Box the multi-read shortcut to bound nested collector stacks.
                let result = crate::client::optional_selector_read(Box::pin(async {
                    let response = self
                        .graphql(
                            SELECTORS,
                            json!({"owner":owner,"repo":repo,"number":number}),
                            freshness,
                        )
                        .await?;
                    if response.validated_at_ms < seed.validated_at_ms || !recent(&response, age) {
                        return Ok(None);
                    }
                    if matches(&seed.data, &response.data) {
                        let mut proof = project(&seed.data, response.validated_at_ms);
                        proof.selectors["mergeable"] =
                            match response.data["data"]["repository"]["pullRequest"]["mergeable"]
                                .as_str()
                            {
                                Some("MERGEABLE") => json!(true),
                                Some("CONFLICTING") => json!(false),
                                _ => Value::Null,
                            };
                        return Ok(Some(proof));
                    }
                    let node = &response.data["data"]["repository"]["pullRequest"];
                    if lifecycle(&seed.data) != Some("OPEN")
                        || seed.data["mergeable"] != true
                        || !identity_matches(&seed.data, &response.data)
                        || node["mergeable"] != "UNKNOWN"
                        || node.get("potentialMergeCommit") != Some(&Value::Null)
                    {
                        return Ok(None);
                    }
                    // GitHub can omit the test merge from GraphQL while its
                    // REST ref still exists. A retained ref alone is not proof:
                    // the fresh node must also match the known-clean seed's
                    // exact PR, head, base and standalone membership above.
                    // Confirm only selectors, never mergeability or REST fields.
                    // Both optional reads share one budget and personal auth.
                    let merge_ref = self
                        .get(
                            &format!("repos/{repository}/git/ref/pull/{number}/merge"),
                            Freshness::Revalidate,
                        )
                        .await?;
                    let confirmed = merge_ref.validated_at_ms >= response.validated_at_ms
                        && recent(&response, age)
                        && recent(&merge_ref, age)
                        && merge_ref.data["ref"] == format!("refs/pull/{number}/merge")
                        && merge_ref.data["object"]["type"] == "commit"
                        && merge_ref.data["object"]["sha"] == seed.data["merge_commit_sha"];
                    Ok(confirmed.then(|| {
                        let mut proof = project(&seed.data, response.validated_at_ms);
                        proof.selectors["mergeable"] = Value::Null;
                        proof
                    }))
                }))
                .await;
                match result {
                    Ok(Some(selectors)) => {
                        return Ok(Confirmation {
                            rest: None,
                            selectors,
                        });
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
                // A changed, unavailable or ambiguous selector cannot lend its
                // freshness to the seed, even if a REST cache entry is recent.
                return self
                    .pull_request(repository, number, Freshness::Revalidate)
                    .await
                    .map(Confirmation::rest);
            }
        }
        self.pull_request(repository, number, freshness)
            .await
            .map(Confirmation::rest)
    }
}
