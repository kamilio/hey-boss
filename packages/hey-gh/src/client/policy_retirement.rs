use super::*;

impl Client {
    pub(crate) async fn policy_terminal_cached(
        &self,
        repository: &str,
        number: u64,
        node: &Value,
    ) -> Result<bool> {
        let Some(node_id) = node["id"].as_str().filter(|id| !id.is_empty()) else {
            return Ok(false);
        };
        let updated_at = match node.get("updatedAt") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(value.as_str()),
            _ => return Ok(false),
        };
        let Some(owner) = self
            .0
            .store
            .terminal_pr_owner(
                &self.0.scope,
                &format!("pr-status://{}/{repository}/{number}", self.hostname()),
                repository,
                number,
                node_id,
                updated_at,
            )
            .await?
        else {
            return Ok(false);
        };
        // This reads existing personal/App CI metadata only. It never obtains
        // lifecycle evidence from the App or dispatches an upstream request.
        // A newer reopen can precede its publication or account rediscovery.
        if let Some(latest) = self.peek_ci_pull_request(repository, number).await?
            && (latest.validated_at_ms == 0
                || latest.validated_at_ms > now_ms()
                || latest.data["node_id"] != node_id
                || latest.data["number"] != number
                || !latest.data["base"]["repo"]["full_name"]
                    .as_str()
                    .is_some_and(|repo| repo.eq_ignore_ascii_case(repository))
                || latest.data["state"] != "closed")
        {
            return Ok(false);
        }
        // Another reader may have replaced this entity while caches were read.
        self.0.store.owner_is_current(&self.0.scope, &owner).await
    }
}
