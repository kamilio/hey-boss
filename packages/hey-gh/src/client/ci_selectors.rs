use super::{Client, Error, Freshness, Response, Result, validate_repository};
use serde_json::json;

const CI_SELECTORS: &str = r#"query CiSelectors($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      id number state merged mergeable headRefOid baseRefOid
      repository { nameWithOwner }
      commits(last: 1) { nodes { commit { oid status { id contexts { id updatedAt context state description targetUrl } } statusCheckRollup { contexts(first: 1) { checkRunCount statusContextCount } } } } }
      potentialMergeCommit { oid status { id contexts { id updatedAt context state description targetUrl } } statusCheckRollup { contexts(first: 1) { checkRunCount statusContextCount } } parents(first: 2) { totalCount nodes { oid } } }
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
            // Exact legacy queries are cache lookups only, under the same
            // provider/generation fences. Online reads always use the new query.
            let counted = CI_SELECTORS
                .replace(
                    " contexts { id updatedAt context state description targetUrl }",
                    "",
                )
                .replace("checkRunCount statusContextCount", "checkRunCount");
            let legacy = counted.replace(
                " statusCheckRollup { contexts(first: 1) { checkRunCount } }",
                "",
            );
            for query in [counted, legacy] {
                let cached = self.request_versioned(
                    self.0.config.graphql_url.to_string(),
                    Some(json!({"query":query,"variables":{"owner":owner,"repo":repo,"number":number}})),
                    Freshness::CachedOnly,
                    None,
                    self.ci_uses_installation(repository),
                ).await;
                if !matches!(cached, Err(Error::CacheMiss)) {
                    return cached;
                }
            }
        }
        response
    }
}
