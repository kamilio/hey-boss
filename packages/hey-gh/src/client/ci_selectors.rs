use super::{Client, Error, Freshness, Response, Result, validate_repository};
use serde_json::json;

const CI_SELECTORS: &str = r#"query CiSelectors($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      id number state merged mergeable headRefOid baseRefOid
      repository { nameWithOwner }
      commits(last: 1) { nodes { commit { oid status { id } statusCheckRollup { contexts(first: 1) { checkRunCount } } } } }
      potentialMergeCommit { oid status { id } statusCheckRollup { contexts(first: 1) { checkRunCount } } parents(first: 2) { totalCount nodes { oid } } }
    }
  }
}"#;

impl Client {
    pub(crate) async fn ci_selectors(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Response> {
        let response = self
            .ci_selector_response(repository, number, freshness)
            .await?;
        crate::report::record_validation(self.0.config.graphql_url.as_str(), &response);
        Ok(response)
    }

    // No caller-supplied query or opt-in on the generic GraphQL interface.
    // Cache-only evidence peeks use exactly the same route as fresh reads.
    pub(crate) async fn ci_selector_response(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Response> {
        validate_repository(repository)?;
        if number == 0 || number > i32::MAX as u64 {
            return Err(Error::Invalid("invalid CI pull request number".into()));
        }
        let (owner, repo) = repository.split_once('/').expect("validated repository");
        let response = self.request_versioned(
            self.0.config.graphql_url.to_string(),
            Some(json!({"query":CI_SELECTORS,"variables":{"owner":owner,"repo":repo,"number":number}})),
            freshness,
            None,
            self.ci_uses_installation(repository),
        ).await;
        if matches!(freshness, Freshness::CachedOnly) && matches!(response, Err(Error::CacheMiss)) {
            // Preserve pre-count selector/status evidence across upgrades.
            // This exact old query is only a cache lookup, under the same
            // provider and generation fences; online reads use the new query.
            let legacy = CI_SELECTORS.replace(
                " statusCheckRollup { contexts(first: 1) { checkRunCount } }",
                "",
            );
            return self.request_versioned(
                self.0.config.graphql_url.to_string(),
                Some(json!({"query":legacy,"variables":{"owner":owner,"repo":repo,"number":number}})),
                Freshness::CachedOnly,
                None,
                self.ci_uses_installation(repository),
            ).await;
        }
        response
    }
}
