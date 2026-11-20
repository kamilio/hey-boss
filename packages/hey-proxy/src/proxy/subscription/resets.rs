use super::*;
use hey_proxy::usage::{ResetRequest, ResetResult};

pub(in crate::proxy) const CLAUDE_USER_AGENT: &str = "claude-cli/2.1.329 (external, cli)";

pub(in crate::proxy) fn result(success: bool, message: &str) -> Value {
    json!(ResetResult {
        schema_version: SCHEMA_VERSION,
        success,
        message: message.into()
    })
}

pub(in crate::proxy) fn validate(
    data: Option<&Value>,
    updated_at: Option<u64>,
    request: &ResetRequest,
) -> bool {
    let now = crate::claude_auth::now();
    updated_at.is_some_and(|t| t <= now && now - t < 300)
        && data
            .and_then(|d| d["resets"].as_array())
            .is_some_and(|grants| {
                grants.iter().any(|g| {
                    g["id"] == request.grant_id
                        && g["usable"] == true
                        && g["remaining"].as_u64().is_some_and(|n| n > 0)
                        && g["expires_at"].as_str().is_none_or(|s| {
                            hey_proxy::usage::parse_timestamp(s).is_some_and(|t| t > now)
                        })
                })
            })
}

// Never retry a spend automatically or return arbitrary provider response fields.
pub(in crate::proxy) async fn send(request: reqwest::RequestBuilder, provider: &str) -> Value {
    let response = async {
        let mut response = request.timeout(Duration::from_secs(25)).send().await?;
        anyhow::ensure!(response.status().is_success(), "Reset rejected");
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= 1024 * 1024,
                "Reset response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok::<Value, anyhow::Error>(serde_json::from_slice(&bytes)?)
    }
    .await;
    let Ok(value) = response else {
        return result(
            false,
            "Reset could not be confirmed. Refresh usage before trying again.",
        );
    };
    let outcome = if provider == "claude" {
        value["result"].as_str()
    } else {
        value["code"].as_str()
    };
    match outcome {
        Some("reset" | "redeemed") => result(true, "Reset applied."),
        Some("already_used" | "already_redeemed") => {
            result(false, "This reset was already used. Refresh usage.")
        }
        Some("not_limited" | "nothing_to_reset") => {
            result(false, "There is no eligible usage to reset.")
        }
        Some("cooldown") => result(
            false,
            "The provider requires a cooldown before another reset.",
        ),
        _ => result(
            false,
            "The provider did not confirm a reset. Refresh usage before trying again.",
        ),
    }
}

