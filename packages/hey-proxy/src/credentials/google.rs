//! Application Default Credentials over OAuth/metadata HTTP endpoints.
use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use reqwest::{
    Client, RequestBuilder,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

pub(super) struct Token {
    pub headers: HeaderMap,
    issued: SystemTime,
    expires: SystemTime,
}
impl Token {
    pub fn valid(&self) -> bool {
        let now = SystemTime::now();
        now >= self.issued
            && self
                .expires
                .duration_since(now)
                .is_ok_and(|d| d > Duration::from_secs(60))
    }
}

fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("Missing ADC credential field")
}

async fn read_json(path: &std::path::Path) -> Result<Value> {
    let bytes = tokio::fs::read(path)
        .await
        .context("Cannot read ADC credential file")?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "ADC credential file is too large"
    );
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("Invalid ADC credential file"))
}

async fn response(request: RequestBuilder) -> Result<Value> {
    let response = request
        .send()
        .await
        .map_err(|_| anyhow!("ADC credential endpoint unavailable"))?;
    response_json(response).await
}
async fn response_json(mut response: reqwest::Response) -> Result<Value> {
    ensure!(
        response.status().is_success(),
        "ADC credential endpoint rejected the request (HTTP {})",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Cannot read ADC response"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= 1024 * 1024,
            "ADC response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("Invalid ADC response"))
}

fn access(value: &Value, issued: SystemTime) -> Result<(String, SystemTime)> {
    let token = text(value, "access_token")?.to_owned();
    let seconds = value["expires_in"]
        .as_u64()
        .filter(|n| *n > 0)
        .context("ADC response has no token lifetime")?;
    Ok((
        token,
        issued
            .checked_add(Duration::from_secs(seconds))
            .context("Invalid ADC token lifetime")?,
    ))
}

fn authorization(token: &str) -> Result<HeaderValue> {
    let mut header = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| anyhow!("Invalid ADC access token"))?;
    header.set_sensitive(true);
    Ok(header)
}

pub(super) async fn load() -> Result<Token> {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()?;
    let explicit = std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    let default = std::env::var_os("CLOUDSDK_CONFIG")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config/gcloud")))
        .map(|p| p.join("application_default_credentials.json"));
    let path = match explicit {
        Some(path) => Some(path),
        None => match default {
            Some(path) => match tokio::fs::metadata(&path).await {
                Ok(_) => Some(path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => bail!("Cannot access default ADC credential file"),
            },
            None => None,
        },
    };
    let issued = SystemTime::now();
    let (token, expires, quota) = if let Some(path) = path {
        let config = read_json(&path).await?;
        let (token, expires) = from_config(&client, &config, 0).await?;
        (
            token,
            expires,
            config["quota_project_id"].as_str().map(str::to_owned),
        )
    } else {
        let host = std::env::var("GCE_METADATA_HOST")
            .unwrap_or_else(|_| "metadata.google.internal".into());
        // Metadata must not go through an ambient HTTP proxy.
        let metadata = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()?;
        let response = metadata
            .get(format!(
                "http://{host}/computeMetadata/v1/instance/service-accounts/default/token"
            ))
            .header("metadata-flavor", "Google")
            .send()
            .await
            .map_err(|_| {
                anyhow!("ADC credentials unavailable; configure Application Default Credentials")
            })?;
        ensure!(
            response
                .headers()
                .get("metadata-flavor")
                .is_some_and(|v| v == "Google"),
            "Invalid Google metadata response"
        );
        ensure!(
            response.status().is_success(),
            "Google metadata credentials unavailable"
        );
        let value = response_json(response).await?;
        let (token, expires) = access(&value, issued)?;
        (token, expires, None)
    };
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, authorization(&token)?);
    if let Some(project) = std::env::var("GOOGLE_CLOUD_QUOTA_PROJECT")
        .ok()
        .or(quota)
        .filter(|p| !p.is_empty())
    {
        headers.insert(
            "x-goog-user-project",
            HeaderValue::from_str(&project).map_err(|_| anyhow!("Invalid ADC quota project"))?,
        );
    }
    Ok(Token {
        headers,
        issued,
        expires,
    })
}

