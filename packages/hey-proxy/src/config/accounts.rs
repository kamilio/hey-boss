//! Named connections are independent of agent runtimes and routing policy.
use super::*;
use std::path::PathBuf;

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "implementation", rename_all = "lowercase", deny_unknown_fields)]
pub enum AccountConfig {
    Codex {
        auth: Subscription,
        credentials_file: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        issuer: Option<String>,
    },
    Claude {
        auth: Subscription,
        credentials_file: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
    },
    Openai {
        auth: Api,
        endpoint: String,
        credential: String,
    },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Subscription {
    Subscription,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Api {
    Api,
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "default"
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}
impl AccountConfig {
    pub fn implementation(&self) -> &'static str {
        match self {
            Self::Codex { .. } => "codex",
            Self::Claude { .. } => "claude",
            Self::Openai { .. } => "openai",
        }
    }
    pub fn auth(&self) -> &'static str {
        match self {
            Self::Openai { .. } => "api",
            _ => "subscription",
        }
    }
    pub fn apply(&self, config: &Config) -> Config {
        let mut selected = config.clone();
        selected.claude = None;
        selected.codex = None;
        selected.gemini = None;
        selected.api_keys.clear();
        // Explicit account selection cannot fall through to another credential.
        selected.routes.clear();
        selected.worker_candidates.clear();
        selected.overrides.clear();
        selected.aliases.clear();
        selected.fallbacks.clear();
        selected.default.api_key = "default".into();
        match self {
            Self::Codex {
                credentials_file,
                endpoint,
                issuer,
                ..
            } => {
                let mut p = crate::proxy::codex::ProviderConfig {
                    credentials_file: Some(credentials_file.clone()),
                    ..Default::default()
                };
                if let Some(endpoint) = endpoint {
                    p.upstream_url = endpoint.clone();
                }
                if let Some(issuer) = issuer {
                    p.issuer_url = issuer.clone();
                }
                selected.codex = Some(p);
            }
            Self::Claude {
                credentials_file,
                endpoint,
                ..
            } => {
                let mut p = crate::proxy::claude::ProviderConfig {
                    credentials_file: Some(credentials_file.clone()),
                    ..Default::default()
                };
                if let Some(endpoint) = endpoint {
                    p.upstream_url = endpoint.clone();
                }
                selected.claude = Some(p);
            }
            Self::Openai {
                endpoint,
                credential,
                ..
            } => {
                selected.upstream_url = endpoint.clone();
                selected
                    .api_keys
                    .insert("default".into(), credential.clone());
            }
        }
        selected
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Openai {
                endpoint,
                credential,
                ..
            } => {
                validate_root_url(endpoint)?;
                let url = url::Url::parse(endpoint)?;
                anyhow::ensure!(
                    url.scheme() == "https"
                        || url.host_str().is_some_and(|h| h == "localhost"
                            || h.parse::<std::net::IpAddr>()
                                .is_ok_and(|ip| ip.is_loopback())),
                    "Account endpoint requires HTTPS"
                );
                anyhow::ensure!(
                    credential.starts_with("op://") || credential.starts_with("file://"),
                    "Named API accounts require a protected credential reference"
                );
                hey_proxy::credentials::validate_source(credential)?;
            }
            Self::Codex {
                credentials_file, ..
            }
            | Self::Claude {
                credentials_file, ..
            } => {
                anyhow::ensure!(
                    !credentials_file.as_os_str().is_empty(),
                    "Account credential store is required"
                );
                let selected = self.apply(&Config::default());
                if let Some(p) = selected.codex {
                    p.validate(Mode::Standalone)?;
                }
                if let Some(p) = selected.claude {
                    p.validate(Mode::Standalone)?;
                }
            }
        }
        Ok(())
    }
}
impl Config {
    pub fn select_account(&self, name: &str, implementation: &str) -> Result<Config> {
        anyhow::ensure!(
            self.mode != Mode::Client,
            "Named credentials belong to the host"
        );
        let account = self.accounts.get(name).context("Unknown named provider")?;
        anyhow::ensure!(
            account.implementation() == implementation,
            "Provider implementation does not match"
        );
        Ok(account.apply(self))
    }
}

/// Refuse to turn an unrelated CLI login or arbitrary file into a proxy store.
pub(crate) fn owned_store(path: &Path) -> Result<()> {
    anyhow::ensure!(
        !path
            .components()
            .any(|c| matches!(c.as_os_str().to_str(), Some(".codex" | ".claude"))),
        "Use a proxy-owned credential store outside CLI login directories"
    );
    if let Some(home) = std::env::var_os("CODEX_HOME") {
        anyhow::ensure!(
            path != PathBuf::from(home).join("auth.json"),
            "Cannot overwrite a CLI login"
        );
    }
    if path.exists() {
        use std::io::Read;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path)
            .map_err(|_| anyhow::anyhow!("Cannot open proxy-owned credential store"))?;
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Existing file is not a proxy-owned credential store"))?;
        anyhow::ensure!(
            value["encrypted"]
                .as_str()
                .is_some_and(|s| s.starts_with("v1:")),
            "Existing file is not a proxy-owned credential store"
        );
    }
    Ok(())
}

impl Config {
    pub(crate) fn validate_account_paths(&self, source: &Path) -> Result<()> {
        let mut stores = BTreeMap::new();
        for account in self.accounts.values() {
            let selected = account.apply(self);
            let path = if let Some(p) = selected.codex {
                p.credentials_path(Some(source))?
            } else if let Some(p) = selected.claude {
                p.credentials_path(Some(source))?
            } else {
                continue;
            };
            let normalized = fs::canonicalize(&path).unwrap_or_else(|_| {
                let parent = path.parent().unwrap_or(Path::new("."));
                fs::canonicalize(parent)
                    .unwrap_or_else(|_| parent.to_owned())
                    .join(path.file_name().unwrap_or_default())
            });
            if let Some(previous) = stores.insert(normalized, account.implementation()) {
                anyhow::ensure!(
                    previous == account.implementation(),
                    "Different implementations cannot share an OAuth store"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrelated_login_files_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let content = br#"{"access_token":"synthetic-cli-secret"}"#;
        fs::write(&path, content).unwrap();
        let error = owned_store(&path).unwrap_err().to_string();
        assert!(!error.contains("synthetic"));
        assert_eq!(fs::read(&path).unwrap(), content);
        assert!(owned_store(&dir.path().join(".codex/auth.json")).is_err());
        assert!(owned_store(&dir.path().join("new-proxy-store.json")).is_ok());
    }
    #[test]
    fn named_login_rejects_dangling_or_wrong_implementation_names() {
        let config: Config = serde_json::from_value(serde_json::json!({"listen":"127.0.0.1:8080","account_schema_version":1,
            "accounts":{"work":{"implementation":"codex","auth":"subscription","credentials_file":"work.json"}}})).unwrap();
        assert!(config.select_account("missing", "codex").is_err());
        assert!(config.select_account("work", "claude").is_err());
        assert_eq!(
            config
                .select_account("work", "codex")
                .unwrap()
                .codex
                .unwrap()
                .credentials_file
                .unwrap(),
            PathBuf::from("work.json")
        );
    }
}
