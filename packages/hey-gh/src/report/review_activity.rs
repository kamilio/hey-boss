//! One first-page read shared only by the sources in this PR collection.
use super::{Client, Error, Freshness, Result, json};
use crate::Response;
use std::sync::Arc;
use tokio::sync::OnceCell;

const QUERY: &str = r#"query ReviewActivity($owner:String!,$repo:String!,$number:Int!,$after:String) {
 repository(owner:$owner,name:$repo) { pullRequest(number:$number) {
 reviewThreads(first:100,after:$after) {
 pageInfo { hasNextPage endCursor } nodes { id isResolved isOutdated path line startLine diffSide startDiffSide
 resolvedBy { login } comments(first:100) { pageInfo { hasNextPage endCursor } nodes {
 id databaseId body createdAt updatedAt url path line originalLine pullRequestReview { databaseId } author { login __typename } replyTo { id databaseId } } } } }
 timelineItems(first:100,after:$after,itemTypes:[REVIEW_REQUESTED_EVENT,REVIEW_REQUEST_REMOVED_EVENT]) {
 pageInfo { hasNextPage endCursor } nodes {
 __typename ... on ReviewRequestedEvent { id createdAt requestedReviewer { __typename ... on User { login } ... on Team { slug name } } }
 ... on ReviewRequestRemovedEvent { id createdAt requestedReviewer { __typename ... on User { login } ... on Team { slug name } } }
 } }
 } }
}"#;

pub(super) struct FirstPage<'a> {
    client: &'a Client,
    repository: &'a str,
    number: u64,
    freshness: Freshness,
    response: OnceCell<Result<Arc<Response>>>,
}

impl<'a> FirstPage<'a> {
    pub(super) fn new(
        client: &'a Client,
        repository: &'a str,
        number: u64,
        freshness: Freshness,
    ) -> Self {
        Self {
            client,
            repository,
            number,
            freshness,
            response: OnceCell::new(),
        }
    }

    async fn read(&self, query: &str, freshness: Freshness) -> Result<Arc<Response>> {
        let repository = self
            .client
            .pr_repository_spelling(self.repository, self.number)
            .await?;
        let (owner, repo) = repository.split_once('/').expect("validated repository");
        self.client
            .graphql(
                query,
                json!({"owner":owner,"repo":repo,"number":self.number,"after":null}),
                freshness,
            )
            .await
            .map(Arc::new)
    }

    pub(super) async fn get(&self, query: &str, connection: &str) -> Result<Arc<Response>> {
        let shared = self
            .response
            .get_or_init(|| self.read(QUERY, self.freshness))
            .await;
        match shared {
            Ok(response)
                if response.data["data"]["repository"]["pullRequest"][connection].is_object()
                    && response.data.to_string().len() <= self.client.collection_limit() =>
            {
                // Each source counts its own connection against its existing
                // pagination limit. The shared cache entry remains unchanged;
                // both projections retain the actual response's validation time.
                Ok(Arc::new(Response {
                    data: json!({"data":{"repository":{"pullRequest":{
                        connection: &response.data["data"]["repository"]["pullRequest"][connection]
                    }}}}),
                    fetched_at_ms: response.fetched_at_ms,
                    validated_at_ms: response.validated_at_ms,
                    source: response.source.clone(),
                    etag: response.etag.clone(),
                    last_modified: response.last_modified.clone(),
                    link: response.link.clone(),
                }))
            }
            // A combined failure cannot suppress an independently readable
            // source. This also retains legacy offline query-cache evidence.
            Ok(_) | Err(Error::GraphQL { .. } | Error::Invalid(_) | Error::CacheMiss) => {
                // A new denial must not recover from an older successful
                // standalone cache entry. Offline reads still never dispatch.
                let freshness = if matches!(self.freshness, Freshness::CachedOnly) {
                    Freshness::CachedOnly
                } else {
                    Freshness::Revalidate
                };
                self.read(query, freshness).await
            }
            Err(error) => Err(error.clone()),
        }
    }
}
