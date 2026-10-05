use super::{Client, Freshness, Response, Result};
use serde_json::json;

const QUERY: &str = r#"query RequiredPolicyCi($owner: String!, $repo: String!, $head: GitObjectID!, $merge: GitObjectID!, $hasMerge: Boolean!) {
  repository(owner: $owner, name: $repo) {
    id nameWithOwner
    head: object(oid: $head) { ...RequiredCiCommit }
    merge: object(oid: $merge) @include(if: $hasMerge) { ...RequiredCiCommit }
  }
}
fragment RequiredCiCommit on Commit {
  __typename oid
  checkSuites(first: 24) {
    totalCount pageInfo { hasNextPage }
    nodes {
      id databaseId
      checkRuns(first: 32, filterBy: {checkType: LATEST}) {
        totalCount pageInfo { hasNextPage }
        nodes {
          __typename id databaseId name status conclusion startedAt completedAt detailsUrl
          checkSuite { app { databaseId } commit { oid } }
        }
      }
    }
  }
  status {
    contexts { id context state updatedAt targetUrl }
  }
}"#;

impl Client {
    // This fixed query is CI-only. Generic GraphQL and policy configuration
    // keep personal authentication; the existing generation fence still applies.
    pub(crate) async fn policy_ci_response(
        &self,
        repository: &str,
        head: &str,
        merge: Option<&str>,
        freshness: Freshness,
    ) -> Result<(String, Response)> {
        let (owner, repo) = repository.split_once('/').expect("validated repository");
        let body = json!({
            "query": QUERY,
            "variables": {
                "owner": owner, "repo": repo, "head": head,
                "merge": merge.unwrap_or(head),
                "hasMerge": merge.is_some_and(|m| m != head)
            }
        });
        let response = self
            .request_versioned(
                self.0.config.graphql_url.to_string(),
                Some(body),
                freshness,
                None,
                self.ci_uses_installation(repository),
            )
            .await?;
        Ok((self.0.config.graphql_url.to_string(), response))
    }
}
