//! Explicit named connections. Selection pins credentials and identity for a request.
use super::*;
use crate::config::accounts::AccountConfig;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Registry {
    codex: Mutex<HashMap<PathBuf, Arc<codex::CodexState>>>,
    claude: Mutex<HashMap<PathBuf, Arc<claude::ClaudeState>>>,
    quotas: Mutex<HashMap<String, Arc<Quota>>>,
}
#[derive(Default)]
pub(super) struct Quota {
    pub codex: tokio::sync::Mutex<codex::usage::Cache>,
    pub claude: tokio::sync::Mutex<claude::usage::Cache>,
}
#[derive(Clone)]
pub(super) struct Binding {
    pub reference: String,
    pub token: String,
    pub account_id: Option<String>,
    pub implementation: &'static str,
    pub quota: Arc<Quota>,
}
fn reference(key: &[u8; 32], implementation: &str, identity: &str) -> String {
    // Secret prefix plus a fixed-length digest prevents guessing public identities.
    let digest = Sha256::digest(identity.as_bytes());
    let mut hash = Sha256::new();
    hash.update(key);
    hash.update(b"hey-proxy account v1\0");
    hash.update(implementation.as_bytes());
    hash.update([0]);
    hash.update(digest);
    format!("acct_{:x}", hash.finalize())
}
fn state<T: Default>(map: &Mutex<HashMap<PathBuf, Arc<T>>>, path: PathBuf) -> Arc<T> {
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    // Config edits must not cause unbounded retained credential caches.
    if map.len() >= 256 {
        map.retain(|_, value| Arc::strong_count(value) > 1);
    }
    map.entry(path).or_default().clone()
}
impl Registry {
    pub(super) fn defaults(
        &self,
        config: &Config,
        source: Option<&std::path::Path>,
    ) -> (Arc<claude::ClaudeState>, Arc<codex::CodexState>) {
        let normalize = |path: PathBuf| std::fs::canonicalize(&path).unwrap_or(path);
        let claude = config
            .claude
            .clone()
            .unwrap_or_default()
            .credentials_path(source)
            .ok()
            .map(|p| state(&self.claude, normalize(p)))
            .unwrap_or_default();
        let codex = config
            .codex
            .clone()
            .unwrap_or_default()
            .credentials_path(source)
            .ok()
            .map(|p| state(&self.codex, normalize(p)))
            .unwrap_or_default();
        (claude, codex)
    }
}
impl Proxy {
    /// Rotate only the selected store for the next request; never rebind this request.
    pub(super) async fn rejected_binding(&self) {
        let Some(binding) = &self.binding else { return };
        if let Some(provider) = &self.config.codex {
            if let Ok(path) = provider.credentials_path(self.service.source.as_deref())
                && let Ok(path) = canonical(path).await
            {
                let _ = self
                    .codex
                    .tokens
                    .rejected(
                        &path,
                        &self.client,
                        &provider.token_url(),
                        binding.token.clone(),
                    )
                    .await;
            }
        } else if let Some(provider) = &self.config.claude
            && let Ok(path) = provider.credentials_path(self.service.source.as_deref())
            && let Ok(path) = canonical(path).await
        {
            let _ = self
                .claude
                .tokens
                .rejected(&path, &self.client, binding.token.clone())
                .await;
        }
    }
    pub(super) async fn quota_for(
        &self,
        implementation: &str,
        identity: &str,
    ) -> Result<(String, Arc<Quota>)> {
        let path = self
            .service
            .source
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Named accounts require a file-backed config"))?;
        let key = tokio::task::spawn_blocking(move || config::account_key(&path)).await??;
        let reference = reference(&key, implementation, identity);
        let quota = {
            let mut quotas = self
                .service
                .accounts
                .quotas
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // Keep cooldowns through renames/removals; bounded by a generous host limit.
            anyhow::ensure!(
                quotas.contains_key(&reference) || quotas.len() < 4096,
                "Account identity cache is full; restart the service"
            );
            quotas.entry(reference.clone()).or_default().clone()
        };
        Ok((reference, quota))
    }
    pub(crate) async fn select(&self, name: &str) -> Result<Self> {
        anyhow::ensure!(
            self.config.mode != Mode::Client,
            "Select accounts on the credential-owning host"
        );
        let account = self
            .config
            .accounts
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown named provider"))?;
        let mut config = account.apply(&self.config);
        let mut selected = Self {
            config: self.config.clone(),
            client: self.client.clone(),
            service: self.service.clone(),
            log_id: self.log_id,
            fallback_attempt: self.fallback_attempt,
            claude: self.claude.clone(),
            codex: self.codex.clone(),
            binding: None,
        };
        let (token, identity, account_id) = match account {
            AccountConfig::Codex { .. } => {
                let provider = config.codex.as_ref().unwrap();
                let path =
                    canonical(provider.credentials_path(self.service.source.as_deref())?).await?;
                selected.codex = state(&self.service.accounts.codex, path.clone());
                let creds = selected
                    .codex
                    .tokens
                    .credentials(&path, &self.client, &provider.token_url())
                    .await?;
                let id = creds
                    .account_id
                    .clone()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Codex account identity unavailable; sign in again")
                    })?;
                config.upstream_url = format!(
                    "{}/backend-api/codex",
                    provider
                        .upstream_url
                        .trim_end_matches('/')
                        .trim_end_matches("/backend-api")
                );
                config
                    .api_keys
                    .insert("default".into(), creds.access_token.clone());
                (creds.access_token, id, creds.account_id)
            }
            AccountConfig::Claude { .. } => {
                let provider = config.claude.as_ref().unwrap();
                let path =
                    canonical(provider.credentials_path(self.service.source.as_deref())?).await?;
                selected.claude = state(&self.service.accounts.claude, path.clone());
                let token = selected.claude.tokens.token(&path, &self.client).await?;
                let id = crate::claude_auth::account_identity(
                    &path,
                    &self.client,
                    &provider.upstream_url,
                    &token,
                )
                .await?;
                (token, id, None)
            }
            AccountConfig::Openai {
                credential,
                endpoint,
                ..
            } => {
                let token = self
                    .service
                    .credentials
                    .resolve(
                        credential,
                        Duration::from_secs(config.credential_cache_seconds),
                    )
                    .await?;
                let token = token.to_str()?.to_owned();
                // API connections have no subscription quota. Key rotation changes the pin.
                let identity = format!("{}\0{}", endpoint.trim_end_matches('/'), token);
                config.api_keys.insert("default".into(), token.clone());
                (token, identity, None)
            }
        };
        let (reference, quota) = self.quota_for(account.implementation(), &identity).await?;
        selected.binding = Some(Binding {
            reference,
            token,
            account_id,
            implementation: account.implementation(),
            quota,
        });
        selected.config = Arc::new(config);
        Ok(selected)
    }
}
async fn canonical(path: PathBuf) -> Result<PathBuf> {
    tokio::task::spawn_blocking(move || {
        std::fs::canonicalize(path)
            .map_err(|_| anyhow::anyhow!("Account credential store unavailable"))
    })
    .await?
}

