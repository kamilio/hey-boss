//! Explicit Bedrock namespace. Ordinary model routing never enters this transport.
pub(super) mod catalog;
use super::*;
use anyhow::{Context, ensure};
use aws_credential_types::provider::ProvideCredentials;
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use axum::http::Method;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields)]
pub(crate) enum Provider {
    Aws { region: String },
    Relay { relay_socket: PathBuf },
}
impl Provider {
    pub(crate) fn validate(&self) -> Result<()> {
        match self {
            Self::Aws { region } => ensure!(
                !region.is_empty()
                    && region.len() < 64
                    && region
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
                "Invalid Bedrock region"
            ),
            Self::Relay { relay_socket } => ensure!(
                relay_socket.is_absolute(),
                "Bedrock relay socket requires an absolute path"
            ),
        }
        Ok(())
    }
}

async fn signed(
    client: &reqwest::Client,
    region: &str,
    method: Method,
    url: &str,
    mut headers: HeaderMap,
    body: Bytes,
) -> Result<reqwest::Response> {
    static PROVIDER: tokio::sync::OnceCell<
        aws_config::default_provider::credentials::DefaultCredentialsChain,
    > = tokio::sync::OnceCell::const_new();
    let provider = PROVIDER
        .get_or_init(|| {
            aws_config::default_provider::credentials::DefaultCredentialsChain::builder().build()
        })
        .await;
    let identity = provider
        .provide_credentials()
        .await
        .context("AWS credentials unavailable")?
        .into();
    let params = aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name("bedrock")
        .time(SystemTime::now())
        .settings(SigningSettings::default())
        .build()?
        .into();
    let request = SignableRequest::new(
        method.as_str(),
        url,
        headers
            .iter()
            .map(|(k, v)| Ok((k.as_str(), v.to_str()?)))
            .collect::<Result<Vec<_>>>()?
            .into_iter(),
        SignableBody::Bytes(&body),
    )?;
    let (instructions, _) = sign(request, &params)?.into_parts();
    for h in instructions.into_parts().0 {
        let mut value = header::HeaderValue::from_str(h.value())?;
        value.set_sensitive(h.sensitive());
        headers.insert(h.name(), value);
    }
    Ok(client
        .request(method, url)
        .headers(headers)
        .body(body)
        .send()
        .await?)
}

fn family(id: &str) -> Option<&str> {
    let (_, rest) = id.split_once('.')?;
    match rest.split_once('.')?.0 {
        "anthropic" => Some("anthropic"),
        "openai" => Some("openai"),
        _ => None,
    }
}

async fn relay(
    socket: &std::path::Path,
    method: Method,
    path: &str,
    headers: HeaderMap,
    body: Bytes,
) -> Result<reqwest::Response> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .unix_socket(socket.to_path_buf())
        .build()?;
    Ok(client
        .request(method, format!("http://localhost{path}"))
        .headers(headers)
        .body(body)
        .send()
        .await?)
}

fn passthrough(res: reqwest::Response) -> Response {
    let status = res.status();
    let mut headers = res.headers().clone();
    clean_headers(&mut headers);
    let mut response = Response::new(Body::from_stream(res.bytes_stream()));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

pub(super) async fn forward(proxy: Arc<Proxy>, request: Request) -> Response {
    let Some(provider) = &proxy.config.bedrock else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Bedrock is not configured on this host",
        );
    };
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let (expected, destination) = match (parts.method.clone(), path) {
        (Method::GET, "/bedrock/v1/models") => return catalog::reply(&proxy, provider),
        (Method::POST, "/bedrock/v1/responses") => ("openai", "/openai/v1/responses"),
        (Method::POST, "/bedrock/v1/messages") => ("anthropic", "/anthropic/v1/messages"),
        _ => return error(StatusCode::NOT_FOUND, "Unsupported Bedrock endpoint"),
    };
    let mut headers = HeaderMap::new();
    let body = {
        let Ok(bytes) = axum::body::to_bytes(body, 64 * 1024 * 1024).await else {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "Bedrock request too large");
        };
        #[derive(Deserialize)]
        struct Model<'a> {
            model: &'a str,
        }
        let Ok(model) = serde_json::from_slice::<Model>(&bytes) else {
            return error(StatusCode::BAD_REQUEST, "Bedrock requires a JSON model");
        };
        if family(model.model) != Some(expected) {
            return error(
                StatusCode::BAD_REQUEST,
                "Use a matching Bedrock profile ID from /bedrock/v1/models",
            );
        }
        proxy.service.logs.route(
            proxy.log_id,
            Some(model.model.into()),
            Some(model.model.into()),
            "bedrock",
        );
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        if expected == "anthropic" {
            headers.insert(
                "anthropic-version",
                parts
                    .headers
                    .get("anthropic-version")
                    .cloned()
                    .unwrap_or_else(|| header::HeaderValue::from_static("2023-06-01")),
            );
            if let Some(beta) = parts.headers.get("anthropic-beta") {
                headers.insert("anthropic-beta", beta.clone());
            }
        }
        bytes
    };
    proxy
        .service
        .logs
        .attribution(proxy.log_id, "bedrock", "bedrock", "Bedrock", "api");
    let result = match provider {
        Provider::Aws { region } => signed(
            &proxy.client,
            region,
            parts.method,
            &format!("https://bedrock-runtime.{region}.amazonaws.com{destination}"),
            headers,
            body,
        )
        .await
        .map(passthrough),
        Provider::Relay { relay_socket } => relay(relay_socket, parts.method, path, headers, body)
            .await
            .map(passthrough),
    };
    result.unwrap_or_else(|_| {
        error(
            StatusCode::BAD_GATEWAY,
            "Bedrock unavailable; check the AWS identity or SSH relay on the provider host",
        )
    })
}

#[cfg(test)]
mod tests;
