use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy)]
enum Capture {
    Blocked,
    Failed,
    Missing,
    CachedOnly,
    BothBlocked,
    LiveFailed,
}

struct Gate {
    capture: Capture,
    cache_started: Notify,
    cached: AtomicUsize,
    live: AtomicUsize,
}

async fn gate(
    State(gate): State<Arc<Gate>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if request
        .uri()
        .query()
        .is_some_and(|q| q.contains("cached_only=true"))
    {
        gate.cached.fetch_add(1, Ordering::SeqCst);
        gate.cache_started.notify_one();
        match gate.capture {
            Capture::Blocked | Capture::BothBlocked => std::future::pending::<()>().await,
            Capture::Failed => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(json!({
                        "code":"storage", "error":"cache capture unavailable", "cause":"fixture"
                    })),
                )
                    .into_response();
            }
            Capture::Missing => {
                return (
                    StatusCode::NOT_FOUND,
                    axum::Json(json!({
                        "code":"cache_miss", "error":"no captured evidence"
                    })),
                )
                    .into_response();
            }
            Capture::CachedOnly | Capture::LiveFailed => {}
        }
    } else {
        gate.live.fetch_add(1, Ordering::SeqCst);
        // Force both requests to reach the daemon before returning success.
        // This tests overlap, independently of socket/CPU timing.
        gate.cache_started.notified().await;
        if matches!(gate.capture, Capture::BothBlocked) {
            std::future::pending::<()>().await;
        }
        if matches!(gate.capture, Capture::LiveFailed) {
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(
                    json!({"code":"transport", "error":"live failed", "cause":"live failed"}),
                ),
            )
                .into_response();
        }
    }
    next.run(request).await
}

async fn scenario(capture: Capture) {
    let h = Harness::new().await;
    let client = h.client();
    client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let api = hey_gh::api::Api::new(client.clone()).await.unwrap();
    for command in [
        vec!["pr", "view", "7", "-R", "acme/demo"],
        vec![
            "pr",
            "view",
            "7",
            "-R",
            "acme/demo",
            "--json",
            "number,headRefOid",
        ],
        vec!["pr", "checks", "7", "-R", "acme/demo"],
        vec!["pr", "acme/demo", "7"],
        vec!["ci", "acme/demo", "7"],
    ] {
        let gate = Arc::new(Gate {
            capture,
            cache_started: Notify::new(),
            cached: AtomicUsize::new(0),
            live: AtomicUsize::new(0),
        });
        let router = api.router().layer(axum::middleware::from_fn_with_state(
            gate.clone(),
            self::gate,
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let before = h.calls().len();
        let mut cli = tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"));
        cli.kill_on_drop(true)
            .args(["--server", &base])
            .args(&command)
            .args(["--timeout", "1"]);
        if matches!(capture, Capture::CachedOnly) {
            cli.arg("--cached-only");
        }
        let output = tokio::time::timeout(Duration::from_secs(2), cli.output())
            .await
            .expect("caller exceeded its total deadline")
            .unwrap();
        server.abort();
        if matches!(capture, Capture::LiveFailed) {
            assert!(!output.status.success());
            assert!(
                output.stdout.is_empty(),
                "Do not substitute cached success for a failed live read"
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains("live failed"));
        } else {
            let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!("{error}: {}", String::from_utf8_lossy(&output.stderr))
            });
            if matches!(capture, Capture::BothBlocked) {
                assert!(!output.status.success());
                assert_eq!(value["code"], "deadline");
                assert_eq!(value["available"], false);
                assert_eq!(value["complete"], false);
                assert_eq!(value["validations"], json!([]));
                assert_eq!(value["cursor"], Value::Null);
            } else {
                assert!(
                    output.status.success(),
                    "Live result must not wait for fallback capture: command={command:?} {value}"
                );
                assert_eq!(value["complete"], true);
                assert!(value["deadlineExceeded"].is_null());
            }
        }
        assert_eq!(gate.cached.load(Ordering::SeqCst), 1);
        assert_eq!(
            gate.live.load(Ordering::SeqCst),
            usize::from(!matches!(capture, Capture::CachedOnly))
        );
        assert_eq!(
            h.calls().len(),
            before,
            "Fresh cached sources need no extra GitHub requests"
        );
    }
    api.stop().await;
}

#[tokio::test]
async fn blocked_fallback_capture_does_not_delay_a_live_success() {
    scenario(Capture::Blocked).await;
}

#[tokio::test]
async fn failed_fallback_capture_does_not_discard_a_live_success() {
    scenario(Capture::Failed).await;
}

#[tokio::test]
async fn missing_fallback_capture_still_allows_live_reads() {
    scenario(Capture::Missing).await;
}

#[tokio::test]
async fn cached_only_deadline_never_starts_a_live_read() {
    scenario(Capture::CachedOnly).await;
}

#[tokio::test]
async fn blocked_cache_and_live_reads_still_obey_the_total_deadline() {
    scenario(Capture::BothBlocked).await;
}

#[tokio::test]
async fn cached_success_never_masks_a_live_failure() {
    scenario(Capture::LiveFailed).await;
}
