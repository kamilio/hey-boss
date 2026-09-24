use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use hey_gh::{Client, Config, Error, Freshness, Source};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tempfile::TempDir;
use tokio::{sync::Notify, task::JoinHandle};

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NEW_HEAD: &str = "cccccccccccccccccccccccccccccccccccccccc";
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MERGE: &str = "dddddddddddddddddddddddddddddddddddddddd";
const OTHER_BASE: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

#[derive(Clone, Debug)]
struct Call {
    path: String,
    query: String,
    conditional: bool,
    token: String,
    body: Value,
}

#[derive(Clone)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[derive(Default)]
struct MockData {
    phase: u8,
    calls: Vec<Call>,
    mode: String,
}
#[derive(Clone, Default)]
struct Mock {
    data: Arc<Mutex<MockData>>,
    release: Arc<Notify>,
}
struct Harness {
    mock: Mock,
    url: String,
    task: JoinHandle<()>,
    dir: TempDir,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Harness {
    async fn new() -> Self {
        let mock = Mock::default();
        let router = Router::new().fallback(handler).with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            mock,
            url,
            task,
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn config(&self) -> Config {
        Config {
            rest_url: self.url.parse().unwrap(),
            graphql_url: format!("{}graphql", self.url).parse().unwrap(),
            cache_path: self.dir.path().join("cache.sqlite"),
            min_spacing: Duration::ZERO,
            queue_timeout: Duration::from_secs(2),
            max_attempts: 2,
            ..Config::default()
        }
    }
    fn client(&self) -> Client {
        Client::with_token(self.config(), "synthetic-token".into()).unwrap()
    }
    fn phase(&self, phase: u8) {
        self.mock.data.lock().unwrap().phase = phase;
    }
    fn mode(&self, mode: &str) {
        self.mock.data.lock().unwrap().mode = mode.into();
    }
    fn calls(&self) -> Vec<Call> {
        self.mock.data.lock().unwrap().calls.clone()
    }
}

async fn handler(State(mock): State<Mock>, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let path = uri.path().to_owned();
    let query = uri.query().unwrap_or_default().to_owned();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let (phase, mode, call_number) = {
        let mut data = mock.data.lock().unwrap();
        data.calls.push(Call {
            path: path.clone(),
            query: query.clone(),
            conditional: headers.contains_key("if-none-match")
                || headers.contains_key("if-modified-since"),
            token: headers
                .get("authorization")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("")
                .into(),
            body: body.clone(),
        });
        (
            data.phase,
            data.mode.clone(),
            data.calls.iter().filter(|c| c.path == path).count(),
        )
    };
    if matches!(mode.as_str(), "ruleset-only-policy" | "account-large")
        && path == "/foreground-gate"
    {
        mock.release.notified().await;
    }
    if mode == "scheduler-slow-graphql" && path == "/graphql" {
        mock.release.notified().await;
    }
    if mode == "account-discovery-stalled"
        && path == "/graphql"
        && body["query"]
            .as_str()
            .is_some_and(|query| query.contains("MyOpenPullRequests"))
    {
        mock.release.notified().await;
    }
    let path = if mode == "account-fallback-case-change" {
        path.to_ascii_lowercase()
    } else {
        path
    };
    if mode.starts_with("account-fallback") {
        if path == "/graphql"
            && mode != "account-fallback-recovered"
            && body["query"]
                .as_str()
                .is_some_and(|q| q.contains("MyOpenPullRequests"))
        {
            return reply(
                200,
                json!({"data":{"viewer":{"pullRequests":{"nodes":[{"number":777}]}}},"errors":[{"type":"FORBIDDEN","message":"synthetic IP allow list denied"}]}),
                &[],
            );
        }
        if path == "/search/issues" {
            let host = headers.get("host").unwrap().to_str().unwrap();
            let item = |repo: &str, number: u64| {
                let repo = if mode == "account-fallback-case-change" {
                    repo.to_ascii_uppercase()
                } else {
                    repo.into()
                };
                json!({"number":number,"pull_request":{"url":format!("http://{host}/repos/{repo}/pulls/{number}")}})
            };
            let mut items = vec![
                item("acme/demo", 7),
                item("acme/fresh", 11),
                item("acme/fresh", 12),
                item("acme/fresh", 13),
                json!({"number":14,"pull_request":{"url":"http://127.0.0.1:1/repos/acme/fresh/pulls/14"}}),
                item("acme/extra", 15),
            ];
            items.extend((16..25).map(|n| item("acme/fresh", n)));
            if mode == "account-fallback-stall" {
                items.remove(0);
            }
            if mode == "account-fallback-denied-prefix" {
                items = (11..21).map(|n| item("acme/fresh", n)).collect();
            }
            // A capped/incomplete search can supply positives, never omissions.
            return reply(
                200,
                json!({"items":items,"total_count":1500,"incomplete_results":true}),
                &[],
            );
        }
        if path.starts_with("/repos/acme/fresh/pulls/")
            || path.starts_with("/repos/acme/extra/pulls/")
        {
            let number = path.rsplit('/').next().unwrap().parse::<u64>().unwrap();
            if mode == "account-fallback-denied-prefix" && number <= 15 {
                return reply(403, json!({"message":"synthetic permission denial"}), &[]);
            }
            if mode == "account-fallback-stall" && number == 12 {
                mock.release.notified().await;
            }
            let repo = if path.contains("/extra/") {
                "acme/extra"
            } else {
                "acme/fresh"
            };
            return reply(
                200,
                json!({"node_id":format!("PR_{repo}_{}{number}",if number==11 && phase>=3 {"new_"} else {""}),"number":number,"title":"Fallback PR","html_url":format!("https://github.com/{repo}/pull/{number}"),"state":if number==12 || phase==2 {"closed"} else {"open"},"merged":false,"user":{"login":if number==13 {"another-author"} else {"me"}},"draft":false,"created_at":"2026-09-19T00:00:00Z","updated_at":if phase>=2 {"2026-09-19T02:00:00Z"} else {"2026-09-19T00:00:00Z"},"head":{"sha":HEAD,"ref":"feature"},"base":{"sha":BASE,"ref":"main","repo":{"full_name":repo}},"merge_commit_sha":null,"mergeable":true}),
                &[],
            );
        }
    }
    if mode == "account-reopen-failed-rest" && path == "/repos/acme/demo/pulls/7" {
        return reply(403, json!({"message":"temporary lack of access"}), &[]);
    }
    if mode == "account-ci-permission-fails" && path.contains("/check-runs") {
        return reply(403, json!({"message":"insufficient permissions"}), &[]);
    }
    if mode == "account-slow-threads"
        && path == "/graphql"
        && body["query"]
            .as_str()
            .is_some_and(|query| query.contains("reviewThreads"))
    {
        mock.release.notified().await;
    }
    if mode == "account-slow-reviews" && path == "/repos/acme/demo/pulls/7/reviews" {
        mock.release.notified().await;
    }
    if mode == "ci-batch-blocked-checks"
        && path == format!("/repos/acme/demo/commits/{HEAD}/check-runs")
    {
        mock.release.notified().await;
    }
    if mode == "ci-batch-blocked-checks" {
        if path == format!("/repos/acme/demo/commits/{MERGE}/check-runs") {
            return reply(
                200,
                json!({"check_runs":[{"id":55,"name":"merge tests","head_sha":MERGE,"status":"completed","conclusion":"success"}]}),
                &[],
            );
        }
        if path == format!("/repos/acme/demo/commits/{MERGE}/status") {
            return reply(
                200,
                json!({"statuses":[{"id":66,"context":"merge external","state":"success"}]}),
                &[],
            );
        }
        if path == "/repos/acme/demo/actions/runs" && query.contains(MERGE) {
            return reply(200, json!({"workflow_runs":[]}), &[]);
        }
    }
    if path == "/slow" {
        mock.release.notified().await;
    }
    if path == "/graphql" && (mode == "correlation-retry" || mode == "correlation-timeout") {
        let attempts = mock
            .data
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|call| call.path == "/graphql")
            .count();
        if attempts > 1 && mode == "correlation-timeout" {
            mock.release.notified().await;
        }
        return reply(
            if attempts == 1 { 503 } else { 200 },
            if attempts == 1 {
                json!({"message":"private-upstream-error-message"})
            } else {
                json!({"data":{"viewer":{"login":"private-response-body-value"}}})
            },
            &[],
        );
    }
    if mode == "correlation-policy" {
        return reply(404, json!({"message":"private-policy-error"}), &[]);
    }
    if path == "/repos/acme/demo/pulls/7" && mode.starts_with("sdk-upstream-") {
        return reply(
            mode.strip_prefix("sdk-upstream-").unwrap().parse().unwrap(),
            json!({"message":"synthetic upstream diagnostic"}),
            &[],
        );
    }
    if path == "/graphql" && mode.starts_with("graphql-error-") {
        let mut errors = if mode == "graphql-error-generic" {
            vec![json!({"type":"INTERNAL","message":"operation failed"})]
        } else {
            vec![json!({"type":"FORBIDDEN","message":"organization IP allow list denied"})]
        };
        if mode == "graphql-error-mixed" {
            errors.push(json!({"type":"INTERNAL","message":"independent operation error"}));
        }
        return reply(
            if mode == "graphql-error-http403" {
                403
            } else {
                200
            },
            json!({"data":{"viewer":{"login":"partial"}},"errors":errors}),
            &[],
        );
    }
    if mode == "issue72-stall-metadata" && path == "/repos/acme/demo/pulls/7" {
        mock.release.notified().await;
    }
    if mode == "issue73-backoff" && path == "/repos/acme/demo/pulls/7" {
        return reply(
            429,
            json!({"message":"secondary rate limit"}),
            &[("retry-after", "60")],
        );
    }
    if mode == "issue73-stall-list" && path == "/repos/acme/demo/pulls" {
        mock.release.notified().await;
    }
    if mode == "issue72-partial-auth-stall"
        && path == "/graphql"
        && body["query"]
            .as_str()
            .is_some_and(|q| q.contains("MyOpenPullRequests"))
    {
        return reply(
            200,
            json!({"data":{"viewer":{"pullRequests":{"nodes":[{"number":7}]}}},"errors":[{"type":"FORBIDDEN","message":"quora-internal IP allow list denied"}]}),
            &[],
        );
    }
    if mode == "issue72-partial-auth-stall" && path.contains("/check-runs") {
        mock.release.notified().await;
    }
    if path == "/conditional-paced" {
        let reset = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600)
            .to_string();
        let rate_headers = [
            ("etag", "\"stable\""),
            ("x-ratelimit-resource", "core"),
            (
                "x-ratelimit-remaining",
                if phase == 0 { "1000" } else { "0" },
            ),
            ("x-ratelimit-reset", reset.as_str()),
        ];
        return reply(
            if headers.contains_key("if-none-match") {
                304
            } else {
                200
            },
            json!({"stable":true}),
            &rate_headers,
        );
    }
    if path == "/forbidden" {
        return reply(
            403,
            json!({"message":"Resource not accessible by integration"}),
            &[],
        );
    }
    if path == "/search/issues" && mode == "primary" {
        let reset = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 60)
            .to_string();
        return reply(
            403,
            json!({"message":"API rate limit exceeded"}),
            &[
                ("x-ratelimit-resource", "search"),
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", &reset),
            ],
        );
    }
    if path == "/secondary" && call_number == 1 {
        return reply(
            429,
            json!({"message":"secondary rate limit"}),
            &[("retry-after", "0")],
        );
    }
    if path == "/secondary-no-header" {
        return reply(
            403,
            json!({"message":"You have exceeded a secondary rate limit"}),
            &[],
        );
    }
    if path == "/server-error" && call_number == 1 {
        return reply(
            503,
            json!({"message":"unavailable"}),
            &[("retry-after", "0")],
        );
    }
    if path == "/redirect" {
        return (
            StatusCode::FOUND,
            [("location", "http://not-github.invalid/stolen")],
        )
            .into_response();
    }
    if path == "/large" {
        return reply(200, json!({"body":"x".repeat(10000)}), &[]);
    }
    if path == "/bad-json" {
        return (StatusCode::OK, "not json").into_response();
    }
    if path == "/page-cycle" {
        let host = headers["host"].to_str().unwrap();
        return reply(
            200,
            json!([]),
            &[("link", &format!("<http://{host}/page-cycle>; rel=\"next\""))],
        );
    }
    if path == "/page-escape" {
        return reply(
            200,
            json!([]),
            &[("link", "<https://evil.invalid/data>; rel=\"next\"")],
        );
    }
    if path == "/page-large" {
        return reply(200, json!([{ "body":"x".repeat(100) }]), &[]);
    }
    if path == "/secondary-large" {
        return reply(
            429,
            json!({"message":"x".repeat(10000)}),
            &[("retry-after", "60")],
        );
    }
    if path == "/graphql" {
        if mode == "graphql-primary" {
            let reset = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 60)
                .to_string();
            return reply(
                200,
                json!({"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}),
                &[
                    ("x-ratelimit-resource", "graphql"),
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &reset),
                ],
            );
        }
        if mode == "graphql-errors" {
            return reply(
                200,
                json!({"data":{"repository":null},"errors":[{"message":"missing permission"}]}),
                &[],
            );
        }
        if mode == "graphql-rate" {
            return reply(
                200,
                json!({"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}),
                &[],
            );
        }
        if body["query"]
            .as_str()
            .unwrap_or("")
            .contains("query MyOpenPullRequests")
        {
            if mode == "account-discovery-errors" {
                return reply(
                    200,
                    json!({"data":{"viewer":null},"errors":[{"message":"discovery unavailable"}]}),
                    &[],
                );
            }
            let node = |repository: &str| {
                let mut node = json!({"id":format!("PR_{repository}_{}7", if mode=="account-new-identity" {"new_"} else {""}),"number":7,"title":if mode=="account-metadata-change" && phase>=1 {"Updated title"} else {"A PR"},"url":format!("https://github.com/{repository}/pull/7"),"state":"OPEN","isDraft":mode=="account-metadata-change" && phase>=1,"mergeable":if mode=="account-metadata-change" && phase>=1 {"CONFLICTING"} else {"MERGEABLE"},"reviewDecision":if mode=="account-review-change" && phase>=1 {"CHANGES_REQUESTED"} else {"REVIEW_REQUIRED"},"mergeStateStatus":if mode=="account-review-change" && phase>=1 {"BLOCKED"} else {"CLEAN"},"headRefOid":if (mode=="account-head-change" || mode=="account-page-order") && phase>=1 {NEW_HEAD} else {HEAD},"baseRefOid":BASE,"headRefName":"feature","baseRefName":"main","createdAt":"2026-09-19T00:00:00Z","updatedAt":"2026-09-19T00:00:00Z","repository":{"nameWithOwner":repository},"author":{"login":"me"},"commits":{"nodes":[{"commit":{"oid":if (mode=="account-head-change" || mode=="account-page-order") && phase>=1 {NEW_HEAD} else {HEAD},"statusCheckRollup":{"state":if phase>=2 {"SUCCESS"} else {"PENDING"},"contexts":{"totalCount":1,"nodes":[{"__typename":"CheckRun","name":"tests","status":"IN_PROGRESS","conclusion":null,"detailsUrl":"https://github.com/checks/5"}]}}}}]}});
                if mode == "account-policy-selectors" {
                    node["baseRefName"] = json!(if phase >= 1 { "release" } else { "main" });
                    node["baseRefOid"] = json!(if phase >= 2 { NEW_HEAD } else { BASE });
                }
                if mode == "account-repository-case-change" && phase >= 1 {
                    node["repository"]["nameWithOwner"] = json!(repository.to_ascii_uppercase());
                }
                node
            };
            let (nodes, next, total) = if mode == "account-large" {
                let mut nodes = vec![node("acme/demo")];
                nodes.extend((0..24).map(|n| node(&format!("acme/watch{n:02}"))));
                (nodes, None, 25)
            } else if mode == "account-state-version" && phase == 7 {
                (vec![node("acme/other")], None, 1)
            } else if phase >= 9 {
                (vec![], None, 0)
            } else if phase >= 8 {
                (vec![node("acme/other")], None, 1)
            } else if body["variables"]["after"] == "PR-next" {
                (
                    vec![node("acme/other")],
                    if mode == "account-badpage" {
                        Some("PR-next")
                    } else {
                        None
                    },
                    2,
                )
            } else {
                (vec![node("acme/demo")], Some("PR-next"), 2)
            };
            let mut nodes = nodes;
            if mode == "account-state-version" {
                for node in &mut nodes {
                    if phase == 1 {
                        node["updatedAt"] = json!("2026-09-19T00:00:02Z");
                    } else if phase >= 2 {
                        node["updatedAt"] = json!("2026-09-19T00:00:01Z");
                    }
                }
            }
            if mode == "account-version-change" {
                for node in &mut nodes {
                    if phase == 1 {
                        node["title"] = json!("Updated title");
                        node["isDraft"] = json!(true);
                        node["updatedAt"] = json!("2026-09-19T00:00:01.250Z");
                    } else if phase >= 2 {
                        // Different RFC3339 offset, but an older instant.
                        node["updatedAt"] = json!("2026-09-19T01:00:00+01:00");
                    }
                }
            }
            if mode == "account-discovery-light" {
                for node in &mut nodes {
                    node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]
                        .as_object_mut()
                        .unwrap()
                        .remove("contexts");
                }
            }
            return reply(
                200,
                json!({"data":{"viewer":{"pullRequests":{"totalCount":total,"nodes":nodes,"pageInfo":{"hasNextPage":next.is_some(),"endCursor":next}}}}}),
                &[],
            );
        }
        let data = if body["query"]
            .as_str()
            .unwrap_or("")
            .contains("query ReviewEvents")
        {
            json!({"data":{"repository":{"pullRequest":{"timelineItems":{"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}})
        } else if body["query"]
            .as_str()
            .unwrap_or("")
            .contains("query Comments")
        {
            json!({"data":{"node":{"comments":{"nodes":[{"id":"C2","body":"reply","updatedAt":"2026-09-19T00:00:00Z"}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}})
        } else if body["variables"]["after"] == "T-next" {
            json!({"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}})
        } else {
            json!({"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[{"id":"T1","isResolved":phase>=4,"isOutdated":false,"path":"src/file.rs","comments":{"nodes":[{"id":"C1","body":"please fix"}],"pageInfo":{"hasNextPage":true,"endCursor":"C-next"}}}],"pageInfo":{"hasNextPage":true,"endCursor":"T-next"}}}}}})
        };
        return reply(200, data, &[]);
    }
    if mode == "account-slow-one" && path == "/repos/acme/demo/pulls/7" {
        mock.release.notified().await;
    }
    if mode == "account-identity-stalled" && path == "/repos/acme/other/issues/7/comments" {
        mock.release.notified().await;
    }
    if mode == "account-slow-policy" && path == "/repos/acme/demo/branches/main" {
        mock.release.notified().await;
    }
    let current_head = if (mode == "push-during-report" && call_number >= 2)
        || ((mode == "account-head-change" || mode == "account-page-order") && phase >= 1)
    {
        NEW_HEAD
    } else {
        HEAD
    };
    let other = path.contains("/acme/other/");
    let normalized = path
        .replace("/ACME/DEMO/", "/acme/demo/")
        .replace("/acme/other/", "/acme/demo/");
    let normalized = if mode == "account-large" && path.starts_with("/repos/acme/watch") {
        let repository = path.split('/').nth(3).unwrap();
        normalized.replace(&format!("/acme/{repository}/"), "/acme/demo/")
    } else {
        normalized
    };
    let value = match normalized.as_str() {
        "/user" => json!({"login":"me"}),
        "/repos/acme/demo/pulls" => json!([
            {"number":7,"user":{"login":"me"}}, {"number":8,"user":{"login":"other"}}]),
        "/repos/acme/demo/pulls/7" => {
            json!({"node_id":if mode=="account-large" {format!("PR_acme/{}_7",path.split('/').nth(3).unwrap())} else {(if mode=="account-new-identity" {if other {"PR_acme/other_new_7"} else {"PR_acme/demo_new_7"}} else if other {"PR_acme/other_7"} else {"PR_acme/demo_7"}).to_owned()},"number":7,"title":if mode=="account-version-change" && phase>=4 {"Latest REST title"} else {"A PR"},"state":if mode=="account-state-version" && phase>=1 || mode.starts_with("account") && ((!other && phase>=8) || (other && phase>=9)) {"closed"} else {"open"},
            "merged":mode.starts_with("account") && other && phase>=9,"merged_at":if mode.starts_with("account") && other && phase>=9 {json!("2026-09-19T02:00:00Z")} else {Value::Null},
            "closed_at":if mode=="account-state-version" && phase>=8 {json!("2026-09-19T00:00:03Z")} else if mode=="account-state-version" && phase>=1 {json!("2026-09-19T00:00:01Z")} else if mode.starts_with("account") && ((!other && phase>=8) || (other && phase>=9)) {json!("2026-09-19T02:00:00Z")} else {Value::Null},
            "updated_at":if mode=="account-state-version" && phase>=8 {"2026-09-19T00:00:03Z"} else if mode=="account-state-version" && phase>=1 {"2026-09-19T00:00:01Z"} else if mode=="account-version-change" && phase>=4 {"2026-09-19T00:00:02Z"} else {"2026-09-19T00:00:00Z"},
            "head":{"sha":current_head,"ref":"feature"},"base":{"sha":if mode=="account-policy-selectors" && phase>=2 {NEW_HEAD} else {BASE},"ref":if mode=="account-policy-selectors" && phase>=1 {"release"} else {"main"}},"merge_commit_sha":if mode=="ci-batch-blocked-checks" || mode=="account-policy-selectors" && phase>=3 { json!(MERGE) } else { Value::Null },
            "mergeable":if phase==3 { json!(false) } else if phase==5 { Value::Null } else { json!(true) },"draft":false})
        }
        "/repos/acme/demo/issues/7/comments" => {
            if query.contains("page=2") {
                if phase >= 1 {
                    json!([{"id":2,"body":"new comment","updated_at":"2026-09-19T01:00:00Z"}])
                } else {
                    json!([])
                }
            } else {
                json!([{"id":1,"body":"initial comment"}])
            }
        }
        "/repos/acme/demo/pulls/7/comments" => {
            json!([{"id":3,"body":"inline review","path":"src/file.rs"}])
        }
        "/repos/acme/demo/issues/7/timeline" => json!([{ "id":11,"event":"review_dismissed" }]),
        "/repos/acme/demo/pulls/7/reviews" => {
            json!([{"id":4,"state":"CHANGES_REQUESTED","body":"fix this"}])
        }
        "/repos/acme/demo/branches/main" if mode == "ruleset-cache-policy" => {
            json!({"commit":{"sha":BASE},"protected":true})
        }
        p if mode == "ruleset-cache-policy"
            && p.ends_with("/protection/required_status_checks") =>
        {
            return reply(404, json!({"message":"Branch not protected"}), &[]);
        }
        p if mode == "ruleset-cache-policy" && p.contains("/compare/") => {
            return reply(403, json!({"message":"Comparison inaccessible"}), &[]);
        }
        p if mode == "ruleset-cache-policy" && p.contains("/rules/branches/") => {
            if phase == 6 {
                return reply(403, json!({"message":"Rulesets inaccessible"}), &[]);
            }
            json!([{"type":"required_status_checks","parameters":{
                "strict_required_status_checks_policy":if phase == 7 {Value::Null} else {json!(phase == 2)},
                "required_status_checks":[{"context":"pre-commit","integration_id":15368}]
            }}])
        }
        p if mode == "ruleset-cache-policy" && p.contains("/check-runs") => {
            if phase == 3 {
                return reply(403, json!({"message":"Checks inaccessible"}), &[]);
            }
            json!({"total_count":if phase == 4 {0} else {1},"check_runs":if phase == 4 {json!([])} else {json!([
                {"id":5,"name":"pre-commit","head_sha":if p.contains(MERGE) {MERGE} else {HEAD},
                 "app":{"id":if phase == 5 {42} else {15368}},"status":"completed","conclusion":"success"}
            ])}})
        }
        "/repos/acme/demo/actions/runs" if mode == "ruleset-only-policy" && phase >= 7 => {
            return reply(
                403,
                json!({"message":"Optional workflow details inaccessible"}),
                &[],
            );
        }
        "/repos/acme/demo/branches/main" if mode == "ruleset-only-policy" => {
            json!({"commit":{"sha":BASE},"protected":true})
        }
        p if mode == "ruleset-only-policy" && p.ends_with("/protection/required_status_checks") => {
            return reply(
                if phase == 4 { 403 } else { 404 },
                json!({"message":if phase==3 {"Not Found"} else if phase==4 {"Resource not accessible"} else {"Branch not protected"}}),
                &[],
            );
        }
        p if mode == "ruleset-only-policy" && phase >= 6 && p.contains("/compare/") => {
            json!({"merge_base_commit":{"sha":BASE}})
        }
        p if mode == "ruleset-only-policy" && p.contains("/rules/branches/") => {
            json!([{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":phase>=5,"required_status_checks":[{"context":"tests"}]}}])
        }
        "/repos/acme/demo/branches/main" | "/repos/acme/demo/branches/release"
            if mode == "account-policy-selectors" =>
        {
            json!({"commit":{"sha":if phase>=4 {OTHER_BASE} else if phase>=2 {NEW_HEAD} else {BASE}},"protected":false})
        }
        p if mode == "account-policy-selectors"
            && p.ends_with("/protection/required_status_checks") =>
        {
            json!({"strict":false,"contexts":[],"checks":[]})
        }
        p if mode == "account-policy-selectors" && p.contains("/rules/branches/") => json!([]),
        p if mode == "account-policy-selectors" && p.contains("/compare/") => {
            json!({"merge_base_commit":{"sha":if phase>=4 {OTHER_BASE} else if phase>=2 {NEW_HEAD} else {BASE}}})
        }
        p if p.contains("/check-runs") => {
            json!({"total_count":1,"check_runs":[{"id":5,"name":"tests","head_sha":if mode=="account-policy-selectors" && p.contains(MERGE) {MERGE} else if p.contains(NEW_HEAD) {NEW_HEAD} else {HEAD},
            "status":if phase>=2 {"completed"} else {"in_progress"},"conclusion":if phase>=2 {json!("success")} else {Value::Null}}]})
        }
        p if p.ends_with("/status") => {
            json!({"state":if phase>=2 {"success"} else {"pending"},"statuses":[{"id":6,"context":"external","state":if phase>=2 {"success"} else {"pending"}}]})
        }
        "/repos/acme/demo/actions/runs" => json!({"workflow_runs":[
            {"id":8,"workflow_id":1,"run_number":1,"run_attempt":1,"event":"pull_request","head_sha":if query.contains(NEW_HEAD) {NEW_HEAD} else {HEAD},"status":"completed","conclusion":"cancelled"},
            {"id":9,"workflow_id":1,"run_number":2,"run_attempt":if phase>=7 {3} else {2},"event":"pull_request","head_sha":if query.contains(NEW_HEAD) {NEW_HEAD} else {HEAD},"status":if phase>=2 {"completed"} else {"in_progress"},"conclusion":if phase>=2 {json!("success")} else {Value::Null}}]}),
        "/repos/acme/demo/actions/runs/9/attempts/2/jobs"
        | "/repos/acme/demo/actions/runs/9/attempts/3/jobs" => {
            json!({"jobs":[{"id":10,"name":"build","status":if phase>=2 {"completed"} else {"in_progress"},"conclusion":if phase>=2 {json!("success")} else {Value::Null},"steps":[{"name":"test","status":"completed","conclusion":"success"}]}]})
        }
        _ => json!({"answer":phase}),
    };
    use sha2::Digest;
    let etag = format!(
        "\"{:x}\"",
        sha2::Sha256::digest(value.to_string().as_bytes())
    );
    if headers.get("if-none-match").and_then(|h| h.to_str().ok()) == Some(&etag) {
        return (StatusCode::NOT_MODIFIED, [("etag", etag)]).into_response();
    }
    let mut extras = vec![("etag", etag)];
    if normalized == "/repos/acme/demo/issues/7/comments" && !query.contains("page=2") {
        // Derive the next URL from this mock's actual origin.
        let host = headers.get("host").unwrap().to_str().unwrap();
        extras.push((
            "link",
            format!("<http://{host}{path}?per_page=100&page=2>; rel=\"next\""),
        ));
    }
    let refs: Vec<_> = extras.iter().map(|(k, v)| (*k, v.as_str())).collect();
    reply(200, value, &refs)
}

fn reply(status: u16, value: Value, headers: &[(&str, &str)]) -> Response {
    let mut response = (StatusCode::from_u16(status).unwrap(), axum::Json(value)).into_response();
    for (k, v) in headers {
        response.headers_mut().insert(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    response
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("condition became true");
}

#[tokio::test]
async fn cache_revalidates_and_persists_without_credentials() {
    let h = Harness::new().await;
    let client = h.client();
    let first = client.get("plain", Freshness::Revalidate).await.unwrap();
    assert!(matches!(first.source, Source::Network));
    let second = client.get("plain", Freshness::default()).await.unwrap();
    assert!(matches!(second.source, Source::Cache));
    assert_eq!(h.calls().len(), 1);
    let third = client.get("plain", Freshness::Revalidate).await.unwrap();
    assert!(matches!(third.source, Source::Revalidated));
    assert_eq!(third.fetched_at_ms, first.fetched_at_ms);
    assert!(h.calls()[1].conditional);
    assert_eq!(client.status().network_requests, 2);
    assert_eq!(client.status().conditional_requests, 1);
    assert_eq!(client.status().not_modified_responses, 1);
    // Sending a validator is not proof of reuse: changed content returns 200.
    h.phase(1);
    let changed = client.get("plain", Freshness::Revalidate).await.unwrap();
    assert!(matches!(changed.source, Source::Network));
    assert_eq!(changed.data["answer"], 1);
    assert_eq!(client.status().network_requests, 3);
    assert_eq!(client.status().conditional_requests, 2);
    assert_eq!(client.status().not_modified_responses, 1);
    drop(client);
    let restarted = h.client();
    assert_eq!(
        restarted
            .get("plain", Freshness::CachedOnly)
            .await
            .unwrap()
            .data,
        changed.data
    );
    assert_eq!(restarted.status().network_requests, 0);
    assert_eq!(restarted.status().conditional_requests, 0);
    assert_eq!(restarted.status().not_modified_responses, 0);
    assert!(matches!(
        restarted.get("missing", Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
    let db = std::fs::read(&h.config().cache_path).unwrap();
    assert!(
        !db.windows(b"synthetic-token".len())
            .any(|w| w == b"synthetic-token")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&h.config().cache_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn coalesces_even_when_first_caller_is_cancelled_and_queue_is_full() {
    let h = Harness::new().await;
    let mut config = h.config();
    config.queue_capacity = 1;
    let client = Client::with_token(config, "synthetic-token".into()).unwrap();
    let first = tokio::spawn({
        let c = client.clone();
        async move { c.get("slow", Freshness::Revalidate).await }
    });
    until(|| h.calls().len() == 1).await;
    let mut callers = Vec::new();
    for _ in 0..10 {
        callers.push(tokio::spawn({
            let c = client.clone();
            async move { c.get("slow", Freshness::Revalidate).await }
        }));
    }
    until(|| client.status().coalesced_requests == 10).await;
    assert!(matches!(
        client.get("different", Freshness::Revalidate).await,
        Err(Error::QueueFull)
    ));
    first.abort();
    h.mock.release.notify_one();
    for caller in callers {
        assert!(caller.await.unwrap().is_ok());
    }
    assert_eq!(h.calls().len(), 1);
    assert_eq!(client.status().outstanding_requests, 0);
    assert_eq!(client.status().max_active_requests, 1);
    assert!(client.get("slow", Freshness::CachedOnly).await.is_ok());
}

#[tokio::test]
async fn primary_bucket_does_not_block_core_and_secondary_retries_are_bounded() {
    let h = Harness::new().await;
    h.mode("primary");
    let client = h.client();
    assert!(matches!(
        client.get("search/issues", Freshness::Revalidate).await,
        Err(Error::RateLimited { .. })
    ));
    assert!(client.get("plain", Freshness::Revalidate).await.is_ok());
    // An impossible deadline must fail immediately, not wait for its full timeout.
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            client.get("search/issues", Freshness::Revalidate)
        )
        .await
        .unwrap()
        .is_err()
    );
    assert_eq!(
        h.calls()
            .iter()
            .filter(|c| c.path == "/search/issues")
            .count(),
        1
    );
    assert_eq!(client.status().rate_limits["search"].remaining, 0);
    assert!(client.get("secondary", Freshness::Revalidate).await.is_ok());
    assert_eq!(
        h.calls().iter().filter(|c| c.path == "/secondary").count(),
        2
    );
    assert!(
        matches!(client.get("secondary-no-header",Freshness::Revalidate).await,Err(Error::RateLimited { retry_after_seconds }) if retry_after_seconds>=60)
    );
    let calls = h.calls().len();
    assert!(matches!(
        client.get("other", Freshness::Revalidate).await,
        Err(Error::RateLimited { .. })
    ));
    assert_eq!(h.calls().len(), calls);
}

#[tokio::test]
async fn permission_failures_are_not_retried_and_bad_responses_are_not_cached() {
    let h = Harness::new().await;
    let client = h.client();
    assert!(matches!(
        client.get("forbidden", Freshness::Revalidate).await,
        Err(Error::GitHub { status: 403, .. })
    ));
    assert_eq!(h.calls().len(), 1);
    assert!(client.get("redirect", Freshness::Revalidate).await.is_err());
    assert!(client.get("bad-json", Freshness::Revalidate).await.is_err());
    assert!(matches!(
        client.get("bad-json", Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
    let mut config = h.config();
    config.max_body_bytes = 100;
    let small = Client::with_token(config, "different-synthetic-token".into()).unwrap();
    assert!(small.get("large", Freshness::Revalidate).await.is_err());
    assert!(matches!(
        small.get("large", Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
}

#[tokio::test]
async fn complete_reports_cover_pagination_replies_current_attempts_and_conflicts() {
    let h = Harness::new().await;
    let client = h.client();
    let report = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(report.complete, "{:?}", report.data.errors);
    assert_eq!(report.data.conflicts, "clean");
    assert_eq!(report.data.ci.summary.state, "running");
    assert_eq!(report.data.review_comments[0]["body"], "inline review");
    assert_eq!(report.data.reviews[0]["state"], "CHANGES_REQUESTED");
    assert_eq!(
        report.data.review_threads[0]["comments"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(report.data.ci.workflow_runs.len(), 1);
    assert_eq!(report.data.ci.workflow_runs[0]["id"], 9);
    assert_eq!(report.data.ci.jobs[0]["steps"][0]["name"], "test");
    assert!(h.calls().iter().any(|c| c.query.contains("page=2")));
    assert!(
        h.calls()
            .iter()
            .all(|c| c.token == "Bearer synthetic-token")
    );
    assert!(
        h.calls()
            .iter()
            .any(|c| c.body["variables"]["after"] == "T-next")
    );
    h.phase(5);
    assert_eq!(
        client
            .pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .data
            .conflicts,
        "unknown"
    );
    h.phase(3);
    let report = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(report.data.conflicts, "conflicting");
    assert_eq!(report.data.ci.summary.state, "success");
}

#[tokio::test]
async fn cursors_replay_changes_survive_restarts_and_reject_other_scopes() {
    let h = Harness::new().await;
    let client = h.client();
    let first = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(client.changes(None, 100).await.unwrap().changes.len(), 10);
    let unchanged = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(unchanged.cursor, first.cursor);
    assert!(
        client
            .changes(first.cursor.as_deref(), 100)
            .await
            .unwrap()
            .changes
            .is_empty()
    );
    h.phase(1);
    let comment = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let changes = client.changes(first.cursor.as_deref(), 100).await.unwrap();
    assert_eq!(changes.changes.len(), 2);
    assert_eq!(changes.changes[0].changed_fields, vec!["comments"]);
    assert_eq!(changes.next_cursor, comment.cursor.unwrap());
    h.phase(2);
    client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let mut cursor = first.cursor.unwrap();
    let mut delivered = Vec::new();
    loop {
        let page = client.changes(Some(&cursor), 1).await.unwrap();
        cursor = page.next_cursor;
        delivered.extend(page.changes);
        if !page.has_more {
            break;
        }
    }
    assert_eq!(delivered.len(), 4);
    assert!(
        delivered.iter().any(
            |c| c.resource.starts_with("ci://") && c.changed_fields.contains(&"summary".into())
        )
    );
    assert!(
        delivered
            .iter()
            .any(|c| c.resource.starts_with("pr://") && c.changed_fields == vec!["ci"])
    );
    drop(client);
    let restarted = h.client();
    assert!(
        restarted
            .changes(Some(&cursor), 100)
            .await
            .unwrap()
            .changes
            .is_empty()
    );
    let other = Client::with_token(h.config(), "other-credential".into()).unwrap();
    assert!(other.changes(Some(&cursor), 100).await.is_err());
    let mut config = h.config();
    config.cache_path = h.dir.path().join("another.sqlite");
    assert!(
        Client::with_token(config, "synthetic-token".into())
            .unwrap()
            .changes(Some(&cursor), 100)
            .await
            .is_err()
    );
    assert!(restarted.changes(Some("garbage"), 100).await.is_err());
    assert!(restarted.changes(None, 0).await.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn retry_logs_correlate_the_actual_result_without_exposing_request_data() {
    // Scoped tracing callsite interest is process-global. Concurrent untraced
    // SDK tests can disable INFO callsites while this test captures WARNs.
    // Use an isolated harness so the privacy/correlation assertions cannot
    // depend on another test's subscriber or scheduling.
    const CHILD: &str = "HEY_GH_CORRELATION_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "retry_logs_correlate_the_actual_result_without_exposing_request_data",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let h = Harness::new().await;
    h.mode("correlation-retry");
    let captured = CapturedLogs(Arc::new(Mutex::new(vec![])));
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let c = h.client();
    let (graphql, rest) = tokio::join!(
        c.graphql(
            "query { viewer { login } }",
            json!({"privateVariable":"private-query-value"}),
            Freshness::Revalidate
        ),
        c.get("plain?sentinel=private-url-query", Freshness::Revalidate),
    );
    assert!(graphql.is_ok());
    assert!(rest.is_ok());
    let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    for secret in [
        "synthetic-token",
        "private-query-value",
        "private-url-query",
        "private-response-body-value",
        "private-upstream-error-message",
        h.url.as_str(),
    ] {
        assert!(
            !text.contains(secret),
            "diagnostic leaked a synthetic private value"
        );
    }
    let id = |line: &str| {
        line.split("request_id=")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned()
    };
    let retry = text
        .lines()
        .find(|line| line.contains("GitHub server error; retry scheduled"))
        .unwrap();
    let retry_id = id(retry);
    assert_eq!(retry_id.len(), 32);
    assert!(retry_id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let completed: Vec<_> = text
        .lines()
        .filter(|line| line.contains("GitHub request finished"))
        .collect();
    assert_eq!(completed.len(), 2, "{text}");
    let recovered = completed.iter().find(|line| id(line) == retry_id).unwrap();
    assert!(recovered.contains("attempts=2"));
    assert!(recovered.contains("succeeded=true"));
    assert!(recovered.contains("http_status=200"));
    assert!(recovered.contains("endpoint=\"graphql\""));
    let other = completed.iter().find(|line| id(line) != retry_id).unwrap();
    assert!(other.contains("attempts=1"));
    assert!(other.contains("succeeded=true"));
    assert!(other.contains("endpoint=\"rest_other\""));
    // Identify an inaccessible policy without exposing any selector, branch,
    // query parameter, or raw diagnostic in the log.
    h.mode("correlation-policy");
    let mut enterprise_config = h.config();
    enterprise_config.rest_url = enterprise_config.rest_url.join("api/v3/").unwrap();
    let enterprise =
        Client::with_token(enterprise_config, "private-enterprise-token".into()).unwrap();
    assert!(matches!(
        enterprise.get(
            "repos/private-owner/private-repository/branches/private%2Fbranch/protection/required_status_checks?sentinel=private-policy-query",
            Freshness::Revalidate
        ).await,
        Err(Error::GitHub { status: 404, .. })
    ));
    let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let policy = text
        .lines()
        .find(|line| line.contains("endpoint=\"branch_protection\""))
        .unwrap();
    assert!(policy.contains("http_status=404"));
    assert!(policy.contains("succeeded=false"));
    assert!(policy.contains("error_code=\"github_http\""));
    for private in [
        "private-owner",
        "private-repository",
        "private%2Fbranch",
        "private-policy-query",
        "private-policy-error",
        "private-enterprise-token",
    ] {
        assert!(!text.contains(private));
    }
    // A received 503 must not become the status of a later transport timeout
    // for which no response headers arrived.
    h.mode("correlation-timeout");
    h.mock.data.lock().unwrap().calls.clear();
    let mut config = h.config();
    config.request_timeout = Duration::from_millis(100);
    let timed = Client::with_token(config, "timeout-synthetic-token".into()).unwrap();
    assert!(matches!(
        timed
            .graphql(
                "query { viewer { login } }",
                json!({}),
                Freshness::Revalidate
            )
            .await,
        Err(Error::Transport(_))
    ));
    let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let failed = text
        .lines()
        .find(|line| {
            line.contains("GitHub request finished") && line.contains("error_code=\"transport\"")
        })
        .unwrap();
    assert!(failed.contains("attempts=2"));
    assert!(failed.contains("error_code=\"transport\""));
    assert!(!failed.contains("http_status="));
    assert_ne!(id(failed), retry_id);
    assert!(!text.contains("timeout-synthetic-token"));
    h.mock.release.notify_waiters();
}

#[tokio::test]
async fn daemon_sdk_preserves_real_upstream_http_status_and_message() {
    let h = Harness::new().await;
    let mut config = h.config();
    config.max_attempts = 1;
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let sdk = hey_gh::ApiClient::new(base.parse().unwrap()).unwrap();
    for status in [302, 403, 503] {
        h.mode(&format!("sdk-upstream-{status}"));
        let direct = c
            .pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap_err();
        assert!(matches!(direct, Error::GitHub { status: actual, .. } if actual == status));
        let remote = sdk
            .pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap_err();
        assert!(matches!(remote, Error::GitHub { status: actual, .. } if actual == status));
        assert_eq!(remote.to_string(), direct.to_string());
    }
    assert!(c.bootstrap().await.unwrap().snapshots.is_empty());
    api.stop().await;
    task.abort();
}

#[tokio::test]
async fn graphql_permission_errors_are_not_misreported_as_http_failures_or_cached() {
    let h = Harness::new().await;
    let c = h.client();
    let query = "query { viewer { login } }";
    for mode in ["graphql-error-forbidden", "graphql-error-mixed"] {
        h.mode(mode);
        let before = h.calls().len();
        let error = c
            .graphql(query, json!({}), Freshness::Revalidate)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("IP allow list denied"));
        assert!(!error.to_string().contains("HTTP 502"));
        assert!(matches!(
            error,
            Error::GraphQL {
                access_denied: true,
                ..
            }
        ));
        if mode == "graphql-error-mixed" {
            assert!(error.to_string().contains("independent operation error"));
        }
        assert_eq!(h.calls().len(), before + 1);
        assert!(matches!(
            c.graphql(query, json!({}), Freshness::CachedOnly).await,
            Err(Error::CacheMiss)
        ));
        assert!(c.bootstrap().await.unwrap().snapshots.is_empty());
    }
    h.mode("graphql-error-http403");
    let before = h.calls().len();
    assert!(matches!(
        c.graphql(query, json!({}), Freshness::Revalidate).await,
        Err(Error::GitHub { status: 403, .. })
    ));
    assert_eq!(h.calls().len(), before + 1);
    h.mode("graphql-error-generic");
    assert!(matches!(
        c.graphql(query, json!({}), Freshness::Revalidate).await,
        Err(Error::GraphQL {
            access_denied: false,
            ..
        })
    ));
}

#[tokio::test]
async fn graph_ql_partial_errors_are_explicit_not_cached_or_published() {
    let h = Harness::new().await;
    let client = h.client();
    let initial = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    h.phase(2);
    h.mode("graphql-errors");
    let incomplete = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(!incomplete.complete);
    assert!(
        incomplete
            .data
            .errors
            .iter()
            .any(|e| e.source == "review_threads")
    );
    assert!(
        incomplete
            .data
            .errors
            .iter()
            .any(|e| e.source == "review_events")
    );
    assert!(incomplete.cursor.is_none());
    let updates = client
        .changes(initial.cursor.as_deref(), 100)
        .await
        .unwrap();
    assert!(
        updates
            .changes
            .iter()
            .any(|c| c.resource.starts_with("ci://"))
    );
    assert!(
        updates
            .changes
            .iter()
            .any(|c| c.resource.starts_with("comments://"))
    );
    assert!(!updates.changes.iter().any(|c|c.resource.starts_with("pr://") || c.resource.starts_with("review_threads://")));
    let query = "query { viewer { login } }";
    assert!(
        client
            .graphql(query, json!({}), Freshness::Revalidate)
            .await
            .is_err()
    );
    assert!(matches!(
        client
            .graphql(query, json!({}), Freshness::CachedOnly)
            .await,
        Err(Error::CacheMiss)
    ));
    assert!(
        client
            .graphql("mutation { dangerous }", json!({}), Freshness::Revalidate)
            .await
            .is_err()
    );
    h.mode("graphql-rate");
    assert!(matches!(
        client
            .graphql(query, json!({}), Freshness::Revalidate)
            .await,
        Err(Error::RateLimited { .. })
    ));
}

#[tokio::test]
async fn immutable_head_is_rechecked_and_long_poll_wakes_on_new_comment() {
    let h = Harness::new().await;
    h.mode("push-during-report");
    let client = h.client();
    let report = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(report.data.ci.head_sha, NEW_HEAD);
    assert!(
        report
            .data
            .ci
            .check_runs
            .iter()
            .all(|r| r["head_sha"] == NEW_HEAD)
    );
    h.mode("");
    let initial = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let waiting = tokio::spawn({
        let c = client.clone();
        let cursor = initial.cursor.unwrap();
        async move {
            c.wait_changes(Some(&cursor), 100, Duration::from_secs(2))
                .await
        }
    });
    h.phase(1);
    client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let changes = waiting.await.unwrap().unwrap();
    assert_eq!(changes.changes[0].changed_fields, vec!["comments"]);
}

#[tokio::test]
async fn daemon_watch_api_is_durable_and_rejects_browser_and_rebinding_requests() {
    let h = Harness::new().await;
    let client = h.client();
    let api = hey_gh::api::Api::new(client.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let http = reqwest::Client::new();
    let response = http
        .post(format!("{url}/v1/watches"))
        .json(&json!({"repository":"acme/demo","pull_number":7,"interval_seconds":10}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let watch: Value = response.json().await.unwrap();
    assert_eq!(client.watches().await.unwrap().len(), 1);
    until(|| {
        h.calls()
            .iter()
            .any(|c| c.path == "/repos/acme/demo/pulls/7")
    })
    .await;
    let browser = http
        .get(format!("{url}/v1/status"))
        .header("Origin", "https://evil.invalid")
        .send()
        .await
        .unwrap();
    assert_eq!(browser.status(), 403);
    let rebound = http
        .get(format!("{url}/v1/status"))
        .header("Host", "evil.invalid")
        .send()
        .await
        .unwrap();
    assert_eq!(rebound.status(), 403);
    let bad = http
        .get(format!("{url}/v1/changes?cursor=bad"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    let bad = http
        .get(format!("{url}/v1/changes?wait_seconds=31"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    api.stop().await;
    let restarted = hey_gh::api::Api::new(client.clone()).await.unwrap();
    assert_eq!(client.watches().await.unwrap().len(), 1);
    restarted.stop().await;
    let deleted = http
        .delete(format!(
            "{url}/v1/watches/{}",
            watch["id"].as_str().unwrap()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    assert!(client.watches().await.unwrap().is_empty());
    task.abort();
}

#[tokio::test]
async fn pagination_and_credentials_cannot_escape_the_configured_origin() {
    let h = Harness::new().await;
    let client = h.client();
    assert!(
        client
            .get("https://evil.invalid/plain", Freshness::Revalidate)
            .await
            .is_err()
    );
    assert!(
        client
            .get("//evil.invalid/plain", Freshness::Revalidate)
            .await
            .is_ok()
    ); // normalized as a path within the same origin
    assert!(
        client
            .get("plain#secret", Freshness::Revalidate)
            .await
            .is_err()
    );
    assert!(
        client
            .pr_report("acme/../../escape", 7, Freshness::Revalidate)
            .await
            .is_err()
    );
    let mut config = h.config();
    config.rest_url = format!("{}api/v3/", h.url).parse().unwrap();
    let prefixed = Client::with_token(config, "synthetic-token".into()).unwrap();
    assert!(
        prefixed
            .get("../../escape", Freshness::Revalidate)
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn authentication_consumes_existing_gh_login_without_printing_or_persisting_token() {
    use std::os::unix::fs::PermissionsExt;
    let h = Harness::new().await;
    let fake = h.dir.path().join("gh");
    std::fs::write(&fake,"#!/bin/sh\n[ \"$1 $2 $3 $4\" = 'auth token --hostname github.com' ] || exit 2\nprintf '%s\\n' 'synthetic-token'\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = h.config();
    config.gh_program = fake;
    let client = Client::from_gh(config).await.unwrap();
    client.get("plain", Freshness::Revalidate).await.unwrap();
    assert_eq!(h.calls()[0].token, "Bearer synthetic-token");
    let mut config = h.config();
    config.gh_program = h.dir.path().join("missing-gh");
    assert!(matches!(Client::from_gh(config).await, Err(Error::Auth(_))));
}

#[tokio::test]
async fn pruned_cursors_require_atomic_bootstrap_and_resume_without_gaps() {
    let h = Harness::new().await;
    let mut config = h.config();
    config.max_change_events = 2;
    let client = Client::with_token(config, "synthetic-token".into()).unwrap();
    let first = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    h.phase(1);
    client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    h.phase(2);
    client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(matches!(
        client.changes(first.cursor.as_deref(), 100).await,
        Err(Error::CursorExpired)
    ));
    assert!(matches!(
        client.changes(None, 100).await,
        Err(Error::CursorExpired)
    ));
    let snapshot = client.bootstrap().await.unwrap();
    assert_eq!(snapshot.snapshots.len(), 10);
    assert_eq!(
        snapshot
            .snapshots
            .iter()
            .find(|s| s.resource.starts_with("ci://"))
            .unwrap()
            .data["summary"]["state"],
        "success"
    );
    assert!(
        client
            .changes(Some(&snapshot.cursor), 100)
            .await
            .unwrap()
            .changes
            .is_empty()
    );
    h.phase(3);
    client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let updates = client.changes(Some(&snapshot.cursor), 100).await.unwrap();
    assert_eq!(updates.changes.len(), 2);
    assert!(
        updates
            .changes
            .iter()
            .any(|c| c.data["conflicts"] == "conflicting")
    );
}

#[tokio::test]
async fn ci_only_path_uses_no_graphql_and_sdk_consumes_the_same_daemon() {
    let h = Harness::new().await;
    h.mode("graphql-rate");
    h.phase(2);
    let client = h.client();
    let api = hey_gh::api::Api::new(client.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (key, registration) = hey_gh::local_auth::register(listener.local_addr().unwrap()).unwrap();
    let router = api.router_with_auth(Some(key));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let anonymous = reqwest::get(format!("{base}v1/status")).await.unwrap();
    assert_eq!(anonymous.status(), 401);
    let sdk = hey_gh::ApiClient::new(base.parse().unwrap()).unwrap();
    let ci = sdk
        .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(ci.complete);
    assert_eq!(ci.data.summary.state, "success");
    assert!(
        h.calls()
            .iter()
            .all(|c| c.path != "/graphql" && !c.path.contains("/comments"))
    );
    let calls = h.calls().len();
    let cached = sdk
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(
        cached
            .validations
            .iter()
            .all(|v| matches!(v.source, Source::Cache))
    );
    assert_eq!(h.calls().len(), calls);
    assert_eq!(sdk.status().await.unwrap().network_requests, calls as u64);
    let baseline = sdk.bootstrap().await.unwrap();
    assert!(
        sdk.changes(Some(&baseline.cursor), 100, Duration::ZERO)
            .await
            .unwrap()
            .changes
            .is_empty()
    );
    assert!(matches!(
        sdk.changes(Some("invalid"), 100, Duration::ZERO).await,
        Err(Error::Invalid(_))
    ));
    let watch = sdk.watch("acme/demo", Some(7), 10).await.unwrap();
    assert_eq!(sdk.watches().await.unwrap().len(), 1);
    sdk.unwatch(&watch.id).await.unwrap();
    assert!(client.watches().await.unwrap().is_empty());
    api.stop().await;
    task.abort();
    drop(registration);
}

#[tokio::test]
async fn ci_batches_independent_sources_but_preserves_single_slot_queue_reads() {
    for capacity in [1, 256] {
        let h = Harness::new().await;
        h.phase(2);
        h.mode("ci-batch-blocked-checks");
        let mut config = h.config();
        config.queue_capacity = capacity;
        config.queue_timeout = Duration::from_secs(5);
        let c = Client::with_token(config, "synthetic-token".into()).unwrap();
        let worker = c.clone();
        let task = tokio::spawn(async move {
            worker
                .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
                .await
        });
        until(|| {
            h.calls()
                .iter()
                .any(|call| call.path == format!("/repos/acme/demo/commits/{HEAD}/check-runs"))
        })
        .await;
        if capacity == 1 {
            assert_eq!(c.status().outstanding_requests, 1);
        } else {
            // Cache lookups may let another source dispatch before checks.
            // While checks are blocked, both siblings must already have been
            // dispatched or occupy pending queue slots, not await checks.
            until(|| {
                let dispatched = h
                    .calls()
                    .iter()
                    .filter(|call| {
                        call.path == format!("/repos/acme/demo/commits/{HEAD}/status")
                            || (call.path == "/repos/acme/demo/actions/runs"
                                && call.query.contains(HEAD))
                    })
                    .count();
                c.status().outstanding_requests + dispatched >= 3
            })
            .await;
        }
        h.mock.release.notify_one();
        let report = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(report.complete, "{:?}", report.data.errors);
        assert_eq!(report.data.summary.state, "success");
        assert_eq!(report.data.head_sha, HEAD);
        assert_eq!(report.data.merge_sha.as_deref(), Some(MERGE));
        assert!(
            report
                .data
                .check_runs
                .iter()
                .any(|check| check["head_sha"] == MERGE)
        );
        assert!(
            report
                .data
                .commit_statuses
                .iter()
                .any(|status| status["observed_sha"] == MERGE)
        );
        assert!(!report.data.jobs.is_empty());
        assert!(
            report
                .validations
                .iter()
                .any(|v| v.resource.contains(MERGE))
        );
        assert!(h.calls().iter().all(|call| call.path != "/graphql"));
    }
}

#[tokio::test]
async fn completed_jobs_reuse_verified_parent_versions_but_refresh_and_reruns_refetch() {
    let h = Harness::new().await;
    h.phase(2);
    let client = h.client();
    let policy = Freshness::MaxAge(Duration::ZERO);
    assert!(
        client
            .ci_for_pr("acme/demo", 7, policy)
            .await
            .unwrap()
            .complete
    );
    let count = || {
        h.calls()
            .iter()
            .filter(|c| c.path.ends_with("/jobs"))
            .count()
    };
    assert_eq!(count(), 1);
    assert!(
        client
            .ci_for_pr("acme/demo", 7, policy)
            .await
            .unwrap()
            .complete
    );
    assert_eq!(count(), 1);
    assert!(
        client
            .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    assert_eq!(count(), 2);
    h.phase(7);
    let rerun = client.ci_for_pr("acme/demo", 7, policy).await.unwrap();
    assert!(rerun.complete, "{:?}", rerun.data.errors);
    assert_eq!(rerun.data.workflow_runs[0]["run_attempt"], 3);
    assert_eq!(count(), 3);
    assert!(
        h.calls()
            .iter()
            .any(|c| c.path.contains("/attempts/3/jobs"))
    );
}

#[tokio::test]
async fn graphql_primary_exhaustion_leaves_ci_and_conversation_updates_available() {
    let h = Harness::new().await;
    let client = h.client();
    let baseline = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    h.phase(2);
    h.mode("graphql-primary");
    let report = client
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(!report.complete);
    assert_eq!(client.status().rate_limits["graphql"].remaining, 0);
    let ci = client
        .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(ci.complete);
    assert_eq!(ci.data.summary.state, "success");
    let changes = client
        .changes(baseline.cursor.as_deref(), 100)
        .await
        .unwrap();
    assert!(
        changes
            .changes
            .iter()
            .any(|c| c.resource.starts_with("ci://"))
    );
    assert!(
        changes
            .changes
            .iter()
            .any(|c| c.resource.starts_with("comments://"))
    );
}

#[tokio::test]
async fn throttle_headers_are_honored_even_if_the_error_body_exceeds_its_limit() {
    let h = Harness::new().await;
    let mut config = h.config();
    config.max_body_bytes = 100;
    let client = Client::with_token(config, "synthetic-token".into()).unwrap();
    assert!(matches!(
        client.get("secondary-large", Freshness::Revalidate).await,
        Err(Error::RateLimited {
            retry_after_seconds: 60
        })
    ));
    assert!(matches!(
        client.get("plain", Freshness::Revalidate).await,
        Err(Error::RateLimited { .. })
    ));
    assert_eq!(h.calls().len(), 1);
}

#[tokio::test]
async fn ready_queue_is_fifo_and_network_deadlines_release_capacity() {
    let h = Harness::new().await;
    let client = h.client();
    let first = tokio::spawn({
        let c = client.clone();
        async move { c.get("slow", Freshness::Revalidate).await }
    });
    until(|| h.calls().len() == 1).await;
    let second = tokio::spawn({
        let c = client.clone();
        async move { c.get("ordered/1", Freshness::Revalidate).await }
    });
    until(|| client.status().outstanding_requests == 2).await;
    let third = tokio::spawn({
        let c = client.clone();
        async move { c.get("ordered/2", Freshness::Revalidate).await }
    });
    until(|| client.status().outstanding_requests == 3).await;
    h.mock.release.notify_one();
    assert!(first.await.unwrap().is_ok());
    assert!(second.await.unwrap().is_ok());
    assert!(third.await.unwrap().is_ok());
    assert_eq!(
        h.calls()
            .iter()
            .map(|c| c.path.as_str())
            .collect::<Vec<_>>(),
        vec!["/slow", "/ordered/1", "/ordered/2"]
    );
    let mut config = h.config();
    config.queue_timeout = Duration::from_millis(25);
    config.max_attempts = 1;
    let short = Client::with_token(config, "other-credential".into()).unwrap();
    assert!(matches!(
        short.get("slow", Freshness::Revalidate).await,
        Err(Error::Deadline)
    ));
    assert_eq!(short.status().outstanding_requests, 0);
    assert!(matches!(
        short.get("slow", Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
}

#[tokio::test]
async fn stalled_graphql_lane_allows_rest_but_keeps_same_bucket_fifo() {
    let h = Harness::new().await;
    h.mode("scheduler-slow-graphql");
    let c = h.client();
    let first = tokio::spawn({
        let c = c.clone();
        async move {
            c.graphql(
                "query First { viewer { login } }",
                json!({}),
                Freshness::Revalidate,
            )
            .await
        }
    });
    until(|| h.calls().len() == 1).await;
    let second = tokio::spawn({
        let c = c.clone();
        async move {
            c.graphql(
                "query Second { viewer { login } }",
                json!({}),
                Freshness::Revalidate,
            )
            .await
        }
    });
    until(|| c.status().outstanding_requests == 2).await;
    let rest = tokio::time::timeout(
        Duration::from_millis(500),
        c.get("plain", Freshness::Revalidate),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(rest.source, Source::Network));
    assert_eq!(
        h.calls()
            .iter()
            .filter(|call| call.path == "/graphql")
            .count(),
        1
    );
    assert_eq!(c.status().outstanding_requests, 2);
    let slow_rest = tokio::spawn({
        let c = c.clone();
        async move { c.get("slow", Freshness::Revalidate).await }
    });
    until(|| h.calls().iter().any(|call| call.path == "/slow") && c.status().active_requests == 2)
        .await;
    assert_eq!(c.status().max_active_requests, 3);
    assert_eq!(c.status().outstanding_requests, 3);
    h.mock.release.notify_waiters();
    assert!(first.await.unwrap().is_ok());
    assert!(slow_rest.await.unwrap().is_ok());
    until(|| {
        h.calls()
            .iter()
            .filter(|call| call.path == "/graphql")
            .count()
            == 2
    })
    .await;
    h.mock.release.notify_one();
    assert!(second.await.unwrap().is_ok());
    assert_eq!(c.status().outstanding_requests, 0);
}

#[tokio::test]
async fn slow_throttle_body_pauses_other_buckets_as_soon_as_headers_arrive() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let release = Arc::new(Notify::new());
    let server = tokio::spawn({
        let release = release.clone();
        async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut bytes = [0; 1024];
                let count = socket.read(&mut bytes).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let reset = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 3600;
            socket.write_all(format!("HTTP/1.1 429 Too Many Requests\r\nContent-Length: 2\r\nRetry-After: 60\r\nX-RateLimit-Resource: core\r\nX-RateLimit-Remaining: 5000\r\nX-RateLimit-Reset: {reset}\r\n\r\n").as_bytes()).await.unwrap();
            release.notified().await;
            socket.write_all(b"{}").await.unwrap();
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        rest_url: origin.parse().unwrap(),
        graphql_url: format!("{origin}graphql").parse().unwrap(),
        cache_path: dir.path().join("cache.sqlite"),
        min_spacing: Duration::ZERO,
        queue_timeout: Duration::from_secs(2),
        max_attempts: 1,
        ..Config::default()
    };
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    let throttle = tokio::spawn({
        let c = c.clone();
        async move { c.get("throttle", Freshness::Revalidate).await }
    });
    until(|| c.status().rate_limits.contains_key("core")).await;
    let blocked = tokio::time::timeout(
        Duration::from_millis(200),
        c.graphql(
            "query { viewer { login } }",
            json!({}),
            Freshness::Revalidate,
        ),
    )
    .await
    .unwrap();
    assert!(matches!(blocked, Err(Error::RateLimited { .. })));
    assert_eq!(
        c.status().network_requests,
        1,
        "no new request after authoritative cooldown headers"
    );
    release.notify_one();
    assert!(matches!(
        throttle.await.unwrap(),
        Err(Error::RateLimited { .. })
    ));
    server.await.unwrap();
    assert_eq!(c.status().outstanding_requests, 0);
}

#[tokio::test]
async fn pagination_cycles_cross_origin_links_and_collections_fail_explicitly() {
    let h = Harness::new().await;
    let client = h.client();
    assert!(
        client
            .pages("page-cycle", None, Freshness::Revalidate)
            .await
            .is_err()
    );
    assert_eq!(h.calls().len(), 1);
    assert!(
        client
            .pages("page-escape", None, Freshness::Revalidate)
            .await
            .is_err()
    );
    assert_eq!(h.calls().len(), 2);
    let mut config = h.config();
    config.max_collection_bytes = 50;
    let bounded = Client::with_token(config, "synthetic-token".into()).unwrap();
    assert!(
        bounded
            .pages("page-large", None, Freshness::Revalidate)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn graphql_accepts_real_read_syntax_and_rejects_writes_and_ambiguous_operations() {
    let h = Harness::new().await;
    let client = h.client();
    assert!(
        client
            .graphql(
                "# comment\nquery Test { repository(owner:\"mutation\",name:\"demo\") { id } }",
                json!({}),
                Freshness::Revalidate
            )
            .await
            .is_ok()
    );
    assert!(
        client
            .graphql(
                "query Test { viewer { ...Person } } fragment Person on User { login }",
                json!({}),
                Freshness::Revalidate
            )
            .await
            .is_ok()
    );
    let calls = h.calls().len();
    for query in [
        "mutation Test { remove }",
        "subscription Test { events }",
        "query One { viewer { login } } query Two { viewer { login } }",
        "fragment Person on User { login }",
    ] {
        assert!(
            client
                .graphql(query, json!({}), Freshness::Revalidate)
                .await
                .is_err()
        );
    }
    assert_eq!(h.calls().len(), calls);
}

#[cfg(unix)]
#[tokio::test]
async fn cli_uses_existing_login_protects_its_api_and_prevents_duplicate_daemons() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncBufReadExt, BufReader};
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("gh");
    std::fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' synthetic-token\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let cache = dir.path().join("cli.sqlite");
    let mut process = tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
        .args(["serve", "--listen", "127.0.0.1:0", "--cache"])
        .arg(&cache)
        .env("PATH", &path)
        .env("RUST_LOG", "warn")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(process.stderr.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), stderr.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let base = line
        .split("listening on ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    let api = hey_gh::ApiClient::new(base.parse().unwrap()).unwrap();
    assert_eq!(api.status().await.unwrap().network_requests, 0);
    assert_eq!(
        reqwest::get(format!("{base}/v1/status"))
            .await
            .unwrap()
            .status(),
        401
    );
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
        .args(["status", "--server", base])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["network_requests"], 0);
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("synthetic-token")
    );
    let duplicate = tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
        .args(["serve", "--listen", "127.0.0.1:0", "--cache"])
        .arg(&cache)
        .env("PATH", &path)
        .output()
        .await
        .unwrap();
    assert!(!duplicate.status.success());
    assert!(
        String::from_utf8(duplicate.stderr)
            .unwrap()
            .contains("another hey-gh daemon")
    );
    let pid = process.id().unwrap().to_string();
    assert!(
        tokio::process::Command::new("kill")
            .args(["-INT", &pid])
            .status()
            .await
            .unwrap()
            .success()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(5), process.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn account_status_reports_all_repositories_and_replays_activity_and_terminal_events() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    assert!(
        c.refresh_pr_status(Freshness::Revalidate, false)
            .await
            .unwrap()
            .is_empty()
    );
    let initial = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(initial.complete);
    assert_eq!(initial.pull_requests.len(), 2);
    assert!(
        h.calls()
            .iter()
            .any(|call| call.body["variables"]["after"] == "PR-next")
    );
    let calls = h.calls().len();
    c.pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(h.calls().len(), calls);
    // Identical conditional revalidations don't generate another status event.
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    assert!(
        c.pr_status_page(None, Some(&initial.cursor), 1000, Duration::ZERO)
            .await
            .unwrap()
            .changes
            .is_empty()
    );
    h.phase(3);
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let activity = c
        .pr_status_page(None, Some(&initial.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(activity.changes.len(), 2);
    assert!(
        activity.changes[0]
            .activity
            .iter()
            .any(|a| a["kind"] == "comment_added" && a["id"] == 2)
    );
    assert!(
        activity.changes[0]
            .activity
            .iter()
            .any(|a| a["kind"] == "ci_changed")
    );
    assert!(
        activity.changes[0]
            .activity
            .iter()
            .any(|a| a["kind"] == "conflicts_changed")
    );
    assert!(
        activity.changes[0]
            .changed_fields
            .contains(&"comments".to_owned())
    );
    h.phase(8);
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let closed = c
        .pr_status_page(None, Some(&activity.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(closed.changes.iter().any(|e| e.kind == "closed"
        && e.pull_request["removed"] == true
        && e.pull_request["closedAt"].is_string()));
    assert_eq!(
        c.pr_status_page(None, None, 1000, Duration::ZERO)
            .await
            .unwrap()
            .pull_requests
            .len(),
        1
    );
    // Cursor and terminal events survive a new client/database connection.
    drop(c);
    let c = h.client();
    h.phase(9);
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let merged = c
        .pr_status_page(None, Some(&closed.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(merged.changes.iter().any(|e| e.kind == "merged"
        && e.pull_request["state"] == "MERGED"
        && e.pull_request["mergedAt"].is_string()));
    let replay = c
        .pr_status_page(None, Some(&closed.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&replay.changes).unwrap(),
        serde_json::to_value(&merged.changes).unwrap()
    );
    assert!(
        c.pr_status_page(None, None, 1000, Duration::ZERO)
            .await
            .unwrap()
            .pull_requests
            .is_empty()
    );
    // A closed PR can reopen, but a merged PR cannot return to the open snapshot.
    h.phase(0);
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let reopened = c
        .pr_status_page(None, Some(&merged.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .changes
            .iter()
            .filter(|e| e.kind == "reopened")
            .count(),
        1
    );
    assert!(
        reopened.changes.iter().any(|e| e.kind == "reopened"
            && e.pull_request["repository"]["nameWithOwner"] == "acme/demo")
    );
    assert!(
        !reopened.changes.iter().any(|e| e.kind == "reopened"
            && e.pull_request["repository"]["nameWithOwner"] == "acme/other")
    );
    let open = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(open.pull_requests.len(), 1);
    assert_eq!(
        open.pull_requests[0]["repository"]["nameWithOwner"],
        "acme/demo"
    );
    let scoped = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(scoped.pull_requests.len(), 1);
    assert!(matches!(
        c.pr_status_page(None, Some(&scoped.cursor), 1000, Duration::ZERO)
            .await,
        Err(Error::Invalid(_))
    ));
}

#[tokio::test]
async fn cold_account_discovery_publishes_pending_prs_before_ci_is_available() {
    let h = Harness::new().await;
    h.mode("account-ci-permission-fails");
    let c = h.client();
    c.save_account_watch(86400).await.unwrap();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let page = c
                .pr_status_page(None, None, 1000, Duration::ZERO)
                .await
                .unwrap();
            if page.pull_requests.len() == 2 {
                assert!(!page.complete);
                assert!(
                    page.pull_requests
                        .iter()
                        .all(|row| row["complete"] == false && row["removed"] == false)
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        h.calls()
            .iter()
            .filter(|call| call.body["query"]
                .as_str()
                .is_some_and(|q| q.contains("MyOpenPullRequests")))
            .count(),
        2,
        "projection reuses the one paginated scan"
    );
    api.stop().await;
}

#[tokio::test]
async fn lightweight_discovery_keeps_missing_checks_explicit_until_ci_hydration() {
    let h = Harness::new().await;
    h.mode("account-discovery-light");
    let c = h.client();
    assert!(
        c.prepare_pr_status(Freshness::Revalidate)
            .await
            .unwrap()
            .is_empty()
    );
    let baseline = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(baseline.pull_requests.len(), 2);
    assert!(
        baseline
            .pull_requests
            .iter()
            .all(|p| p["headCiState"] == "PENDING"
                && p["statusCheckRollupComplete"] == false
                && p["complete"] == false)
    );
    assert!(
        !h.calls()[0].body["query"]
            .as_str()
            .unwrap()
            .contains("contexts(")
    );
    assert!(
        c.refresh_pr_status(Freshness::Revalidate, true)
            .await
            .unwrap()
            .is_empty()
    );
    let hydrated = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        hydrated
            .pull_requests
            .iter()
            .all(|p| p["statusCheckRollupComplete"] == true
                && !p["statusCheckRollup"].as_array().unwrap().is_empty())
    );
}

#[tokio::test]
async fn independent_detail_watch_reuses_ci_without_clearing_ci_failure_health() {
    let h = Harness::new().await;
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let initial = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let before = h.calls().len();
    h.phase(1);
    h.mode("account-ci-permission-fails");
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',0,'$.fetched_at_ms',0) WHERE key LIKE '%repos/%'", []).unwrap();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    api.watch("acme/demo", 7, 10).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/watches", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let watches: Value = http.get(&url).send().await.unwrap().json().await.unwrap();
            if watches[0]["last_success_at_ms"].is_number()
                && watches[0]["ci_last_error"].is_string()
            {
                assert!(watches[0]["last_error"].is_null());
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let page = c
        .pr_status_page(
            Some("acme/demo"),
            Some(&initial.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    let latest = &page.changes.last().unwrap().pull_request;
    assert!(
        latest["comments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|comment| comment["id"] == 2)
    );
    assert!(
        latest["sourceErrors"]["ci"].is_string(),
        "cached combined projection cannot clear a live CI failure"
    );
    assert_eq!(latest["complete"], false);
    let combined = c
        .bootstrap()
        .await
        .unwrap()
        .snapshots
        .into_iter()
        .find(|snapshot| {
            snapshot.resource.ends_with("/acme/demo/7") && snapshot.resource.starts_with("pr://")
        })
        .unwrap();
    assert!(
        combined.data["comments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|comment| comment["id"] == 2)
    );
    assert_eq!(
        h.calls()[before..]
            .iter()
            .filter(|call| call.path.contains("/check-runs"))
            .count(),
        1,
        "only the independent CI loop should refetch checks"
    );
    api.stop().await;
    task.abort();
}

#[tokio::test]
async fn background_discovery_failure_does_not_taint_successful_account_hydration() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    c.save_account_watch(86400).await.unwrap();
    h.mode("account-discovery-errors");
    h.phase(2);
    // Expire only the completed collection's reuse window, retaining its
    // successful contents for independent cached hydration.
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.fetched_at_ms',0) WHERE key='account-discovery-complete:v1'", []).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',0) WHERE json_type(response,'$.data.data.viewer') IS NOT NULL", []).unwrap();
    let calls = h.calls().len();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/watches", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let w: Value = http.get(&url).send().await.unwrap().json().await.unwrap();
            if w[0]["discovery_last_error"].is_string()
                && w[0]["last_success_at_ms"].is_number()
                && w[0]["ci_last_success_at_ms"].is_number()
            {
                assert!(w[0]["last_error"].is_null());
                assert!(w[0]["ci_last_error"].is_null());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        h.calls()[calls..]
            .iter()
            .filter(|call| call.body["query"]
                .as_str()
                .is_some_and(|q| q.contains("MyOpenPullRequests")))
            .count(),
        1,
        "only the dedicated discovery loop scans GitHub"
    );
    let baseline = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let calls_before_feed = h.calls().len();
    let response: Value = http
        .get(url.replace("/v1/watches", "/v1/pr-status"))
        .query(&[
            ("cursor", baseline.cursor.as_str()),
            ("cached_only", "true"),
        ])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["complete"], false);
    assert!(response["errors"].as_array().unwrap().iter().any(|error| {
        error
            .as_str()
            .is_some_and(|message| message.starts_with("discovery:"))
    }));
    assert_eq!(response["cursor"], baseline.cursor);
    assert!(response["changes"].as_array().unwrap().is_empty());
    assert_eq!(
        h.calls().len(),
        calls_before_feed,
        "reporting discovery health spends no quota"
    );
    api.stop().await;
    task.abort();
}

#[tokio::test]
async fn account_discovery_shares_complete_scans_and_retains_last_good_on_failure() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    let freshness = Freshness::MaxAge(Duration::from_secs(60));
    let (a, b) = tokio::join!(
        c.all_my_open_pull_requests(freshness),
        c.all_my_open_pull_requests(freshness)
    );
    assert_eq!(a.unwrap().len(), 2);
    assert_eq!(b.unwrap().len(), 2);
    assert_eq!(
        h.calls().len(),
        2,
        "one paginated scan, shared by both callers"
    );
    assert!(
        h.calls()[0].body["query"]
            .as_str()
            .unwrap()
            .contains("first: 100")
    );

    // A reconstructed client reuses the durable collection without requests.
    let restarted = h.client();
    assert_eq!(
        restarted
            .all_my_open_pull_requests(freshness)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(h.calls().len(), 2);
    h.mode("account-discovery-errors");
    assert!(
        restarted
            .all_my_open_pull_requests(Freshness::Revalidate)
            .await
            .is_err()
    );
    let calls = h.calls().len();
    assert_eq!(
        restarted
            .all_my_open_pull_requests(Freshness::CachedOnly)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(h.calls().len(), calls);

    // An explicit refresh bypasses the collection cache, observes closures,
    // and replaces it only after the entire scan succeeds.
    h.mode("account");
    h.phase(9);
    assert!(
        restarted
            .all_my_open_pull_requests(Freshness::Revalidate)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        restarted
            .all_my_open_pull_requests(freshness)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn discovery_deadline_is_durable_and_cached_collection_reuse_does_not_clear_it() {
    let h = Harness::new().await;
    h.mode("account");
    h.phase(2);
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let cursor = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap()
        .cursor;
    drop(c);
    let mut config = h.config();
    config.report_timeout = Duration::from_millis(100);
    config.request_timeout = Duration::from_millis(500);
    config.max_attempts = 1;
    h.mode("account-discovery-stalled");
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    assert!(matches!(
        c.all_my_open_pull_requests(Freshness::Revalidate).await,
        Err(Error::Deadline)
    ));
    drop(c);
    let c = h.client();
    let before = h.calls().len();
    assert_eq!(
        c.all_my_open_pull_requests(Freshness::CachedOnly)
            .await
            .unwrap()
            .len(),
        2
    );
    // Even MaxAge reuse of a recent last-good collection cannot prove recovery.
    c.all_my_open_pull_requests(Freshness::MaxAge(Duration::from_secs(30)))
        .await
        .unwrap();
    let feed = c
        .pr_status_page(None, Some(&cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(!feed.complete);
    assert!(
        feed.errors
            .iter()
            .any(|error| error == "discovery: request deadline exceeded")
    );
    assert_eq!(feed.cursor, cursor);
    assert!(feed.changes.is_empty());
    assert_eq!(h.calls().len(), before);
    h.mode("account");
    h.mock.release.notify_waiters();
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    let recovered = c
        .pr_status_page(None, Some(&cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(recovered.complete);
    assert!(recovered.errors.is_empty());
    assert_eq!(recovered.cursor, cursor);
}

#[tokio::test]
async fn discovery_failure_survives_restart_and_cached_reads_until_a_validated_scan_recovers() {
    let h = Harness::new().await;
    h.mode("account");
    h.phase(2);
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    c.save_account_watch(86400).await.unwrap();
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.fetched_at_ms',0) WHERE key='account-discovery-complete:v1'", []).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.validated_at_ms',0) WHERE json_type(response,'$.data.data.viewer') IS NOT NULL", []).unwrap();
    h.mode("account-discovery-errors");
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status: Value = http
                .get(format!("{base}/v1/watches"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if status[0]["discovery_last_error"].is_string()
                && status[0]["last_success_at_ms"].is_number()
                && status[0]["ci_last_success_at_ms"].is_number()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let cursor = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap()
        .cursor;
    api.stop().await;
    task.abort();
    drop(api);
    drop(c);
    h.mode("account-discovery-stalled");
    let before_restart = h.calls().len();
    let c = h.client();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    until(|| {
        h.calls()[before_restart..].iter().any(|call| {
            call.body["query"]
                .as_str()
                .is_some_and(|q| q.contains("MyOpenPullRequests"))
        })
    })
    .await;
    // Let the independent CI/detail startup polls finish before measuring
    // requests from cached reads; the discovery request remains stalled.
    let status: Value = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status: Value = http
                .get(format!("{base}/v1/watches"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if status[0]["last_success_at_ms"].is_number()
                && status[0]["ci_last_success_at_ms"].is_number()
            {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        status[0]["discovery_last_error"].is_string(),
        "restart must retain known failure while the new request is still pending"
    );
    let cached_before = h.calls().len();
    c.all_my_open_pull_requests(Freshness::CachedOnly)
        .await
        .unwrap();
    let feed = c
        .pr_status_page(None, Some(&cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(!feed.complete);
    assert!(
        feed.errors
            .iter()
            .any(|error| error.starts_with("discovery:"))
    );
    let response: Value = http
        .get(format!("{base}/v1/pr-status"))
        .query(&[("cursor", cursor.as_str()), ("cached_only", "true")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["complete"], false);
    assert!(response["errors"].as_array().unwrap().iter().any(|error| {
        error
            .as_str()
            .is_some_and(|message| message.starts_with("discovery:"))
    }));
    assert_eq!(response["cursor"], cursor);
    assert!(response["changes"].as_array().unwrap().is_empty());
    assert_eq!(
        h.calls().len(),
        cached_before,
        "cached health reporting must spend no GitHub quota"
    );
    h.mode("account");
    h.mock.release.notify_waiters();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status: Value = http
                .get(format!("{base}/v1/watches"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if status[0]["discovery_last_error"].is_null()
                && status[0]["discovery_last_success_at_ms"].is_number()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let recovered = c
        .pr_status_page(None, Some(&cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(recovered.complete);
    assert!(recovered.errors.is_empty());
    assert_eq!(recovered.cursor, cursor);
    api.stop().await;
    task.abort();
}

#[tokio::test]
async fn account_discovery_failure_preserves_known_prs_and_does_not_stop_rest_ci() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let initial = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.mode("account-discovery-errors");
    h.phase(2);
    let errors = c
        .refresh_pr_status(Freshness::Revalidate, true)
        .await
        .unwrap();
    assert!(errors.iter().any(|e| e.starts_with("discovery:")));
    let delta = c
        .pr_status_page(None, Some(&initial.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(delta.changes.len(), 2);
    assert!(
        delta
            .changes
            .iter()
            .all(|e| e.pull_request["ci"]["summary"]["state"] == "success"
                && e.pull_request["removed"] == false)
    );
    assert_eq!(
        c.pr_status_page(None, None, 1000, Duration::ZERO)
            .await
            .unwrap()
            .pull_requests
            .len(),
        2
    );
    h.mode("account-badpage");
    h.phase(0);
    assert!(matches!(
        c.all_my_open_pull_requests(Freshness::Revalidate).await,
        Err(Error::Invalid(_))
    ));
}

#[tokio::test]
async fn rest_discovery_adds_verified_prs_without_clearing_errors_or_inventing_closures() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    // Exercise a real independent cache failure before discovery fallback.
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute(
        "DELETE FROM cache WHERE key LIKE '%/repos/acme/demo/commits/%/check-runs%'",
        [],
    )
    .unwrap();
    assert!(
        !c.ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap()
            .complete
    );
    let baseline = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let prior = baseline
        .pull_requests
        .iter()
        .find(|x| x["repository"]["nameWithOwner"] == "acme/demo")
        .unwrap()["sourceErrors"]["ci"]
        .clone();
    assert!(!prior.is_null());
    h.mode("account-fallback");
    let call_start = h.calls().len();
    let errors = c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    assert!(errors.iter().any(|e| e.starts_with("discovery:")));
    let page = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(page.pull_requests.len(), 4);
    assert!(!page.complete);
    assert_eq!(page.errors.len(), 1);
    assert!(page.pull_requests.iter().all(|x| x["removed"] == false));
    let old = page
        .pull_requests
        .iter()
        .find(|x| x["repository"]["nameWithOwner"] == "acme/demo")
        .unwrap();
    assert_eq!(old["sourceErrors"]["ci"], prior);
    assert!(
        page.pull_requests
            .iter()
            .any(|x| x["repository"]["nameWithOwner"] == "acme/other")
    );
    for number in [11, 15] {
        let added = page
            .pull_requests
            .iter()
            .find(|x| x["number"] == number)
            .unwrap();
        assert!(added["ci"].is_null());
        assert!(added["headCiState"].is_null());
        assert_eq!(added["complete"], false);
    }
    let delta = c
        .pr_status_page(None, Some(&baseline.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        delta.changes.iter().filter(|x| x.kind == "opened").count(),
        2
    );
    let calls = h.calls();
    let probes: Vec<_> = calls[call_start..]
        .iter()
        .filter(|x| {
            x.path.starts_with("/repos/acme/fresh/pulls/")
                || x.path.starts_with("/repos/acme/extra/pulls/")
        })
        .collect();
    assert_eq!(probes.len(), 4); // fifth attempt rejects origin before sending.
    assert!(
        probes
            .iter()
            .all(|x| !x.path.ends_with("/14") && !x.path.ends_with("/16"))
    );
    assert!(matches!(
        c.all_my_open_pull_requests(Freshness::Revalidate).await,
        Err(Error::GraphQL {
            access_denied: true,
            ..
        })
    ));
    // Restart and cached projection retain positives without GitHub calls.
    let restarted = h.client();
    let before = h.calls().len();
    restarted
        .prepare_pr_status(Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(h.calls().len(), before);
    assert_eq!(
        restarted
            .pr_status_page(None, None, 1000, Duration::ZERO)
            .await
            .unwrap()
            .pull_requests
            .len(),
        4
    );
    assert_eq!(
        restarted
            .all_my_open_pull_requests(Freshness::CachedOnly)
            .await
            .unwrap()
            .len(),
        2,
        "the complete-collection SDK never returns the partial overlay"
    );
    let graph_clock:u64=db.query_row("SELECT COALESCE(MAX(validated_at_ms),0) FROM snapshot_validation WHERE resource='pr-status://github.com/acme/fresh/11#discovery'",[],|row|row.get(0)).unwrap();
    assert_eq!(
        graph_clock, 0,
        "REST additions cannot inherit a GraphQL validation clock"
    );
    // Actual complete scan retires additions. Confirm closures through REST.
    h.mode("account-fallback-recovered");
    h.phase(2);
    restarted
        .prepare_pr_status(Freshness::Revalidate)
        .await
        .unwrap();
    let recovered = restarted
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(recovered.errors.is_empty());
    assert_eq!(recovered.pull_requests.len(), 2);
    let terminal = restarted
        .pr_status_page(None, Some(&page.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        terminal
            .changes
            .iter()
            .filter(|x| x.kind == "closed")
            .count(),
        2
    );
}

#[tokio::test]
async fn rest_discovery_cold_cache_retains_validated_additions_when_a_later_probe_stalls() {
    let h = Harness::new().await;
    h.mode("account-fallback-stall");
    let mut config = h.config();
    config.report_timeout = Duration::from_millis(300);
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    let start = std::time::Instant::now();
    let errors = c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(errors.iter().any(|e| e.starts_with("discovery:")));
    let page = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(page.pull_requests.len(), 1);
    assert_eq!(page.pull_requests[0]["number"], 11);
    assert!(!page.complete);
    assert_eq!(page.errors.len(), 1);
    assert!(matches!(
        c.all_my_open_pull_requests(Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
    let calls = h.calls().len();
    c.prepare_pr_status(Freshness::CachedOnly).await.unwrap();
    assert_eq!(h.calls().len(), calls);
    assert_eq!(
        c.pr_status_page(None, None, 1000, Duration::ZERO)
            .await
            .unwrap()
            .pull_requests
            .len(),
        1
    );
    h.mock.release.notify_waiters();
}

#[tokio::test]
async fn rest_discovery_permission_cooldown_prevents_a_denied_prefix_from_starving_other_prs() {
    let h = Harness::new().await;
    h.mode("account-fallback-denied-prefix");
    let c = h.client();
    c.prepare_pr_status(Freshness::MaxAge(Duration::ZERO))
        .await
        .unwrap();
    assert!(
        c.pr_status_page(None, None, 1000, Duration::ZERO)
            .await
            .unwrap()
            .pull_requests
            .is_empty()
    );
    // Simulate cooldowns written by a version using mixed-case search URLs.
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    let stored: String = db
        .query_row(
            "SELECT response FROM cache WHERE key='account-discovery-candidate-denials:v1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut stored: Value = serde_json::from_str(&stored).unwrap();
    let entries = stored["data"].as_object().unwrap();
    let mut legacy = serde_json::Map::new();
    for (key, value) in entries {
        legacy.insert(key.to_ascii_uppercase(), value.clone());
        // An older alias must not shorten the newer denial's cooldown.
        legacy.insert(key.clone(), json!(0));
    }
    stored["data"] = Value::Object(legacy);
    db.execute(
        "UPDATE cache SET response=?1 WHERE key='account-discovery-candidate-denials:v1'",
        [stored.to_string()],
    )
    .unwrap();
    let before = h.calls().len();
    // The same incomplete search still starts with five denied PRs. Background
    // retries skip only those known denials and validate the next five.
    c.prepare_pr_status(Freshness::MaxAge(Duration::ZERO))
        .await
        .unwrap();
    let page = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(page.pull_requests.len(), 5);
    assert_eq!(page.errors.len(), 1);
    assert!(!page.complete);
    let calls = h.calls();
    let probes: Vec<_> = calls[before..]
        .iter()
        .filter(|x| x.path.starts_with("/repos/acme/fresh/pulls/"))
        .collect();
    assert_eq!(probes.len(), 5);
    assert!(
        probes
            .iter()
            .all(|x| x.path.rsplit('/').next().unwrap().parse::<u64>().unwrap() >= 16)
    );
    // Explicit refresh bypasses the private cooldown to probe changed access.
    let before = h.calls().len();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    assert_eq!(
        h.calls()[before..]
            .iter()
            .filter(|x| x.path.starts_with("/repos/acme/fresh/pulls/"))
            .count(),
        5
    );
}

#[tokio::test]
async fn transport_projection_retains_failed_ci_health_and_cli_failure_exit() {
    let h = Harness::new().await;
    h.mode("account");
    let client = h.client();
    client
        .refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.mode("account-ci-permission-fails");
    client
        .refresh_pr_status(Freshness::Revalidate, true)
        .await
        .unwrap();
    let api = hey_gh::api::Api::new(client.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, api.router()).await.unwrap() });
    let transport = hey_gh::ApiClient::new(url::Url::parse(&base).unwrap()).unwrap();
    let calls = h.calls().len();
    let page = transport
        .pr_status_selected(
            hey_gh::PrStatusSelection {
                fields: Some(&["number"]),
                ..Default::default()
            },
            1000,
            Duration::ZERO,
            Freshness::CachedOnly,
        )
        .await
        .unwrap();
    assert!(!page.complete);
    assert!(
        page.pull_requests
            .iter()
            .any(|row| row["sourceErrors"]["ci"].is_string())
    );
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
        .args(["pr", "--cached-only", "--json", "number", "--server", &base])
        .output()
        .await
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "projection cannot conceal failed evidence"
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["complete"], false);
    assert!(
        value["pullRequests"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.as_object().unwrap().len() == 1)
    );
    assert_eq!(
        h.calls().len(),
        calls,
        "transport projection must consume only cached observations"
    );
    task.abort();
}

#[tokio::test]
async fn account_cli_supports_gh_style_commands_projections_and_cursor_shorthand() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let invoke = |arguments: Vec<String>| {
        let base = base.clone();
        async move {
            tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
                .args(arguments)
                .args(["--server", &base])
                .output()
                .await
                .unwrap()
        }
    };
    let output = invoke(vec!["pr".into(), "--cached-only".into()]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let page: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(page["pullRequests"].as_array().unwrap().len(), 2);
    let transport = hey_gh::ApiClient::new(url::Url::parse(&base).unwrap()).unwrap();
    let selected = transport
        .pr_status_selected(
            hey_gh::PrStatusSelection {
                fields: Some(&["number", "complete", "sourceErrors", "number"]),
                ..Default::default()
            },
            1000,
            Duration::ZERO,
            Freshness::CachedOnly,
        )
        .await
        .unwrap();
    assert_eq!(selected.cursor, page["cursor"]);
    assert_eq!(selected.complete, page["complete"]);
    assert_eq!(
        selected.errors,
        serde_json::from_value::<Vec<String>>(page["errors"].clone()).unwrap()
    );
    for (row, full) in selected
        .pull_requests
        .iter()
        .zip(page["pullRequests"].as_array().unwrap())
    {
        assert_eq!(row.as_object().unwrap().len(), 3);
        for field in ["number", "complete", "sourceErrors"] {
            assert_eq!(row[field], full[field]);
        }
    }
    let calls = h.calls().len();
    let before_watches = c.watches().await.unwrap();
    let invalid = reqwest::Client::new()
        .get(format!("{base}v1/pr-status"))
        .query(&[("fields", "number,unknown"), ("refresh", "true")])
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        h.calls().len(),
        calls,
        "invalid projection cannot spend GitHub requests"
    );
    assert_eq!(c.watches().await.unwrap().len(), before_watches.len());
    let output = invoke(vec![
        "pr".into(),
        "list".into(),
        "-R".into(),
        "acme/demo".into(),
        "--json".into(),
        "id,number,title,ci".into(),
        "--cached-only".into(),
    ])
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let filtered: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(filtered["pullRequests"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["pullRequests"][0].as_object().unwrap().len(), 4);
    assert_eq!(filtered["pullRequests"][0]["id"], "PR_acme/demo_7");
    assert_eq!(h.calls().len(), calls);
    h.phase(8);
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let output = invoke(vec![
        "--cursor".into(),
        page["cursor"].as_str().unwrap().into(),
        "pr".into(),
        "--cached-only".into(),
    ])
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let delta: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        delta["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "closed")
    );
    let projected_delta = transport
        .pr_status_selected(
            hey_gh::PrStatusSelection {
                cursor: page["cursor"].as_str(),
                fields: Some(&["id", "state", "removed"]),
                ..Default::default()
            },
            1000,
            Duration::ZERO,
            Freshness::CachedOnly,
        )
        .await
        .unwrap();
    assert_eq!(projected_delta.cursor, delta["cursor"]);
    assert_eq!(projected_delta.has_more, delta["hasMore"]);
    assert_eq!(
        projected_delta.changes.len(),
        delta["changes"].as_array().unwrap().len()
    );
    for (change, full) in projected_delta
        .changes
        .iter()
        .zip(delta["changes"].as_array().unwrap())
    {
        assert_eq!(change.kind, full["kind"]);
        assert_eq!(change.cursor, full["cursor"]);
        assert_eq!(change.observed_at_ms, full["observedAtMs"]);
        assert_eq!(
            change.activity,
            serde_json::from_value::<Vec<Value>>(full["activity"].clone()).unwrap()
        );
        assert_eq!(
            change.changed_fields,
            serde_json::from_value::<Vec<String>>(full["changedFields"].clone()).unwrap()
        );
        assert_eq!(change.pull_request.as_object().unwrap().len(), 5);
        for field in ["id", "state", "removed", "complete", "sourceErrors"] {
            assert_eq!(change.pull_request[field], full["pullRequest"][field]);
        }
    }
    let full_open = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let selected_open = transport
        .pr_status_selected(
            hey_gh::PrStatusSelection {
                fields: Some(&["number"]),
                ..Default::default()
            },
            1000,
            Duration::ZERO,
            Freshness::CachedOnly,
        )
        .await
        .unwrap();
    assert_eq!(
        selected_open.pull_requests.len(),
        full_open.pull_requests.len(),
        "projection must retain internal lifecycle fields for bootstrap filtering"
    );
    assert_eq!(selected_open.cursor, full_open.cursor);
    for (a, b) in selected_open
        .pull_requests
        .iter()
        .zip(&full_open.pull_requests)
    {
        for field in ["number", "complete", "sourceErrors"] {
            assert_eq!(a[field], b[field]);
        }
    }
    // Bare hey-gh and top-level --cursor select the dashboard by default.
    let output = invoke(vec![
        "--cursor".into(),
        page["cursor"].as_str().unwrap().into(),
    ])
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for arguments in [
        vec!["pr", "view", "7", "-R", "acme/demo", "--cached-only"],
        vec!["pr", "checks", "7", "-R", "acme/demo", "--cached-only"],
        vec!["pr", "acme/demo", "7", "--cached-only"],
    ] {
        let output = invoke(arguments.into_iter().map(str::to_owned).collect()).await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = invoke(vec!["pr".into(), "--json".into(), "typo".into()]).await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown PR JSON field"));
    let output = invoke(
        vec![
            "pr",
            "view",
            "7",
            "-R",
            "acme/demo",
            "--json",
            "id,number,title,ci",
            "--cached-only",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let view: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(view["id"], "PR_acme/demo_7");
    assert_eq!(view["number"], 7);
    assert!(view["ci"]["summary"].is_object());
    assert!(view["observedAtMs"].is_number());
    assert!(view["oldestValidationAtMs"].is_number());
    assert!(!view["validations"].as_array().unwrap().is_empty());
    api.stop().await;
    task.abort();
}

#[tokio::test]
async fn invalid_account_page_limits_do_not_fetch_or_register_monitoring() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::new();
    let cursor = c.bootstrap().await.unwrap().cursor;
    for query in [
        "limit=0&refresh=true",
        "limit=1001",
        "limit=0&cached_only=true",
    ] {
        let response = http
            .get(format!("{base}v1/pr-status?{query}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error: Value = response.json().await.unwrap();
        assert_eq!(error["code"], "invalid");
        assert!(
            h.calls().is_empty(),
            "invalid pagination must not spend GitHub requests: {query}"
        );
        assert!(
            c.watches().await.unwrap().is_empty(),
            "invalid pagination must not register durable polling: {query}"
        );
        assert_eq!(
            c.bootstrap().await.unwrap().cursor,
            cursor,
            "invalid pagination must not publish observations"
        );
        assert!(
            c.pr_status_page(None, None, 1000, Duration::ZERO)
                .await
                .unwrap()
                .pull_requests
                .is_empty()
        );
    }
    api.stop().await;
    task.abort();
}

#[tokio::test]
async fn invalid_account_cursors_do_not_fetch_register_or_publish_before_refresh() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let before = c.bootstrap().await.unwrap().cursor;
    let cursor = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap()
        .cursor;
    let prefix = cursor.rsplit_once('.').unwrap().0;
    let selected = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap()
        .cursor;
    let foreign = Client::with_token(h.config(), "other-synthetic-token".into()).unwrap();
    let foreign_cursor = foreign
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap()
        .cursor;
    let http = reqwest::Client::new();
    for invalid in [
        "invalid".to_owned(),
        before.clone(),
        selected,
        foreign_cursor,
        format!("{prefix}.1"),
        format!("{prefix}.9223372036854775808"),
    ] {
        let response = http
            .get(format!("{base}v1/pr-status"))
            .query(&[("cursor", invalid.as_str()), ("refresh", "true")])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error: Value = response.json().await.unwrap();
        assert_eq!(error["code"], "invalid");
        assert!(
            h.calls().is_empty(),
            "invalid cursors must not spend GitHub requests"
        );
        assert!(
            c.watches().await.unwrap().is_empty(),
            "invalid cursors must not register durable polling"
        );
        assert_eq!(
            c.bootstrap().await.unwrap().cursor,
            before,
            "invalid cursors must not publish observations"
        );
    }
    api.stop().await;
    task.abort();
}

#[tokio::test]
async fn account_cursor_pages_long_poll_expiry_and_watch_restart_are_safe() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let initial = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.phase(1);
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let mut cursor = initial.cursor.clone();
    let mut changes = vec![];
    for _ in 0..100 {
        let page = c
            .pr_status_page(None, Some(&cursor), 1, Duration::ZERO)
            .await
            .unwrap();
        changes.extend(page.changes);
        cursor = page.cursor;
        if !page.has_more {
            break;
        }
    }
    assert_eq!(changes.len(), 2);
    assert!(
        c.pr_status_page(None, Some(&cursor), 1, Duration::ZERO)
            .await
            .unwrap()
            .changes
            .is_empty()
    );
    let waiting = c.clone();
    let waiting_cursor = cursor.clone();
    let task = tokio::spawn(async move {
        waiting
            .pr_status_page(None, Some(&waiting_cursor), 1000, Duration::from_secs(2))
            .await
            .unwrap()
    });
    h.phase(2);
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let page = task.await.unwrap();
    assert!(!page.changes.is_empty());
    // A persisted account watch restarts with independent CI/detail loops.
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let watch = api.watch_account(10).await.unwrap();
    assert_eq!(watch.kind, hey_gh::WatchKind::Account);
    assert!(c.watches().await.unwrap().iter().any(|w| w.id == watch.id));
    api.stop().await;
    h.phase(8);
    let restarted = hey_gh::api::Api::new(c.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if c.pr_status_page(None, None, 1000, Duration::ZERO)
                .await
                .unwrap()
                .pull_requests
                .len()
                == 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    restarted.stop().await;
    c.delete_watch(&watch.id).await.unwrap();
    // The wrapper retains the underlying cursor's pruning/recovery guarantees.
    let mut config = h.config();
    config.cache_path = h.dir.path().join("short-feed.sqlite");
    config.max_change_events = 1;
    let short = Client::with_token(config, "synthetic-token".into()).unwrap();
    h.phase(0);
    short
        .refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let baseline = short
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.phase(2);
    short
        .refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    assert!(matches!(
        short
            .pr_status_page(None, Some(&baseline.cursor), 1000, Duration::ZERO)
            .await,
        Err(Error::CursorExpired)
    ));
    let recovered = short
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(recovered.pull_requests.len(), 2);
    assert!(
        short
            .pr_status_page(None, Some(&recovered.cursor), 1000, Duration::ZERO)
            .await
            .unwrap()
            .changes
            .is_empty()
    );
}

#[tokio::test]
async fn account_cursor_reads_do_not_trigger_discovery_but_explicit_refresh_does() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let watch = c.save_account_watch(86400).await.unwrap();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let statuses: Value = http
                .get(format!("{base}/v1/watches"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if statuses[0]["last_success_at_ms"].is_number()
                && statuses[0]["ci_last_success_at_ms"].is_number()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let initial = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.mode("account-discovery-errors");
    let calls = h.calls().len();
    for _ in 0..3 {
        let page: Value = http
            .get(format!("{base}/v1/pr-status"))
            .query(&[
                ("cursor", initial.cursor.as_str()),
                ("max_age_seconds", "0"),
            ])
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(page["errors"], json!([]));
        assert_eq!(page["changes"], json!([]));
        assert_eq!(page["cursor"], initial.cursor);
        assert_eq!(h.calls().len(), calls);
        let statuses: Value = http
            .get(format!("{base}/v1/watches"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        for field in ["last_cycle", "ci_last_cycle"] {
            assert_eq!(statuses[0][field]["total"], 2);
            assert_eq!(statuses[0][field]["succeeded"], 2);
            assert_eq!(statuses[0][field]["failed"], 0);
            assert_eq!(statuses[0][field]["interrupted"], 0);
            assert_eq!(statuses[0][field]["deferred"], 0);
        }
        assert_eq!(h.calls().len(), calls);
    }
    let refreshed: Value = http
        .get(format!("{base}/v1/pr-status"))
        .query(&[("cursor", initial.cursor.as_str()), ("refresh", "true")])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!refreshed["errors"].as_array().unwrap().is_empty());
    assert!(h.calls().len() > calls);
    api.stop().await;
    c.delete_watch(&watch.id).await.unwrap();
    task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn cli_sigterm_bounds_shutdown_with_an_active_long_poll_and_keeps_logs() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncBufReadExt, BufReader};
    let root = tempfile::tempdir().unwrap();
    let fake = root.path().join("gh");
    std::fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' synthetic-token\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let log_directory = root.path().join("logs");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
        .args(["serve", "--listen", &address.to_string(), "--cache"])
        .arg(root.path().join("cache.sqlite"))
        .arg("--log-dir")
        .arg(&log_directory)
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.path().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env_remove("RUST_LOG")
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap()).lines();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(line) = stderr.next_line().await.unwrap() {
            if line.contains("hey-gh API listening") {
                return;
            }
        }
        panic!("daemon exited before becoming ready");
    })
    .await
    .unwrap();
    let sdk = hey_gh::ApiClient::new(format!("http://{address}").parse().unwrap()).unwrap();
    let baseline = sdk.bootstrap().await.unwrap();
    let waiting = tokio::spawn(async move {
        sdk.changes(Some(&baseline.cursor), 100, Duration::from_secs(30))
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!waiting.is_finished());
    let signal = tokio::process::Command::new("kill")
        .args(["-TERM", &child.id().unwrap().to_string()])
        .status()
        .await
        .unwrap();
    assert!(signal.success());
    let status = tokio::time::timeout(Duration::from_secs(8), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    waiting.abort();
    let logs = std::fs::read_to_string(log_directory.join("hey-gh.log")).unwrap();
    assert!(logs.contains("daemon shutdown requested"));
    assert!(logs.contains("graceful shutdown budget elapsed"));
    assert!(logs.contains("daemon stopped"));
    assert!(!logs.contains("synthetic-token"));
}

#[tokio::test]
async fn account_slow_policy_does_not_mark_successfully_observed_ci_as_failed() {
    let h = Harness::new().await;
    h.mode("account-slow-policy");
    h.phase(2);
    let mut config = h.config();
    config.report_timeout = Duration::from_millis(150);
    config.request_timeout = Duration::from_millis(500);
    config.max_attempts = 1;
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let errors = c
        .refresh_pr_status(Freshness::Revalidate, true)
        .await
        .unwrap();
    assert!(
        errors
            .iter()
            .all(|error| !error.starts_with("acme/demo#7:"))
    );
    let page = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let first = page
        .pull_requests
        .iter()
        .find(|row| row["repository"]["nameWithOwner"] == "acme/demo")
        .unwrap();
    assert_eq!(first["ci"]["summary"]["state"], "success");
    assert!(first["sourceErrors"]["ci"].is_null());
    assert!(!first["complete"].as_bool().unwrap());
    h.mock.release.notify_waiters();
}

#[tokio::test]
async fn account_monitor_covers_owned_pr_watches_but_not_other_authors() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    c.save_account_watch(60).await.unwrap();
    let owned = c.save_watch("ACME/DEMO", 7, 60).await.unwrap();
    let other = c.save_watch("acme/demo", 8, 60).await.unwrap();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let statuses: Value = http
                .get(format!("{base}/v1/watches"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let rows = statuses.as_array().unwrap();
            let owned_row = rows.iter().find(|row| row["id"] == owned.id).unwrap();
            let other_row = rows.iter().find(|row| row["id"] == other.id).unwrap();
            assert_eq!(other_row["covered_by_account"], false);
            if owned_row["covered_by_account"] == true
                && !owned_row["last_success_at_ms"].is_null()
                && h.calls()
                    .iter()
                    .any(|call| call.path == "/repos/acme/demo/pulls/8")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        c.watches()
            .await
            .unwrap()
            .iter()
            .any(|watch| watch.id == owned.id)
    );
    api.stop().await;
    task.abort();
}

#[tokio::test]
async fn account_background_details_collect_reviews_even_when_ci_is_incomplete() {
    let h = Harness::new().await;
    h.mode("account-ci-permission-fails");
    h.phase(1);
    let c = h.client();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    api.watch_account(60).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let page = c
                .pr_status_page(None, None, 1000, Duration::ZERO)
                .await
                .unwrap();
            if page.pull_requests.iter().all(|row| {
                row["reviewThreads"].is_array()
                    && row["reviewStatus"].is_object()
                    && row["comments"]
                        .as_array()
                        .is_some_and(|comments| comments.len() == 2)
            }) {
                assert!(!page.complete);
                assert!(page.pull_requests.iter().all(|row| row["ci"].is_null()));
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    api.stop().await;
}

#[tokio::test]
async fn account_detail_timeout_preserves_new_comments_and_last_good_threads() {
    for mode in ["account-slow-threads", "account-slow-reviews"] {
        let h = Harness::new().await;
        h.mode("account");
        let mut config = h.config();
        // Leave time for the healthy sources to publish before the deliberately
        // stalled source expires, including on a busy CI runner.
        config.report_timeout = Duration::from_secs(2);
        config.request_timeout = Duration::from_secs(4);
        config.max_attempts = 1;
        let c = Client::with_token(config, "synthetic-token".into()).unwrap();
        c.refresh_pr_status(Freshness::Revalidate, false)
            .await
            .unwrap();
        let initial = c
            .pr_status_page(None, None, 1000, Duration::ZERO)
            .await
            .unwrap();
        let old_threads = initial
            .pull_requests
            .iter()
            .find(|row| row["repository"]["nameWithOwner"] == "acme/demo")
            .unwrap()["reviewThreads"]
            .clone();
        // Expire network cache validators without removing last-good source snapshots.
        rusqlite::Connection::open(h.config().cache_path)
            .unwrap()
            .execute("DELETE FROM cache", [])
            .unwrap();
        h.mode(mode);
        h.phase(4);
        let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
        api.watch_account(60).await.unwrap();
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                let page = c
                    .pr_status_page(None, None, 1000, Duration::ZERO)
                    .await
                    .unwrap();
                let row = page
                    .pull_requests
                    .iter()
                    .find(|row| row["repository"]["nameWithOwner"] == "acme/demo")
                    .unwrap();
                if row["comments"]
                    .as_array()
                    .is_some_and(|comments| comments.len() == 2)
                    && row["sourceErrors"]["details"].is_string()
                {
                    assert_eq!(row["reviewThreads"], old_threads);
                    assert!(!row["complete"].as_bool().unwrap());
                    assert!(row["sourceErrors"]["details"].is_string());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        api.stop().await;
        h.mock.release.notify_waiters();
    }
}

#[tokio::test]
async fn stalled_rest_review_hydration_does_not_block_ci_or_terminal_lifecycle() {
    let h = Harness::new().await;
    h.mode("account");
    let mut config = h.config();
    config.report_timeout = Duration::from_secs(10);
    config.request_timeout = Duration::from_secs(10);
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let before = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        before
            .pull_requests
            .iter()
            .all(|row| row["ci"]["summary"]["state"] == "running")
    );
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute("DELETE FROM cache", [])
        .unwrap();
    h.mode("account-slow-reviews");
    h.phase(9);
    let details = tokio::spawn({
        let c = c.clone();
        async move { c.pr_report("acme/demo", 7, Freshness::Revalidate).await }
    });
    until(|| {
        h.calls()
            .iter()
            .filter(|call| call.path == "/repos/acme/demo/pulls/7/reviews")
            .count()
            >= 2
    })
    .await;
    // No release of the blocked review request: core must progress while its
    // socket is still occupied, including for another PR in the account.
    tokio::time::timeout(Duration::from_secs(1), async {
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        c.ci_for_pr("acme/other", 7, Freshness::Revalidate)
            .await
            .unwrap();
    })
    .await
    .unwrap();
    assert!(!details.is_finished());
    let after = c
        .pr_status_page(None, Some(&before.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    for (repo, state) in [("acme/demo", "CLOSED"), ("acme/other", "MERGED")] {
        let row = &after
            .changes
            .iter()
            .rev()
            .find(|change| change.pull_request["repository"]["nameWithOwner"] == repo)
            .unwrap()
            .pull_request;
        assert_eq!(row["state"], state);
        assert_eq!(row["ci"]["summary"]["state"], "success");
    }
    h.mock.release.notify_waiters();
    assert!(details.await.unwrap().unwrap().complete);
}

#[tokio::test]
async fn account_background_ci_moves_past_one_stalled_pr_in_the_same_cycle() {
    let h = Harness::new().await;
    h.mode("account");
    h.phase(2);
    let mut config = h.config();
    config.report_timeout = Duration::from_secs(8);
    config.request_timeout = Duration::from_secs(10);
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    h.mode("account-slow-one");
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    api.watch_account(60).await.unwrap();
    tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            if let Some(cycle) = c.account_refresh_cycle(true).await.unwrap() {
                assert_eq!(cycle.total, 2);
                assert_eq!(cycle.attempted, 2);
                assert_eq!(cycle.interrupted, 1);
                assert_eq!(cycle.succeeded, 1);
                assert_eq!(cycle.deferred, 0);
                assert!(!cycle.cycle_budget_exhausted);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let page = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let other = page
        .pull_requests
        .iter()
        .find(|row| row["repository"]["nameWithOwner"] == "acme/other")
        .unwrap();
    assert_eq!(other["ci"]["summary"]["state"], "success");
    api.stop().await;
    h.mock.release.notify_waiters();
    h.mode("account");
    assert!(
        c.refresh_pr_status(Freshness::Revalidate, false)
            .await
            .unwrap()
            .is_empty()
    );
    let recovered = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(recovered.complete);
    assert!(
        recovered
            .pull_requests
            .iter()
            .all(|row| row["sourceErrors"] == json!({})
                && row["ci"]["summary"]["state"] == "success")
    );
}

#[tokio::test]
async fn conditional_validations_skip_soft_pacing_without_bypassing_exhaustion() {
    let h = Harness::new().await;
    let c = h.client();
    c.get("conditional-paced", Freshness::Revalidate)
        .await
        .unwrap();
    let validated = tokio::time::timeout(
        Duration::from_millis(250),
        c.get("conditional-paced", Freshness::Revalidate),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(validated.source, Source::Revalidated));
    assert_eq!(validated.data, json!({"stable":true}));
    // A free validation does not erase the charged request's pacing debt.
    assert!(matches!(
        c.get("charged-after-conditional", Freshness::Revalidate)
            .await,
        Err(Error::RateLimited { .. })
    ));
    assert!(
        !h.calls()
            .iter()
            .any(|call| call.path == "/charged-after-conditional")
    );
    h.phase(1);
    c.get("conditional-paced", Freshness::Revalidate)
        .await
        .unwrap();
    let calls = h.calls().len();
    assert!(matches!(
        c.get("conditional-paced", Freshness::Revalidate).await,
        Err(Error::RateLimited { .. })
    ));
    assert_eq!(h.calls().len(), calls);
}

#[tokio::test]
async fn account_initial_checks_are_available_before_slow_detail_hydration_and_resume_is_fair() {
    let h = Harness::new().await;
    h.mode("account-slow-one");
    h.phase(2);
    let mut config = h.config();
    // Exercise cycle expiry at the blocked request, not during SQLite setup or
    // discovery when the runner is contended. The request must outlive the cycle.
    config.report_timeout = Duration::from_secs(2);
    config.request_timeout = Duration::from_secs(4);
    config.max_attempts = 1;
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let initial = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(initial.pull_requests.len(), 2);
    assert!(
        initial
            .pull_requests
            .iter()
            .all(|p| p["headCiState"] == "SUCCESS"
                && p["ci"].is_null()
                && p["conflicts"] == "clean")
    );
    assert!(!initial.complete);
    assert!(h.calls().iter().all(|call| call.path == "/graphql"));
    let errors = c
        .refresh_pr_status(Freshness::Revalidate, true)
        .await
        .unwrap();
    assert_eq!(
        errors.iter().filter(|error| error.contains("#7:")).count(),
        1
    );
    assert!(
        errors
            .iter()
            .any(|error| error.contains("1 PRs remain queued"))
    );
    assert!(errors.iter().any(|error| {
        error.contains("#7: refresh cycle budget exhausted; retry queued; prior evidence retained")
    }));
    let deferred = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let cycle = c.account_refresh_cycle(true).await.unwrap().unwrap();
    assert_eq!(cycle.total, 2);
    assert_eq!(cycle.attempted, 1);
    assert_eq!(cycle.succeeded, 0);
    assert_eq!(cycle.failed, 0);
    assert_eq!(cycle.interrupted, 1);
    assert_eq!(cycle.deferred, 1);
    assert!(cycle.cycle_budget_exhausted);
    assert!(cycle.finished_at_ms >= cycle.started_at_ms);
    assert!(c.account_refresh_cycle(false).await.unwrap().is_none());
    let reads = h.calls().len();
    c.prepare_pr_status(Freshness::CachedOnly).await.unwrap();
    assert_eq!(
        c.account_refresh_cycle(true).await.unwrap(),
        Some(cycle.clone())
    );
    assert_eq!(h.calls().len(), reads);
    let same_scope = h.client();
    assert_eq!(
        same_scope.account_refresh_cycle(true).await.unwrap(),
        Some(cycle.clone())
    );
    let other_scope = Client::with_token(h.config(), "other-synthetic-token".into()).unwrap();
    assert!(
        other_scope
            .account_refresh_cycle(true)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        c.pr_status_page(None, Some(&deferred.cursor), 1000, Duration::ZERO)
            .await
            .unwrap()
            .cursor,
        deferred.cursor
    );
    let other = deferred
        .pull_requests
        .iter()
        .find(|row| row["repository"]["nameWithOwner"] == "acme/other")
        .unwrap();
    assert_eq!(other["sourceErrors"], json!({}));
    let interrupted = deferred
        .pull_requests
        .iter()
        .find(|row| row["repository"]["nameWithOwner"] == "acme/demo")
        .unwrap();
    assert_eq!(
        interrupted["sourceErrors"]["ci"],
        "refresh cycle budget exhausted; retry queued; prior evidence retained"
    );
    assert_eq!(interrupted["complete"], false);
    tokio::time::timeout(Duration::from_secs(5), async {
        while c.status().outstanding_requests != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // Persisted resume starts after the timed-out PR, even on a new connection.
    drop(c);
    let mut config = h.config();
    config.report_timeout = Duration::from_secs(2);
    config.request_timeout = Duration::from_secs(4);
    config.max_attempts = 1;
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.refresh_pr_status(Freshness::Revalidate, true)
        .await
        .unwrap();
    let page = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let other = page
        .pull_requests
        .iter()
        .find(|p| p["repository"]["nameWithOwner"] == "acme/other")
        .unwrap();
    assert_eq!(other["ci"]["summary"]["state"], "success");
    h.mock.release.notify_waiters();
}

#[tokio::test]
async fn upstream_timeout_is_not_reclassified_as_local_cycle_deferral() {
    let h = Harness::new().await;
    h.mode("account");
    h.phase(2);
    let mut config = h.config();
    config.report_timeout = Duration::from_secs(2);
    config.request_timeout = Duration::from_millis(100);
    config.max_attempts = 1;
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute("DELETE FROM cache", [])
        .unwrap();
    h.mode("account-slow-one");
    let errors = c
        .refresh_pr_status(Freshness::Revalidate, true)
        .await
        .unwrap();
    assert!(errors.iter().any(|error| error.starts_with("acme/demo#7:")));
    assert!(errors.iter().all(|error| !error.contains("cycle budget")));
    let cycle = c.account_refresh_cycle(true).await.unwrap().unwrap();
    assert_eq!(cycle.failed, 1);
    assert_eq!(cycle.interrupted, 0);
    assert_eq!(cycle.deferred, 0);
    assert!(!cycle.cycle_budget_exhausted);
    let page = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let failed = page
        .pull_requests
        .iter()
        .find(|row| row["repository"]["nameWithOwner"] == "acme/demo")
        .unwrap();
    assert!(failed["sourceErrors"]["ci"].is_string());
    assert!(
        !failed["sourceErrors"]["ci"]
            .as_str()
            .unwrap()
            .contains("cycle budget")
    );
    assert_eq!(failed["complete"], false);
    // Last-good CI survives the failed refresh, without becoming current evidence.
    assert_eq!(failed["ci"]["summary"]["state"], "success");
    let other = page
        .pull_requests
        .iter()
        .find(|row| row["repository"]["nameWithOwner"] == "acme/other")
        .unwrap();
    assert_eq!(other["sourceErrors"], json!({}));
    assert_eq!(other["complete"], true);
}

#[tokio::test]
async fn account_cached_discovery_cannot_roll_back_a_newer_rest_head_or_relabel_its_ci() {
    let h = Harness::new().await;
    h.mode("account-head-change");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.phase(1);
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    // The discovery query is still cached at the old head.
    c.prepare_pr_status(Freshness::default()).await.unwrap();
    let page = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(page.pull_requests[0]["headRefOid"], NEW_HEAD);
    assert_eq!(page.pull_requests[0]["ci"]["head_sha"], NEW_HEAD);
    assert!(page.pull_requests[0]["headCiState"].is_null());
    assert!(page.pull_requests[0]["reviewDecision"].is_null());
    assert!(page.pull_requests[0]["mergeStateStatus"].is_null());
    // A generic REST cache read isn't a published metadata observation. Its
    // validation timestamp must not be applied to an older source snapshot.
    h.phase(0);
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.phase(1);
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    c.pull_request("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    c.prepare_pr_status(Freshness::default()).await.unwrap();
    let page = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(page.pull_requests[0]["headRefOid"], NEW_HEAD);
    assert!(page.pull_requests[0]["ci"].is_null());
}

#[tokio::test]
async fn newer_same_head_discovery_preserves_title_draft_and_conflicts_over_older_rest() {
    let h = Harness::new().await;
    h.mode("account-metadata-change");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    h.phase(1);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let page = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let row = &page.pull_requests[0];
    assert_eq!(row["headRefOid"], HEAD);
    assert_eq!(row["ci"]["head_sha"], HEAD);
    assert_eq!(row["title"], "Updated title");
    assert_eq!(row["isDraft"], true);
    assert_eq!(row["conflicts"], "conflicting");
    // Same-head CI evidence remains available; it cannot justify replacing
    // freshly observed metadata with an older REST projection.
    let rest = c
        .pull_request("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(rest.data["draft"], false);
    assert_eq!(rest.data["mergeable"], true);
    tokio::time::sleep(Duration::from_millis(2)).await;
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    c.prepare_pr_status(Freshness::default()).await.unwrap();
    let updated = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(updated.pull_requests[0]["title"], "A PR");
    assert_eq!(updated.pull_requests[0]["isDraft"], false);
    assert_eq!(updated.pull_requests[0]["conflicts"], "clean");
}

#[tokio::test]
async fn older_successful_state_responses_cannot_reclose_or_reopen_a_pr() {
    let h = Harness::new().await;
    h.mode("account-state-version");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.phase(3);
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let closed = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(closed.pull_requests.is_empty());
    h.phase(1);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let reopened = c
        .pr_status_page(
            Some("acme/demo"),
            Some(&closed.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    let row = &reopened
        .changes
        .iter()
        .find(|event| event.kind == "reopened")
        .expect("newer open discovery must survive an older successful closed REST response")
        .pull_request;
    assert_eq!(row["state"], "OPEN");
    assert_eq!(row["closedAt"], Value::Null);
    assert_eq!(row["mergedAt"], Value::Null);
    h.phase(7);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let disappeared = c
        .pr_status_page(
            Some("acme/demo"),
            Some(&reopened.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    let removed = &disappeared
        .changes
        .iter()
        .find(|event| event.kind == "removed")
        .expect("disappearance with stale terminal evidence must stay unresolved")
        .pull_request;
    assert_eq!(removed["state"], "UNKNOWN");
    assert_eq!(removed["closedAt"], Value::Null);
    assert_eq!(removed["complete"], false);
    assert!(removed["sourceErrors"]["state"].is_string());
    assert!(
        disappeared
            .changes
            .iter()
            .all(|event| event.kind != "closed")
    );
    h.phase(2);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let unresolved = c
        .pr_status_page(
            Some("acme/demo"),
            Some(&disappeared.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    assert!(
        unresolved
            .changes
            .iter()
            .all(|event| event.kind != "reopened")
    );
    assert!(
        c.pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
            .await
            .unwrap()
            .pull_requests
            .is_empty()
    );
    h.phase(8);
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let latest_closed = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(latest_closed.pull_requests.is_empty());
    h.phase(2);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let stale_open = c
        .pr_status_page(
            Some("acme/demo"),
            Some(&latest_closed.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    assert!(
        stale_open
            .changes
            .iter()
            .all(|event| event.kind != "reopened")
    );
    assert!(
        c.pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
            .await
            .unwrap()
            .pull_requests
            .is_empty()
    );
}

#[tokio::test]
async fn older_pr_versions_cannot_roll_back_metadata_but_ci_keeps_updating() {
    let h = Harness::new().await;
    h.mode("account-version-change");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.phase(1);
    tokio::time::sleep(Duration::from_millis(2)).await;
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    h.phase(2);
    // REST is validated later, but its PR updated_at is older than discovery.
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    drop(c);
    let c = h.client();
    let before = h.calls().len();
    c.prepare_pr_status(Freshness::CachedOnly).await.unwrap();
    let projected = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let row = &projected.pull_requests[0];
    assert_eq!(row["title"], "Updated title");
    assert_eq!(row["isDraft"], true);
    assert_eq!(row["updatedAt"], "2026-09-19T00:00:01.250Z");
    assert_eq!(row["ci"]["summary"]["state"], "success");
    assert_eq!(h.calls().len(), before);
    tokio::time::sleep(Duration::from_millis(2)).await;
    // Another REST validation still must not roll the PR fields backward.
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let after_rest = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(after_rest.pull_requests[0]["title"], "Updated title");
    assert_eq!(after_rest.pull_requests[0]["isDraft"], true);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let after_graph = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(after_graph.pull_requests[0]["title"], "Updated title");
    assert_eq!(after_graph.pull_requests[0]["isDraft"], true);
    assert_eq!(after_graph.pull_requests[0]["updatedAt"], row["updatedAt"]);
    assert_eq!(after_graph.pull_requests[0]["headCiState"], "SUCCESS");
    let activity = c
        .pr_status_page(
            Some("acme/demo"),
            Some(&after_rest.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    assert!(activity.changes.iter().all(|event| {
        !event
            .changed_fields
            .iter()
            .any(|field| matches!(field.as_str(), "title" | "isDraft" | "updatedAt"))
    }));
    c.prepare_pr_status(Freshness::CachedOnly).await.unwrap();
    let repeated = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(repeated.cursor, after_graph.cursor);
    h.phase(3);
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let conflicting = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(conflicting.pull_requests[0]["title"], "Updated title");
    assert_eq!(conflicting.pull_requests[0]["isDraft"], true);
    assert_eq!(conflicting.pull_requests[0]["conflicts"], "conflicting");
    // A genuinely newer REST version can advance the mutable fields again.
    h.phase(4);
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let latest = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(latest.pull_requests[0]["title"], "Latest REST title");
    assert_eq!(latest.pull_requests[0]["isDraft"], false);
    assert_eq!(latest.pull_requests[0]["updatedAt"], "2026-09-19T00:00:02Z");
}

#[tokio::test]
async fn newer_rest_metadata_cannot_suppress_graphql_only_review_and_merge_updates() {
    let h = Harness::new().await;
    h.mode("account-review-change");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let initial = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        initial.pull_requests[0]["reviewDecision"],
        "REVIEW_REQUIRED"
    );
    h.phase(1);
    tokio::time::sleep(Duration::from_millis(2)).await;
    // Discover an update without projecting it yet, as in concurrent poll loops.
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    drop(c);
    let c = h.client();
    let before = h.calls().len();
    c.prepare_pr_status(Freshness::CachedOnly).await.unwrap();
    let projected = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        projected.pull_requests[0]["reviewDecision"],
        "CHANGES_REQUESTED"
    );
    assert_eq!(projected.pull_requests[0]["mergeStateStatus"], "BLOCKED");
    assert_eq!(projected.pull_requests[0]["title"], "A PR");
    assert_eq!(h.calls().len(), before);
    c.ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    c.prepare_pr_status(Freshness::CachedOnly).await.unwrap();
    let unchanged = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(unchanged.cursor, projected.cursor);
    assert_eq!(unchanged.pull_requests, projected.pull_requests);
    assert_eq!(h.calls().len(), before);
}

#[tokio::test]
async fn cached_individual_reads_after_restart_cannot_roll_back_newer_feed_metadata() {
    let h = Harness::new().await;
    h.mode("account-metadata-change");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    h.phase(1);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let initial = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    drop(c);
    let restarted = h.client();
    let before = h.calls().len();
    restarted
        .pr_report("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    restarted
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    let page = restarted
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(page.pull_requests[0]["title"], "Updated title");
    assert_eq!(page.pull_requests[0]["isDraft"], true);
    assert_eq!(page.pull_requests[0]["conflicts"], "conflicting");
    assert_eq!(page.cursor, initial.cursor);
    assert_eq!(h.calls().len(), before);
    // Existing installations predate the private clock table. Migrated rows
    // retain their last semantic observation as a conservative barrier too.
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute("DELETE FROM snapshot_validation", [])
        .unwrap();
    let migrated = h.client();
    migrated
        .pr_report("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    let legacy = migrated
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(legacy.pull_requests[0]["title"], "Updated title");
    assert_eq!(legacy.pull_requests[0]["conflicts"], "conflicting");
    assert_eq!(legacy.cursor, initial.cursor);
    assert_eq!(h.calls().len(), before);
}

#[tokio::test]
async fn later_discovery_page_wins_over_rest_observed_between_pages_without_cursor_churn() {
    let h = Harness::new().await;
    h.mode("account-page-order");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let pages: Vec<_> = h
        .calls()
        .into_iter()
        .filter(|call| {
            call.path == "/graphql"
                && call.body["query"]
                    .as_str()
                    .is_some_and(|q| q.contains("MyOpenPullRequests"))
        })
        .collect();
    // Cache a first-page new-head observation, then publish old-head REST
    // metadata, then cache the later page's new-head observation. Reassembling
    // cached pages is a real discovery path with different validation clocks.
    h.phase(1);
    c.graphql(
        pages[0].body["query"].as_str().unwrap(),
        pages[0].body["variables"].clone(),
        Freshness::Revalidate,
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    h.phase(0);
    c.ci_for_pr("acme/other", 7, Freshness::Revalidate)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    h.phase(1);
    c.graphql(
        pages[1].body["query"].as_str().unwrap(),
        pages[1].body["variables"].clone(),
        Freshness::Revalidate,
    )
    .await
    .unwrap();
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute("UPDATE cache SET response=json_set(response,'$.fetched_at_ms',0) WHERE key='account-discovery-complete:v1'", []).unwrap();
    c.all_my_open_pull_requests(Freshness::default())
        .await
        .unwrap();
    c.prepare_pr_status(Freshness::default()).await.unwrap();
    let page = c
        .pr_status_page(Some("acme/other"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(page.pull_requests[0]["headRefOid"], NEW_HEAD);
    assert!(
        page.pull_requests[0]["ci"].is_null(),
        "old-head CI must not label the discovered head"
    );
    c.prepare_pr_status(Freshness::default()).await.unwrap();
    let unchanged = c
        .pr_status_page(Some("acme/other"), Some(&page.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(unchanged.changes.is_empty());
    assert_eq!(unchanged.cursor, page.cursor);
}

#[tokio::test]
async fn account_fast_disappearance_resolves_close_and_keeps_the_final_comment_scan_queued() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let initial = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.phase(8);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let terminal = c
        .pr_status_page(None, Some(&initial.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(terminal.changes.iter().any(|event| event.kind == "closed"
        && event.pull_request["repository"]["nameWithOwner"] == "acme/demo"));
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let final_scan = c
        .pr_status_page(None, Some(&terminal.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(final_scan.changes.iter().any(|event| {
        event.pull_request["repository"]["nameWithOwner"] == "acme/demo"
            && event.pull_request["state"] == "CLOSED"
            && event
                .activity
                .iter()
                .any(|a| a["kind"] == "comment_added" && a["id"] == 2)
    }));
    h.phase(0);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let reopened = c
        .pr_status_page(None, Some(&final_scan.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(reopened.changes.iter().any(|event| event.kind == "reopened"
        && event.pull_request["repository"]["nameWithOwner"] == "acme/demo"));
}

#[tokio::test]
async fn reopened_discovery_clears_terminal_dates_when_rest_retry_fails() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.phase(8);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let closed = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.phase(0);
    h.mode("account-reopen-failed-rest");
    assert!(
        !c.prepare_pr_status(Freshness::Revalidate)
            .await
            .unwrap()
            .is_empty()
    );
    let page = c
        .pr_status_page(None, Some(&closed.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    let reopened = page
        .changes
        .iter()
        .find(|change| {
            change.kind == "reopened"
                && change.pull_request["repository"]["nameWithOwner"] == "acme/demo"
        })
        .unwrap();
    assert_eq!(reopened.pull_request["state"], "OPEN");
    assert!(reopened.pull_request["closedAt"].is_null());
    assert!(reopened.pull_request["mergedAt"].is_null());
    assert_eq!(reopened.pull_request["complete"], false);
    assert!(reopened.pull_request["sourceErrors"]["discovery"].is_string());
}

#[tokio::test]
async fn merged_pr_cannot_be_reopened_by_a_later_stale_open_listing() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.phase(9);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let merged = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.phase(0);
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    let calls = h.calls().len();
    c.prepare_pr_status(Freshness::CachedOnly).await.unwrap();
    let page = c
        .pr_status_page(None, Some(&merged.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(!page.changes.iter().any(|event| event.kind == "reopened"
        && event.pull_request["repository"]["nameWithOwner"] == "acme/other"));
    let open = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        !open
            .pull_requests
            .iter()
            .any(|row| row["repository"]["nameWithOwner"] == "acme/other")
    );
    assert_eq!(
        h.calls().len(),
        calls,
        "cached terminal probes must not request GitHub"
    );
}

#[tokio::test]
async fn account_policy_is_invalidated_when_base_branch_tip_or_test_merge_changes() {
    let h = Harness::new().await;
    h.mode("account-policy-selectors");
    let c = h.client();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    c.required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    c.prepare_pr_status(Freshness::CachedOnly).await.unwrap();
    let mut page = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        page.pull_requests[0]["requiredChecks"]["state"],
        "not_required"
    );
    for phase in 1..=4 {
        h.phase(phase);
        if phase == 4 {
            c.get("repos/acme/demo/branches/release", Freshness::Revalidate)
                .await
                .unwrap();
        }
        c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
        let calls = h.calls().len();
        let changes = c
            .pr_status_page(Some("acme/demo"), Some(&page.cursor), 1000, Duration::ZERO)
            .await
            .unwrap();
        let event = changes.changes.last().unwrap();
        assert!(
            event.pull_request["requiredChecks"].is_null(),
            "phase {phase} must invalidate prior policy despite an unchanged head"
        );
        assert_eq!(event.pull_request["headRefOid"], HEAD);
        assert_eq!(event.pull_request["state"], "OPEN");
        assert!(changes.changes.iter().any(|change| {
            change
                .activity
                .iter()
                .any(|a| a["kind"] == "required_checks_changed")
        }));
        assert_eq!(h.calls().len(), calls, "cursor reads stay offline");
        c.required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        c.prepare_pr_status(Freshness::CachedOnly).await.unwrap();
        page = c
            .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
            .await
            .unwrap();
        let policy = &page.pull_requests[0]["requiredChecks"];
        assert!(
            !policy.is_null(),
            "fresh matching policy must attach in phase {phase}"
        );
        assert_eq!(policy["base_branch"], "release");
        assert_eq!(
            policy["base_sha"],
            if phase >= 4 {
                OTHER_BASE
            } else if phase >= 2 {
                NEW_HEAD
            } else {
                BASE
            }
        );
        assert_eq!(
            policy["pr_base_sha"],
            if phase >= 2 { NEW_HEAD } else { BASE }
        );
        assert_eq!(
            policy["merge_sha"],
            if phase >= 3 {
                json!(MERGE)
            } else {
                Value::Null
            }
        );
    }
    // Legacy reports without immutable selectors are withheld after restart.
    let db = rusqlite::Connection::open(h.dir.path().join("cache.sqlite")).unwrap();
    let raw: String = db
        .query_row(
            "SELECT data FROM snapshots WHERE resource LIKE 'required_checks://%/acme/demo/7'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut legacy: Value = serde_json::from_str(&raw).unwrap();
    legacy.as_object_mut().unwrap().remove("base_sha");
    legacy.as_object_mut().unwrap().remove("merge_sha");
    db.execute(
        "UPDATE snapshots SET data=?1 WHERE resource LIKE 'required_checks://%/acme/demo/7'",
        [legacy.to_string()],
    )
    .unwrap();
    let restarted = h.client();
    let calls = h.calls().len();
    restarted
        .prepare_pr_status(Freshness::CachedOnly)
        .await
        .unwrap();
    let legacy_page = restarted
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(legacy_page.pull_requests[0]["requiredChecks"].is_null());
    assert_eq!(h.calls().len(), calls);
}

#[tokio::test]
async fn different_pr_node_reusing_a_repository_number_is_opened_instead_of_reopened() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.phase(9);
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let merged = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.phase(0);
    h.mode("account-new-identity");
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    let calls = h.calls().len();
    c.prepare_pr_status(Freshness::CachedOnly).await.unwrap();
    let page = c
        .pr_status_page(None, Some(&merged.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    let new_pr = page
        .changes
        .iter()
        .find(|event| event.pull_request["repository"]["nameWithOwner"] == "acme/other")
        .unwrap();
    assert_eq!(new_pr.kind, "opened");
    assert_eq!(new_pr.pull_request["id"], "PR_acme/other_new_7");
    assert_eq!(new_pr.pull_request["state"], "OPEN");
    assert!(new_pr.pull_request["closedAt"].is_null());
    assert!(new_pr.pull_request["mergedAt"].is_null());
    assert!(new_pr.pull_request["ci"].is_null());
    for source in [
        "comments",
        "reviewComments",
        "reviews",
        "reviewThreads",
        "reviewStatus",
        "requiredChecks",
    ] {
        assert!(
            new_pr.pull_request[source].is_null(),
            "a different PR node must not inherit the previous node's {source}"
        );
    }
    assert_eq!(new_pr.pull_request["complete"], false);
    assert!(!new_pr.activity.iter().any(|a| matches!(
        a["kind"].as_str(),
        Some("comment_deleted" | "closed" | "merged" | "reopened")
    )));
    assert_eq!(h.calls().len(), calls);

    // Raw response caches, not just projections, must be fenced. A second SDK
    // client and a restart cannot hydrate the new node from the old URL cache.
    let restarted = h.client();
    assert!(matches!(
        restarted
            .pull_request("acme/other", 7, Freshness::CachedOnly)
            .await,
        Err(Error::CacheMiss)
    ));
    assert_eq!(h.calls().len(), calls);
    assert!(
        restarted
            .ci_for_pr("acme/other", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    let ci_page = restarted
        .pr_status_page(Some("acme/other"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let row = &ci_page.pull_requests[0];
    assert_eq!(row["id"], "PR_acme/other_new_7");
    assert!(!row["ci"].is_null());
    assert!(row["comments"].is_null());
    assert!(row["reviews"].is_null());
    assert!(row["requiredChecks"].is_null());
    assert_eq!(row["complete"], false);

    let calls = h.calls().len();
    let restarted = h.client();
    let partial = restarted
        .pr_report("acme/other", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(!partial.complete);
    assert!(partial.data.comments.is_empty());
    assert!(partial.data.reviews.is_empty());
    assert!(partial.data.errors.iter().any(|e| e.source == "comments"));
    assert_eq!(h.calls().len(), calls);
    let complete = restarted
        .pr_report("acme/other", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(complete.complete);
    assert_eq!(complete.data.pull_request["node_id"], "PR_acme/other_new_7");
    assert!(!complete.data.comments.is_empty());
    let calls = h.calls().len();
    assert!(
        h.client()
            .pr_report("acme/other", 7, Freshness::CachedOnly)
            .await
            .unwrap()
            .complete
    );
    assert_eq!(h.calls().len(), calls);
    let settled = restarted
        .pr_status_page(Some("acme/other"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        settled.pull_requests[0]["complete"], true,
        "{}",
        settled.pull_requests[0]
    );
    restarted
        .prepare_pr_status(Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(
        restarted
            .pr_status_page(
                Some("acme/other"),
                Some(&settled.cursor),
                1000,
                Duration::ZERO
            )
            .await
            .unwrap()
            .changes
            .is_empty()
    );
    assert_eq!(h.calls().len(), calls);
}

#[tokio::test]
async fn rest_positive_discovery_can_replace_a_known_retired_node_without_clearing_graph_health() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    h.mode("account-fallback");
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let before = c
        .pr_status_page(Some("acme/fresh"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        before
            .pull_requests
            .iter()
            .any(|p| p["number"] == 11 && p["id"] == "PR_acme/fresh_11")
    );
    h.phase(3);
    // Normal polling notices the changed native identity at an existing URL.
    c.ci_for_pr("acme/fresh", 11, Freshness::Revalidate)
        .await
        .unwrap();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let replaced = c
        .pr_status_page(
            Some("acme/fresh"),
            Some(&before.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    let new = replaced
        .changes
        .iter()
        .find(|e| e.kind == "opened" && e.pull_request["id"] == "PR_acme/fresh_new_11")
        .expect("validated new positive");
    assert!(new.pull_request["comments"].is_null());
    assert!(new.pull_request["reviews"].is_null());
    assert!(new.pull_request["reviewDecision"].is_null());
    assert!(new.pull_request["mergeStateStatus"].is_null());
    assert_eq!(new.pull_request["state"], "OPEN");
    assert!(
        !replaced.errors.is_empty(),
        "REST positives must not clear failed GraphQL discovery health"
    );
}

#[tokio::test]
async fn delayed_old_entity_reads_cannot_overwrite_a_new_nodes_evidence_or_health() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.mode("account-identity-stalled");
    let comments_before = h
        .calls()
        .iter()
        .filter(|c| c.path == "/repos/acme/other/issues/7/comments")
        .count();
    let slow = h.client();
    let old =
        tokio::spawn(async move { slow.pr_report("acme/other", 7, Freshness::Revalidate).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while h
            .calls()
            .iter()
            .filter(|c| c.path == "/repos/acme/other/issues/7/comments")
            .count()
            <= comments_before
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    h.mode("account-new-identity");
    c.all_my_open_pull_requests(Freshness::Revalidate)
        .await
        .unwrap();
    c.prepare_pr_status(Freshness::CachedOnly).await.unwrap();
    assert!(
        c.ci_for_pr("acme/other", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    let before = c
        .pr_status_page(Some("acme/other"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.mock.release.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), old)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let after = h
        .client()
        .pr_status_page(
            Some("acme/other"),
            Some(&before.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    assert!(
        after.changes.is_empty(),
        "retired work must not publish failure health or replacements"
    );
    let calls = h.calls().len();
    let report = h
        .client()
        .pr_report("acme/other", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(!report.complete);
    assert!(report.data.comments.is_empty());
    assert!(report.data.reviews.is_empty());
    assert_eq!(h.calls().len(), calls);
}

#[tokio::test]
async fn discovery_repository_casing_changes_do_not_remove_or_reopen_the_same_nodes() {
    let h = Harness::new().await;
    h.mode("account");
    h.phase(2);
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let before = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.mode("account-repository-case-change");
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let page = c
        .pr_status_page(None, Some(&before.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        page.changes.iter().all(|event| !matches!(
            event.kind.as_str(),
            "removed" | "closed" | "merged" | "reopened"
        )),
        "cosmetic casing changes must not become lifecycle transitions: {:?}",
        page.changes
            .iter()
            .map(|e| e.kind.as_str())
            .collect::<Vec<_>>()
    );
    assert!(page.changes.iter().all(
        |event| event.pull_request["state"] == "OPEN" && event.pull_request["removed"] == false
    ));
    let current = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(current.pull_requests.len(), 2);
    assert!(
        current
            .pull_requests
            .iter()
            .all(|row| row["complete"] == true)
    );
    let calls = h.calls().len();
    c.refresh_pr_status(Freshness::CachedOnly, false)
        .await
        .unwrap();
    assert_eq!(h.calls().len(), calls);
    assert_eq!(
        c.account_refresh_cycle(false).await.unwrap().unwrap().total,
        2
    );
    let refreshed = c
        .pr_status_page(None, Some(&current.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        refreshed.changes.is_empty(),
        "cached refresh must not churn the PR cursor"
    );
    let cursor = c.bootstrap().await.unwrap().cursor;
    h.client()
        .prepare_pr_status(Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(c.bootstrap().await.unwrap().cursor, cursor);
}

#[tokio::test]
async fn rest_discovery_repository_case_aliases_retain_known_nodes_and_independent_errors() {
    let h = Harness::new().await;
    h.mode("account");
    h.phase(1);
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let before = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let calls = h.calls().len();
    h.mode("account-fallback-case-change");
    let errors = c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    assert!(errors.iter().any(|error| error.starts_with("discovery:")));
    assert!(
        !h.calls()[calls..]
            .iter()
            .any(|call| call.path.eq_ignore_ascii_case("/repos/acme/demo/pulls/7")),
        "known aliases must not consume the bounded REST probe budget"
    );
    let page = c
        .pr_status_page(None, Some(&before.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        page.changes
            .iter()
            .filter(|event| event.kind == "opened")
            .count(),
        2
    );
    assert!(page.changes.iter().all(|event| !matches!(
        event.kind.as_str(),
        "removed" | "closed" | "merged" | "reopened"
    )));
    let current = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(current.pull_requests.len(), 4);
    assert_eq!(
        current.errors.len(),
        1,
        "positive REST discoveries do not clear GraphQL discovery errors"
    );
    assert!(
        current
            .pull_requests
            .iter()
            .all(|row| row["removed"] == false)
    );
    assert!(
        current
            .pull_requests
            .iter()
            .filter(|row| row["number"] == 11 || row["number"] == 15)
            .all(|row| row["repository"]["nameWithOwner"]
                .as_str()
                .unwrap()
                .starts_with("acme/")),
        "new additions use validated REST repository spelling"
    );
    let calls = h.calls().len();
    h.client()
        .prepare_pr_status(Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(h.calls().len(), calls);
    let restarted = c
        .pr_status_page(None, Some(&current.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(restarted.changes.is_empty());
    assert_eq!(restarted.errors.len(), 1);
}

#[tokio::test]
async fn repository_aliases_work_when_capitals_were_cached_first_and_ref_case_stays_exact() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    assert!(
        c.pr_report("ACME/DEMO", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    let calls = h.calls().len();
    assert!(
        h.client()
            .pr_report("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap()
            .complete
    );
    assert_eq!(h.calls().len(), calls);
    let uri = "repos/ACME/DEMO/branches/Feature?ref=Feature";
    c.get(uri, Freshness::Revalidate).await.unwrap();
    let calls = h.calls().len();
    assert!(
        h.client()
            .get(
                "repos/acme/demo/branches/Feature?ref=Feature",
                Freshness::CachedOnly
            )
            .await
            .is_ok()
    );
    assert!(matches!(
        c.get(
            "repos/acme/demo/branches/feature?ref=Feature",
            Freshness::CachedOnly
        )
        .await,
        Err(Error::CacheMiss)
    ));
    assert!(matches!(
        c.get(
            "repos/acme/demo/branches/Feature?ref=feature",
            Freshness::CachedOnly
        )
        .await,
        Err(Error::CacheMiss)
    ));
    let other = Client::with_token(h.config(), "other-synthetic-token".into()).unwrap();
    assert!(matches!(
        other.get(uri, Freshness::CachedOnly).await,
        Err(Error::CacheMiss)
    ));
    assert_eq!(h.calls().len(), calls);
}

#[tokio::test]
async fn repository_case_aliases_share_cached_pr_evidence_and_recovery_replacements() {
    let h = Harness::new().await;
    h.mode("account");
    h.phase(2);
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.mode("account-ci-permission-fails");
    c.refresh_pr_status(Freshness::Revalidate, true)
        .await
        .unwrap();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let broken = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(broken.pull_requests[0]["sourceErrors"]["ci"].is_string());
    assert!(broken.pull_requests[0]["sourceErrors"]["details"].is_string());
    h.mode("account");
    let calls = h.calls().len();
    assert!(
        c.ci_for_pr("ACME/DEMO", 7, Freshness::CachedOnly)
            .await
            .unwrap()
            .complete
    );
    let recovered = c
        .pr_status_page(
            Some("ACME/DEMO"),
            Some(&broken.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    let row = &recovered
        .changes
        .last()
        .expect("recovery from the alias")
        .pull_request;
    assert!(row["sourceErrors"]["ci"].is_null());
    assert!(row["sourceErrors"]["details"].is_string());
    assert_eq!(h.calls().len(), calls);
    assert!(
        h.client()
            .pr_report("ACME/DEMO", 7, Freshness::CachedOnly)
            .await
            .unwrap()
            .complete
    );
    let settled = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        settled.pull_requests.len(),
        1,
        "an alias must not create a duplicate feed row"
    );
    assert_eq!(settled.pull_requests[0]["complete"], true);
    let cursor = c.bootstrap().await.unwrap().cursor;
    c.pr_report("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(c.bootstrap().await.unwrap().cursor, cursor);
    assert_eq!(h.calls().len(), calls);
}

#[tokio::test]
async fn issue64_complete_cached_report_recovers_same_head_feed_without_requests() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.mode("account-ci-permission-fails");
    assert!(
        !c.refresh_pr_status(Freshness::Revalidate, true)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !c.refresh_pr_status(Freshness::Revalidate, false)
            .await
            .unwrap()
            .is_empty()
    );
    let broken = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(broken.pull_requests[0]["complete"], false);
    assert!(broken.pull_requests[0]["sourceErrors"]["ci"].is_string());
    assert!(broken.pull_requests[0]["sourceErrors"]["details"].is_string());
    let calls = h.calls().len();
    let report = c
        .pr_report("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(report.complete);
    let recovered = c
        .pr_status_page(
            Some("acme/demo"),
            Some(&broken.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    let row = &recovered
        .changes
        .last()
        .expect("recovery replacement")
        .pull_request;
    assert_eq!(row["headRefOid"], broken.pull_requests[0]["headRefOid"]);
    assert_eq!(row["complete"], true);
    assert_eq!(row["sourceErrors"], json!({}));
    assert_eq!(h.calls().len(), calls);
    c.pr_report("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(
        c.pr_status_page(
            Some("acme/demo"),
            Some(&recovered.cursor),
            1000,
            Duration::ZERO
        )
        .await
        .unwrap()
        .changes
        .is_empty()
    );
}

#[tokio::test]
async fn individual_metadata_failures_reach_the_feed_without_losing_evidence_or_other_errors() {
    for mode in ["ci", "details"] {
        let h = Harness::new().await;
        h.mode("account");
        h.phase(2);
        let c = h.client();
        c.refresh_pr_status(Freshness::Revalidate, false)
            .await
            .unwrap();
        let before = c
            .pr_status_page(None, None, 1000, Duration::ZERO)
            .await
            .unwrap();
        assert!(before.complete);
        let old = before
            .pull_requests
            .iter()
            .find(|x| x["repository"]["nameWithOwner"] == "acme/demo")
            .unwrap();
        let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
        let clock:u64=db.query_row("SELECT validated_at_ms FROM snapshot_validation WHERE resource='pr-status://github.com/acme/demo/7'",[],|row|row.get(0)).unwrap();
        h.mode("sdk-upstream-403");
        let error = if mode == "ci" {
            c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
                .await
                .unwrap_err()
        } else {
            c.pr_report("acme/demo", 7, Freshness::Revalidate)
                .await
                .unwrap_err()
        };
        assert!(matches!(error, Error::GitHub { status: 403, .. }));
        let delta = c
            .pr_status_page(None, Some(&before.cursor), 1000, Duration::ZERO)
            .await
            .unwrap();
        let changed = delta
            .changes
            .iter()
            .find(|x| x.pull_request["repository"]["nameWithOwner"] == "acme/demo")
            .expect("an initial metadata failure must publish explicit source health");
        let row = &changed.pull_request;
        assert_eq!(row["sourceErrors"][mode], error.to_string());
        assert_eq!(row["complete"], false);
        assert_eq!(row["ci"], old["ci"]);
        assert_eq!(row["comments"], old["comments"]);
        let after_clock:u64=db.query_row("SELECT validated_at_ms FROM snapshot_validation WHERE resource='pr-status://github.com/acme/demo/7'",[],|row|row.get(0)).unwrap();
        assert_eq!(
            after_clock, clock,
            "a failed read must not invent successful validation"
        );
        h.mode("account");
        let ci = c
            .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert!(ci.complete);
        let page = c
            .pr_status_page(None, None, 1000, Duration::ZERO)
            .await
            .unwrap();
        let row = page
            .pull_requests
            .iter()
            .find(|x| x["repository"]["nameWithOwner"] == "acme/demo")
            .unwrap();
        if mode == "details" {
            assert_eq!(row["sourceErrors"]["details"], error.to_string());
            assert_eq!(row["complete"], false);
        } else {
            assert!(row["sourceErrors"]["ci"].is_null());
        }
        assert!(
            c.pr_report("acme/demo", 7, Freshness::Revalidate)
                .await
                .unwrap()
                .complete
        );
        assert!(
            c.pr_status_page(None, None, 1000, Duration::ZERO)
                .await
                .unwrap()
                .complete
        );
    }
}

#[tokio::test]
async fn cached_metadata_failure_under_ci_contention_does_not_publish_or_wait_for_refresh() {
    let h = Harness::new().await;
    h.mode("account");
    h.phase(2);
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let baseline = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.mode("issue72-stall-metadata");
    let before = h.calls().len();
    let fresh_client = c.clone();
    let fresh = tokio::spawn(async move {
        fresh_client
            .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !h.calls()[before..]
            .iter()
            .any(|x| x.path == "/repos/acme/demo/pulls/7")
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!fresh.is_finished());
    let db = rusqlite::Connection::open(h.config().cache_path).unwrap();
    db.execute(
        "DELETE FROM cache WHERE key LIKE '%/repos/acme/demo/pulls/7'",
        [],
    )
    .unwrap();
    let calls = h.calls().len();
    let result = tokio::time::timeout(
        Duration::from_millis(250),
        c.ci_for_pr("acme/demo", 7, Freshness::CachedOnly),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(Error::CacheMiss)));
    assert_eq!(h.calls().len(), calls);
    let page = c
        .pr_status_page(None, Some(&baseline.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(page.changes.is_empty());
    assert_eq!(page.cursor, baseline.cursor);
    h.mode("account");
    h.mock.release.notify_waiters();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), fresh)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .complete
    );
}

#[tokio::test]
async fn health_storage_failure_does_not_mask_the_original_upstream_metadata_error() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    // Break only health lookup in this isolated test DB, not request caching.
    // A real metadata403 must survive the additional storage failure unchanged.
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute("DROP TABLE snapshots", [])
        .unwrap();
    h.mode("sdk-upstream-403");
    for ci_only in [true, false] {
        let error = if ci_only {
            c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
                .await
                .unwrap_err()
        } else {
            c.pr_report("acme/demo", 7, Freshness::Revalidate)
                .await
                .unwrap_err()
        };
        assert!(
            matches!(error,Error::GitHub{status:403,ref message} if message=="synthetic upstream diagnostic")
        );
    }
}

#[tokio::test]
async fn issue64_ci_recovery_preserves_independent_detail_failure() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    h.mode("account-ci-permission-fails");
    c.refresh_pr_status(Freshness::Revalidate, true)
        .await
        .unwrap();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let broken = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    let report = c
        .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert!(report.complete);
    let recovered = c
        .pr_status_page(
            Some("acme/demo"),
            Some(&broken.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    let row = &recovered
        .changes
        .last()
        .expect("CI recovery replacement")
        .pull_request;
    assert!(row["sourceErrors"]["ci"].is_null());
    assert!(row["sourceErrors"]["details"].is_string());
    assert_eq!(row["complete"], false);
}

#[tokio::test]
async fn issue64_individual_incomplete_ci_emits_failure_instead_of_green_feed() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let initial = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.mode("account-ci-permission-fails");
    assert!(
        !c.ci_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    let page = c
        .pr_status_page(
            Some("acme/demo"),
            Some(&initial.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    let row = &page
        .changes
        .last()
        .expect("failure replacement")
        .pull_request;
    assert_eq!(row["complete"], false);
    assert!(row["sourceErrors"]["ci"].is_string());
}

#[tokio::test]
async fn issue64_detail_failure_remains_explicit_until_a_complete_report() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let initial = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.mode("graphql-errors");
    let report = c
        .pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(!report.complete);
    let page = c
        .pr_status_page(
            Some("acme/demo"),
            Some(&initial.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    let row = &page.changes.last().unwrap().pull_request;
    assert_eq!(row["complete"], false);
    assert!(
        row["sourceErrors"]["details"]
            .as_str()
            .unwrap()
            .contains("review_threads")
    );
    assert!(row["sourceErrors"]["ci"].is_null());
    h.mode("account");
    assert!(
        c.pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    let recovered = c
        .pr_status_page(Some("acme/demo"), Some(&page.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        recovered.changes.last().unwrap().pull_request["complete"],
        true
    );
}

#[tokio::test]
async fn issue64_individual_reads_do_not_add_untracked_prs_to_the_account_feed() {
    let h = Harness::new().await;
    let c = h.client();
    let initial = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        c.pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    let page = c
        .pr_status_page(None, Some(&initial.cursor), 1000, Duration::ZERO)
        .await
        .unwrap();
    assert!(page.changes.is_empty());
    assert!(
        c.pr_status_page(None, None, 1000, Duration::ZERO)
            .await
            .unwrap()
            .pull_requests
            .is_empty()
    );
}

#[tokio::test]
async fn issue64_new_head_recovery_does_not_reuse_the_old_head_rollup() {
    let h = Harness::new().await;
    h.mode("account-head-change");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    let initial = c
        .pr_status_page(Some("acme/demo"), None, 1000, Duration::ZERO)
        .await
        .unwrap();
    h.phase(1);
    assert!(
        c.pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap()
            .complete
    );
    let page = c
        .pr_status_page(
            Some("acme/demo"),
            Some(&initial.cursor),
            1000,
            Duration::ZERO,
        )
        .await
        .unwrap();
    let row = &page.changes.last().unwrap().pull_request;
    assert_eq!(row["headRefOid"], NEW_HEAD);
    assert_eq!(row["ci"]["head_sha"], NEW_HEAD);
    assert!(row["headCiState"].is_null());
    assert_eq!(row["complete"], true);
}

async fn issue72_wait_for_call(h: &Harness, start: usize, needle: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if h.calls()[start..]
                .iter()
                .any(|call| call.path.contains(needle))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("background request did not start");
}

#[tokio::test]
async fn issue72_cached_reports_return_during_stalled_refresh_without_publication() {
    for ci_only in [false, true] {
        let h = Harness::new().await;
        h.mode("account");
        let c = h.client();
        c.refresh_pr_status(Freshness::Revalidate, false)
            .await
            .unwrap();
        let baseline = c
            .pr_report("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        let before = c.bootstrap().await.unwrap();
        let calls = h.calls().len();
        h.mode("issue72-stall-metadata");
        let background = c.clone();
        let task = tokio::spawn(async move {
            if ci_only {
                background
                    .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
                    .await
                    .map(|_| ())
            } else {
                background
                    .pr_report("acme/demo", 7, Freshness::Revalidate)
                    .await
                    .map(|_| ())
            }
        });
        issue72_wait_for_call(&h, calls, "/pulls/7").await;
        let stalled_calls = h.calls().len();
        let reports = tokio::time::timeout(Duration::from_millis(500), async {
            let pr = c
                .pr_report("acme/demo", 7, Freshness::CachedOnly)
                .await
                .unwrap();
            let ci = c
                .ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
                .await
                .unwrap();
            (pr, ci)
        })
        .await
        .expect("cached read waited for upstream refresh");
        assert!(reports.0.complete && reports.1.complete);
        assert_eq!(reports.0.data.pull_request["head"]["sha"], HEAD);
        assert_eq!(reports.1.data.head_sha, HEAD);
        for validations in [&reports.0.validations, &reports.1.validations] {
            assert!(!validations.is_empty());
            assert!(
                validations
                    .iter()
                    .all(|v| matches!(v.source, Source::Cache))
            );
        }
        assert_eq!(
            reports.0.oldest_validation_at_ms,
            baseline.oldest_validation_at_ms
        );
        assert!(reports.0.oldest_validation_at_ms <= reports.0.observed_at_ms);
        issue72_assert_cached_cli(&c, true).await;
        assert_eq!(h.calls().len(), stalled_calls);
        assert_eq!(c.bootstrap().await.unwrap().cursor, before.cursor);
        task.abort();
        h.mock.release.notify_waiters();
    }
}

#[tokio::test]
async fn issue72_cached_reads_keep_missing_reviews_and_ci_explicit_during_partial_auth_failure() {
    let h = Harness::new().await;
    h.mode("account");
    let c = h.client();
    c.refresh_pr_status(Freshness::Revalidate, false)
        .await
        .unwrap();
    rusqlite::Connection::open(h.config().cache_path)
        .unwrap()
        .execute(
            "DELETE FROM cache WHERE key LIKE '%graphql%' OR key LIKE '%check-runs%'",
            [],
        )
        .unwrap();
    h.mode("issue72-partial-auth-stall");
    let calls = h.calls().len();
    let background = c.clone();
    let task = tokio::spawn(async move {
        background
            .refresh_pr_status(Freshness::Revalidate, true)
            .await
    });
    issue72_wait_for_call(&h, calls, "/check-runs").await;
    assert!(h.calls()[calls..].iter().any(|c| {
        c.body["query"]
            .as_str()
            .is_some_and(|q| q.contains("MyOpenPullRequests"))
    }));
    let stalled_calls = h.calls().len();
    let (pr, ci) = tokio::time::timeout(Duration::from_millis(500), async {
        (
            c.pr_report("acme/demo", 7, Freshness::CachedOnly)
                .await
                .unwrap(),
            c.ci_for_pr("acme/demo", 7, Freshness::CachedOnly)
                .await
                .unwrap(),
        )
    })
    .await
    .expect("partial authorization failure blocked cached reads");
    assert!(!pr.complete && !ci.complete);
    assert!(pr.data.errors.iter().any(|e| e.source == "review_threads"));
    assert!(!ci.data.errors.is_empty());
    assert_ne!(ci.data.summary.state, "success");
    assert!(ci.data.summary.unknown > 0);
    issue72_assert_cached_cli(&c, false).await;
    assert_eq!(h.calls().len(), stalled_calls);
    task.abort();
    h.mock.release.notify_waiters();
}

#[tokio::test]
async fn issue72_cold_cached_cli_returns_unavailable_json_without_requests() {
    let h = Harness::new().await;
    let api = hey_gh::api::Api::new(h.client()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for path in ["v1/prs/acme/demo/7", "v1/prs/acme/demo/7/ci"] {
        let response = reqwest::Client::new()
            .get(format!("{base}{path}?cached_only=true"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let value: Value = response.json().await.unwrap();
        assert_eq!(value["complete"], false);
        assert_eq!(value["available"], false);
        assert_eq!(value["code"], "cache_miss");
    }
    for arguments in [
        vec!["pr", "view", "7", "-R", "acme/demo", "--cached-only"],
        vec!["pr", "checks", "7", "-R", "acme/demo", "--cached-only"],
        vec!["pr", "acme/demo", "7", "--cached-only"],
        vec!["ci", "acme/demo", "7", "--cached-only"],
    ] {
        let output = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
                .args(["--server", &base])
                .args(arguments)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let value: Value =
            serde_json::from_slice(&output.stdout).expect("missing unavailable envelope");
        assert_eq!(value["complete"], false);
        assert_eq!(value["available"], false);
        assert_eq!(value["code"], "cache_miss");
        assert_eq!(value["number"], 7);
        assert_eq!(value["oldestValidationAtMs"], Value::Null);
        assert!(
            !value["sourceErrors"]["sources"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    assert!(h.calls().is_empty());
    api.stop().await;
    task.abort();
}

async fn issue72_assert_cached_cli(c: &Client, complete: bool) {
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for action in ["view", "checks"] {
        let output = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
                .args([
                    "--server",
                    &base,
                    "pr",
                    action,
                    "7",
                    "-R",
                    "acme/demo",
                    "--cached-only",
                ])
                .output(),
        )
        .await
        .expect("cached CLI stalled")
        .unwrap();
        assert_eq!(
            output.status.success(),
            complete,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["complete"], complete);
        let oldest = if action == "view" {
            "oldestValidationAtMs"
        } else {
            "oldest_validation_at_ms"
        };
        assert!(value[oldest].as_u64().unwrap() > 0);
        assert!(!value["validations"].as_array().unwrap().is_empty());
        if action == "view" {
            assert_eq!(value["headRefOid"], HEAD);
            assert!(value["observedAtMs"].as_u64().unwrap() >= value[oldest].as_u64().unwrap());
            if !complete {
                assert!(
                    !value["sourceErrors"]["sources"]
                        .as_array()
                        .unwrap()
                        .is_empty()
                );
            }
        } else {
            assert_eq!(value["data"]["head_sha"], HEAD);
            if !complete {
                assert!(!value["data"]["errors"].as_array().unwrap().is_empty());
            }
        }
    }
    api.stop().await;
    task.abort();
}

#[tokio::test]
async fn issue73_deadline_returns_cached_evidence_without_stopping_shared_work() {
    for command in [
        vec!["pr", "view", "7", "-R", "acme/demo"],
        vec!["pr", "checks", "7", "-R", "acme/demo"],
        vec!["pr", "acme/demo", "7"],
        vec!["ci", "acme/demo", "7"],
        vec![
            "pr",
            "view",
            "7",
            "-R",
            "acme/demo",
            "--refresh",
            "--json",
            "headRefOid",
        ],
        vec!["required-checks", "acme/demo", "7"],
    ] {
        let h = Harness::new().await;
        let c = h.client();
        c.pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        // Make ordinary reads need validation while retaining known old evidence.
        rusqlite::Connection::open(&h.config().cache_path)
            .unwrap()
            .execute(
                "UPDATE cache SET response = json_set(response, '$.validated_at_ms', 1)",
                [],
            )
            .unwrap();
        let baseline = c
            .pr_report("acme/demo", 7, Freshness::CachedOnly)
            .await
            .unwrap();
        let before = c.bootstrap().await.unwrap();
        h.mode("issue72-stall-metadata");
        let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let router = api.router();
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let start = std::time::Instant::now();
        let output = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
                .args(["--server", &base])
                .args(&command)
                .args(["--timeout", "1"])
                .output(),
        )
        .await
        .expect("total deadline exceeded")
        .unwrap();
        assert!(start.elapsed() >= Duration::from_millis(900));
        assert!(!output.status.success());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: stderr={} command={command:?}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(value["code"], "deadline", "{value}");
        assert_eq!(value["complete"], false);
        assert_eq!(value["pendingSources"], json!(["upstream_read"]));
        assert_eq!(value["cursor"], Value::Null);
        if command[0] == "required-checks" {
            assert_eq!(value["available"], false);
            assert_eq!(value["state"], "unknown");
        } else {
            assert_eq!(value["available"], true);
            let oldest = if command[1] == "view" {
                "oldestValidationAtMs"
            } else {
                "oldest_validation_at_ms"
            };
            assert_eq!(value[oldest], baseline.oldest_validation_at_ms);
            assert!(!value["validations"].as_array().unwrap().is_empty());
        }
        assert!(
            c.status().outstanding_requests > 0,
            "caller canceled daemon work"
        );
        assert_eq!(c.bootstrap().await.unwrap().cursor, before.cursor);
        h.mock.release.notify_waiters();
        until(|| c.status().outstanding_requests == 0).await;
        h.mode("");
        // A timed-out read must not leave its report lock permanently held.
        tokio::time::timeout(
            Duration::from_secs(2),
            c.pr_report("acme/demo", 7, Freshness::Revalidate),
        )
        .await
        .unwrap()
        .unwrap();
        api.stop().await;
        task.abort();
    }
}

#[tokio::test]
async fn issue73_deadline_bounds_cold_cache_branch_resolution_and_backoff() {
    for (mode, command) in [
        (
            "issue72-stall-metadata",
            vec!["pr", "view", "7", "-R", "acme/demo"],
        ),
        ("issue73-backoff", vec!["required-checks", "acme/demo", "7"]),
        (
            "issue73-stall-list",
            vec!["pr", "view", "feature", "-R", "acme/demo"],
        ),
    ] {
        let h = Harness::new().await;
        h.mode(mode);
        let mut config = h.config();
        config.queue_timeout = Duration::from_secs(120);
        let c = Client::with_token(config, "synthetic-token".into()).unwrap();
        let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let router = api.router();
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let output = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
                .args(["--server", &base])
                .args(&command)
                .args(["--timeout", "1"])
                .output(),
        )
        .await
        .expect("cold read exceeded deadline")
        .unwrap();
        assert!(!output.status.success());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: stderr={} mode={mode}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(value["code"], "deadline", "{value}");
        assert_eq!(value["available"], false);
        assert_eq!(value["complete"], false);
        assert_eq!(value["validations"], json!([]));
        assert_eq!(value["oldestValidationAtMs"], Value::Null);
        if mode == "issue73-backoff" {
            assert!(c.status().outstanding_requests > 0);
            assert_eq!(
                h.calls()
                    .iter()
                    .filter(|call| call.path.ends_with("/pulls/7"))
                    .count(),
                1
            );
        }
        h.mock.release.notify_waiters();
        api.stop().await;
        task.abort();
    }
}

#[tokio::test]
async fn issue73_fast_success_cached_contention_and_option_validation() {
    let h = Harness::new().await;
    let c = h.client();
    c.pr_report("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let router = api.router();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for action in ["view", "checks"] {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
            .args([
                "--server",
                &base,
                "pr",
                action,
                "7",
                "-R",
                "acme/demo",
                "--timeout",
                "1",
            ])
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["complete"], true);
        assert!(value["deadlineExceeded"].is_null());
    }
    h.mode("issue72-stall-metadata");
    let before_calls = h.calls().len();
    let background = c.clone();
    let refresh = tokio::spawn(async move {
        background
            .pr_report("acme/demo", 7, Freshness::Revalidate)
            .await
    });
    issue72_wait_for_call(&h, before_calls, "/pulls/7").await;
    let calls = h.calls().len();
    for action in ["view", "checks"] {
        let output = tokio::time::timeout(
            Duration::from_millis(500),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
                .args([
                    "--server",
                    &base,
                    "pr",
                    action,
                    "7",
                    "-R",
                    "acme/demo",
                    "--cached-only",
                    "--timeout",
                    "1",
                ])
                .output(),
        )
        .await
        .expect("cached-only behavior became slow")
        .unwrap();
        assert!(output.status.success());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["complete"], true);
        assert!(value["deadlineExceeded"].is_null());
    }
    assert_eq!(h.calls().len(), calls);
    let cursor = c.bootstrap().await.unwrap().cursor;
    let sdk = hey_gh::ApiClient::new(base.parse().unwrap()).unwrap();
    assert!(
        sdk.changes(Some(&cursor), 100, Duration::ZERO)
            .await
            .unwrap()
            .changes
            .is_empty()
    );
    for command in [
        vec!["pr", "list", "--timeout", "1"],
        vec!["pr", "view", "7", "--timeout", "0"],
        vec!["pr", "checks", "7", "--timeout", "3601"],
        vec!["pr", "view", "7", "--timeout", "1", "--wait", "1"],
        vec!["pr", "view", "7", "--timeout", "1", "--cursor", "invalid"],
    ] {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
            .args(["--server", &base])
            .args(command)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("timeout"));
    }
    for command in [
        vec!["pr", "view", "--help"],
        vec!["pr", "checks", "--help"],
        vec!["required-checks", "--help"],
        vec!["ci", "--help"],
    ] {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_hey-gh"))
            .args(command)
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("--timeout"));
    }
    refresh.abort();
    h.mock.release.notify_waiters();
    api.stop().await;
    task.abort();
}

#[tokio::test]
async fn ruleset_only_policy_is_known_without_unnecessary_ancestry_reads() {
    let h = Harness::new().await;
    h.mode("ruleset-only-policy");
    h.phase(2);
    let c = h.client();
    let policy = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(policy.state, "satisfied", "{:?}", policy.errors);
    assert!(!policy.strict);
    assert!(policy.up_to_date.is_none());
    assert!(policy.errors.is_empty());
    assert!(!h.calls().iter().any(|call| call.path.contains("/compare/")));
    assert!(
        !h.calls().iter().any(|call| call.path.contains("/actions/")),
        "required status checks must not depend on workflow/job detail reads"
    );
    for phase in [3, 4] {
        h.phase(phase);
        let policy = c
            .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert_eq!(policy.state, "unknown");
        assert!(
            policy
                .errors
                .iter()
                .any(|error| error.source == "branch_protection")
        );
    }
    h.phase(5);
    let policy = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(policy.strict);
    assert_eq!(policy.state, "unknown");
    assert!(
        policy
            .errors
            .iter()
            .any(|error| error.source == "base_ancestry")
    );
    assert!(h.calls().iter().any(|call| call.path.contains("/compare/")));
    h.phase(6);
    let policy = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(policy.strict);
    assert_eq!(policy.up_to_date, Some(true));
    assert_eq!(policy.state, "satisfied", "{:?}", policy.errors);
    h.phase(7);
    let full_ci = c
        .ci_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(!full_ci.complete);
    assert!(
        full_ci
            .data
            .errors
            .iter()
            .any(|error| error.source.starts_with("workflow_runs"))
    );
    let calls = h.calls().len();
    let policy = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(policy.state, "satisfied", "{:?}", policy.errors);
    assert!(
        !h.calls()[calls..]
            .iter()
            .any(|call| call.path.contains("/actions/"))
    );
    let snapshots = rusqlite::Connection::open(h.config().cache_path).unwrap();
    let full_ci_snapshots: i64 = snapshots
        .query_row(
            "SELECT count(*) FROM snapshots WHERE resource LIKE 'ci://%/acme/demo/7'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        full_ci_snapshots, 0,
        "required-check success cannot publish complete CI or erase optional-source failure"
    );
}

#[tokio::test]
async fn required_policy_promotes_coalesced_requests_and_keeps_background_fair() {
    let h = Harness::new().await;
    h.mode("ruleset-only-policy");
    h.phase(2);
    let c = h.client();
    let gate_client = c.clone();
    let gate = tokio::spawn(async move {
        gate_client
            .get("foreground-gate", Freshness::Revalidate)
            .await
    });
    until(|| h.calls().iter().any(|call| call.path == "/foreground-gate")).await;
    let mut background = Vec::new();
    for n in 0..12 {
        let client = c.clone();
        background.push(tokio::spawn(async move {
            client
                .get(&format!("queued/{n}"), Freshness::Revalidate)
                .await
        }));
    }
    let client = c.clone();
    background.push(tokio::spawn(async move {
        client
            .get("repos/acme/demo/pulls/7", Freshness::Revalidate)
            .await
    }));
    until(|| c.status().outstanding_requests == 14).await;
    let client = c.clone();
    let foreground = tokio::spawn(async move {
        client
            .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
    });
    until(|| c.status().coalesced_requests >= 1).await;
    h.mock.release.notify_waiters();
    assert!(gate.await.unwrap().is_ok());
    assert_eq!(foreground.await.unwrap().unwrap().state, "satisfied");
    for job in background {
        assert!(job.await.unwrap().is_ok());
    }
    let calls = h.calls();
    assert_eq!(
        calls[1].path, "/repos/acme/demo/pulls/7",
        "interactive coalescing must promote already queued background work"
    );
    let first_background = calls
        .iter()
        .position(|call| call.path.starts_with("/queued/"))
        .unwrap();
    assert!(
        first_background <= 4,
        "background work must dispatch after at most three foreground requests"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.path == "/repos/acme/demo/pulls/7")
            .count(),
        2,
        "coalescing preserves one original request plus final confirmation"
    );
}

#[tokio::test]
async fn ruleset_cache_policy_preserves_required_evidence_and_strict_ancestry_errors() {
    let h = Harness::new().await;
    h.mode("ruleset-cache-policy");
    let c = h.client();
    let policy = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert_eq!(policy.state, "satisfied", "{:?}", policy.errors);
    assert_eq!(policy.checks[0].app_id, Some(15368));
    assert!(policy.errors.is_empty());
    assert!(!h.calls().iter().any(|call| call.path.contains("/compare/")));
    drop(c);

    // Restart with only the checks/statuses/policy cache: neither comparison
    // evidence nor a complete CI observation has ever been fetched.
    let c = h.client();
    let calls = h.calls().len();
    let cached = c
        .required_checks_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(cached.state, "satisfied", "{:?}", cached.errors);
    assert_eq!(cached.up_to_date, None);
    assert_eq!(h.calls().len(), calls);

    h.phase(2);
    let strict = c
        .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
        .await
        .unwrap();
    assert!(strict.strict);
    assert_eq!(strict.state, "unknown");
    assert!(strict.errors.iter().any(|e| e.source == "base_ancestry"));
    let calls = h.calls().len();
    let cached = c
        .required_checks_for_pr("acme/demo", 7, Freshness::CachedOnly)
        .await
        .unwrap();
    assert_eq!(cached.state, "unknown");
    assert!(
        cached
            .errors
            .iter()
            .any(|e| e.source == "base_ancestry" && e.message == Error::CacheMiss.to_string())
    );
    assert_eq!(h.calls().len(), calls);

    for (phase, state, source) in [
        (3, "unknown", Some("check_runs:")),
        (4, "missing", None),
        (5, "missing", None),
        (6, "unknown", Some("rulesets")),
        (7, "unknown", Some("rulesets")),
    ] {
        h.phase(phase);
        let policy = c
            .required_checks_for_pr("acme/demo", 7, Freshness::Revalidate)
            .await
            .unwrap();
        assert_eq!(policy.state, state, "phase {phase}: {:?}", policy.errors);
        if let Some(source) = source {
            assert!(policy.errors.iter().any(|e| e.source.starts_with(source)));
        }
    }
}

#[tokio::test]
async fn large_account_watch_publishes_replacements_while_foreground_read_progresses() {
    let h = Harness::new().await;
    h.mode("account-large");
    h.phase(2);
    let mut config = h.config();
    config.report_timeout = Duration::from_secs(10);
    config.queue_timeout = Duration::from_secs(10);
    config.min_spacing = Duration::from_millis(5);
    let c = Client::with_token(config, "synthetic-token".into()).unwrap();
    c.prepare_pr_status(Freshness::Revalidate).await.unwrap();
    let initial = c
        .pr_status_page(None, None, 1000, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(initial.pull_requests.len(), 25);
    assert!(!initial.complete);

    let gate_client = c.clone();
    let gate = tokio::spawn(async move {
        gate_client
            .get("foreground-gate", Freshness::Revalidate)
            .await
    });
    until(|| h.calls().iter().any(|call| call.path == "/foreground-gate")).await;
    let gate_index = h.calls().len() - 1;
    let mut queued = Vec::new();
    for n in 0..60 {
        let client = c.clone();
        queued.push(tokio::spawn(async move {
            client
                .get(&format!("queued/{n}"), Freshness::Revalidate)
                .await
        }));
    }
    until(|| c.status().outstanding_requests == 61).await;
    let api = hey_gh::api::Api::new(c.clone()).await.unwrap();
    api.watch_account(10).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/prs/acme/demo/7?refresh=true",
        listener.local_addr().unwrap()
    );
    let router = api.router();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let foreground = tokio::spawn(async move {
        reqwest::Client::new()
            .get(url)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    });
    // The foreground read joins metadata work queued by the account watch.
    until(|| c.status().coalesced_requests > 0).await;
    h.mock.release.notify_waiters();
    assert!(gate.await.unwrap().is_ok());
    // Dispatch order below proves priority without a machine-speed assertion.
    let report = tokio::time::timeout(Duration::from_secs(10), foreground)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report["complete"], true, "{report}");
    assert_eq!(report["data"]["ci"]["head_sha"], HEAD);
    assert_eq!(
        h.calls()[gate_index + 1].path,
        "/repos/acme/demo/pulls/7",
        "foreground work must progress ahead of the bulk queue"
    );
    for job in queued {
        assert!(job.await.unwrap().is_ok());
    }

    tokio::time::timeout(Duration::from_secs(35), async {
        loop {
            let page = c
                .pr_status_page(None, None, 1000, Duration::ZERO)
                .await
                .unwrap();
            let cycle = c.account_refresh_cycle(true).await.unwrap();
            if cycle.is_some_and(|cycle| cycle.succeeded > 0)
                && page.pull_requests.iter().all(|row| {
                    row["ci"]["summary"]["state"] == "success"
                        && row["sourceErrors"]["ci"].is_null()
                })
            {
                let updates = c
                    .pr_status_page(None, Some(&initial.cursor), 1000, Duration::ZERO)
                    .await
                    .unwrap();
                assert!(
                    !updates.changes.is_empty(),
                    "background replacements must reach the feed"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    api.stop().await;
    server.abort();
}
