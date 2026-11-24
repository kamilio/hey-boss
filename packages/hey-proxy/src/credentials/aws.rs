//! AWS credentials and Signature V4 for the proxy's Bedrock requests.
use anyhow::{Context, Result, anyhow, bail, ensure};
use reqwest::{
    Client, Request,
    header::{AUTHORIZATION, HOST, HeaderMap, HeaderValue},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, SystemTime},
};

struct Credentials {
    access: String,
    secret: String,
    token: Option<String>,
    expires: Option<SystemTime>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}
fn required<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("Missing AWS credential field")
}
fn credentials(value: &Value) -> Result<Credentials> {
    let expires = value["Expiration"]
        .as_str()
        .map(|v| -> Result<SystemTime> {
            let timestamp: jiff::Timestamp = v
                .parse()
                .map_err(|_| anyhow!("Invalid AWS credential expiry"))?;
            Ok(SystemTime::try_from(timestamp)?)
        })
        .transpose()?;
    ensure!(
        expires.is_none_or(|t| t > SystemTime::now()),
        "AWS credentials have expired"
    );
    Ok(Credentials {
        access: required(value, "AccessKeyId")?.into(),
        secret: required(value, "SecretAccessKey")?.into(),
        token: value["SessionToken"]
            .as_str()
            .or_else(|| value["Token"].as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        expires,
    })
}

/// Sign a buffered request using the host's AWS identity. Credential discovery
/// runs once per refresh, never once per stream chunk.
pub async fn sign(request: &mut Request, region: &str, service: &str) -> Result<()> {
    static CACHE: tokio::sync::Mutex<Option<(SystemTime, Credentials)>> =
        tokio::sync::Mutex::const_new(None);
    let mut cache = CACHE.lock().await;
    let now = SystemTime::now();
    if !cache.as_ref().is_some_and(|(issued, c)| {
        now.duration_since(*issued)
            .is_ok_and(|age| age < Duration::from_secs(900))
            && c.expires.is_none_or(|t| {
                t.duration_since(now)
                    .is_ok_and(|remaining| remaining > Duration::from_secs(60))
            })
    }) {
        *cache = None;
        let fresh = tokio::time::timeout(Duration::from_secs(30), load(region))
            .await
            .context("AWS credential discovery timed out")??;
        *cache = Some((SystemTime::now(), fresh));
    }
    sign_with(
        request,
        region,
        service,
        &cache.as_ref().unwrap().1,
        SystemTime::now(),
    )
}

fn sensitive(value: &str) -> Result<HeaderValue> {
    let mut value =
        HeaderValue::from_str(value).map_err(|_| anyhow!("Invalid AWS credential header"))?;
    value.set_sensitive(true);
    Ok(value)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
fn hmac(key: &[u8], bytes: &[u8]) -> Vec<u8> {
    ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key), bytes)
        .as_ref()
        .to_vec()
}

