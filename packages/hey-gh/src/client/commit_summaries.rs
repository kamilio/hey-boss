use super::{Client, Freshness, Response, Result, validate_repository};
use serde_json::json;

// Complete suites prove empty commit lists and validate cached workflow versions.
// Rollups can omit workflow checks; they cannot establish the workflow roster.
const QUERY: &str = r#"query CommitLists($owner: String!, $repo: String!, $head: GitObjectID!, $merge: GitObjectID!, $hasMerge: Boolean!) {
  repository(owner: $owner, name: $repo) {
    id nameWithOwner
    head: object(oid: $head) { ...CommitListsCommit }
    merge: object(oid: $merge) @include(if: $hasMerge) { ...CommitListsCommit }
  }
}
fragment CommitListsCommit on Commit {
  __typename oid
  status { id }
  checkSuites(first: 24) {
    totalCount pageInfo { hasNextPage }
    nodes {
      id databaseId status conclusion updatedAt
      checkRuns(first: 1, filterBy: {checkType: LATEST}) { totalCount }
      workflowRun {
        id databaseId runAttempt runNumber event createdAt updatedAt displayTitle
        workflow { databaseId name }
        file { path }
        checkSuite { databaseId commit { oid } }
      }
    }
  }
}"#;

impl Client {
    pub(crate) async fn commit_lists_response(
        &self,
        repository: &str,
        head: &str,
        merge: Option<&str>,
        freshness: Freshness,
    ) -> Result<Response> {
        validate_repository(repository)?;
        let (owner, repo) = repository.split_once('/').expect("validated repository");
        self.request_versioned(
            self.0.config.graphql_url.to_string(),
            Some(json!({"query":QUERY,"variables":{
                "owner":owner,"repo":repo,"head":head,"merge":merge.unwrap_or(head),
                "hasMerge":merge.is_some_and(|sha| sha != head)
            }})),
            freshness,
            None,
            self.ci_uses_installation(repository),
        )
        .await
    }
}