async fn from_config(
    client: &Client,
    config: &Value,
    depth: usize,
) -> Result<(String, SystemTime)> {
    ensure!(depth < 8, "ADC impersonation chain is too deep");
    let issued = SystemTime::now();
    match text(config, "type")? {
        "authorized_user" => {
            let value = response(
                client
                    .post(config["token_uri"].as_str().unwrap_or(TOKEN_URL))
                    .form(&[
                        ("grant_type", "refresh_token"),
                        ("client_id", text(config, "client_id")?),
                        ("client_secret", text(config, "client_secret")?),
                        ("refresh_token", text(config, "refresh_token")?),
                    ]),
            )
            .await?;
            access(&value, issued)
        }
        "service_account" => {
            let endpoint = config["token_uri"].as_str().unwrap_or(TOKEN_URL);
            let assertion = assertion(config, endpoint, issued)?;
            let value = response(client.post(endpoint).form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ]))
            .await?;
            access(&value, issued)
        }
        "impersonated_service_account" => {
            let (source, _) = Box::pin(from_config(
                client,
                &config["source_credentials"],
                depth + 1,
            ))
            .await?;
            impersonate(client, config, &source).await
        }
        "external_account" => {
            let subject = if config["credential_source"]["environment_id"].is_string() {
                super::aws::federated_subject(config).await?
            } else if config["credential_source"]["executable"].is_object() {
                executable_subject(config).await?
            } else {
                subject_token(client, &config["credential_source"]).await?
            };
            let mut form = vec![
                (
                    "grant_type",
                    "urn:ietf:params:oauth:grant-type:token-exchange".to_owned(),
                ),
                (
                    "requested_token_type",
                    "urn:ietf:params:oauth:token-type:access_token".to_owned(),
                ),
                ("audience", text(config, "audience")?.to_owned()),
                (
                    "subject_token_type",
                    text(config, "subject_token_type")?.to_owned(),
                ),
                ("subject_token", subject),
                ("scope", SCOPE.to_owned()),
            ];
            if let Some(project) = config["workforce_pool_user_project"].as_str() {
                form.push(("options", json!({"userProject":project}).to_string()));
            }
            let value = response(client.post(text(config, "token_url")?).form(&form)).await?;
            let token = access(&value, issued)?;
            if config["service_account_impersonation_url"].is_string() {
                impersonate(client, config, &token.0).await
            } else {
                Ok(token)
            }
        }
        _ => bail!("Unsupported ADC credential type"),
    }
}

fn assertion(config: &Value, endpoint: &str, now: SystemTime) -> Result<String> {
    let seconds = now.duration_since(UNIX_EPOCH)?.as_secs();
    let mut header = json!({"alg":"RS256","typ":"JWT"});
    if let Some(id) = config["private_key_id"].as_str() {
        header["kid"] = json!(id);
    }
    let claims = json!({"iss":text(config,"client_email")?,"scope":SCOPE,"aud":endpoint,"iat":seconds,"exp":seconds+3600});
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let pem = text(config, "private_key")?;
    let encoded = pem
        .strip_prefix("-----BEGIN PRIVATE KEY-----")
        .and_then(|s| s.trim().strip_suffix("-----END PRIVATE KEY-----"))
        .context("Invalid ADC private key")?;
    let bytes = STANDARD
        .decode(encoded.split_whitespace().collect::<String>())
        .map_err(|_| anyhow!("Invalid ADC private key"))?;
    let key = ring::signature::RsaKeyPair::from_pkcs8(&bytes)
        .map_err(|_| anyhow!("Invalid ADC RSA private key"))?;
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        input.as_bytes(),
        &mut signature,
    )
    .map_err(|_| anyhow!("Cannot sign ADC assertion"))?;
    Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

async fn impersonate(
    client: &Client,
    config: &Value,
    source: &str,
) -> Result<(String, SystemTime)> {
    let value = response(client.post(text(config,"service_account_impersonation_url")?)
        .header(AUTHORIZATION, authorization(source)?)
        .json(&json!({"scope":[SCOPE],"lifetime":"3600s","delegates":config["delegates"].as_array().cloned().unwrap_or_default()}))).await?;
    let expires: jiff::Timestamp = text(&value, "expireTime")?
        .parse()
        .map_err(|_| anyhow!("Invalid ADC impersonation expiry"))?;
    Ok((
        text(&value, "accessToken")?.to_owned(),
        SystemTime::try_from(expires)?,
    ))
}