fn sign_with(
    request: &mut Request,
    region: &str,
    service: &str,
    credentials: &Credentials,
    now: SystemTime,
) -> Result<()> {
    let time = jiff::Timestamp::try_from(now)?
        .strftime("%Y%m%dT%H%M%SZ")
        .to_string();
    let date = &time[..8];
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let host = request.url()[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
    request
        .headers_mut()
        .insert(HOST, HeaderValue::from_str(&host)?);
    request
        .headers_mut()
        .insert("x-amz-date", HeaderValue::from_str(&time)?);
    request.headers_mut().remove(AUTHORIZATION);
    request.headers_mut().remove("x-amz-security-token");
    if let Some(token) = &credentials.token {
        request
            .headers_mut()
            .insert("x-amz-security-token", sensitive(token)?);
    }
    let mut headers = BTreeMap::new();
    for name in request.headers().keys() {
        if [
            "authorization",
            "user-agent",
            "x-amzn-trace-id",
            "transfer-encoding",
        ]
        .contains(&name.as_str())
        {
            continue;
        }
        let values = request
            .headers()
            .get_all(name)
            .iter()
            .map(|v| {
                v.to_str()
                    .map(|s| s.split_ascii_whitespace().collect::<Vec<_>>().join(" "))
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        headers.insert(name.as_str(), values.join(","));
    }
    let names = headers.keys().copied().collect::<Vec<_>>().join(";");
    let canonical_headers = headers
        .iter()
        .map(|(k, v)| format!("{k}:{v}\n"))
        .collect::<String>();
    let mut query = request
        .url()
        .query_pairs()
        .map(|(key, value)| (encode(key.as_bytes()), encode(value.as_bytes())))
        .collect::<Vec<_>>();
    query.sort();
    let query = query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let path = request.url().path();
    let normalized = format!(
        "/{}{}",
        path.split('/')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/"),
        if path.len() > 1 && path.ends_with('/') {
            "/"
        } else {
            ""
        }
    );
    let body = match request.body() {
        Some(body) => body
            .as_bytes()
            .context("AWS signing requires a buffered body")?,
        None => &[],
    };
    let canonical = format!(
        "{}\n{}\n{query}\n{canonical_headers}\n{names}\n{}",
        request.method(),
        normalized.replace('%', "%25"),
        hex(&Sha256::digest(body))
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{time}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let date_key = hmac(
        format!("AWS4{}", credentials.secret).as_bytes(),
        date.as_bytes(),
    );
    let region_key = hmac(&date_key, region.as_bytes());
    let service_key = hmac(&region_key, service.as_bytes());
    let signing_key = hmac(&service_key, b"aws4_request");
    let signature = hex(&hmac(&signing_key, string_to_sign.as_bytes()));
    request.headers_mut().insert(
        AUTHORIZATION,
        sensitive(&format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={names}, Signature={signature}",
            credentials.access
        ))?,
    );
    Ok(())
}

fn environment() -> Result<Option<Credentials>> {
    match (env("AWS_ACCESS_KEY_ID"), env("AWS_SECRET_ACCESS_KEY")) {
        (Some(access), Some(secret)) => Ok(Some(Credentials {
            access,
            secret,
            token: env("AWS_SESSION_TOKEN"),
            expires: None,
        })),
        (None, None) => Ok(None),
        _ => bail!("AWS environment credentials are incomplete"),
    }
}
type Profiles = BTreeMap<String, BTreeMap<String, String>>;
fn profiles(text: &str, config: bool, profiles: &mut Profiles) -> Result<()> {
    let mut section = None;
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with(['#', ';']))
    {
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = if config {
                name.strip_prefix("profile ")
                    .or_else(|| (name == "default").then_some(name))
            } else {
                Some(name)
            }
            .map(str::to_owned);
        } else if let Some(section) = &section {
            let (key, value) = line.split_once('=').context("Invalid AWS profile file")?;
            profiles
                .entry(section.clone())
                .or_default()
                .insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    Ok(())
}
async fn load(region: &str) -> Result<Credentials> {
    if let Some(credentials) = environment()? {
        return Ok(credentials);
    }
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()?;
    let mut all = Profiles::new();
    for (name, file, config) in [
        ("AWS_CONFIG_FILE", "config", true),
        ("AWS_SHARED_CREDENTIALS_FILE", "credentials", false),
    ] {
        let path = env(name).map(PathBuf::from).or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".aws").join(file))
        });
        if let Some(path) = path {
            match tokio::fs::read_to_string(path).await {
                Ok(text) => {
                    ensure!(text.len() <= 1024 * 1024, "AWS profile file is too large");
                    profiles(&text, config, &mut all)?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => bail!("Cannot read AWS profile file"),
            }
        }
    }
    let name = env("AWS_PROFILE")
        .or_else(|| env("AWS_DEFAULT_PROFILE"))
        .unwrap_or_else(|| "default".into());
    if all.get(&name).is_some_and(|p| {
        ["aws_access_key_id", "credential_process", "role_arn"]
            .iter()
            .any(|key| p.contains_key(*key))
    }) {
        return profile(&client, &all, &name, region, 0).await;
    }
    if let Some(role) = env("AWS_ROLE_ARN")
        && let Some(file) = env("AWS_WEB_IDENTITY_TOKEN_FILE")
    {
        return web_identity(
            &client,
            region,
            &role,
            &file,
            env("AWS_ROLE_SESSION_NAME")
                .as_deref()
                .unwrap_or("hey-proxy"),
        )
        .await;
    }
    metadata().await
}