pub(in crate::proxy) async fn reset(
    State(service): State<Arc<Service>>,
    axum::extract::Path((provider, account)): axum::extract::Path<(String, String)>,
    axum::Json(request): axum::Json<ResetRequest>,
) -> Response {
    if [&provider, &account, &request.grant_id, &request.request_id]
        .iter()
        .any(|s| {
            s.is_empty()
                || s.len() > 128
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        })
    {
        return failure(
            StatusCode::BAD_REQUEST,
            "invalid_identifier",
            "Invalid reset request",
        );
    }
    let proxy = service.snapshot();
    if proxy.config.mode == Mode::Client {
        return relay_request::<ResetResult>(
            &proxy,
            &format!("/usage/v1/{provider}/{account}/reset"),
            Some(&request),
        )
        .await;
    }
    let selected;
    let proxy = if account != "default" {
        if !proxy
            .config
            .accounts
            .get(&account)
            .is_some_and(|a| a.implementation() == provider && a.auth() == "subscription")
        {
            return failure(
                StatusCode::NOT_FOUND,
                "unknown_account",
                "Unknown subscription account",
            );
        }
        selected = match proxy.select(&account).await {
            Ok(selected) => selected,
            Err(_) => {
                return failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "account_unavailable",
                    "Account is not ready",
                );
            }
        };
        &selected
    } else {
        &proxy
    };
    let value = match provider.as_str() {
        "claude" => claude::usage::read_or_reset(proxy, Some(&request)).await,
        "codex" => codex::usage::read_or_reset(proxy, Some(&request)).await,
        _ => {
            return failure(
                StatusCode::NOT_IMPLEMENTED,
                "unsupported_provider",
                "Unsupported reset provider",
            );
        }
    };
    // Credential failures use the reading schema internally; keep the reset API stable.
    reply(
        serde_json::from_value::<ResetResult>(value).unwrap_or(ResetResult {
            schema_version: SCHEMA_VERSION,
            success: false,
            message:
                "The subscription account is unavailable. Refresh usage and sign in if needed."
                    .into(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        (
            url,
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
        )
    }

    #[tokio::test]
    async fn rejected_or_unknown_spends_are_not_retried_or_reported_as_success() {
        for (status, body) in [
            (StatusCode::TOO_MANY_REQUESTS, "PRIVATE error"),
            (StatusCode::OK, "not JSON"),
            (
                StatusCode::OK,
                r#"{"code":"future_outcome","private":"PRIVATE"}"#,
            ),
            (StatusCode::OK, r#"{"code":"nothing_to_reset"}"#),
        ] {
            let hits = Arc::new(AtomicUsize::new(0));
            let count = hits.clone();
            let (url, task) = serve(Router::new().fallback(axum::routing::post(move || {
                count.fetch_add(1, Ordering::SeqCst);
                async move { (status, body) }
            })))
            .await;
            let response = send(reqwest::Client::new().post(url), "codex").await;
            assert_eq!(response["success"], false);
            assert_eq!(hits.load(Ordering::SeqCst), 1);
            assert!(!response.to_string().contains("PRIVATE"));
            task.abort();
        }
    }

    #[tokio::test]
    async fn resets_are_explicit_allowlisted_and_invalidate_spent_grants() {
        for provider in ["claude", "codex"] {
            let spends = Arc::new(AtomicUsize::new(0));
            let hits = spends.clone();
            let (upstream, upstream_task) = serve(Router::new().fallback(axum::routing::any(move |request: Request| {
                let hits = hits.clone();
                async move {
                    let path = request.uri().path().to_owned();
                    let value = match path.as_str() {
                        "/api/oauth/profile" => json!({"organization":{"uuid":"org-synthetic"}}),
                        "/api/oauth/usage" => {
                            assert_eq!(request.headers()["user-agent"], CLAUDE_USER_AGENT);
                            json!({"five_hour":{"utilization":100},"cedar_ember":{"eligible":true,"grants":[
                                {"id":"grant-1","label":"Full reset","resets_left":1,"usable_now":true,"ends_at":"2099-01-01T00:00:00Z","private":"PRIVATE"},
                                {"id":"expired","label":"Expired","resets_left":1,"usable_now":true,"ends_at":"2020-01-01T00:00:00Z"}]}})
                        },
                        "/backend-api/wham/usage" => json!({"rate_limit":{"primary_window":{"used_percent":100}},"rate_limit_reset_credits":{"available_count":1}}),
                        "/backend-api/wham/rate-limit-reset-credits" => json!({"credits":[
                            {"id":"grant-1","title":"Full reset","status":"available","reset_type":"codex_rate_limits","is_supported_by_plan":true,"expires_at":"2099-01-01T00:00:00Z","private":"PRIVATE"}]}),
                        "/api/organizations/org-synthetic/reset_rate_limits" | "/backend-api/wham/rate-limit-reset-credits/consume" => {
                            assert_eq!(request.method(), reqwest::Method::POST);
                            let body: Value = serde_json::from_slice(&axum::body::to_bytes(request.into_body(), 4096).await.unwrap()).unwrap();
                            if provider == "claude" {
                                assert_eq!(body, json!({"program":"cedar_ember","grant_id":"grant-1","request_id":"request-1"}));
                            } else {
                                assert_eq!(body, json!({"credit_id":"grant-1","redeem_request_id":"request-1"}));
                            }
                            hits.fetch_add(1, Ordering::SeqCst);
                            if provider == "claude" { json!({"result":"reset","private":"PRIVATE"}) } else { json!({"code":"reset","windows_reset":1,"private":"PRIVATE"}) }
                        },
                        _ => panic!("Unexpected path: {path}"),
                    };
                    axum::Json(value)
                }
            }))).await;
            let dir = tempfile::tempdir().unwrap();
            let credentials = dir.path().join("credentials.json");
            if provider == "claude" {
                crate::claude_auth::tests::fixture(&credentials, crate::claude_auth::now() + 3600);
            } else {
                crate::codex_auth::tests::fixture(&credentials, crate::codex_auth::now() + 3600);
            }
            let config: Config = serde_json::from_value(json!({"listen":"127.0.0.1:0","providers":{provider:{"upstream_url":upstream,"credentials_file":credentials}}})).unwrap();
            let (url, task) = serve(router(config).unwrap()).await;
            let client = hey_proxy::usage::Client::new(&url, None).unwrap();
            let request = ResetRequest {
                grant_id: "grant-1".into(),
                request_id: "request-1".into(),
            };
            assert!(
                !client
                    .reset(provider, "default", &request)
                    .await
                    .unwrap()
                    .success,
                "Read before spending"
            );
            let reading = client.usage(provider, "default").await.unwrap();
            assert_eq!(
                reading
                    .reading
                    .data
                    .as_ref()
                    .unwrap()
                    .resets
                    .as_ref()
                    .unwrap()[0]
                    .id,
                "grant-1"
            );
            assert!(!serde_json::to_string(&reading).unwrap().contains("PRIVATE"));
            assert!(client.reset(provider, "missing", &request).await.is_err());
            let invalid = ResetRequest {
                grant_id: "expired".into(),
                request_id: "request-1".into(),
            };
            assert!(
                !client
                    .reset(provider, "default", &invalid)
                    .await
                    .unwrap()
                    .success
            );
            let (first, second) = tokio::join!(
                client.reset(provider, "default", &request),
                client.reset(provider, "default", &request)
            );
            assert_ne!(
                first.as_ref().unwrap().success,
                second.as_ref().unwrap().success
            );
            assert_eq!(spends.load(Ordering::SeqCst), 1);
            assert!(
                !serde_json::to_string(&first.unwrap())
                    .unwrap()
                    .contains("PRIVATE")
            );
            task.abort();
            upstream_task.abort();
        }
    }
}
