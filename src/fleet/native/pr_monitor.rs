//! Only the supervisor polls; never hold the issue store open over GitHub I/O.
use super::{Context, Result};
use crate::issues::Store;
use hey_gh::{Client, Config, Freshness};
use std::time::Duration;

const INTERVAL: Duration = Duration::from_secs(60);

fn selector(url: &str) -> Option<(String, u64)> {
    let tail = url.strip_prefix("https://github.com/")?;
    let parts: Vec<_> = tail.trim_end_matches('/').split('/').collect();
    if parts.len() != 4
        || parts[2] != "pull"
        || parts[..2].iter().any(|s| {
            s.is_empty()
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
    {
        return None;
    }
    let number = parts[3].parse::<u64>().ok().filter(|n| *n > 0)?;
    Some((format!("{}/{}", parts[0], parts[1]), number))
}

fn merged(data: &serde_json::Value, repository: &str, number: u64) -> bool {
    data["merged"] == true
        && data["state"] == "closed"
        && data["number"] == number
        && data["base"]["repo"]["full_name"]
            .as_str()
            .is_some_and(|r| r.eq_ignore_ascii_case(repository))
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
    let mut client = None;
    while !ctx.stopped() {
        if let Err(error) = poll(&ctx, &runtime, &mut client) {
            eprintln!("PR monitor: {error}");
        }
        ctx.wait(INTERVAL);
    }
}

fn poll(
    ctx: &Context,
    runtime: &tokio::runtime::Runtime,
    client: &mut Option<Client>,
) -> Result<()> {
    let mut store = Store::open(&ctx.path)?;
    let mut actor = ctx.actor()?;
    actor.id = "human:pr-monitor".into();
    store.close_merged_pull_requests(&actor)?;
    let urls = store.tracked_pull_requests()?;
    drop(store);
    if urls.is_empty() {
        return Ok(());
    }
    if client.is_none() {
        // Separate from the daemon-owned cache, with the same conditional reads,
        // request queue and GitHub backoff supplied by the embedded hey-gh SDK.
        let resolved = runtime.block_on(Client::from_gh(Config {
            cache_path: ctx.state.join("pr-monitor.sqlite"),
            request_timeout: Duration::from_secs(10),
            queue_timeout: Duration::from_secs(15),
            ..Config::default()
        }));
        match resolved {
            Ok(resolved) => *client = Some(resolved),
            Err(error) => {
                let mut store = Store::open(&ctx.path)?;
                for url in &urls {
                    store.record_pr_status(
                        url,
                        None,
                        crate::issues::worker::now(),
                        Some(&error.to_string()),
                    )?;
                }
                return Err(error.into());
            }
        }
    }
    for url in urls {
        if ctx.stopped() {
            return Ok(());
        }
        let Some((repository, number)) = selector(&url) else {
            Store::open(&ctx.path)?.record_pr_status(
                &url,
                None,
                crate::issues::worker::now(),
                Some("Unsupported PR URL"),
            )?;
            continue;
        };
        let result = runtime.block_on(tokio_read(client.as_ref().unwrap(), &repository, number));
        match result {
            Ok(response) => {
                let checked_at = i64::try_from(response.validated_at_ms)?;
                let data = response.data;
                let status = if merged(&data, &repository, number) {
                    Some("merged")
                } else if data["number"] == number
                    && data["base"]["repo"]["full_name"]
                        .as_str()
                        .is_some_and(|r| r.eq_ignore_ascii_case(&repository))
                {
                    match data["state"].as_str() {
                        Some("open") => Some("open"),
                        Some("closed") if data["merged"] == false => Some("closed"),
                        _ => None,
                    }
                } else {
                    None
                };
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
            }
            Err(error) => {
                Store::open(&ctx.path)?.record_pr_status(
                    &url,
                    None,
                    crate::issues::worker::now(),
                    Some(&error.to_string()),
                )?;
                eprintln!("PR monitor: {repository}#{number}: {error}");
                // Re-resolve gh login next cycle after authentication changes.
                if matches!(
                    error,
                    hey_gh::Error::GitHub { status: 401, .. } | hey_gh::Error::Auth(_)
                ) {
                    *client = None;
                    break;
                }
            }
        }
    }
    let count = Store::open(&ctx.path)?.close_merged_pull_requests(&actor)?;
    if count > 0 {
        eprintln!("PR monitor: closed {count} tasks after their fix PRs merged");
    }
    Ok(())
}

async fn tokio_read(
    client: &Client,
    repository: &str,
    number: u64,
) -> hey_gh::Result<hey_gh::Response> {
    tokio::time::timeout(
        Duration::from_secs(20),
        client.pull_request(repository, number, Freshness::MaxAge(INTERVAL)),
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
        drop(store);
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let origin = format!("http://{}/", server.server_addr());
        let serving = std::thread::spawn(move || {
            let request = server
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .expect("Metadata request");
            assert_eq!(request.url(), "/repos/o/r/pulls/1");
            request.respond(tiny_http::Response::from_string(json!({"number":1,"state":"closed","merged":true,"base":{"repo":{"full_name":"o/r"}}}).to_string()).with_header(tiny_http::Header::from_bytes("Content-Type","application/json").unwrap())).unwrap();
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut client = Some(runtime.block_on(async {
            Client::with_token(
                Config {
                    rest_url: origin.parse().unwrap(),
                    graphql_url: format!("{origin}graphql").parse().unwrap(),
                    cache_path: root.join("cache.sqlite"),
                    ..Config::default()
                },
                "synthetic".into(),
            )
            .unwrap()
        }));
        poll(&ctx, &runtime, &mut client).unwrap();
        serving.join().unwrap();
        // A subsequent cycle needs no server: a confirmed merge is terminal.
        poll(&ctx, &runtime, &mut client).unwrap();
        let mut store = Store::open(&ctx.path).unwrap();
        request.operation = serde_json::from_value(json!({"action":"view","number":1})).unwrap();
        let value = store.execute(&request).unwrap();
        assert_eq!(value["issue"]["state"], "closed");
        assert_eq!(value["issue"]["closed_by"], "human:pr-monitor");
        assert_eq!(value["issue"]["pull_requests"][0]["status"], "merged");
        drop(store);
        drop(client);
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