async fn profile(
    client: &Client,
    all: &Profiles,
    name: &str,
    region: &str,
    depth: usize,
) -> Result<Credentials> {
    ensure!(depth < 8, "AWS role chain is cyclic or too deep");
    let p = all.get(name).context("AWS source profile is unavailable")?;
    let session = p
        .get("role_session_name")
        .map(String::as_str)
        .unwrap_or("hey-proxy");
    if let Some(role) = p.get("role_arn") {
        if let Some(file) = p.get("web_identity_token_file") {
            return web_identity(client, region, role, file, session).await;
        }
        let source = if let Some(source) = p.get("source_profile") {
            Box::pin(profile(client, all, source, region, depth + 1)).await?
        } else {
            match p.get("credential_source").map(String::as_str) {
                Some("Environment") => {
                    environment()?.context("AWS environment credentials unavailable")?
                }
                Some("Ec2InstanceMetadata" | "EcsContainer") => metadata().await?,
                _ => bail!("AWS role requires a source profile or credential source"),
            }
        };
        ensure!(
            !p.contains_key("mfa_serial"),
            "AWS role requires MFA; use a credential_process that supplies the authenticated session"
        );
        let mut form = vec![
            ("Action", "AssumeRole".to_owned()),
            ("Version", "2011-06-15".to_owned()),
            ("RoleArn", role.clone()),
            ("RoleSessionName", session.to_owned()),
        ];
        if let Some(id) = p.get("external_id") {
            form.push(("ExternalId", id.clone()));
        }
        if let Some(duration) = p.get("duration_seconds") {
            form.push(("DurationSeconds", duration.clone()));
        }
        return sts(client, region, &form, Some(&source)).await;
    }
    if let (Some(access), Some(secret)) =
        (p.get("aws_access_key_id"), p.get("aws_secret_access_key"))
    {
        return Ok(Credentials {
            access: access.clone(),
            secret: secret.clone(),
            token: p.get("aws_session_token").cloned(),
            expires: None,
        });
    }
    if let Some(command) = p.get("credential_process") {
        let mut command_process = tokio::process::Command::new("/bin/sh");
        command_process.args(["-c", command]);
        let bytes = super::command_output(command_process).await?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow!("Invalid AWS credential_process response"))?;
        ensure!(
            value["Version"] == 1,
            "Unsupported AWS credential_process version"
        );
        return credentials(&value);
    }
    bail!("AWS profile credentials unavailable")
}

async fn web_identity(
    client: &Client,
    region: &str,
    role: &str,
    file: &str,
    session: &str,
) -> Result<Credentials> {
    let token = tokio::fs::read_to_string(file)
        .await
        .map_err(|_| anyhow!("Cannot read AWS web identity token"))?;
    ensure!(
        !token.trim().is_empty() && token.len() <= 1024 * 1024,
        "Invalid AWS web identity token"
    );
    sts(
        client,
        region,
        &[
            ("Action", "AssumeRoleWithWebIdentity".into()),
            ("Version", "2011-06-15".into()),
            ("RoleArn", role.into()),
            ("RoleSessionName", session.into()),
            ("WebIdentityToken", token.trim().into()),
        ],
        None,
    )
    .await
}

