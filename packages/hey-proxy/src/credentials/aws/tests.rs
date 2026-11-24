use super::*;

#[test]
fn signing_matches_aws_reference_for_bedrock_and_query_encoding() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_440_938_160);
    let c = Credentials {
        access: "AKIDEXAMPLE".into(),
        secret: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
        token: Some("synthetic-session-token".into()),
        expires: None,
    };
    // Golden signatures captured from aws-sigv4 1.6.4 before removing the SDK.
    let signatures = [
        "3624926ca4c438090dd9cb90485df83c327ed66e623eb906084de558ad37ba01",
        "6426bee361ab73aa3dd0a307166f506c93ead66f61467a5a927256e0e56bee2a",
        "9cdb9746edb4d6fa5bfa9a0aa893e15ad9792ccea6e804636c9765a22cb9ce98",
    ];
    for (index,(method,url,body)) in [
        ("GET","https://bedrock.us-east-1.amazonaws.com/inference-profiles?typeEquals=SYSTEM_DEFINED&maxResults=100", ""),
        ("POST","https://bedrock-runtime.us-east-1.amazonaws.com/model/global.anthropic.example%3A0/invoke", "{\"message\":\"synthetic\"}"),
        ("GET","https://example.amazonaws.com/a//b/%E2%98%83/?z=2&a=1&a=0&space=a+b&escaped=%2F", ""),
    ].into_iter().enumerate() {
        let mut request=Client::new().request(method.parse().unwrap(),url).header("content-type","application/json").header("x-custom","  spaced   header  ").body(body).build().unwrap();
        let expected=format!("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/bedrock/aws4_request, SignedHeaders=content-type;host;x-amz-date;x-amz-security-token;x-custom, Signature={}",signatures[index]);
        sign_with(&mut request,"us-east-1","bedrock",&c,now).unwrap();
        assert_eq!(request.headers()[AUTHORIZATION],expected);
        assert!(request.headers()[AUTHORIZATION].is_sensitive());
        assert!(request.headers()["x-amz-security-token"].is_sensitive());
    }
}

#[test]
fn profile_files_merge_without_including_unrelated_sections() {
    let mut all = Profiles::new();
    profiles("[default]\nregion = us-east-1\n[profile work]\ncredential_process = helper --arg\n[sso-session ignored]\nkey = value",true,&mut all).unwrap();
    profiles(
        "[work]\naws_access_key_id = fixture\naws_secret_access_key = synthetic",
        false,
        &mut all,
    )
    .unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all["work"]["credential_process"], "helper --arg");
    assert_eq!(all["work"]["aws_access_key_id"], "fixture");
    assert!(!all.contains_key("sso-session ignored"));
}

#[tokio::test]
async fn credential_process_preserves_expiry_and_redacts_failures() {
    let mut all = Profiles::new();
    all.insert("process".into(),BTreeMap::from([("credential_process".into(),"printf '%s' '{\"Version\":1,\"AccessKeyId\":\"fixture\",\"SecretAccessKey\":\"synthetic\",\"SessionToken\":\"session\",\"Expiration\":\"2030-01-01T00:00:00Z\"}'".into())]));
    let c = profile(&Client::new(), &all, "process", "us-east-1", 0)
        .await
        .unwrap();
    assert_eq!(c.access, "fixture");
    assert_eq!(c.token.as_deref(), Some("session"));
    assert!(c.expires.is_some());
    all.get_mut("process").unwrap().insert(
        "credential_process".into(),
        "printf private-secret >&2; exit 1".into(),
    );
    let error = profile(&Client::new(), &all, "process", "us-east-1", 0)
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(!error.contains("private-secret"));
}

#[tokio::test]
async fn cyclic_role_profiles_fail_without_network_requests() {
    let mut all = Profiles::new();
    all.insert(
        "cycle".into(),
        BTreeMap::from([
            ("role_arn".into(), "role".into()),
            ("source_profile".into(), "cycle".into()),
        ]),
    );
    assert!(
        profile(&Client::new(), &all, "cycle", "us-east-1", 0)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("cyclic")
    );
}

#[test]
fn expired_or_incomplete_credentials_are_rejected() {
    assert!(credentials(&serde_json::json!({"AccessKeyId":"key","SecretAccessKey":"secret","Expiration":"2000-01-01T00:00:00Z"})).is_err());
    assert!(credentials(&serde_json::json!({"AccessKeyId":"key"})).is_err());
}

#[tokio::test]
async fn imdsv2_discovery_is_cached_and_environment_takes_precedence() {
    use axum::{
        Router,
        routing::{get, put},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let router=Router::new()
        .route("/latest/api/token",put(move |headers:HeaderMap|{let count=count.clone();async move{
            assert_eq!(headers["x-aws-ec2-metadata-token-ttl-seconds"],"21600");
            count.fetch_add(1,Ordering::SeqCst);"metadata-session"
        }}))
        .route("/latest/meta-data/iam/security-credentials/",get(|headers:HeaderMap|async move{
            assert_eq!(headers["x-aws-ec2-metadata-token"],"metadata-session");"test-role"
        }))
        .route("/latest/meta-data/iam/security-credentials/test-role",get(|headers:HeaderMap|async move{
            assert_eq!(headers["x-aws-ec2-metadata-token"],"metadata-session");
            axum::Json(serde_json::json!({"Code":"Success","AccessKeyId":"metadata-key","SecretAccessKey":"synthetic","Token":"metadata-token","Expiration":"2030-01-01T00:00:00Z"}))
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    for environment in [false, true] {
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
        child.args([
            "--exact",
            "credentials::aws::tests::metadata_child",
            "--nocapture",
        ]);
        for (name, _) in std::env::vars().filter(|(k, _)| k.starts_with("AWS_")) {
            child.env_remove(name);
        }
        child
            .env("HOME", dir.path())
            .env("AWS_EC2_METADATA_SERVICE_ENDPOINT", &url)
            .env(
                "HEY_PROXY_AWS_TEST_CHILD",
                if environment {
                    "env-key"
                } else {
                    "metadata-key"
                },
            );
        if environment {
            child
                .env("AWS_ACCESS_KEY_ID", "env-key")
                .env("AWS_SECRET_ACCESS_KEY", "synthetic");
        }
        let result = child.output().await.unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    task.abort();
}

#[tokio::test]
async fn metadata_child() {
    let Ok(expected) = std::env::var("HEY_PROXY_AWS_TEST_CHILD") else {
        return;
    };
    let mut jobs = Vec::new();
    for _ in 0..20 {
        let expected = expected.clone();
        jobs.push(tokio::spawn(async move {
            let mut request = Client::new()
                .get("https://bedrock.us-east-1.amazonaws.com/inference-profiles")
                .build()
                .unwrap();
            sign(&mut request, "us-east-1", "bedrock").await.unwrap();
            assert!(
                request.headers()[AUTHORIZATION]
                    .to_str()
                    .unwrap()
                    .contains(&format!("Credential={expected}/"))
            );
        }));
    }
    for job in jobs {
        job.await.unwrap();
    }
}
