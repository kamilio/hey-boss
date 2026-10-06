use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use hey_gh::{
    ApiClient, Error,
    shared_read::{Identity, MAX_RESPONSE_BYTES, Read},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

type MockState = (
    Arc<Mutex<Data>>,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Notify>,
);

#[derive(Default)]
struct Data {
    identity_reads: usize,
    reads: Vec<(String, u64)>,
    mode: &'static str,
}
struct Fixture {
    client: ApiClient,
    data: Arc<Mutex<Data>>,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn identity() -> Identity {
    Identity {
        hostname: "github.com".into(),
        user_id: 42,
        instance: "a".repeat(32),
    }
}
fn read() -> Read {
    Read {
        identity: identity(),
        path: "/v1/prs/acme/demo/7/metadata".into(),
        query: Some(
            "cached_only=true&background=true&fields=number%2Cstate&cursor=opaque%2Bvalue".into(),
        ),
        timeout_ms: 2000,
    }
}
async fn handler(
    State((data, started, release)): State<MockState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let mode = {
        let mut data = data.lock().unwrap();
        if uri.path() == "/v1/identity" {
            data.identity_reads += 1;
            if data.mode == "large_identity" {
                return Json(
                    json!({"hostname":"x".repeat(4096),"user_id":42,"instance":"a".repeat(32)}),
                )
                .into_response();
            }
            let mut identity = identity();
            if data.mode == "restart" && data.identity_reads > 1 {
                identity.instance = "b".repeat(32);
            }
            return Json(identity).into_response();
        }
        data.reads.push((
            uri.to_string(),
            headers["x-hey-gh-read-timeout-ms"]
                .to_str()
                .unwrap()
                .parse()
                .unwrap(),
        ));
        data.mode
    };
    match mode {
        "hold" => {
            started.notify_one();
            release.notified().await;
        }
        "denied" => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"code":"graphql_access_denied","error":"synthetic denial"})),
            )
                .into_response();
        }
        "rate" => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", "17")],
                Json(json!({"code":"rate_limited","error":"synthetic limit"})),
            )
                .into_response();
        }
        "large" => return Json(json!({"data":"x".repeat(MAX_RESPONSE_BYTES)})).into_response(),
        _ => {}
    }
    Json(json!({"data":{"number":7},"validated_at_ms":123,"fetched_at_ms":100,"source":"cache"}))
        .into_response()
}
impl Fixture {
    async fn new(mode: &'static str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = ApiClient::new(
            format!("http://{}/", listener.local_addr().unwrap())
                .parse()
                .unwrap(),
        )
        .unwrap();
        let data = Arc::new(Mutex::new(Data {
            mode,
            ..Default::default()
        }));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let router = Router::new().fallback(handler).with_state((
            data.clone(),
            started.clone(),
            release.clone(),
        ));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            client,
            data,
            started,
            release,
            task,
        }
    }
}

#[tokio::test]
async fn shared_backend_preserves_query_budget_and_validation_clocks() {
    let f = Fixture::new("").await;
    let request = read();
    let value: Value = f
        .client
        .shared_read(&request)
        .await
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(value["validated_at_ms"], 123);
    assert_eq!(value["fetched_at_ms"], 100);
    let data = f.data.lock().unwrap();
    assert_eq!(data.identity_reads, 2);
    assert_eq!(data.reads.len(), 1);
    assert_eq!(
        data.reads[0].0,
        format!("{}?{}", request.path, request.query.unwrap())
    );
    assert!((1..=request.timeout_ms).contains(&data.reads[0].1));
}

#[tokio::test]
async fn shared_backend_rejects_different_user_host_and_daemon_before_data_read() {
    for field in ["user", "host", "instance"] {
        let f = Fixture::new("").await;
        let mut request = read();
        match field {
            "user" => request.identity.user_id += 1,
            "host" => request.identity.hostname = "github.example.com".into(),
            _ => request.identity.instance = "b".repeat(32),
        }
        assert!(matches!(
            f.client.shared_read(&request).await,
            Err(Error::Invalid(_))
        ));
        assert!(f.data.lock().unwrap().reads.is_empty());
    }
}

#[tokio::test]
async fn shared_backend_discards_body_if_identity_changes_during_read() {
    let f = Fixture::new("restart").await;
    assert!(matches!(
        f.client.shared_read(&read()).await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(f.data.lock().unwrap().reads.len(), 1);
}

#[tokio::test]
async fn shared_backend_preserves_upstream_errors_and_retry_after() {
    let f = Fixture::new("denied").await;
    let reply = f.client.shared_read(&read()).await.unwrap();
    assert_eq!(reply.status, 403);
    assert!(matches!(
        reply.decode::<Value>(),
        Err(Error::GraphQL {
            access_denied: true,
            ..
        })
    ));
    f.data.lock().unwrap().mode = "rate";
    let reply = f.client.shared_read(&read()).await.unwrap();
    assert_eq!(reply.retry_after_seconds, Some(17));
    assert!(matches!(
        reply.decode::<Value>(),
        Err(Error::RateLimited {
            retry_after_seconds: 17
        })
    ));
    assert_eq!(f.data.lock().unwrap().reads.len(), 2, "no hidden retry");
}

#[tokio::test]
async fn shared_backend_bounds_response_size() {
    let f = Fixture::new("large").await;
    assert!(
        matches!(f.client.shared_read(&read()).await, Err(Error::Invalid(message)) if message.contains("size limit"))
    );
}

#[tokio::test]
async fn shared_backend_bounds_identity_before_deserializing_it() {
    let f = Fixture::new("large_identity").await;
    assert!(
        matches!(f.client.shared_read(&read()).await, Err(Error::Invalid(message)) if message.contains("size limit"))
    );
    assert!(f.data.lock().unwrap().reads.is_empty());
}

#[tokio::test]
async fn shared_backend_keeps_caller_deadline_and_independent_cancellation() {
    let f = Fixture::new("hold").await;
    let client = f
        .client
        .clone()
        .with_read_deadline(tokio::time::Instant::now() + Duration::from_millis(100));
    let first = tokio::spawn(async move { client.shared_read(&read()).await });
    f.started.notified().await;
    assert!(matches!(first.await.unwrap(), Err(Error::Deadline)));
    let client = f.client.clone();
    let second = tokio::spawn(async move { client.shared_read(&read()).await });
    f.started.notified().await;
    second.abort();
    assert!(second.await.unwrap_err().is_cancelled());
    f.release.notify_waiters();
    f.data.lock().unwrap().mode = "";
    assert!(f.client.shared_read(&read()).await.is_ok());
}
