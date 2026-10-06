use super::*;
use crate::{
    polling::{Coverage, DemandScope},
    shared_read::Identity,
};

impl Api {
    async fn polling_coverage(&self) -> Result<Coverage> {
        // Only an actual local poller can offer ownership. Never form chains.
        if self.0.client.polling().active() {
            return Err(Error::CacheMiss);
        }
        let watches = self.0.client.watches().await?;
        let monitors = self.0.monitors.lock().await;
        let watch = watches
            .iter()
            .find(|w| w.kind == WatchKind::Account)
            .ok_or(Error::CacheMiss)?;
        let monitor = monitors
            .get(&watch.id)
            .filter(|m| !m.task.is_finished())
            .ok_or(Error::CacheMiss)?;
        let status = monitor.state.lock().await;
        let age = self.0.client.report_timeout().as_millis() as u64
            + watch.interval_seconds.saturating_mul(2000);
        let now = now_ms();
        let recent = |at: Option<u64>| {
            at.and_then(|at| now.checked_sub(at))
                .is_some_and(|elapsed| elapsed <= age)
        };
        if !recent(status.discovery_last_poll_at_ms) {
            return Err(Error::CacheMiss);
        }
        let modes = [
            recent(status.ci_last_poll_at_ms),
            recent(status.last_poll_at_ms),
            recent(status.policy_last_poll_at_ms),
        ];
        if !modes.iter().any(|mode| *mode) {
            return Err(Error::CacheMiss);
        }
        let interval_seconds = watch.interval_seconds;
        drop(status);
        drop(monitors);
        let identity = shared_identity(State(self.clone()))
            .await
            .map_err(|error| error.0)?
            .0;
        let roster = self
            .0
            .client
            .stored_snapshot(&self.0.client.roster_resource())
            .await?
            .ok_or(Error::CacheMiss)?;
        let nodes = roster
            .as_array()
            .filter(|nodes| nodes.len() <= crate::polling::MAX_ROWS)
            .ok_or(Error::CacheMiss)?;
        let rows = nodes.iter().filter_map(crate::polling::row).collect();
        Ok(Coverage {
            identity,
            interval_seconds,
            modes,
            rows,
        })
    }

    /// The standard daemon may delegate account hydration over its authenticated
    /// companion connection. Embedded and alternate-port APIs remain local.
    pub fn start_shared_polling(&self, address: std::net::SocketAddr) -> Result<()> {
        if !address.ip().is_loopback() || address.port() != 8787 {
            return Ok(());
        }
        let mut task = self.0.polling_task.lock().unwrap();
        if task.is_some() {
            return Ok(());
        }
        let sdk = crate::ApiClient::new(
            format!("http://{address}/")
                .parse()
                .map_err(|_| Error::Invalid("invalid daemon address".into()))?,
        )?;
        let client = self.0.client.clone();
        let instance = self.0.instance.clone();
        *task = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(15));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let started = tokio::time::Instant::now();
                let result = tokio::time::timeout(Duration::from_secs(2), async {
                    let watches = client.watches().await?;
                    // Existing explicit consumers keep their own local sources.
                    if watches.iter().any(|w| w.kind != WatchKind::Account) {
                        return Err(Error::CacheMiss);
                    }
                    let cadence = watches
                        .iter()
                        .find(|w| w.kind == WatchKind::Account)
                        .ok_or(Error::CacheMiss)?
                        .interval_seconds;
                    let viewer = client.get("user", Freshness::CachedOnly).await?;
                    let local = Identity {
                        hostname: client.hostname().into(),
                        user_id: viewer.data["id"].as_u64().unwrap_or(0),
                        instance: instance.clone(),
                    };
                    let proof = sdk
                        .clone()
                        .with_read_deadline(started + Duration::from_secs(2))
                        .relay_polling_coverage(&local)
                        .await?
                        .ok_or(Error::CacheMiss)?;
                    Ok::<_, Error>((local, cadence, proof))
                })
                .await;
                match result {
                    Ok(Ok((local, cadence, proof))) => {
                        client.polling().renew(&local, cadence, started, proof);
                    }
                    Ok(Err(error)) => client.polling().probe_failed(error.diagnostic_code()),
                    Err(_) => client.polling().probe_failed("deadline"),
                }
            }
        }));
        Ok(())
    }
}

pub(super) async fn coverage(State(api): State<Api>) -> ApiResult<Json<Coverage>> {
    Ok(Json(api.polling_coverage().await?))
}

