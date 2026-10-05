use super::{Client, Error, Freshness, Response, Result, validate_repository};

impl Client {
    // Generic readers must fetch their own payload, but must not publish an old
    // lifecycle or selector after CI has observed a newer conflicting version.
    pub(crate) async fn personal_pr_metadata_superseded(
        &self,
        repository: &str,
        number: u64,
        personal: &Response,
    ) -> Result<bool> {
        if !self.ci_uses_installation(repository) {
            return Ok(false);
        }
        let app = match self
            .ci_pr_response(repository, number, Freshness::CachedOnly)
            .await
        {
            Ok(app) => app,
            Err(Error::CacheMiss) => return Ok(false),
            Err(error) => return Err(error),
        };
        Ok(app.validated_at_ms > 0
            && app.validated_at_ms >= personal.validated_at_ms
            && app.validated_at_ms <= crate::now_ms()
            && [
                "/node_id",
                "/number",
                "/state",
                "/merged",
                "/draft",
                "/updated_at",
                "/closed_at",
                "/merged_at",
                "/head/sha",
                "/base/sha",
                "/base/ref",
                "/merge_commit_sha",
                "/mergeable",
                "/mergeable_state",
            ]
            .iter()
            .any(|field| app.data.pointer(field) != personal.data.pointer(field)))
    }

    // Fixed CI-only endpoint. Generic PR metadata and activity stay personal.
    async fn ci_pr_response(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Response> {
        validate_repository(repository)?;
        if number == 0 {
            return Err(Error::Invalid("pull number must be positive".into()));
        }
        self.request_versioned(
            self.rest_url(&format!("repos/{repository}/pulls/{number}"))?
                .to_string(),
            None,
            freshness,
            None,
            self.ci_uses_installation(repository),
        )
        .await
    }

    // A personal cache read is free evidence, not a network fallback. Prefer the
    // newest full payload, including closed PRs, before choosing a collection seed.
    pub(crate) async fn peek_ci_pull_request(
        &self,
        repository: &str,
        number: u64,
    ) -> Result<Option<Response>> {
        let routed = match self
            .ci_pr_response(repository, number, Freshness::CachedOnly)
            .await
        {
            Ok(response) => Some(response),
            Err(Error::CacheMiss) => None,
            Err(error) => return Err(error),
        };
        if !self.ci_uses_installation(repository) {
            return Ok(routed);
        }
        let personal = match self
            .peek_get(&format!("repos/{repository}/pulls/{number}"))
            .await
        {
            Ok(response) => Some(response),
            Err(Error::CacheMiss) => None,
            Err(error) => return Err(error),
        };
        Ok(match (routed, personal) {
            (Some(app), Some(personal)) if personal.validated_at_ms > app.validated_at_ms => {
                Some(personal)
            }
            (Some(app), _) => Some(app),
            (None, personal) => personal,
        })
    }

    pub(crate) async fn ci_pull_request(
        &self,
        repository: &str,
        number: u64,
        freshness: Freshness,
    ) -> Result<Response> {
        let cached = if !matches!(freshness, Freshness::Revalidate)
            && !matches!(freshness, Freshness::MaxAge(age) if age.is_zero())
        {
            self.peek_ci_pull_request(repository, number)
                .await?
                .filter(|cached| {
                    matches!(freshness, Freshness::CachedOnly)
                        || matches!(freshness, Freshness::MaxAge(age) if crate::now_ms()
                        .checked_sub(cached.validated_at_ms)
                        .is_some_and(|elapsed| (elapsed as u128) < age.as_millis()))
                })
        } else {
            None
        };
        let response = match cached {
            Some(cached) => cached,
            None => self.ci_pr_response(repository, number, freshness).await?,
        };
        crate::report::record_validation(
            self.rest_url(&format!("repos/{repository}/pulls/{number}"))?
                .as_str(),
            &response,
        );
        Ok(response)
    }
}
