//! Let native gh perform selection, validation, stdin handling and formatting.
//! Only its GitHub transport changes for a project that opted into Code Reviews.
use crate::cli_route::{Invocation, option_values, positional_indices};
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct InvalidReview(String);

pub struct Target {
    pub repo: String,
    pub enabled: Vec<String>,
    pub auth: crate::cli_route::Auth,
}
pub fn select(i: &Invocation, bare: bool) -> Result<Option<Target>> {
    if i.help() {
        return Ok(None);
    }
    let positions = positional_indices(&i.args);
    let arg = |n| {
        positions
            .get(n)
            .and_then(|p: &usize| i.args[*p].to_str())
            .unwrap_or("")
    };
    let is_pr = i.root() == "pr" && matches!(i.subcommand(), "review" | "view");
    let endpoint = arg(1);
    let url = url::Url::parse(endpoint).ok();
    if url
        .as_ref()
        .is_some_and(|u| u.host_str() != Some("api.github.com"))
        || option_values(&i.args)
            .iter()
            .any(|(k, v)| k == "--hostname" && v.as_deref().is_some_and(|v| v != "github.com"))
    {
        return Ok(None);
    }
    let options = option_values(&i.args);
    if i.root() == "api" && url.is_none() {
        let host = options
            .iter()
            .rev()
            .find(|(k, _)| k == "--hostname")
            .and_then(|(_, v)| v.as_deref()?.to_str().map(str::to_owned))
            .or_else(|| std::env::var("GH_HOST").ok());
        if host.is_some_and(|h| !h.eq_ignore_ascii_case("github.com")) {
            return Ok(None);
        }
    }
    let endpoint = url
        .as_ref()
        .map(|u| u.path())
        .unwrap_or(endpoint)
        .trim_start_matches('/')
        .split('?')
        .next()
        .unwrap_or("");
    let graphql = i.root() == "api" && endpoint == "graphql";
    let path = endpoint.split('/').collect::<Vec<_>>();
    let api = i.root() == "api"
        && path.len() >= 5
        && path[0] == "repos"
        && path[3] == "pulls"
        && (path[4] == "comments"
            || path
                .get(5)
                .is_some_and(|s| matches!(*s, "comments" | "reviews")));
    if !is_pr && !api && !graphql {
        return Ok(None);
    }
    let repo = if api && !path[1].starts_with('{') && !path[2].starts_with('{') {
        Some(format!("{}/{}", path[1], path[2]))
    } else if is_pr && arg(2).starts_with("https://") {
        let url = url::Url::parse(arg(2))?;
        if url.host_str() != Some("github.com") {
            return Ok(None);
        }
        let mut parts = url.path_segments().ok_or("Invalid GitHub PR URL")?;
        Some(format!(
            "{}/{}",
            parts.next().unwrap_or(""),
            parts.next().unwrap_or("")
        ))
    } else {
        option_values(&i.args)
            .iter()
            .rev()
            .find(|(k, _)| matches!(k.as_str(), "-R" | "--repo"))
            .and_then(|(_, v)| v.as_ref()?.to_str().map(str::to_owned))
            .or_else(|| std::env::var("GH_REPO").ok())
    };
    if is_pr && repo.is_some() && crate::cli_auth::hostname(i, &options)? != "github.com" {
        return Ok(None);
    }
    // The standalone utility also runs where the project store is not installed.
    let listing = match Command::new("hey-boss")
        .args(["issue", "--json", "code-review-projects"])
        .stdin(Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(e) if !bare && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !listing.status.success() {
        return Err(
            "Could not check project review settings; refusing to guess the review destination"
                .into(),
        );
    }
    let enabled: Vec<String> = serde_json::from_slice(&listing.stdout)?;
    if enabled.is_empty() {
        return Ok(None);
    }
    let enabled: Vec<String> = enabled
        .iter()
        .filter_map(|p| p.strip_prefix("github.com/").map(str::to_owned))
        .collect();
    let auth = if bare {
        crate::cli_route::Auth::User
    } else {
        i.auth
    };
    if graphql {
        return Ok(Some(Target {
            repo: String::new(),
            enabled,
            auth,
        }));
    }
    let repo = if let Some(repo) = repo {
        repo
    } else {
        let out = Command::new("gh")
            .env("HEY_GH_NATIVE_CALL", "1")
            .args(["repo", "view", "--json", "url", "--jq", ".url"])
            .stdin(Stdio::null())
            .output()?;
        if !out.status.success() {
            if is_pr {
                // Let gh diagnose a missing checkout or invalid flags. Keep its transport guarded.
                return Ok(Some(Target {
                    repo: String::new(),
                    enabled,
                    auth,
                }));
            }
            return Err("Cannot resolve repository; specify -R OWNER/REPO or a PR URL".into());
        }
        String::from_utf8(out.stdout)?.trim().to_owned()
    };
    let repo = repo
        .trim_start_matches("https://")
        .trim_start_matches("github.com/")
        .trim_end_matches('/');
    Ok(enabled
        .iter()
        .find(|p| p.eq_ignore_ascii_case(repo))
        .map(|p| Target {
            repo: p.to_owned(),
            enabled: enabled.clone(),
            auth,
        }))
}
impl Target {
    pub fn store(&self, op: &Value) -> Result<Value> {
        let mut child = Command::new("hey-boss")
            .args([
                "issue",
                "--project",
                &format!("github.com/{}", self.repo),
                "--json",
                "code-review",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(op)?)?;
        let out = child.wait_with_output()?;
        let v: Value = serde_json::from_slice(&out.stdout)
            .map_err(|_| "Could not read local review database")?;
        if v["ok"] != true || !out.status.success() {
            let reason = v["error"]["message"]
                .as_str()
                .unwrap_or("Local review write failed");
            if matches!(
                v["error"]["code"].as_str(),
                Some("invalid_input" | "conflict" | "not_found" | "identity_unavailable")
            ) {
                return Err(InvalidReview(reason.into()).into());
            }
            return Err(reason.to_owned().into());
        }
        Ok(v)
    }
}
pub fn metadata(repo: &str, n: u64) -> Result<Value> {
    let out = Command::new("gh")
        .env("HEY_GH_NATIVE_CALL", "1")
        .args(["api", &format!("repos/{repo}/pulls/{n}")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err("Cannot read PR metadata using personal GitHub credentials".into());
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}
fn comment_json(c: &Value, r: &Value) -> Value {
    let mut x = c["anchor"].clone();
    for (k, v) in [
        ("id", &c["id"]),
        ("body", &c["body"]),
        ("commit_id", &r["head"]),
        ("diff_hunk", &c["snippet"]),
        ("pull_request_review_id", &r["id"]),
    ] {
        x[k] = v.clone()
    }
    x
}
pub fn review_json(r: &Value) -> Value {
    json!({"id":r["id"],"body":r["body"],"commit_id":r["head"],"state":if r["stage"]=="draft"{"PENDING"}else{match r["verdict"].as_str(){Some("APPROVE")=>"APPROVED",Some("REQUEST_CHANGES")=>"CHANGES_REQUESTED",_=>"COMMENTED"}}})
}
impl Target {
    pub fn rest(&self, method: &str, path: &str, input: &Value) -> Result<Option<Value>> {
        let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
        if parts.len() < 6
            || parts[0] != "repos"
            || parts[3] != "pulls"
            || !matches!(parts[5], "comments" | "reviews")
        {
            return Ok(None);
        }
        if !format!("{}/{}", parts[1], parts[2]).eq_ignore_ascii_case(&self.repo) {
            return Err(InvalidReview("Review repository changed during command".into()).into());
        }
        let n = parts[4].parse::<u64>()?;
        if method == "GET" && parts[5] == "reviews" && parts.len() >= 7 {
            let id = parts[6].parse::<i64>()?;
            let local = self.store(&json!({"command":"for_pr","number":n}))?;
            if let Some(r) = local["reviews"].as_array().into_iter().flatten().find(|r| {
                r["id"] == id
                    && matches!(
                        r["stage"].as_str(),
                        Some("draft" | "submitted" | "publishing")
                    )
            }) {
                return Ok(Some(if parts.len() == 7 {
                    review_json(r)
                } else if parts.get(7) == Some(&"comments") {
                    Value::Array(
                        r["comments"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|c| comment_json(c, r))
                            .collect(),
                    )
                } else {
                    return Err(InvalidReview("Unknown local review endpoint".into()).into());
                }));
            }
        }
        if method == "GET" {
            return Ok(None);
        }
        if method == "POST" && parts.len() == 8 && parts[5] == "reviews" && parts[7] == "events" {
            let id = parts[6].parse::<i64>()?;
            let v = self.store(&json!({"command":"view","id":id}))?;
            let r = &v["review"];
            if r["number"] != n || r["stage"] != "draft" {
                return Err(InvalidReview("No matching pending review".into()).into());
            }
            return self.rest(
                "POST",
                &format!("/repos/{}/pulls/{n}/reviews", self.repo),
                &json!({"event":input["event"],"body":input["body"],"commit_id":r["head"]}),
            );
        }
        if method != "POST" || parts.len() != 6 {
            return Err(InvalidReview(
                "This review write is not supported by the local review store".into(),
            )
            .into());
        }
        if parts[5] == "comments" && input["commit_id"].as_str().is_none() {
            return Err(InvalidReview("Inline review comments require commit_id".into()).into());
        }
        let pr = metadata(&self.repo, n)?;
        let head = pr["head"]["sha"].as_str().ok_or("Missing PR head")?;
        if input["commit_id"].as_str().is_some_and(|s| s != head) {
            return Err(InvalidReview(
                "PR head changed; review the latest diff before submitting".into(),
            )
            .into());
        }
        let mut comments = if parts[5] == "comments" {
            vec![input.clone()]
        } else {
            input["comments"].as_array().cloned().unwrap_or_default()
        };
        if !comments.is_empty() {
            let out = Command::new("gh")
                .env("HEY_GH_NATIVE_CALL", "1")
                .args([
                    "api",
                    "--paginate",
                    "--slurp",
                    &format!("repos/{}/pulls/{n}/files?per_page=100", self.repo),
                ])
                .stdin(Stdio::null())
                .output()?;
            if !out.status.success() {
                return Err("Cannot verify PR diff".into());
            }
            let pages: Vec<Vec<Value>> = serde_json::from_slice(&out.stdout)?;
            for c in &mut comments {
                if c["side"].is_null() {
                    c["side"] = json!("RIGHT")
                }
                let patch = pages
                    .iter()
                    .flatten()
                    .find(|f| f["filename"] == c["path"])
                    .and_then(|f| f["patch"].as_str())
                    .ok_or_else(|| InvalidReview("File has no reviewable diff".into()))?;
                c["snippet"] = json!(snippet(
                    patch,
                    c["side"]
                        .as_str()
                        .ok_or_else(|| InvalidReview("Invalid side".into()))?,
                    c["line"]
                        .as_u64()
                        .ok_or_else(|| InvalidReview("Expected diff line".into()))?
                )?);
                let obj = c
                    .as_object_mut()
                    .ok_or_else(|| InvalidReview("Invalid inline comment".into()))?;
                obj.remove("commit_id");
            }
            if metadata(&self.repo, n)?["head"]["sha"] != head {
                return Err(InvalidReview("PR changed while reading the diff".into()).into());
            }
        }
        let r=self.store(&json!({"command":"record","number":n,"title":pr["title"],"head":head,"comments":comments,"verdict":if parts[5]=="reviews"{input["event"].clone()}else{Value::Null},"body":if parts[5]=="reviews"{input["body"].clone()}else{Value::Null}}))?;
        let r = &r["review"];
        if parts[5] == "comments" {
            let c = r["comments"]
                .as_array()
                .and_then(|cs| {
                    cs.iter().rev().find(|c| {
                        c["body"] == input["body"]
                            && c["anchor"]["path"] == input["path"]
                            && c["anchor"]["line"] == input["line"]
                    })
                })
                .ok_or("Missing stored comment")?;
            Ok(Some(comment_json(c, r)))
        } else {
            Ok(Some(review_json(r)))
        }
    }
}
fn snippet(patch: &str, side: &str, line: u64) -> Result<String> {
    let lines = patch.lines().collect::<Vec<_>>();
    let (mut old, mut new) = (0, 0);
    let mut hunk = None;
    for (i, s) in lines.iter().enumerate() {
        if s.starts_with("@@ ") {
            let mut words = s.split_whitespace().skip(1);
            let start =
                |w: Option<&str>| -> Option<u64> { w?.get(1..)?.split(',').next()?.parse().ok() };
            old = start(words.next()).ok_or("Invalid diff hunk")?;
            new = start(words.next()).ok_or("Invalid diff hunk")?;
            hunk = Some(i);
            continue;
        }
        let Some(h) = hunk else { continue };
        let first = s.as_bytes().first().copied();
        let left = matches!(first, Some(b' ' | b'-'));
        let right = matches!(first, Some(b' ' | b'+'));
        if (side == "LEFT" && left && line == old) || (side == "RIGHT" && right && line == new) {
            let end = (i + 3).min(lines.len());
            let begin = i.saturating_sub(2).max(h + 1);
            let before = &lines[begin..i];
            let slice = lines[begin..end]
                .iter()
                .take_while(|l| !l.starts_with("@@"))
                .copied()
                .collect::<Vec<_>>();
            let count = |lines: &[&str], skip: char| {
                lines
                    .iter()
                    .filter(|l| !l.starts_with(skip) && !l.starts_with('\\'))
                    .count() as u64
            };
            return Ok(format!(
                "@@ -{},{} +{},{} @@\n{}",
                old - count(before, '+'),
                count(&slice, '+'),
                new - count(before, '-'),
                count(&slice, '-'),
                slice.join("\n")
            ));
        }
        old += left as u64;
        new += right as u64
    }
    Err(InvalidReview("Line is outside the PR diff".into()).into())
}

use axum::{
    body::Body,
    http::{Request, Response},
};
use std::sync::Arc;
pub async fn relay(
    target: Arc<Target>,
    client: &reqwest::Client,
    socket_upstream: bool,
    request: Request<Body>,
) -> std::result::Result<Response<Body>, String> {
    let (parts, body) = request.into_parts();
    if parts.headers.get("host").and_then(|h| h.to_str().ok()) != Some("api.github.com") {
        return Err("Unexpected host for GitHub review".into());
    }
    let path = parts.uri.path().to_owned();
    let method = parts.method.as_str().to_owned();
    let bytes = axum::body::to_bytes(body, 16 * 1024 * 1024)
        .await
        .map_err(|_| "Review input exceeds 16 MiB")?;
    let input: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).map_err(|_| "Expected a GitHub JSON request")?
    };
    let make_json = |value: Value, status: u16| {
        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .unwrap()
    };
    if method == "GET" && path.split('/').count() >= 8 && path.contains("/reviews/") {
        let t = target.clone();
        let p = path.clone();
        if let Some(v) = tokio::task::spawn_blocking(move || {
            t.rest("GET", &p, &Value::Null).map_err(|e| e.to_string())
        })
        .await
        .map_err(|_| "Review read failed")??
        {
            return Ok(make_json(v, 200));
        }
    }
    let mut use_app = path == "/graphql"
        && method == "GET"
        && target.repo.is_empty()
        && target.auth == crate::cli_route::Auth::App;
    if path == "/graphql" && method != "GET" {
        let t = target.clone();
        let i = input.clone();
        let decision = tokio::task::spawn_blocking(move || crate::review_graphql::handle(&t, &i))
            .await
            .map_err(|_| "GraphQL review inspection failed")?;
        let decision = match decision {
            Ok(d) => d,
            Err(reason) => {
                return Ok(make_json(
                    json!({"errors":[{"message":format!("Code Reviews: {reason}")}]}),
                    200,
                ));
            }
        };
        match decision {
            crate::review_graphql::Decision::Local(v) => return Ok(make_json(v, 200)),
            d => {
                let needs_app = target.auth == crate::cli_route::Auth::App
                    || target.auth == crate::cli_route::Auth::Auto
                        && matches!(d, crate::review_graphql::Decision::Forward(true));
                use_app = needs_app && target.repo.is_empty();
            }
        }
    } else if !matches!(method.as_str(), "GET" | "HEAD") {
        let t = target.clone();
        let p = path.clone();
        let v = tokio::task::spawn_blocking(move || {
            t.rest(&method, &p, &input).map_err(|e| {
                let status = if e.is::<InvalidReview>() { 422 } else { 502 };
                (status, e.to_string())
            })
        })
        .await
        .map_err(|_| "Review storage failed")?;
        return Ok(match v {
            Ok(Some(v)) => make_json(v, 201),
            Ok(None) => make_json(
                json!({"message":"Code Reviews: unsupported review operation; nothing was sent to GitHub"}),
                422,
            ),
            Err((status, reason)) => {
                make_json(json!({"message":format!("Code Reviews: {reason}")}), status)
            }
        });
    }
    let scheme = if socket_upstream { "http" } else { "https" };
    let url = format!(
        "{scheme}://api.github.com{}",
        parts
            .uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/")
    );
    if use_app {
        let token = tokio::task::spawn_blocking(|| {
            crate::cli_auth::app_token("github.com").map_err(|e| e.to_string())
        })
        .await
        .map_err(|_| "App token lookup failed")??;
        return crate::cli_identity::forward_review_app(
            client,
            Request::from_parts(parts, Body::from(bytes)),
            url.parse().map_err(|_| "Invalid GitHub URL")?,
            token,
        )
        .await;
    }
    let mut headers = parts.headers;
    for name in [
        "host",
        "connection",
        "content-length",
        "transfer-encoding",
        "accept-encoding",
    ] {
        headers.remove(name);
    }
    let upstream = client.request(parts.method, url).headers(headers);
    let upstream = upstream
        .body(bytes)
        .send()
        .await
        .map_err(|_| "GitHub review read failed")?;
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let mut response = Response::builder().status(status);
    for (name, value) in &headers {
        if !matches!(
            name.as_str(),
            "content-length" | "transfer-encoding" | "connection" | "content-encoding"
        ) {
            response = response.header(name, value)
        }
    }
    let raw = upstream
        .bytes()
        .await
        .map_err(|_| "Incomplete GitHub response")?;
    if status.is_success()
        && !headers
            .get("link")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|s| s.contains("rel=\"next\""))
    {
        if let Ok(mut v) = serde_json::from_slice::<Value>(&raw) {
            let rest = path.trim_matches('/').split('/').collect::<Vec<_>>();
            let pr = v.pointer("/data/repository/pullRequest");
            let gql_number = pr
                .and_then(|p| p["number"].as_u64())
                .or_else(|| input.pointer("/variables/number").and_then(Value::as_u64));
            let overlays = if rest.len() == 6
                && rest[0] == "repos"
                && rest[3] == "pulls"
                && matches!(rest[5], "reviews" | "comments")
                && rest[1..3].join("/").eq_ignore_ascii_case(&target.repo)
            {
                rest[4].parse().ok()
            } else if pr.is_some_and(|p| p.get("reviews").is_some()) {
                gql_number
            } else {
                None
            };
            if let Some(n) = overlays.filter(|_| !target.repo.is_empty()) {
                let t = target.clone();
                let local = tokio::task::spawn_blocking(move || {
                    t.store(&json!({"command":"for_pr","number":n}))
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|_| "Local review read failed")??;
                for r in local["reviews"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|r| {
                        matches!(
                            r["stage"].as_str(),
                            Some("draft" | "submitted" | "publishing")
                        )
                    })
                {
                    if path == "/graphql" {
                        if let Some(nodes) = v
                            .pointer_mut("/data/repository/pullRequest/reviews/nodes")
                            .and_then(Value::as_array_mut)
                        {
                            nodes.push(json!({"id":format!("local-{}",r["id"]),"body":r["body"],"state":review_json(r)["state"],"author":{"login":"local"},"commit":{"oid":r["head"]}}))
                        }
                    } else if let Some(a) = v.as_array_mut() {
                        if rest[5] == "reviews" {
                            a.push(review_json(r))
                        } else {
                            for c in r["comments"].as_array().into_iter().flatten() {
                                a.push(comment_json(c, r))
                            }
                        }
                    }
                }
                return response
                    .body(Body::from(v.to_string()))
                    .map_err(|e| e.to_string());
            }
        }
    }
    response.body(Body::from(raw)).map_err(|e| e.to_string())
}
