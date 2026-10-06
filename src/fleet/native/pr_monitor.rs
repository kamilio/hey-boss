//! Only the supervisor polls; never hold the issue store open over GitHub I/O.
use super::{Context, Result};
use crate::issues::Store;
use hey_gh::{ApiClient, Freshness};
use std::time::Duration;
mod cached_merges;
#[cfg(test)]
mod lifecycle_tests;
mod lifecycles;
#[cfg(test)]
mod read_deadlines;
mod schedule;
mod watches;

const INTERVAL: Duration = Duration::from_secs(30);

fn selector(url: &str) -> Option<(String, u64)> {
    hey_gh::watcher::pull_request_selector(url)
}

fn merged(data: &serde_json::Value, repository: &str, number: u64) -> bool {
    data["merged"] == true
        && data["state"] == "closed"
        && data["number"] == number
        && data["base"]["repo"]["full_name"]
            .as_str()
            .is_some_and(|r| r.eq_ignore_ascii_case(repository))
}

fn pr_status(data: &serde_json::Value, repository: &str, number: u64) -> Option<&'static str> {
    if merged(data, repository, number) {
        return Some("merged");
    }
    if data["number"] != number
        || !data["base"]["repo"]["full_name"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(repository))
    {
        return None;
    }
    match data["state"].as_str() {
        Some("open") => Some("open"),
        Some("closed") if data["merged"] == false => Some("closed"),
        _ => None,
    }
}

pub(super) fn run(ctx: Context) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("PR monitor: {error}");
            return;
        }
    };
    let client = match ApiClient::new("http://127.0.0.1:8787/".parse().unwrap()) {
        Ok(client) => client.background(),
        Err(error) => {
            eprintln!("PR monitor: {error}");
            return;
        }
    };
    let mut cached_after = None;
    while !ctx.stopped() {
        let started = std::time::Instant::now();
        // Start the same shared daemon used by the CLI if it is absent. Never
        // fall back to a private GitHub queue that bypasses its quota backoff.
        if let Err(error) = ensure_daemon(&ctx, "127.0.0.1:8787".parse().unwrap()) {
            eprintln!("PR monitor: cannot start hey-gh service: {error}");
        }
        poll_cycle(&ctx, &runtime, &client, &mut cached_after);
        ctx.wait(INTERVAL.saturating_sub(started.elapsed()));
    }
}

fn poll_cycle(
    ctx: &Context,
    runtime: &tokio::runtime::Runtime,
    client: &ApiClient,
    cached_after: &mut Option<String>,
) {
    // Both use the shared hey-gh queue, but waiting for ordinary metadata must
    // not add another serial batch before the next required-check observation.
    // Active issue watches run on the foreground priority lane so background
    // account/metadata sweeps never starve tracked PRs.
    let watch_client = client.clone().foreground();
    let watched = runtime.block_on(async {
        let finished = tokio::sync::Notify::new();
        let (watched, ()) = tokio::join!(
            async {
                let result = watches::poll_once(ctx, &watch_client).await;
                finished.notify_one();
                result
            },
            async {
                loop {
                    let started = tokio::time::Instant::now();
                    let deadline = started + Duration::from_secs(40);
                    let metadata_client = client.clone().with_read_deadline(deadline);
                    match tokio::time::timeout_at(deadline, async {
                        // Lifecycle already learned by the shared daemon must
                        // not wait behind this watcher's expensive CI queue.
                        match tokio::time::timeout(
                            Duration::from_secs(10),
                            cached_merges::poll(ctx, &metadata_client, cached_after),
                        )
                        .await
                        {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => eprintln!("PR monitor: cached merges: {error}"),
                            Err(_) => eprintln!("PR monitor: cached merge batch deadline reached"),
                        }
                        poll_once(ctx, &metadata_client).await
                    })
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => eprintln!("PR monitor: {error}"),
                        Err(_) => eprintln!("PR monitor: metadata batch deadline reached"),
                    }
                    if ctx.stopped() {
                        break;
                    }
                    // Keep draining the bounded metadata queue while a slow
                    // required-check batch runs. Finish the current read before
                    // returning; never overlap metadata batches or reset backoff.
                    tokio::select! {
                        biased;
                        _ = finished.notified() => break,
                        _ = tokio::time::sleep_until(started + INTERVAL) => {}
                    }
                }
            }
        );
        watched
    });
    if let Err(error) = watched {
        eprintln!("GitHub PR watcher: {error}");
    }
}

