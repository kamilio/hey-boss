use super::*;
use axum::{Router, routing::post};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

async fn server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, task)
}

fn user(endpoint: &str) -> Value {
    json!({"type":"authorized_user","client_id":"client","client_secret":"secret","refresh_token":"refresh","token_uri":endpoint,"quota_project_id":"quota-project"})
}

#[tokio::test]
async fn refresh_uses_oauth_form_and_never_returns_endpoint_secrets() {
    let (url, task) = server(Router::new().route(
        "/",
        post(|body: String| async move {
            let fields: std::collections::HashMap<_, _> =
                url::form_urlencoded::parse(body.as_bytes())
                    .into_owned()
                    .collect();
            assert_eq!(fields["grant_type"], "refresh_token");
            assert_eq!(fields["client_id"], "client");
            assert_eq!(fields["client_secret"], "secret");
            assert_eq!(fields["refresh_token"], "refresh");
            axum::Json(json!({"access_token":"access","expires_in":3600}))
        }),
    ))
    .await;
    let (token, expires) = from_config(&Client::new(), &user(&url), 0).await.unwrap();
    assert_eq!(token, "access");
    assert!(expires.duration_since(SystemTime::now()).unwrap().as_secs() >= 3590);
    assert!(authorization(&token).unwrap().is_sensitive());
    task.abort();
    for status in [400, 401, 403, 500] {
        let (url, task) = server(Router::new().route(
            "/",
            post(move || async move {
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    "private-refresh-token",
                )
            }),
        ))
        .await;
        let error = from_config(&Client::new(), &user(&url), 0)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(&status.to_string()));
        assert!(!error.contains("private-refresh-token"));
        task.abort();
    }
}

#[tokio::test]
async fn malformed_and_unbounded_token_responses_fail_closed() {
    for body in [
        "not json".to_owned(),
        json!({"access_token":"secret"}).to_string(),
        json!({"access_token":"secret","expires_in":0}).to_string(),
        "x".repeat(1024 * 1024 + 1),
    ] {
        let (url, task) = server(Router::new().route(
            "/",
            post(move || {
                let body = body.clone();
                async move { body }
            }),
        ))
        .await;
        assert!(from_config(&Client::new(), &user(&url), 0).await.is_err());
        task.abort();
    }
}