fn demand_scope(request: &axum::extract::Request) -> DemandScope {
    if request.method() != axum::http::Method::GET {
        return DemandScope::Global;
    }
    let path = request.uri().path();
    let repository = |value: String| {
        (value.len() <= 512 && crate::client::validate_repository(&value).is_ok())
            .then(|| value.to_ascii_lowercase())
    };
    if path == "/v1/pr-status" {
        let mut selectors =
            url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                .filter(|(key, _)| key == "repository");
        if let Some((_, value)) = selectors.next()
            && selectors.next().is_none()
            && let Some(repo) = repository(value.into_owned())
        {
            return DemandScope::Repository(repo);
        }
        return DemandScope::Global;
    }
    let parts: Vec<_> = path.split('/').collect();
    match parts.as_slice() {
        ["", "v1", kind @ ("prs" | "repos"), owner, repo, tail @ ..] => {
            let Some(repo) = repository(format!("{owner}/{repo}")) else {
                return DemandScope::Global;
            };
            match (*kind, tail) {
                (_, []) | ("repos", ["prs" | "pr-lifecycles"]) => DemandScope::Repository(repo),
                ("prs", [number] | [number, "ci" | "metadata" | "required-checks"]) => number
                    .parse::<u64>()
                    .ok()
                    .filter(|n| *n > 0)
                    .map_or(DemandScope::Global, |n| {
                        DemandScope::PullRequest(format!("{repo}/{n}"))
                    }),
                _ => DemandScope::Global,
            }
        }
        _ => DemandScope::Global,
    }
}

