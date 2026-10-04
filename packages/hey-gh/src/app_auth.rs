//! Read-only installation authentication. Private keys and minted tokens are
//! never included in Debug, errors, the response cache, or diagnostic records.
use crate::{Error, Result, digest, now_ms};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

mod storage;
pub use storage::{configure, load};

#[derive(Clone)]
pub struct AppInstallation(Arc<Installation>);

struct Installation {
    client_id: String,
    installation_id: u64,
    repositories: BTreeSet<String>,
    key: EncodingKey,
    scope: String,
    state: Mutex<TokenState>,
}

#[derive(Default)]
struct TokenState {
    token: Option<(String, Instant)>,
    generation: u64,
    failure: Option<(Error, Instant)>,
}

impl AppInstallation {
    pub fn new(
        client_id: String,
        installation_id: u64,
        repositories: Vec<String>,
        private_key: &str,
    ) -> Result<Self> {
        if client_id.is_empty()
            || client_id.len() > 200
            || !client_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || installation_id == 0
        {
            return Err(Error::Invalid(
                "invalid GitHub App client/installation ID".into(),
            ));
        }
        let repositories: BTreeSet<_> = repositories
            .into_iter()
            .map(|r| r.to_ascii_lowercase())
            .collect();
        if repositories.is_empty()
            || repositories.len() > 500
            || repositories
                .iter()
                .any(|r| crate::client::validate_repository(r).is_err())
            || repositories
                .iter()
                .filter_map(|r| r.split('/').next())
                .collect::<BTreeSet<_>>()
                .len()
                != 1
        {
            return Err(Error::Invalid(
                "GitHub App requires 1..500 explicit repositories from one installation owner"
                    .into(),
            ));
        }
        let key = EncodingKey::from_rsa_pem(private_key.as_bytes())
            .map_err(|_| Error::Invalid("invalid GitHub App RSA private key".into()))?;
        let scope = digest(
            &json!([
                "installation-read-v2",
                client_id,
                installation_id,
                repositories
            ])
            .to_string(),
        );
        Ok(Self(Arc::new(Installation {
            client_id,
            installation_id,
            repositories,
            key,
            scope,
            state: Mutex::new(TokenState::default()),
        })))
    }

    pub(crate) fn covers(&self, repository: &str) -> bool {
        self.0
            .repositories
            .contains(&repository.to_ascii_lowercase())
    }
    pub(crate) fn scope(&self) -> &str {
        &self.0.scope
    }

    pub(crate) fn token(&self) -> Result<Option<(String, u64)>> {
        let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if let Some((error, until)) = &state.failure
            && *until > now
        {
            return Err(error.clone());
        }
        Ok(state
            .token
            .as_ref()
            .filter(|(_, until)| *until > now)
            .map(|(token, _)| (token.clone(), state.generation)))
    }