#[test]
fn service_account_assertion_is_verifiable_and_has_oauth_claims() {
    let pem = include_str!("test-key.pem"); // Generated solely for this test; never a deployed credential.
    let config =
        json!({"client_email":"test@example.invalid","private_key_id":"fixture","private_key":pem});
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let jwt = assertion(&config, TOKEN_URL, now).unwrap();
    let parts: Vec<_> = jwt.split('.').collect();
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    assert_eq!(
        claims,
        json!({"iss":"test@example.invalid","scope":SCOPE,"aud":TOKEN_URL,"iat":1_700_000_000u64,"exp":1_700_003_600u64})
    );
    let der = STANDARD
        .decode(
            pem.lines()
                .filter(|l| !l.starts_with('-'))
                .collect::<String>(),
        )
        .unwrap();
    let key = ring::signature::RsaKeyPair::from_pkcs8(&der).unwrap();
    ring::signature::UnparsedPublicKey::new(
        &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        key.public().as_ref(),
    )
    .verify(
        format!("{}.{}", parts[0], parts[1]).as_bytes(),
        &URL_SAFE_NO_PAD.decode(parts[2]).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn federation_and_impersonation_exchange_the_correct_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let subject = dir.path().join("subject.json");
    std::fs::write(&subject, r#"{"id_token":"subject-token"}"#).unwrap();
    let (url, task) = server(Router::new()
        .route("/sts",post(|body: String| async move {
            let fields: std::collections::HashMap<_,_> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
            assert_eq!(fields["subject_token"],"subject-token");
            assert_eq!(fields["audience"],"audience");
            axum::Json(json!({"access_token":"federated","expires_in":3600}))
        }))
        .route("/impersonate",post(|headers: HeaderMap, axum::Json(body): axum::Json<Value>| async move {
            assert_eq!(headers[AUTHORIZATION],"Bearer federated");
            assert_eq!(body["scope"],json!([SCOPE]));
            axum::Json(json!({"accessToken":"impersonated","expireTime":"2030-01-02T03:04:05Z"}))
        }))).await;
    let config = json!({"type":"external_account","audience":"audience","subject_token_type":"urn:ietf:params:oauth:token-type:jwt","token_url":format!("{url}/sts"),"service_account_impersonation_url":format!("{url}/impersonate"),"credential_source":{"file":subject,"format":{"type":"json","subject_token_field_name":"id_token"}}});
    let (token, expiry) = from_config(&Client::new(), &config, 0).await.unwrap();
    assert_eq!(token, "impersonated");
    assert_eq!(
        jiff::Timestamp::try_from(expiry).unwrap().to_string(),
        "2030-01-02T03:04:05Z"
    );
    task.abort();
}

#[test]
fn wall_clock_expiry_and_backward_clock_changes_invalidate_cached_tokens() {
    let now = SystemTime::now();
    let mut token = Token {
        headers: HeaderMap::new(),
        issued: now - Duration::from_secs(3600),
        expires: now - Duration::from_secs(1),
    };
    assert!(!token.valid());
    token.expires = now + Duration::from_secs(3600);
    assert!(token.valid());
    token.issued = now + Duration::from_secs(300);
    assert!(!token.valid());
}

#[tokio::test]
async fn adc_discovery_cache_and_invalidation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let (url, task) = server(Router::new().route(
        "/",
        post(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::Json(json!({"access_token":"synthetic","expires_in":3600}))
            }
        }),
    ))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("adc.json");
    std::fs::write(&config, user(&url).to_string()).unwrap();
    let result = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "credentials::google::tests::discovery_child",
            "--nocapture",
        ])
        .env("HEY_PROXY_ADC_TEST_CHILD", "1")
        .env("GOOGLE_APPLICATION_CREDENTIALS", &config)
        .env("GOOGLE_CLOUD_QUOTA_PROJECT", "quota-override")
        .output()
        .await
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn discovery_child() {
    if std::env::var_os("HEY_PROXY_ADC_TEST_CHILD").is_none() {
        return;
    }
    let resolver = Arc::new(super::super::CredentialResolver::default());
    let mut requests = Vec::new();
    for _ in 0..20 {
        let resolver = resolver.clone();
        requests.push(tokio::spawn(async move {
            resolver.adc_headers().await.unwrap()
        }));
    }
    for request in requests {
        let headers = request.await.unwrap();
        assert_eq!(headers[AUTHORIZATION], "Bearer synthetic");
        assert!(headers[AUTHORIZATION].is_sensitive());
        assert_eq!(headers["x-goog-user-project"], "quota-override");
    }
    resolver.invalidate_adc().await;
    resolver.adc_headers().await.unwrap();
}

#[tokio::test]
async fn metadata_identity_requires_google_header_and_explicit_files_never_fall_back() {
    use axum::routing::get;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let (url, task) = server(Router::new().route(
        "/computeMetadata/v1/instance/service-accounts/default/token",
        get(move |headers: HeaderMap| {
            let counter = counter.clone();
            async move {
                assert_eq!(headers["metadata-flavor"], "Google");
                counter.fetch_add(1, Ordering::SeqCst);
                (
                    [("metadata-flavor", "Google")],
                    axum::Json(json!({"access_token":"synthetic","expires_in":3600})),
                )
            }
        }),
    ))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "credentials::google::tests::discovery_child",
            "--nocapture",
        ])
        .env("HEY_PROXY_ADC_TEST_CHILD", "1")
        .env_remove("GOOGLE_APPLICATION_CREDENTIALS")
        .env("CLOUDSDK_CONFIG", dir.path())
        .env("GOOGLE_CLOUD_QUOTA_PROJECT", "quota-override")
        .env("GCE_METADATA_HOST", url.strip_prefix("http://").unwrap());
    let result = child.output().await.unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    child.env(
        "GOOGLE_APPLICATION_CREDENTIALS",
        dir.path().join("missing.json"),
    );
    assert!(!child.output().await.unwrap().status.success());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    task.abort();
}
