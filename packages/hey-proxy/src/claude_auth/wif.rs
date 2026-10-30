//! RFC 7523 federation, following Anthropic's official SDK. No refresh tokens or disk store.
use super::*;
use hey_proxy::credentials::CredentialResolver;
use std::collections::HashMap;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub organization_id: String,
    pub service_account_id: String,
    pub federation_rule_id: String,
    /// A trusted command obtaining a fresh JWT, or an owner-only rotating JWT file.
    pub identity_token: String,
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        for id in [
            &self.organization_id,
            &self.service_account_id,
            &self.federation_rule_id,
        ] {
            ensure!(
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
                "Invalid federation identity ID"
            );
        }
        ensure!(
            self.service_account_id.starts_with("svac_")
                && self.federation_rule_id.starts_with("fdrl_"),
            "Federation requires service account and federation rule IDs"
        );
        ensure!(
            self.identity_token.starts_with("sh://") || self.identity_token.starts_with("file://"),
            "Federation requires a rotating file:// or sh:// identity token source"
        );
        hey_proxy::credentials::validate_source(&self.identity_token)
    }
    pub fn identity(&self, workspace: &str) -> String {
        format!(
            "wif:{}:{}:{workspace}",
            self.organization_id, self.service_account_id
        )
    }
}

#[derive(Default)]
struct Entry {
    token: Option<(String, Instant)>,
    failed: Option<Instant>,
}
type Cell = Arc<Mutex<Entry>>;
#[derive(Default)]
pub(crate) struct Manager(Mutex<HashMap<(Config, String, String), Cell>>);
impl Manager {
    pub async fn token(
        &self,
        provider: &crate::proxy::claude::ProviderConfig,
        client: &reqwest::Client,
        credentials: &CredentialResolver,
        rejected: Option<&str>,
    ) -> Result<String> {
        let config = provider
            .federation
            .as_ref()
            .context("Federation is not configured")?;
        let workspace = provider
            .workspace_id
            .as_deref()
            .context("Federation requires a workspace")?;
        let key = (
            config.clone(),
            workspace.to_owned(),
            provider.upstream_url.clone(),
        );
        let cell = {
            let mut entries = self.0.lock().await;
            if entries.len() >= 128 && !entries.contains_key(&key) {
                entries.retain(|_, cell| Arc::strong_count(cell) > 1);
                ensure!(entries.len() < 128, "Federation cache busy; retry shortly");
            }
            entries.entry(key).or_default().clone()
        };
        let mut entry = cell.lock().await;
        if let Some((token, expires)) = &entry.token {
            if *expires > Instant::now() && rejected != Some(token.as_str()) {
                return Ok(token.clone());
            }
        }
        entry.token = None;
        ensure!(
            !entry
                .failed
                .is_some_and(|when| when.elapsed() < Duration::from_secs(30)),
            "Federation exchange unavailable; retry shortly"
        );
        let result = async {
            let assertion = credentials.resolve(&config.identity_token, Duration::ZERO).await
                .map_err(|_| anyhow::anyhow!("Federation identity token source unavailable"))?;
            let assertion = assertion.to_str().map_err(|_| anyhow::anyhow!("Invalid federation identity token"))?;
            ensure!(assertion.len() <= 16 * 1024 && assertion.split('.').count() == 3, "Federation source must return an OIDC JWT (at most 16 KiB)");
            let mut response = client.post(format!("{}/v1/oauth/token", provider.upstream_url.trim_end_matches('/')))
                .header("anthropic-beta", "oauth-2025-04-20,oidc-federation-2026-04-01")
                .timeout(Duration::from_secs(30))
                .json(&json!({"grant_type":"urn:ietf:params:oauth:grant-type:jwt-bearer",
                    "assertion":assertion,"organization_id":config.organization_id,
                    "service_account_id":config.service_account_id,"federation_rule_id":config.federation_rule_id,
                    "workspace_id":workspace}))
                .send().await.map_err(|_| anyhow::anyhow!("Cannot reach Claude federation endpoint"))?;
            ensure!(response.status().is_success(), "Claude federation exchange failed (HTTP {}); check the Console authentication history and IAM role", response.status().as_u16());
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| anyhow::anyhow!("Cannot read federation response"))? {
                ensure!(bytes.len() + chunk.len() <= 65536, "Federation response too large");
                bytes.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("Invalid federation response"))?;
            let token = value["access_token"].as_str().filter(|t| is_oauth_token(t)).context("Federation returned no access token")?;
            ensure!(value["token_type"].is_null() || value["token_type"].as_str().is_some_and(|t| t.eq_ignore_ascii_case("bearer")), "Invalid federation token type");
            let expires = value["expires_in"].as_u64().filter(|t| *t > 0 && *t <= 86400).context("Invalid federation token lifetime")?;
            Ok((token.to_owned(), Instant::now() + Duration::from_secs(expires.saturating_sub(60))))
        }.await;
        match result {
            Ok((token, expiry)) => {
                entry.token = Some((token.clone(), expiry));
                entry.failed = None;
                Ok(token)
            }
            Err(error) => {
                entry.failed = Some(Instant::now());
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::post};
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn wif_exchange_caches_by_workspace_and_reacquires_assertions_after_rejection() {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let app = Router::new().route("/v1/oauth/token", post(move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
            let calls = calls.clone();
            async move {
                assert_eq!(body["grant_type"], "urn:ietf:params:oauth:grant-type:jwt-bearer");
                assert_eq!(body["federation_rule_id"], "fdrl_test");
                assert_eq!(body["organization_id"], "org-test");
                assert_eq!(body["service_account_id"], "svac_test");
                assert!(headers["anthropic-beta"].to_str().unwrap().contains("oidc-federation-2026-04-01"));
                let n = calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(body["assertion"], if n == 0 {"old.jwt.assertion"} else {"new.jwt.assertion"});
                Json(json!({"access_token":format!("sk-ant-oat01-wif-{n}"),"token_type":"Bearer","expires_in":if body["workspace_id"] == "wrkspc_other" { 60 } else { 3600 }}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jwt");
        crate::rollout::write_private(&path, b"old.jwt.assertion", false).unwrap();
        let mut provider: crate::proxy::claude::ProviderConfig = serde_json::from_value(json!({
            "auth":"wif","upstream_url":endpoint,"workspace_id":"wrkspc_example",
            "federation":{"organization_id":"org-test","service_account_id":"svac_test","federation_rule_id":"fdrl_test","identity_token":format!("file://{}",path.display())}
        })).unwrap();
        provider.validate(crate::config::Mode::Standalone).unwrap();
        let manager = Manager::default();
        let client = reqwest::Client::new();
        let credentials = CredentialResolver::default();
        let (a, b) = tokio::join!(
            manager.token(&provider, &client, &credentials, None),
            manager.token(&provider, &client, &credentials, None)
        );
        let first = a.unwrap();
        assert_eq!(first, b.unwrap());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        crate::rollout::write_private(&path, b"new.jwt.assertion", false).unwrap();
        let second = manager
            .token(&provider, &client, &credentials, Some(&first))
            .await
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(
            manager
                .token(&provider, &client, &credentials, Some(&first))
                .await
                .unwrap(),
            second
        );
        provider.workspace_id = Some("wrkspc_other".into());
        assert_ne!(
            manager
                .token(&provider, &client, &credentials, None)
                .await
                .unwrap(),
            second
        );
        assert_eq!(count.load(Ordering::SeqCst), 3);
        manager
            .token(&provider, &client, &credentials, None)
            .await
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 4);
        server.abort();
    }
    #[tokio::test]
    async fn wif_failures_back_off_and_never_expose_assertion_or_response() {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/v1/oauth/token",
            post(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { (axum::http::StatusCode::UNAUTHORIZED, "SECRET_ASSERTION") }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = serde_json::from_value(json!({"auth":"wif","upstream_url":endpoint,"workspace_id":"wrkspc_example",
            "federation":{"organization_id":"org-test","service_account_id":"svac_test","federation_rule_id":"fdrl_test","identity_token":"sh://printf SECRET_ASSERTION.jwt.token"}})).unwrap();
        let manager = Manager::default();
        for _ in 0..2 {
            let error = manager
                .token(
                    &provider,
                    &reqwest::Client::new(),
                    &CredentialResolver::default(),
                    None,
                )
                .await
                .unwrap_err()
                .to_string();
            assert!(!error.contains("SECRET_ASSERTION"));
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