pub(super) async fn catalog(State(service): State<Arc<Service>>) -> Response {
    let proxy = service.snapshot();
    if proxy.config.mode == Mode::Client {
        return subscription::relay::<hey_proxy::usage::Connections>(&proxy, "/providers/v1").await;
    }
    let items: Vec<_> = proxy
        .config
        .accounts
        .iter()
        .map(|(name, a)| {
            (
                name.clone(),
                a.implementation().to_owned(),
                a.auth().to_owned(),
            )
        })
        .collect();
    let connections: Vec<_> = futures_util::stream::iter(items)
        .map(|(name, implementation, auth)| {
            let proxy = &proxy;
            async move {
                let selected = tokio::time::timeout(Duration::from_secs(20), proxy.select(&name))
                    .await
                    .ok()
                    .and_then(Result::ok);
                hey_proxy::usage::Connection {
                    name,
                    implementation,
                    auth,
                    ready: selected.is_some(),
                    account_ref: selected.and_then(|s| s.binding.map(|b| b.reference)),
                }
            }
        })
        .buffered(8)
        .collect()
        .await;
    ([(header::CACHE_CONTROL, "no-store")], axum::Json(json!({"schema_version":1,
        "capabilities":["named_accounts", "pinned_inference", "shared_subscription_quota"], "connections": connections}))).into_response()
}

pub(super) async fn bind(proxy: &Proxy, request: &mut Request) -> Result<Option<Proxy>> {
    let name = request.headers().get("x-hey-proxy-provider");
    let expected = request.headers().get("x-hey-proxy-account");
    anyhow::ensure!(
        name.is_some() || expected.is_none(),
        "Account reference requires a named provider"
    );
    let Some(name) = name else { return Ok(None) };
    if proxy.config.mode == Mode::Client {
        return Ok(None);
    }
    let selected = proxy
        .select(name.to_str()?)
        .await
        .map_err(|_| anyhow::anyhow!("Named provider is unavailable"))?;
    let binding = selected.binding.as_ref().unwrap();
    anyhow::ensure!(
        expected.and_then(|v| v.to_str().ok()) == Some(binding.reference.as_str()),
        "Account binding missing or changed; select the account explicitly"
    );
    let path = request.uri().path();
    match binding.implementation {
        "claude" => anyhow::ensure!(
            claude::is_path(path),
            "Claude subscription requires the Messages API"
        ),
        "codex" => {
            anyhow::ensure!(
                matches!(
                    path,
                    "/v1/responses" | "/responses" | "/v1/responses/compact" | "/responses/compact"
                ),
                "Codex subscription requires the Responses API"
            );
            let mut parts = request.uri().clone().into_parts();
            let path = request
                .uri()
                .path_and_query()
                .unwrap()
                .as_str()
                .trim_start_matches("/v1");
            parts.path_and_query = Some(path.parse()?);
            *request.uri_mut() = axum::http::Uri::from_parts(parts)?;
        }
        _ => {}
    }
    request.headers_mut().remove("x-hey-proxy-provider");
    request.headers_mut().remove("x-hey-proxy-account");
    // Never trust an inbound identity header over the selected encrypted store.
    request.headers_mut().remove("chatgpt-account-id");
    if let Some(id) = &binding.account_id {
        request
            .headers_mut()
            .insert("chatgpt-account-id", id.parse()?);
    }
    Ok(Some(selected))
}

#[cfg(test)]
mod tests;
