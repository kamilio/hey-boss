//! Proxy-owned Claude OAuth credentials. Never reads or writes Claude Code's login.
mod cache;
use aes_gcm_siv::{
    Aes256GcmSiv, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::{Context, Result, bail, ensure};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const REDIRECT: &str = "http://localhost:54545/callback";
const SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const AAD: &[u8] = b"hey-proxy Claude OAuth v1";
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Tokens {
    pub access_token: String,
    refresh_token: String,
    expires_at: u64,
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Cannot generate OAuth randomness"))?;
    Ok(bytes)
}

fn private_open(path: &Path, create: bool) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        .write(create)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options
            .open(path)
            .context("Cannot open private Claude credential file; run hey-proxy claude-login")?;
        ensure!(
            file.metadata()?.is_file() && file.metadata()?.permissions().mode() & 0o077 == 0,
            "Claude credential files must be regular files with private permissions (chmod 600)"
        );
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        Ok(options.open(path)?)
    }
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("Claude credentials require a parent directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|_| anyhow::anyhow!("Cannot save Claude credentials"))?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
fn key(path: &Path, create: bool) -> Result<[u8; 32]> {
    let key_path = path.with_extension("key");
    if create && !key_path.exists() {
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        file.write_all(&random::<32>()?)?;
        file.as_file().sync_all()?;
        if let Err(error) = file.persist_noclobber(&key_path) {
            ensure!(
                error.error.kind() == std::io::ErrorKind::AlreadyExists,
                "Cannot save Claude credential key"
            );
        }
        #[cfg(unix)]
        fs::File::open(path.parent().unwrap())?.sync_all()?;
    }
    let mut file = private_open(&key_path, false)?;
    ensure!(
        file.metadata()?.len() == 32,
        "Invalid Claude credential encryption key"
    );
    let mut bytes = [0; 32];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn load(path: &Path) -> Result<Tokens> {
    let mut data = Vec::new();
    private_open(path, false)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= MAX_BYTES,
        "Claude credential file is too large"
    );
    let envelope: Value = serde_json::from_slice(&data)
        .map_err(|_| anyhow::anyhow!("Invalid Claude credential file"))?;
    let encoded = envelope["encrypted"]
        .as_str()
        .and_then(|s| s.strip_prefix("v1:"))
        .context("Invalid Claude credential envelope")?;
    let bytes = STANDARD
        .decode(encoded)
        .ok()
        .filter(|b| b.len() >= 28)
        .context("Invalid Claude credential encoding")?;
    let cipher = Aes256GcmSiv::new((&key(path, false)?).into());
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&bytes[..12]),
            Payload {
                msg: &bytes[12..],
                aad: AAD,
            },
        )
        .map_err(|_| {
            anyhow::anyhow!("Cannot decrypt Claude credentials; restore the matching key file")
        })?;
    let tokens: Tokens = serde_json::from_slice(&plaintext)
        .map_err(|_| anyhow::anyhow!("Invalid Claude credentials"))?;
    ensure!(
        is_oauth_token(&tokens.access_token) && !tokens.refresh_token.is_empty(),
        "Invalid Claude OAuth tokens; run hey-proxy claude-login"
    );
    Ok(tokens)
}
fn save(path: &Path, tokens: &Tokens) -> Result<()> {
    let cipher = Aes256GcmSiv::new((&key(path, true)?).into());
    let nonce = random::<12>()?;
    let encrypted = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &serde_json::to_vec(tokens)?,
                aad: AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("Cannot encrypt Claude credentials"))?;
    let mut bytes = nonce.to_vec();
    bytes.extend(encrypted);
    atomic_write(
        path,
        &serde_json::to_vec(&json!({"encrypted":format!("v1:{}", STANDARD.encode(bytes))}))?,
    )
}
pub(crate) fn is_oauth_token(token: &str) -> bool {
    token.starts_with("sk-ant-oat")
        && reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).is_ok()
}

async fn lock(path: &Path) -> Result<fs::File> {
    let path = path.with_extension("lock");
    let file = tokio::task::spawn_blocking(move || private_open(&path, true)).await??;
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(_) => bail!("Claude credential refresh is busy; retry shortly"),
        }
    }
}