async fn bytes(mut response: reqwest::Response) -> Result<Vec<u8>> {
    ensure!(
        response.status().is_success(),
        "AWS credential endpoint rejected the request (HTTP {})",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Cannot read AWS credential response"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= 1024 * 1024,
            "AWS credential response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn sts(
    client: &Client,
    region: &str,
    form: &[(&str, String)],
    source: Option<&Credentials>,
) -> Result<Credentials> {
    let domain = if region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    };
    let mut request = client
        .post(format!("https://sts.{region}.{domain}/"))
        .form(form)
        .build()?;
    if let Some(source) = source {
        sign_with(&mut request, region, "sts", source, SystemTime::now())?;
    }
    let response = client
        .execute(request)
        .await
        .map_err(|_| anyhow!("AWS STS endpoint unavailable"))?;
    let bytes = bytes(response).await?;
    let xml = std::str::from_utf8(&bytes).map_err(|_| anyhow!("Invalid AWS STS response"))?;
    let mut value = serde_json::Map::new();
    for name in [
        "AccessKeyId",
        "SecretAccessKey",
        "SessionToken",
        "Expiration",
    ] {
        let token = xml
            .split_once(&format!("<{name}>"))
            .and_then(|(_, s)| s.split_once(&format!("</{name}>")))
            .map(|(s, _)| s)
            .context("Missing AWS STS credential field")?;
        // These credential fields are base64/ASCII and RFC3339, never arbitrary XML.
        ensure!(
            !token.contains(['<', '&']),
            "Invalid AWS STS credential field"
        );
        value.insert(name.into(), Value::String(token.into()));
    }
    credentials(&Value::Object(value))
}

async fn metadata() -> Result<Credentials> {
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()?;
    if let Some(relative) = env("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI") {
        ensure!(
            relative.starts_with('/') && !relative.starts_with("//"),
            "Invalid AWS container credential path"
        );
        return metadata_json(
            &client,
            &format!("http://169.254.170.2{relative}"),
            container_headers().await?,
        )
        .await;
    }
    if let Some(url) = env("AWS_CONTAINER_CREDENTIALS_FULL_URI") {
        let parsed = url::Url::parse(&url).context("Invalid AWS container credential URL")?;
        ensure!(
            parsed.scheme() == "https"
                || parsed.scheme() == "http"
                    && matches!(
                        parsed.host_str(),
                        Some(
                            "localhost"
                                | "127.0.0.1"
                                | "[::1]"
                                | "169.254.170.2"
                                | "169.254.170.23"
                                | "[fd00:ec2::23]"
                        )
                    ),
            "AWS container credentials require HTTPS or a local metadata endpoint"
        );
        return metadata_json(&client, &url, container_headers().await?).await;
    }
    ensure!(
        env("AWS_EC2_METADATA_DISABLED").is_none_or(|v| !v.eq_ignore_ascii_case("true")),
        "AWS instance metadata is disabled"
    );
    let endpoint = env("AWS_EC2_METADATA_SERVICE_ENDPOINT").unwrap_or_else(|| {
        if env("AWS_EC2_METADATA_SERVICE_ENDPOINT_MODE").as_deref() == Some("IPv6") {
            "http://[fd00:ec2::254]".into()
        } else {
            "http://169.254.169.254".into()
        }
    });
    let endpoint = endpoint.trim_end_matches('/');
    let response = client
        .put(format!("{endpoint}/latest/api/token"))
        .header("x-aws-ec2-metadata-token-ttl-seconds", "21600")
        .send()
        .await
        .map_err(|_| anyhow!("AWS instance metadata unavailable"))?;
    let mut headers = HeaderMap::new();
    if response.status().is_success() {
        let token = bytes(response).await?;
        headers.insert(
            "x-aws-ec2-metadata-token",
            sensitive(
                std::str::from_utf8(&token).map_err(|_| anyhow!("Invalid AWS metadata token"))?,
            )?,
        );
    } else {
        ensure!(
            [403, 404, 405].contains(&response.status().as_u16())
                && env("AWS_EC2_METADATA_V1_DISABLED")
                    .is_none_or(|v| !v.eq_ignore_ascii_case("true")),
            "AWS IMDSv2 token unavailable"
        );
    }
    let roles = bytes(
        client
            .get(format!(
                "{endpoint}/latest/meta-data/iam/security-credentials/"
            ))
            .headers(headers.clone())
            .send()
            .await
            .map_err(|_| anyhow!("AWS instance role unavailable"))?,
    )
    .await?;
    let role = std::str::from_utf8(&roles)
        .ok()
        .and_then(|s| s.lines().next())
        .filter(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+=,.@_-".contains(&b))
        })
        .context("Invalid AWS instance role")?;
    metadata_json(
        &client,
        &format!("{endpoint}/latest/meta-data/iam/security-credentials/{role}"),
        headers,
    )
    .await
}
async fn container_headers() -> Result<HeaderMap> {
    let token = if let Some(path) = env("AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE") {
        Some(
            tokio::fs::read_to_string(path)
                .await
                .map_err(|_| anyhow!("Cannot read AWS container authorization token"))?,
        )
    } else {
        env("AWS_CONTAINER_AUTHORIZATION_TOKEN")
    };
    let mut headers = HeaderMap::new();
    if let Some(token) = token {
        headers.insert(AUTHORIZATION, sensitive(token.trim())?);
    }
    Ok(headers)
}
async fn metadata_json(client: &Client, url: &str, headers: HeaderMap) -> Result<Credentials> {
    let response = client
        .get(url)
        .headers(headers)
        .send()
        .await
        .map_err(|_| anyhow!("AWS metadata credential endpoint unavailable"))?;
    let value: Value = serde_json::from_slice(&bytes(response).await?)
        .map_err(|_| anyhow!("Invalid AWS metadata credentials"))?;
    credentials(&value)
}