async fn subject_token(client: &Client, source: &Value) -> Result<String> {
    let bytes = if let Some(path) = source["file"].as_str() {
        tokio::fs::read(path)
            .await
            .map_err(|_| anyhow!("Cannot read ADC subject token file"))?
    } else if let Some(url) = source["url"].as_str() {
        let mut request = client.get(url);
        for (name, value) in source["headers"].as_object().into_iter().flatten() {
            let mut header =
                HeaderValue::from_str(value.as_str().context("Invalid ADC source header")?)
                    .map_err(|_| anyhow!("Invalid ADC source header"))?;
            header.set_sensitive(true);
            request = request.header(name, header);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| anyhow!("ADC subject token endpoint unavailable"))?;
        ensure!(
            response.status().is_success(),
            "ADC subject token endpoint rejected the request"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow!("Cannot read ADC subject token"))?
        {
            ensure!(
                bytes.len() + chunk.len() <= 1024 * 1024,
                "ADC subject token is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        bytes
    } else {
        bail!("Unsupported ADC subject token source");
    };
    ensure!(bytes.len() <= 1024 * 1024, "ADC subject token is too large");
    let value = if source["format"]["type"] == "json" {
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow!("Invalid ADC subject token JSON"))?;
        text(&value, text(&source["format"], "subject_token_field_name")?)?.to_owned()
    } else {
        std::str::from_utf8(&bytes)
            .map_err(|_| anyhow!("Invalid ADC subject token"))?
            .trim()
            .to_owned()
    };
    ensure!(!value.is_empty(), "Empty ADC subject token");
    Ok(value)
}

async fn executable_subject(config: &Value) -> Result<String> {
    ensure!(
        std::env::var("GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES").as_deref() == Ok("1"),
        "ADC executable credentials require GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES=1"
    );
    let executable = &config["credential_source"]["executable"];
    let extract = |value: &Value, cached: bool| -> Result<String> {
        ensure!(
            value["version"] == 1 && value["success"] == true,
            "ADC credential executable failed"
        );
        if cached || value["expiration_time"].is_number() {
            ensure!(
                value["expiration_time"]
                    .as_u64()
                    .is_some_and(|t| SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .is_ok_and(|now| t > now.as_secs() + 60)),
                "ADC executable token expired"
            );
        }
        let field = match text(config, "subject_token_type")? {
            "urn:ietf:params:oauth:token-type:id_token"
            | "urn:ietf:params:oauth:token-type:jwt" => "id_token",
            "urn:ietf:params:oauth:token-type:saml2" => "saml_response",
            _ => bail!("Unsupported ADC executable token type"),
        };
        ensure!(
            value["token_type"] == config["subject_token_type"],
            "ADC executable returned the wrong token type"
        );
        Ok(text(value, field)?.into())
    };
    if let Some(path) = executable["output_file"].as_str()
        && let Ok(value) = read_json(std::path::Path::new(path)).await
        && let Ok(token) = extract(&value, true)
    {
        return Ok(token);
    }
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .args(["-c", text(executable, "command")?])
        .env(
            "GOOGLE_EXTERNAL_ACCOUNT_AUDIENCE",
            text(config, "audience")?,
        )
        .env(
            "GOOGLE_EXTERNAL_ACCOUNT_TOKEN_TYPE",
            text(config, "subject_token_type")?,
        )
        .env("GOOGLE_EXTERNAL_ACCOUNT_INTERACTIVE", "0");
    if let Some(path) = executable["output_file"].as_str() {
        command.env("GOOGLE_EXTERNAL_ACCOUNT_OUTPUT_FILE", path);
    }
    if let Some(url) = config["service_account_impersonation_url"].as_str()
        && let Some(email) = url
            .rsplit('/')
            .next()
            .and_then(|s| s.strip_suffix(":generateAccessToken"))
    {
        command.env("GOOGLE_EXTERNAL_ACCOUNT_IMPERSONATED_EMAIL", email);
    }
    let millis = executable["timeout_millis"].as_u64().unwrap_or(30000);
    ensure!(
        (5000..=120000).contains(&millis),
        "Invalid ADC executable timeout"
    );
    let bytes = tokio::time::timeout(
        Duration::from_millis(millis),
        super::command_output(command),
    )
    .await
    .context("ADC credential executable timed out")??;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| anyhow!("Invalid ADC executable response"))?;
    extract(&value, false)
}

#[cfg(test)]
mod tests;
