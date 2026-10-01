//! On-demand management API. No startup hooks, inference accounting, or model-path locks.
use super::*;
use hey_proxy::usage::{Account, AccountUsage, Accounts, Reading, SCHEMA_VERSION};
use serde::{Serialize, de::DeserializeOwned};

fn reply<T: Serialize>(value: T) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], axum::Json(value)).into_response()
}
fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(json!({"error":{"code":code,"message":message}})),
    )
        .into_response()
}

pub(super) async fn accounts(State(service): State<Arc<Service>>) -> Response {
    let proxy = service.snapshot();
    if proxy.config.mode == Mode::Client {
        return relay::<Accounts>(&proxy, "/usage/v1/accounts").await;
    }
    reply(Accounts {
        schema_version: SCHEMA_VERSION,
        accounts: proxy
            .config
            .claude
            .as_ref()
            .map(|_| Account {
                provider: "claude".into(),
                id: "default".into(),
            })
            .into_iter()
            .collect(),
    })
}

pub(super) async fn usage(
    State(service): State<Arc<Service>>,
    axum::extract::Path((provider, account)): axum::extract::Path<(String, String)>,
) -> Response {
    if [&provider, &account].iter().any(|id| {
        id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    }) {
        return failure(
            StatusCode::BAD_REQUEST,
            "invalid_identifier",
            "Invalid provider or account ID",
        );
    }
    let proxy = service.snapshot();
    // Dispatch on the host so a relay can query providers added by a newer host.
    if proxy.config.mode == Mode::Client {
        return relay::<AccountUsage>(&proxy, &format!("/usage/v1/{provider}/{account}")).await;
    }
    if provider != "claude" {
        return failure(
            StatusCode::NOT_IMPLEMENTED,
            "unsupported_provider",
            "Subscription usage is not implemented for this provider",
        );
    }
    if account != "default" {
        return failure(
            StatusCode::NOT_FOUND,
            "unknown_account",
            "Unknown subscription account",
        );
    }
    let reading: Reading = match serde_json::from_value(claude::reading(&proxy).await) {
        Ok(reading) => reading,
        Err(_) => {
            return failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "invalid_reading",
                "Cannot normalize subscription usage",
            );
        }
    };
    reply(AccountUsage {
        schema_version: SCHEMA_VERSION,
        account: Account {
            provider,
            id: account,
        },
        reading,
    })
}

// Independent of inference forwarding: no retries, body inspection or request-log writes.
// Deserialize through the public schema to retain the allowlist across client relays.
async fn relay<T: DeserializeOwned + Serialize>(proxy: &Proxy, path: &str) -> Response {
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let source = proxy
            .config
            .api_keys
            .get(&proxy.config.default.api_key)
            .ok_or_else(|| anyhow::anyhow!("Host credential is missing"))?;
        let token = proxy
            .service
            .credentials
            .resolve(
                source,
                Duration::from_secs(proxy.config.credential_cache_seconds),
            )
            .await?;
        let mut response = proxy
            .client
            .get(format!(
                "{}{path}",
                proxy.config.upstream_url.trim_end_matches('/')
            ))
            .bearer_auth(token.to_str()?)
            .header("accept", "application/json")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Ok::<_, anyhow::Error>(Err(status));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= 1024 * 1024,
                "Host usage response is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(Ok(serde_json::from_slice::<T>(&bytes)?))
    })
    .await;
    match result {
        Ok(Ok(Ok(value))) => reply(value),
        Ok(Ok(Err(
            status
            @ (StatusCode::NOT_FOUND | StatusCode::NOT_IMPLEMENTED | StatusCode::BAD_REQUEST),
        ))) => failure(
            status,
            "host_usage_rejected",
            "Host does not support this usage provider, account or endpoint",
        ),
        _ => failure(
            StatusCode::BAD_GATEWAY,
            "host_usage_unavailable",
            "Host usage unavailable; check the host connection and access key",
        ),
    }
}