// Google exchanges a serialized, signed GetCallerIdentity request for a token.
// It sends that request itself; we must not send the signed assertion to STS.
pub(super) async fn federated_subject(config: &Value) -> Result<String> {
    let source = &config["credential_source"];
    ensure!(
        source["environment_id"] == "aws1",
        "Unsupported AWS federation environment"
    );
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()?;
    let environment = environment()?;
    let mut headers = HeaderMap::new();
    let configured_region = env("AWS_REGION").or_else(|| env("AWS_DEFAULT_REGION"));
    if (environment.is_none() || configured_region.is_none())
        && let Some(url) = source["imdsv2_session_token_url"].as_str()
    {
        let response = client
            .put(url)
            .header("x-aws-ec2-metadata-token-ttl-seconds", "300")
            .send()
            .await
            .map_err(|_| anyhow!("AWS federation metadata unavailable"))?;
        let token = bytes(response).await?;
        headers.insert(
            "x-aws-ec2-metadata-token",
            sensitive(
                std::str::from_utf8(&token).map_err(|_| anyhow!("Invalid AWS metadata token"))?,
            )?,
        );
    }
    let region = if let Some(region) = configured_region {
        region
    } else {
        let response = client
            .get(required(source, "region_url")?)
            .headers(headers.clone())
            .send()
            .await
            .map_err(|_| anyhow!("AWS federation region unavailable"))?;
        let zone = bytes(response).await?;
        let zone = std::str::from_utf8(&zone)
            .map_err(|_| anyhow!("Invalid AWS availability zone"))?
            .trim();
        ensure!(
            zone.len() > 1 && zone.is_ascii(),
            "Invalid AWS availability zone"
        );
        zone[..zone.len() - 1].to_owned()
    };
    ensure!(
        !region.is_empty()
            && region
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "Invalid AWS federation region"
    );
    let credentials = if let Some(c) = environment {
        c
    } else {
        let endpoint = required(source, "url")?.trim_end_matches('/');
        let roles = bytes(
            client
                .get(endpoint)
                .headers(headers.clone())
                .send()
                .await
                .map_err(|_| anyhow!("AWS federation role unavailable"))?,
        )
        .await?;
        let role = std::str::from_utf8(&roles)
            .ok()
            .and_then(|s| s.lines().next())
            .filter(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"+=,.@_-".contains(&b))
            })
            .context("Invalid AWS federation role")?;
        metadata_json(&client, &format!("{endpoint}/{role}"), headers).await?
    };
    let url = required(source, "regional_cred_verification_url")?.replace("{region}", &region);
    let mut request = client
        .post(&url)
        .header(
            "x-goog-cloud-target-resource",
            required(config, "audience")?,
        )
        .build()?;
    sign_with(
        &mut request,
        &region,
        "sts",
        &credentials,
        SystemTime::now(),
    )?;
    let headers = request
        .headers()
        .iter()
        .map(|(key, value)| Ok(serde_json::json!({"key":key.as_str(),"value":value.to_str()?})))
        .collect::<Result<Vec<_>>>()?;
    Ok(encode(
        serde_json::json!({"url":url,"method":"POST","headers":headers})
            .to_string()
            .as_bytes(),
    ))
}

#[cfg(test)]
mod tests;