fn ensure_daemon(ctx: &Context, address: std::net::SocketAddr) -> Result<()> {
    use std::process::{Command, Stdio};
    if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
        return Ok(());
    }
    // The user service owns the daemon across supervisor exits and upgrades.
    // It also resolves the gh PATH for launchd's minimal environment. Spawning
    // serve here leaves an orphan holding the port after a supervisor reload.
    let mut command = Command::new(ctx.binary.with_file_name("hey-gh"));
    command.args(["service", "start"]).stdin(Stdio::null());
    let output = super::supervisor::output_timeout(command, Duration::from_secs(60))?;
    if !output.status.success() {
        return Err(format!(
            "hey-gh service start failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
fn poll(ctx: &Context, runtime: &tokio::runtime::Runtime, client: &ApiClient) -> Result<()> {
    runtime.block_on(poll_once(ctx, client))
}

async fn poll_once(ctx: &Context, client: &ApiClient) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let viewer_client = client.clone().with_read_deadline(deadline);
    let viewer = tokio::time::timeout_at(
        deadline,
        viewer_client.viewer(Freshness::MaxAge(Duration::from_secs(3600))),
    )
    .await;
    let mut store = Store::open(&ctx.path)?;
    if let Ok(Ok(viewer)) = viewer
        && let Some(id) = viewer.data["id"].as_i64()
    {
        store.record_github_user(id)?;
    }
    let mut actor = ctx.actor()?;
    actor.id = "human:pr-monitor".into();
    store.close_merged_pull_requests(&actor)?;
    store.reconcile_github_assignments(&actor)?;
    let prs = store.tracked_pull_requests()?;
    drop(store);
    let path = ctx.state.join("pr-monitor-schedule.json");
    let mut schedule: schedule::Schedule = serde_json::from_value(
        ctx.read_json(&path, serde_json::json!({"cooldown_until":0,"entries":{}}))?,
    )?;
    lifecycles::poll(ctx, client, &prs, &mut schedule, &path).await?;
    let mut store = Store::open(&ctx.path)?;
    let count = store.close_merged_pull_requests(&actor)?;
    store.reconcile_github_assignments(&actor)?;
    if count > 0 {
        eprintln!("PR monitor: closed {count} tasks after their fix PRs merged");
    }
    Ok(())
}

async fn repair_identity(
    client: &ApiClient,
    repository: &str,
    number: u64,
) -> hey_gh::Result<hey_gh::Response> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let client = client.clone().with_read_deadline(deadline);
    tokio::time::timeout_at(deadline, async {
        // A different node was just observed. Cached REST metadata may still
        // describe the retired identity, including an irreversible merge.
        client
            .pull_request(repository, number, Freshness::Revalidate)
            .await
    })
    .await
    .map_err(|_| hey_gh::Error::Deadline)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(super) fn read_budget(request: &tiny_http::Request, maximum: u64) -> u64 {
        let budget = request
            .headers()
            .iter()
            .find(|header| header.field.equiv("x-hey-gh-read-timeout-ms"))
            .expect("Polling deadlines must reach the daemon, not just stop the local wait")
            .value
            .as_str()
            .parse::<u64>()
            .unwrap();
        assert!(
            (1..=maximum).contains(&budget),
            "Unexpected read budget: {budget}"
        );
        budget
    }

    #[test]
    fn daemon_startup_reports_manager_failure_and_retries_without_retaining_a_child() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let (root, mut ctx, store) = super::super::context::tests::test_context();
        drop(store);
        ctx.binary = root.join("hey-boss");
        let binary = root.join("hey-gh");
        let address = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        fs::write(
            &binary,
            "#!/bin/sh\necho 'user service manager unavailable' >&2\nexit 78\n",
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let error = ensure_daemon(&ctx, address).unwrap_err().to_string();
        assert!(
            error.contains("user service manager unavailable"),
            "{error}"
        );
        fs::write(&binary, "#!/bin/sh\nif test \"$*\" != 'service start'; then exit 79; fi\necho started > \"$0.started\"\n").unwrap();
        ensure_daemon(&ctx, address).unwrap();
        assert_eq!(
            fs::read_to_string(binary.with_extension("started")).unwrap(),
            "started\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn daemon_startup_preserves_an_existing_listener_without_running_a_command() {
        let (root, mut ctx, store) = super::super::context::tests::test_context();
        drop(store);
        ctx.binary = root.join("missing/hey-boss");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        ensure_daemon(&ctx, address).unwrap();
        assert!(std::net::TcpStream::connect(address).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn only_explicit_merge_evidence_for_the_requested_pr_closes_tasks() {
        let mut data =
            json!({"state":"closed","merged":false,"number":1,"base":{"repo":{"full_name":"o/r"}}});
        assert!(!merged(&data, "o/r", 1));
        data["merged"] = json!(true);
        assert!(merged(&data, "o/r", 1));
        assert!(!merged(&data, "o/r", 2));
        assert!(!merged(&data, "another/repo", 1));
        assert!(!merged(
            &json!({"state":"MERGED","complete":false}),
            "o/r",
            1
        ));
    }
    #[test]
    fn only_github_pull_urls_are_polled() {
        assert_eq!(
            selector("https://github.com/o/r/pull/42"),
            Some(("o/r".into(), 42))
        );
        for url in [
            "http://github.com/o/r/pull/1",
            "https://evil.test/o/r/pull/1",
            "https://github.com/o/r/issues/1",
            "https://github.com/o/r/pull/0",
            "https://github.com/o/r/pull/1?x=2",
        ] {
            assert_eq!(selector(url), None);
        }
    }
    #[test]
    fn polling_fetches_only_metadata_persists_status_and_closes_the_task() {
        polling_scenario(false, false);
    }

    #[test]
    fn rate_limit_stops_batch_and_persisted_cooldown_stops_retries() {
        polling_scenario(true, false);
    }

    #[test]
    fn merged_authorship_backfill_uses_lifecycle_batch_when_needed() {
        polling_scenario(false, true);
    }

    fn polling_scenario(rate_limited: bool, backfill: bool) {
        use crate::issues::{Project, Request};
        use std::sync::{Arc, atomic::AtomicBool};
        let root = std::env::temp_dir().join(format!(
            "hb-poll-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let ctx = Context {
            home: root.clone(),
            state: root.clone(),
            desired: root.join("fleet.json"),
            binary: root.join("hey-boss"),
            path: root.join("issues.db"),
            node: "test".into(),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let mut store = Store::open(&ctx.path).unwrap();
        let mut request = Request {
            version: 1,
            project: Project {
                id: "named:test".into(),
                name: "test".into(),
            },
            project_override: None,
            actor: Some(ctx.actor().unwrap()),
            operation: serde_json::from_value(
                json!({"action":"create","title":"Fix","body":"","labels":[]}),
            )
            .unwrap(),
            request_id: None,
        };
        store.execute(&request).unwrap();
        request.operation=serde_json::from_value(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/1","purpose":"fix"})).unwrap();
        store.execute(&request).unwrap();
        if backfill {
            store
                .record_pr_status("https://github.com/o/r/pull/1", Some("merged"), 1, None)
                .unwrap();
        }
        if rate_limited {
            request.operation = serde_json::from_value(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/2","purpose":"fix"})).unwrap();
            store.execute(&request).unwrap();
        }
        drop(store);
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let origin = format!("http://{}/", server.server_addr());
        let serving = std::thread::spawn(move || {
            let viewer = server
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .expect("Viewer request");
            assert_eq!(viewer.url(), "/v1/viewer?max_age_seconds=3600");
            read_budget(&viewer, 5_000);
            viewer.respond(tiny_http::Response::from_string(json!({"data":{"id":42,"login":"me"},"validated_at_ms":123,"fetched_at_ms":123,"source":"cache"}).to_string()).with_header(tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap())).unwrap();
            let request = server
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .expect("Metadata request");
            read_budget(&request, 20_000);
            assert_eq!(
                request.url(),
                if backfill {
                    "/v1/repos/o/r/pr-lifecycles?max_age_seconds=30&numbers=1"
                } else if rate_limited {
                    "/v1/repos/o/r/pr-lifecycles?max_age_seconds=30&numbers=1%2C2"
                } else {
                    "/v1/repos/o/r/pr-lifecycles?max_age_seconds=30&numbers=1"
                }
            );
            let response = if rate_limited {
                tiny_http::Response::from_string(
                    json!({"code":"rate_limited","error":"cooldown"}).to_string(),
                )
                .with_status_code(503)
                .with_header(tiny_http::Header::from_bytes("Retry-After", "600").unwrap())
            } else {
                tiny_http::Response::from_string(lifecycle_tests::from_metadata(&json!({"data":{"number":1,"state":"closed","merged":true,"title":"Merged PR","merged_at":"2026-10-01T00:00:00Z","user":{"id":42},"base":{"repo":{"full_name":"o/r"}}},"validated_at_ms":crate::issues::worker::now(),"fetched_at_ms":crate::issues::worker::now(),"source":"network"}), &[1]).to_string())
            };
            request
                .respond(response.with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                ))
                .unwrap();
            assert!(
                server
                    .recv_timeout(Duration::from_millis(200))
                    .unwrap()
                    .is_none()
            );
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = ApiClient::new(origin.parse().unwrap()).unwrap();
        poll(&ctx, &runtime, &client).unwrap();
        serving.join().unwrap();
        // A subsequent cycle needs no server: a confirmed merge is terminal.
        poll(&ctx, &runtime, &client).unwrap();
        let mut store = Store::open(&ctx.path).unwrap();
        request.operation = serde_json::from_value(json!({"action":"view","number":1})).unwrap();
        let value = store.execute(&request).unwrap();
        assert_eq!(
            value["issue"]["state"],
            if rate_limited { "open" } else { "closed" }
        );
        if !rate_limited {
            assert_eq!(value["issue"]["closed_by"], "human:pr-monitor");
            assert_eq!(value["issue"]["pull_requests"][0]["status"], "merged");
            request.operation = serde_json::from_value(
                json!({"action":"merged_pull_requests","limit":10,"offset":0}),
            )
            .unwrap();
            let history = store.execute(&request).unwrap();
            assert_eq!(history["pull_requests"].as_array().unwrap().len(), 1);
            assert_eq!(history["authorship_pending"], false);
        } else {
            assert!(value["issue"]["pull_requests"][1]["error"].is_string());
            assert!(value["issue"]["pull_requests"][0]["checked_at"].is_null());
        }
        drop(store);
        drop(client);
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