async fn exchange(
    client: &reqwest::Client,
    endpoint: &str,
    body: Value,
    previous: Option<&Tokens>,
) -> Result<Tokens> {
    let mut response = client
        .post(endpoint)
        .timeout(Duration::from_secs(30))
        .header("accept", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Cannot reach Claude OAuth token endpoint"))?;
    let status = response.status();
    if !status.is_success() {
        if matches!(status.as_u16(), 400 | 401 | 403) {
            bail!(
                "Claude OAuth authorization rejected (HTTP {}); run hey-proxy claude-login",
                status.as_u16()
            );
        }
        bail!(
            "Claude OAuth temporarily unavailable (HTTP {}); retry shortly",
            status.as_u16()
        );
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("Cannot read Claude OAuth response"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= MAX_BYTES as usize,
            "Claude OAuth response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid Claude OAuth response"))?;
    let access = value["access_token"]
        .as_str()
        .filter(|s| is_oauth_token(s))
        .context("Claude OAuth did not return an access token")?;
    let refresh = value["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| previous.map(|p| p.refresh_token.as_str()))
        .context("Claude OAuth did not return a refresh token")?;
    let expires = value["expires_in"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 366 * 86400)
        .context("Invalid Claude OAuth token lifetime")?;
    Ok(Tokens {
        access_token: access.into(),
        refresh_token: refresh.into(),
        expires_at: now().saturating_add(expires),
    })
}

#[derive(Default, Clone)]
pub(crate) struct TokenManager {
    // One refresh owner per process, plus a file lock shared by other proxy processes.
    backoff: Arc<Mutex<Option<(PathBuf, Instant)>>>,
    cache: Arc<cache::Cache>,
}
impl TokenManager {
    pub async fn token(&self, path: &Path, client: &reqwest::Client) -> Result<String> {
        self.obtain(path, client, None, TOKEN_URL).await
    }
    pub async fn rejected(
        &self,
        path: &Path,
        client: &reqwest::Client,
        token: String,
    ) -> Result<String> {
        self.obtain(path, client, Some(token), TOKEN_URL).await
    }
    async fn obtain(
        &self,
        path: &Path,
        client: &reqwest::Client,
        rejected: Option<String>,
        endpoint: &str,
    ) -> Result<String> {
        if let Some(cached) = self.cache.get(path, rejected.as_deref()) {
            return cached;
        }
        let path = path.to_owned();
        let client = client.clone();
        let endpoint = endpoint.to_owned();
        let backoff = self.backoff.clone();
        let cache = self.cache.clone();
        // Finish rotating-token exchanges and durable saves even if the caller
        // disconnects. Cold file reads and encryption never run on Tokio workers.
        tokio::spawn(async move {
            let mut gate = backoff.lock().await;
            if let Some(cached) = cache.get(&path, rejected.as_deref()) { return cached; }
            let disk_path = path.clone();
            let tokens = match tokio::task::spawn_blocking(move || load(&disk_path)).await? {
                Ok(tokens) => tokens,
                Err(error) => { cache.failure(path, &error); return Err(error); }
            };
            let usable = |t: &Tokens| t.expires_at > now().saturating_add(60)
                && rejected.as_ref().is_none_or(|r| r != &t.access_token);
            if usable(&tokens) { cache.store(path, &tokens); return Ok(tokens.access_token); }
            let _lock = lock(&path).await?;
            let disk_path = path.clone();
            let tokens = tokio::task::spawn_blocking(move || load(&disk_path)).await??;
            if usable(&tokens) { cache.store(path, &tokens); return Ok(tokens.access_token); }
            if gate.as_ref().is_some_and(|(p, until)| p == &path && *until > Instant::now()) {
                bail!("Claude OAuth refresh is cooling down; retry shortly or run hey-proxy claude-login");
            }
            let body = json!({"grant_type":"refresh_token","refresh_token":tokens.refresh_token,"client_id":CLIENT_ID});
            let fresh = match exchange(&client, &endpoint, body, Some(&tokens)).await {
                Ok(fresh) => fresh,
                Err(error) => { *gate = Some((path, Instant::now() + Duration::from_secs(30))); return Err(error); }
            };
            let (save_path, save_tokens) = (path.clone(), fresh.clone());
            tokio::task::spawn_blocking(move || save(&save_path, &save_tokens)).await??;
            cache.store(path, &fresh);
            *gate = None;
            Ok(fresh.access_token)
        }).await.map_err(|_| anyhow::anyhow!("Claude credential operation failed"))?
    }
}

fn authorization_url(state: &str, verifier: &str) -> Result<url::Url> {
    let mut url = url::Url::parse("https://claude.ai/oauth/authorize")?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    url.query_pairs_mut().extend_pairs([
        ("code", "true"),
        ("client_id", CLIENT_ID),
        ("response_type", "code"),
        ("redirect_uri", REDIRECT),
        ("scope", SCOPES),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ]);
    Ok(url)
}

#[derive(Deserialize)]
struct Callback {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}
struct CallbackState {
    state: String,
    sender: Mutex<Option<tokio::sync::oneshot::Sender<Result<String>>>>,
}
async fn callback(
    axum::extract::State(state): axum::extract::State<Arc<CallbackState>>,
    axum::extract::Query(query): axum::extract::Query<Callback>,
) -> (axum::http::StatusCode, &'static str) {
    use axum::http::StatusCode;
    if query.state.as_deref() != Some(&state.state) {
        return (
            StatusCode::BAD_REQUEST,
            "OAuth state mismatch. Return to the authorization page.",
        );
    }
    let result = if query.error.is_some() {
        Err(anyhow::anyhow!("Claude sign-in was declined"))
    } else if let Some(code) = query.code.filter(|c| !c.is_empty()) {
        Ok(code)
    } else {
        return (StatusCode::BAD_REQUEST, "Missing authorization code.");
    };
    if let Some(sender) = state.sender.lock().await.take() {
        let _ = sender.send(result);
    }
    (
        StatusCode::OK,
        "Authorization received. Return to your terminal to check that sign-in completed. You can close this tab.",
    )
}

pub(crate) async fn login(
    config: &crate::config::Config,
    config_path: &Path,
    no_browser: bool,
) -> Result<()> {
    let provider = config.claude.clone().unwrap_or_default();
    ensure!(
        config.mode != crate::config::Mode::Client,
        "Sign in on the proxy host, not a client relay"
    );
    let path = provider.credentials_path(Some(config_path))?;
    fs::create_dir_all(path.parent().context("Invalid Claude credential path")?)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:54545")
        .await
        .context("Cannot bind Claude OAuth callback on port 54545")?;
    let state = URL_SAFE_NO_PAD.encode(random::<32>()?);
    let verifier = URL_SAFE_NO_PAD.encode(random::<32>()?);
    let url = authorization_url(&state, &verifier)?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let app = axum::Router::new()
        .route("/callback", axum::routing::get(callback))
        .with_state(Arc::new(CallbackState {
            state: state.clone(),
            sender: Mutex::new(Some(sender)),
        }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    println!(
        "Sign in to Claude for hey-proxy:\n{url}\nWaiting up to five minutes for browser authorization…"
    );
    if !no_browser {
        #[cfg(target_os = "macos")]
        let command = "open";
        #[cfg(not(target_os = "macos"))]
        let command = "xdg-open";
        let _ = std::process::Command::new(command)
            .arg(url.as_str())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    let result = tokio::time::timeout(Duration::from_secs(300), receiver).await;
    server.abort();
    let code = result
        .context("Claude sign-in timed out; run hey-proxy claude-login again")?
        .context("Claude sign-in callback closed")??;
    let _lock = lock(&path).await?;
    let client = crate::proxy::build_client(config)?;
    let tokens = exchange(&client, TOKEN_URL, json!({"grant_type":"authorization_code","code":code,"redirect_uri":REDIRECT,"client_id":CLIENT_ID,"code_verifier":verifier,"state":state}), None).await?;
    save(&path, &tokens)?;
    println!("Claude subscription connected. hey-proxy owns and refreshes these credentials.");
    if config.claude.is_none() {
        println!("Enable it in your proxy config: \"providers\": {{\"claude\": {{}}}}");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