pub(super) async fn local_demand(
    State(api): State<Api>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    // Even cache-only/direct feed reads need local monitoring. Handshakes and
    // health inspection alone do not consume local PR evidence.
    if !matches!(
        request.uri().path(),
        "/v1/identity" | "/v1/status" | "/v1/polling-coverage" | "/v1/watches"
    ) || request.method() != axum::http::Method::GET
    {
        // Only fixed route categories enter diagnostics, never selectors,
        // cursors, credentials, or arbitrary request text.
        let path = request.uri().path();
        let source = match path {
            "/v1/snapshot" | "/v1/changes" => "source_feed",
            "/v1/pr-status" => "pr_status",
            "/v1/viewer" => "viewer",
            "/v1/watches" => "watch_registration",
            "/v1/releases/observe" => "release",
            _ if path.starts_with("/v1/prs/") => match path.rsplit('/').next() {
                Some("ci") => "ci",
                Some("required-checks") => "policy",
                Some("metadata") => "metadata",
                _ => "pull_requests",
            },
            _ if path.starts_with("/v1/repos/") => "repository",
            _ => "other",
        };
        let scope = demand_scope(&request);
        if scope == DemandScope::Global {
            api.0.client.polling().demand(source);
        } else {
            api.0.client.polling().demand_scoped(source, scope);
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn http_demand_is_scoped_only_when_the_selector_is_unambiguous() {
        for (path, local_node, local_sibling, local_other) in [
            ("/v1/prs/acme/repo/7?cached_only=true", true, false, false),
            (
                "/v1/prs/acme/repo/7/ci?cached_only=true",
                true,
                false,
                false,
            ),
            (
                "/v1/prs/acme/repo/7/metadata?cached_only=true",
                true,
                false,
                false,
            ),
            (
                "/v1/prs/acme/repo/7/required-checks?cached_only=true",
                true,
                false,
                false,
            ),
            ("/v1/prs/acme/repo?cached_only=true", true, true, false),
            ("/v1/repos/acme/repo?cached_only=true", true, true, false),
            (
                "/v1/pr-status?repository=ACME%2Frepo&cached_only=true",
                true,
                true,
                false,
            ),
            (
                "/v1/pr-status?repository=acme%2Frepo&repository=acme%2Fother&cached_only=true",
                true,
                true,
                true,
            ),
            (
                "/v1/pr-status?repository=acme%2Frepo&%72epository=acme%2Frepo&cached_only=true",
                true,
                true,
                true,
            ),
            (
                "/v1/pr-status?repository=acme%2&cached_only=true",
                true,
                true,
                true,
            ),
            ("/v1/pr-status?cached_only=true", true, true, true),
            ("/v1/prs/acme/repo/0?cached_only=true", true, true, true),
            ("/v1/snapshot", true, true, true),
        ] {
            let (_dir, api, _, node) = fixture().await;
            let local = api.polling_coverage().await.unwrap().identity;
            let mut proof = api.polling_coverage().await.unwrap();
            proof.identity.instance = "b".repeat(32);
            let mut sibling = node.clone();
            sibling["number"] = json!(8);
            let mut other = node.clone();
            other["repository"]["nameWithOwner"] = json!("acme/other");
            proof.rows.extend([
                crate::polling::row(&sibling).unwrap(),
                crate::polling::row(&other).unwrap(),
            ]);
            assert!(
                api.0
                    .client
                    .polling()
                    .renew(&local, 60, tokio::time::Instant::now(), proof)
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let router = api.router();
            let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            reqwest::Client::new()
                .get(format!("{base}{path}"))
                .send()
                .await
                .unwrap();
            for (node, local) in [
                (&node, local_node),
                (&sibling, local_sibling),
                (&other, local_other),
            ] {
                assert_eq!(
                    api.0.client.polling().covers("ci", node),
                    !local,
                    "{path}: {node}"
                );
            }
            server.abort();
        }
    }

    async fn fixture() -> (tempfile::TempDir, Api, String, Value) {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_token(
            crate::Config {
                cache_path: dir.path().join("cache.sqlite"),
                rest_url: "http://127.0.0.1:9/".parse().unwrap(),
                graphql_url: "http://127.0.0.1:9/graphql".parse().unwrap(),
                ..Default::default()
            },
            "synthetic-token".into(),
        )
        .unwrap();
        client
            .save_derived("http://127.0.0.1:9/user", json!({"id":42}))
            .await
            .unwrap();
        let api = Api::new(client.clone()).await.unwrap();
        let watch = client.save_account_watch(60).await.unwrap();
        let id = watch.id.clone();
        let mut status = serde_json::to_value(&watch).unwrap();
        status["covered_by_account"] = json!(false);
        for name in [
            "last_poll_at_ms",
            "ci_last_poll_at_ms",
            "policy_last_poll_at_ms",
            "discovery_last_poll_at_ms",
        ] {
            status[name] = json!(now_ms());
        }
        api.0.monitors.lock().await.insert(
            id.clone(),
            Monitor {
                state: Arc::new(Mutex::new(serde_json::from_value(status).unwrap())),
                task: tokio::spawn(std::future::pending()),
            },
        );
        let node = json!({"id":"PR_one","number":7,"headRefOid":"a".repeat(40),"repository":{"nameWithOwner":"acme/repo"}});
        client
            .observe(&client.roster_resource(), &json!([node]))
            .await
            .unwrap();
        (dir, api, id, node)
    }

    #[tokio::test]
    async fn coverage_requires_a_live_durable_account_watch_and_recent_mode_progress() {
        let (_dir, api, id, node) = fixture().await;
        let proof = api.polling_coverage().await.unwrap();
        assert_eq!(proof.modes, [true; 3]);
        assert_eq!(
            proof.rows,
            BTreeMap::from([crate::polling::row(&node).unwrap()])
        );
        let state = api.0.monitors.lock().await[&id].state.clone();
        state.lock().await.ci_last_poll_at_ms = Some(1);
        assert_eq!(
            api.polling_coverage().await.unwrap().modes,
            [false, true, true]
        );
        state.lock().await.discovery_last_poll_at_ms = Some(1);
        assert!(api.polling_coverage().await.is_err());
        state.lock().await.discovery_last_poll_at_ms = Some(now_ms());
        api.0.client.delete_watch(&id).await.unwrap();
        assert!(api.polling_coverage().await.is_err());
    }

    #[tokio::test]
    async fn stopped_monitors_and_delegated_companions_cannot_offer_coverage() {
        let (_dir, api, id, _) = fixture().await;
        let local = api.polling_coverage().await.unwrap().identity;
        let mut remote = api.polling_coverage().await.unwrap();
        remote.identity.instance = "b".repeat(32);
        assert!(
            api.0
                .client
                .polling()
                .renew(&local, 60, tokio::time::Instant::now(), remote)
        );
        assert!(api.polling_coverage().await.is_err());
        api.0.client.polling().revoke();
        let monitor = api.0.monitors.lock().await.remove(&id).unwrap();
        monitor.task.abort();
        let _ = monitor.task.await;
        assert!(api.polling_coverage().await.is_err());
    }

    #[tokio::test]
    async fn cache_only_local_reads_revoke_delegation_but_handshakes_do_not() {
        let (_dir, api, _, node) = fixture().await;
        let local = api.polling_coverage().await.unwrap().identity;
        let mut remote = api.polling_coverage().await.unwrap();
        remote.identity.instance = "b".repeat(32);
        assert!(api.0.client.polling().renew(
            &local,
            60,
            tokio::time::Instant::now(),
            remote.clone()
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = api.router();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let http = reqwest::Client::new();
        assert!(
            http.get(format!("{base}/v1/identity"))
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
        assert!(api.0.client.polling().covers("ci", &node));
        assert!(
            http.get(format!("{base}/v1/snapshot"))
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
        assert!(!api.0.client.polling().active());
        assert_eq!(
            api.0
                .client
                .polling()
                .health()
                .local_demand_source
                .as_deref(),
            Some("source_feed")
        );
        assert!(
            !api.0
                .client
                .polling()
                .renew(&local, 60, tokio::time::Instant::now(), remote)
        );
        server.abort();
    }
}
