//! Only the supervisor polls; never hold the issue store open over GitHub I/O.
use super::{Context, Result};
use crate::issues::Store;
use hey_gh::{ApiClient, Freshness};
use std::time::Duration;
mod schedule;
mod watches;

const INTERVAL: Duration = Duration::from_secs(30);

// LaunchAgents do not inherit the interactive shell's Homebrew/user PATH.
fn gh_program(home: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .chain([
            home.join(".local/bin"),
            home.join(".cargo/bin"),
            "/opt/homebrew/bin".into(),
            "/usr/local/bin".into(),
        ])
        .map(|directory| directory.join("gh"))
        .find(|path| {
            path.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .unwrap_or_else(|| "gh".into())
}

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
    let mut daemon: Option<std::process::Child> = None;
    while !ctx.stopped() {
        // Start the same shared daemon used by the CLI if it is absent. Never
        // fall back to a private GitHub queue that bypasses its quota backoff.
        if let Err(error) = ensure_daemon(&ctx, &mut daemon) {
            eprintln!("PR monitor: cannot start hey-gh serve: {error}");
        }
        if let Err(error) = watches::poll(&ctx, &runtime, &client) {
            eprintln!("GitHub watcher: {error}");
        }
        if let Err(error) = poll(&ctx, &runtime, &client) {
            eprintln!("PR monitor: {error}");
        }
        ctx.wait(INTERVAL);
    }
}

fn ensure_daemon(ctx: &Context, child: &mut Option<std::process::Child>) -> Result<()> {
    use std::process::{Command, Stdio};
    if let Some(process) = child.as_mut()
        && process.try_wait()?.is_some()
    {
        *child = None;
    }
    if std::net::TcpStream::connect_timeout(&"127.0.0.1:8787".parse()?, Duration::from_millis(200))
        .is_ok()
        || child.is_some()
    {
        return Ok(());
    }
    let binary = ctx.binary.with_file_name("hey-gh");
    // A LaunchAgent needs the resolved gh directory in the daemon's PATH too.
    let gh = gh_program(&ctx.home);
    let path = std::env::join_paths(gh.parent().into_iter().map(|p| p.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))?;
    *child = Some(
        Command::new(binary)
            .arg("serve")
            .env("PATH", path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    Ok(())
}

fn poll(ctx: &Context, runtime: &tokio::runtime::Runtime, client: &ApiClient) -> Result<()> {
    let mut store = Store::open(&ctx.path)?;
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
    let urls = schedule.due(&prs, crate::issues::worker::now());
    if urls.is_empty() {
        return Ok(());
    }
    let started = std::time::Instant::now();
    for url in urls {
        if ctx.stopped() || started.elapsed() >= Duration::from_secs(40) {
            break;
        }
        let Some((repository, number)) = selector(&url) else {
            schedule.failure(
                &url,
                crate::issues::worker::now(),
                &hey_gh::Error::Invalid("Unsupported PR URL".into()),
            );
            ctx.atomic_json(&path, &serde_json::to_value(&schedule)?)?;
            Store::open(&ctx.path)?.record_pr_status(
                &url,
                None,
                crate::issues::worker::now(),
                Some("Unsupported PR URL"),
            )?;
            continue;
        };
        let result = runtime.block_on(tokio_read(client, &repository, number));
        match result {
            Ok(response) => {
                let checked_at = i64::try_from(response.validated_at_ms)?;
                let data = response.data;
                let status = pr_status(&data, &repository, number);
                Store::open(&ctx.path)?.record_pr_status(
                    &url,
                    status,
                    checked_at,
                    if status.is_none() {
                        Some("Incomplete PR metadata")
                    } else {
                        None
                    },
                )?;
                if let Some(status) = status {
                    schedule.success(
                        &url,
                        crate::issues::worker::now(),
                        checked_at,
                        status == "closed",
                    );
                } else {
                    schedule.failure(
                        &url,
                        crate::issues::worker::now(),
                        &hey_gh::Error::Invalid("Incomplete PR metadata".into()),
                    );
                }
                ctx.atomic_json(&path, &serde_json::to_value(&schedule)?)?;
            }
            Err(error) => {
                schedule.failure(&url, crate::issues::worker::now(), &error);
                ctx.atomic_json(&path, &serde_json::to_value(&schedule)?)?;
                Store::open(&ctx.path)?.record_pr_status(
                    &url,
                    None,
                    crate::issues::worker::now(),
                    Some(&error.to_string()),
                )?;
                eprintln!("PR monitor: {repository}#{number}: {error}");
                // Global failures stop the batch. Repo-specific denials back
                // off that PR so they cannot starve unrelated repositories.
                if matches!(
                    error,
                    hey_gh::Error::GitHub { status: 401, .. }
                        | hey_gh::Error::Auth(_)
                        | hey_gh::Error::RateLimited { .. }
                        | hey_gh::Error::Transport(_)
                        | hey_gh::Error::LocalAuth(_)
                        | hey_gh::Error::QueueFull
                        | hey_gh::Error::Deadline
                ) {
                    break;
                }
            }
        }
    }
    let mut store = Store::open(&ctx.path)?;
    let count = store.close_merged_pull_requests(&actor)?;
    store.reconcile_github_assignments(&actor)?;
    if count > 0 {
        eprintln!("PR monitor: closed {count} tasks after their fix PRs merged");
    }
    Ok(())
}

async fn tokio_read(
    client: &ApiClient,
    repository: &str,
    number: u64,
) -> hey_gh::Result<hey_gh::Response> {
    tokio::time::timeout(
        Duration::from_secs(20),
        client.pull_request(
            repository,
            number,
            Freshness::MaxAge(Duration::from_secs(300)),
        ),
    )
    .await
    .map_err(|_| hey_gh::Error::Deadline)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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
        polling_scenario(false);
    }

    #[test]
    fn rate_limit_stops_batch_and_persisted_cooldown_stops_retries() {
        polling_scenario(true);
    }

    fn polling_scenario(rate_limited: bool) {
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
        if rate_limited {
            request.operation = serde_json::from_value(json!({"action":"add_pull_request","number":1,"url":"https://github.com/o/r/pull/2","purpose":"fix"})).unwrap();
            store.execute(&request).unwrap();
        }
        drop(store);
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let origin = format!("http://{}/", server.server_addr());
        let serving = std::thread::spawn(move || {
            let request = server
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .expect("Metadata request");
            assert_eq!(request.url(), "/v1/prs/o/r/1/metadata?max_age_seconds=300");
            let response = if rate_limited {
                tiny_http::Response::from_string(
                    json!({"code":"rate_limited","error":"cooldown"}).to_string(),
                )
                .with_status_code(503)
                .with_header(tiny_http::Header::from_bytes("Retry-After", "600").unwrap())
            } else {
                tiny_http::Response::from_string(json!({"data":{"number":1,"state":"closed","merged":true,"base":{"repo":{"full_name":"o/r"}}},"validated_at_ms":123,"fetched_at_ms":123,"source":"cache"}).to_string())
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
        } else {
            assert!(value["issue"]["pull_requests"][1]["error"].is_null());
            assert!(value["issue"]["pull_requests"][0]["checked_at"].is_null());
        }
        drop(store);
        drop(client);
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