    pub(crate) fn request(
        &self,
        http: &reqwest::Client,
        base: &url::Url,
    ) -> Result<reqwest::RequestBuilder> {
        #[derive(Serialize)]
        struct Claims<'a> {
            iat: u64,
            exp: u64,
            iss: &'a str,
        }
        let seconds = now_ms() / 1000;
        let jwt = encode(
            &Header::new(Algorithm::RS256),
            &Claims {
                iat: seconds.saturating_sub(60),
                exp: seconds + 540,
                iss: &self.0.client_id,
            },
            &self.0.key,
        )
        .map_err(|_| Error::Invalid("cannot sign GitHub App JWT".into()))?;
        let url = base
            .join(&format!(
                "app/installations/{}/access_tokens",
                self.0.installation_id
            ))
            .map_err(|_| Error::Invalid("invalid GitHub App API URL".into()))?;
        Ok(http.post(url).bearer_auth(jwt).json(&json!({
            "repositories":self.0.repositories.iter().map(|r| r.split_once('/').unwrap().1).collect::<Vec<_>>(),
            "permissions":{"actions":"read", "checks":"read", "statuses":"read", "metadata":"read"}
        })))
    }

    pub(crate) fn accept(&self, bytes: &[u8]) -> Result<()> {
        #[derive(Deserialize)]
        struct Token {
            token: String,
            expires_at: String,
        }
        let token: Token = serde_json::from_slice(bytes)
            .map_err(|_| Error::Invalid("invalid GitHub App token response".into()))?;
        let expiry = chrono::DateTime::parse_from_rfc3339(&token.expires_at)
            .map_err(|_| Error::Invalid("invalid GitHub App token expiry".into()))?
            .timestamp_millis();
        let ttl = (expiry.max(0) as u64)
            .saturating_sub(now_ms())
            .min(3_600_000);
        if token.token.is_empty()
            || token.token.len() > 4096
            || token.token.bytes().any(|b| b.is_ascii_control())
            || ttl <= 60_000
        {
            return Err(Error::Invalid(
                "GitHub App returned an empty or expiring token".into(),
            ));
        }
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.token = Some((
            token.token,
            Instant::now() + Duration::from_millis(ttl - 60_000),
        ));
        state.generation = state.generation.wrapping_add(1);
        state.failure = None;
        Ok(())
    }

    pub(crate) fn failed(&self, error: Error) {
        let delay = match &error {
            Error::RateLimited {
                retry_after_seconds,
            } => (*retry_after_seconds).max(1),
            _ => 60,
        };
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .failure = Some((
            error,
            Instant::now() + Duration::from_secs(delay.min(86400)),
        ));
    }

    pub(crate) fn invalidate(&self, generation: u64) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation == generation {
            state.token = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn installation() -> AppInstallation {
        AppInstallation::new(
            "test-client".into(),
            42,
            vec!["Acme/Demo".into()],
            include_str!("../tests/fixtures/github-app-test-key.pem"),
        )
        .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn token_expires_early_without_changing_cache_identity() {
        let app = installation();
        let scope = app.scope().to_owned();
        app.accept(
            br#"{"token":"synthetic-installation-token","expires_at":"2099-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(app.token().unwrap().is_some());
        tokio::time::advance(Duration::from_secs(3541)).await;
        assert!(app.token().unwrap().is_none());
        assert_eq!(app.scope(), scope);
        app.accept(br#"{"token":"synthetic-next-token","expires_at":"2099-01-01T00:00:00Z"}"#)
            .unwrap();
        assert_eq!(app.token().unwrap().unwrap().0, "synthetic-next-token");
        assert_eq!(installation().scope(), scope);
    }

    #[test]
    fn exchange_signs_short_lived_jwt_and_requests_only_selected_read_permissions() {
        let app = installation();
        let request = app
            .request(
                &reqwest::Client::new(),
                &"https://api.github.com/".parse().unwrap(),
            )
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://api.github.com/app/installations/42/access_tokens"
        );
        let jwt = request.headers()["authorization"]
            .to_str()
            .unwrap()
            .strip_prefix("Bearer ")
            .unwrap();
        let key = jsonwebtoken::DecodingKey::from_rsa_pem(include_bytes!(
            "../tests/fixtures/github-app-test-public.pem"
        ))
        .unwrap();
        let claims = jsonwebtoken::decode::<serde_json::Value>(
            jwt,
            &key,
            &jsonwebtoken::Validation::new(Algorithm::RS256),
        )
        .unwrap()
        .claims;
        assert_eq!(claims["iss"], "test-client");
        assert_eq!(
            claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(),
            600
        );
        let body: serde_json::Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["repositories"], json!(["demo"]));
        assert!(
            body["permissions"]
                .as_object()
                .unwrap()
                .values()
                .all(|v| v == "read")
        );
        assert_eq!(
            body["permissions"],
            json!({"actions":"read","checks":"read","statuses":"read","metadata":"read"})
        );
        assert!(app.covers("ACME/demo"));
        assert!(!app.covers("acme/demolition"));
    }

    #[test]
    fn a_late_unauthorized_response_does_not_discard_a_renewed_token() {
        let app = installation();
        let response =
            br#"{"token":"synthetic-installation-token","expires_at":"2099-01-01T00:00:00Z"}"#;
        app.accept(response).unwrap();
        let (_, old) = app.token().unwrap().unwrap();
        app.accept(response).unwrap();
        let (_, current) = app.token().unwrap().unwrap();
        app.invalidate(old);
        assert!(app.token().unwrap().is_some());
        app.invalidate(current);
        assert!(app.token().unwrap().is_none());
    }
}
