use super::*;
use crate::issues::code_reviews::Operation as Review;
use reqwest::blocking::{Client, RequestBuilder};
use std::process::{Command, Stdio};
#[cfg(target_os = "macos")]
#[path = "code_reviews_macos.rs"]
mod macos;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    project: String,
    host: Option<String>,
    id: i64,
    version: i64,
    digest: String,
}

pub(super) fn publish(app: &App, bytes: &[u8]) -> Result<Value> {
    let Publish {
        project,
        host,
        id,
        version,
        digest,
    } = serde_json::from_slice(bytes)?;
    let repo = github_repo(&project)?;
    let op = |operation| {
        app.execute_at(
            Some(project.clone()),
            Operation::CodeReview { operation },
            None,
            host.as_deref(),
        )
    };
    let token = personal_token()?;
    publish_with(
        &op,
        repo,
        id,
        version,
        &digest,
        "https://api.github.com",
        token.trim(),
    )
}
fn github_repo(project: &str) -> Result<&str> {
    project
        .strip_prefix("github.com/")
        .filter(|s| {
            s.split('/').count() == 2
                && s.split('/').all(|p| {
                    !p.is_empty()
                        && p != "."
                        && p != ".."
                        && p.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                })
        })
        .ok_or_else(|| Error::invalid("Select a GitHub project"))
}

fn personal_token() -> Result<String> {
    // Read the person's stored login; never use a worker's installation token.
    let token = Command::new("gh")
        .args(["auth", "token", "--hostname", "github.com"])
        .env("HEY_GH_NATIVE_CALL", "1")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_ENTERPRISE_TOKEN")
        .env_remove("GITHUB_ENTERPRISE_TOKEN")
        .stdin(Stdio::null())
        .output()?;
    let token = if token.status.success() {
        String::from_utf8(token.stdout).map_err(|_| Error::invalid("Invalid GitHub credential"))?
    } else {
        #[cfg(target_os = "macos")]
        {
            macos::desktop_token()?
        }
        #[cfg(not(target_os = "macos"))]
        {
            let reason = String::from_utf8_lossy(&token.stderr);
            return Err(Error::invalid(format!(
                "Code Reviews could not read the personal github.com credential: {}",
                if reason.trim().is_empty() {
                    "gh auth token failed; check gh auth status on the server"
                } else {
                    reason.trim()
                }
            )));
        }
    };
    Ok(token)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Diff {
    project: String,
    host: Option<String>,
    id: i64,
}

pub(super) fn diff(app: &App, bytes: &[u8]) -> Result<Value> {
    let Diff { project, host, id } = serde_json::from_slice(bytes)?;
    let repo = github_repo(&project)?;
    let value = app.execute_at(
        Some(project.clone()),
        Operation::CodeReview {
            operation: Review::View { id },
        },
        None,
        host.as_deref(),
    )?;
    let r = &value["review"];
    if !matches!(r["stage"].as_str(), Some("draft" | "submitted")) {
        return Err(Error::invalid(
            "Code is available for pending reviews only.",
        ));
    }
    let token = personal_token()?;
    let client =
        github_client().map_err(|_| Error::invalid("Cannot initialize GitHub transport"))?;
    let root = format!("https://api.github.com/repos/{repo}");
    let get = |url: String, accept: &str| {
        let mut client = client.clone();
        for attempt in 0..2 {
            let response = client
                .get(&url)
                .bearer_auth(token.trim())
                .header("Accept", accept)
                .send()
                .map_err(|_| Error::invalid("Could not load the PR diff from GitHub."))?;
            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            let body = response.json::<Value>().unwrap_or_default();
            if attempt == 0 && repair_route(&url, status, &body) {
                client = github_client()
                    .map_err(|_| Error::invalid("Cannot initialize GitHub transport"))?;
                continue;
            }
            return Err(Error::invalid(format!(
                "Could not load the PR diff from GitHub ({status}): {}",
                body["message"].as_str().unwrap_or("No reason provided")
            )));
        }
        unreachable!()
    };
    let number = r["number"]
        .as_i64()
        .ok_or_else(|| Error::invalid("Invalid PR number"))?;
    let pr: Value = get(
        format!("{root}/pulls/{number}"),
        "application/vnd.github+json",
    )?
    .json()
    .map_err(|_| Error::invalid("Invalid GitHub PR metadata."))?;
    let head = r["head"].as_str().unwrap_or_default();
    if pr["head"]["sha"].as_str() != Some(head) {
        return Err(Error::conflict(
            "This PR has changed since the review was written.",
        ));
    }
    let base = pr["base"]["sha"]
        .as_str()
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| Error::invalid("Missing PR base commit."))?;
    // Immutable comparison: a force-push between these reads cannot move the displayed lines.
    let mut bytes = Vec::new();
    const LIMIT: usize = 256 * 1024;
    get(
        format!("{root}/compare/{base}...{head}"),
        "application/vnd.github.diff",
    )?
    .take((LIMIT + 1) as u64)
    .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > LIMIT;
    if truncated {
        bytes.truncate(
            bytes[..LIMIT]
                .iter()
                .rposition(|&b| b == b'\n')
                .unwrap_or(0),
        );
    }
    Ok(json!({"ok": true, "patch": String::from_utf8_lossy(&bytes), "truncated": truncated}))
}

fn github_client() -> reqwest::Result<Client> {
    Client::builder()
        .user_agent("hey-gh")
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

fn repair_route(url: &str, status: reqwest::StatusCode, body: &Value) -> bool {
    if !url.starts_with("https://api.github.com/")
        || status != reqwest::StatusCode::FORBIDDEN
        || !body["message"].as_str().is_some_and(|message| {
            message.contains("IP allow list enabled")
                && message.contains("your IP address is not permitted")
        })
    {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("/usr/bin/sudo")
            .args([
                "-n",
                "/Library/PrivilegedHelperTools/local.github-api-vpn-route.sh",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(not(target_os = "macos"))]
    false
}

fn github_json(
    request: RequestBuilder,
    repair: impl Fn(&str, reqwest::StatusCode, &Value) -> bool,
) -> reqwest::Result<(reqwest::StatusCode, Value)> {
    // These requests have replayable bodies. Retry only an explicit rejection,
    // never a connection failure or an uncertain publication response.
    let mut request = request;
    for attempt in 0..2 {
        let retry = request.try_clone();
        let response = request.send()?;
        let url = response.url().to_string();
        let status = response.status();
        let body = match response.json() {
            Ok(body) => body,
            Err(_) if matches!(status.as_u16(), 401 | 403 | 404 | 422) => Value::Null,
            Err(error) => return Err(error),
        };
        if attempt == 0
            && status == reqwest::StatusCode::FORBIDDEN
            && let Some(retry) = retry
            && repair(&url, status, &body)
        {
            // The rejected connection may still use the previous network path.
            request = RequestBuilder::from_parts(github_client()?, retry.build()?);
            continue;
        }
        return Ok((status, body));
    }
    unreachable!()
}

fn publish_with(
    op: &impl Fn(Review) -> Result<Value>,
    repo: &str,
    id: i64,
    version: i64,
    digest: &str,
    api: &str,
    token: &str,
) -> Result<Value> {
    let value = op(Review::View { id })?;
    let review = &value["review"];
    if review["stage"] == "published" {
        return Ok(value);
    }
    if review["digest"] != digest || review["version"] != version || review["stage"] != "submitted"
    {
        return Err(Error::conflict(
            "Reload the submitted review before publishing",
        ));
    }
    let client =
        github_client().map_err(|_| Error::invalid("Cannot initialize GitHub transport"))?;
    let get = |url: &str| -> Result<Value> {
        let (status, body) = github_json(client.get(url).bearer_auth(token), repair_route)
            .map_err(|_| Error::invalid("Could not reach GitHub to verify this review."))?;
        if !status.is_success() {
            return Err(Error::invalid(format!(
                "GitHub could not verify this review ({status}): {}",
                body["message"].as_str().unwrap_or("No reason provided")
            )));
        }
        Ok(body)
    };
    let user = get(&format!("{api}/user"))?;
    if user["type"] != "User" {
        return Err(Error::invalid(
            "Publication requires a personal GitHub account",
        ));
    }
    let url = format!("{api}/repos/{repo}/pulls/{}", review["number"]);
    let pr = get(&url)?;
    let head = pr["head"]["sha"].as_str().ok_or_else(|| {
        Error::invalid("GitHub did not return a PR head. Try again before publishing.")
    })?;
    if review["head"] != head {
        op(Review::MarkOutdated { id, version })?;
        return Err(Error::conflict(
            "The PR head changed. Nothing was published. Review the new commit before submitting again.",
        ));
    }
    if user["id"] == pr["user"]["id"] && review["verdict"] != "COMMENT" {
        return Err(Error::invalid(
            "GitHub does not allow approving or requesting changes on your own PR",
        ));
    }
    let comments = review["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let mut anchor = c["anchor"].clone();
            anchor["body"] = c["body"].clone();
            anchor
        })
        .collect::<Vec<_>>();
    let payload = json!({"commit_id":review["head"],"body":review["body"],"event":review["verdict"],"comments":comments});
    op(Review::ClaimPublication {
        id,
        version,
        digest: digest.into(),
    })?;
    let (status, receipt) = github_json(
        client
            .post(format!("{url}/reviews"))
            .bearer_auth(token)
            .json(&payload),
        repair_route,
    )
    .map_err(|_| {
        Error::conflict("GitHub did not confirm publication. Check the PR before retrying.")
    })?;
    if matches!(status.as_u16(), 401 | 403 | 404 | 422) {
        op(Review::RejectedPublication {
            id,
            version: version + 1,
        })?;
        let reason = receipt["message"].as_str().unwrap_or("No reason provided");
        return Err(Error::invalid(format!(
            "GitHub declined this review ({status}): {reason}. Your edits are saved."
        )));
    }
    if !status.is_success() {
        return Err(Error::conflict(
            "No confirmed GitHub receipt. Check the PR before retrying.",
        ));
    }
    op(Review::FinishPublication {
        id,
        version: version + 1,
        author: user["login"]
            .as_str()
            .ok_or_else(|| Error::invalid("Missing personal GitHub login"))?
            .into(),
        receipt: receipt["html_url"]
            .as_str()
            .ok_or_else(|| Error::conflict("Missing GitHub review URL"))?
            .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_retries_only_repaired_rejections_and_only_once() {
        for method in [reqwest::Method::GET, reqwest::Method::POST] {
            for (first, message, repaired, last, requests) in [
                (
                    403,
                    "IP allow list enabled; your IP address is not permitted",
                    true,
                    200,
                    2,
                ),
                (
                    403,
                    "IP allow list enabled; your IP address is not permitted",
                    true,
                    403,
                    2,
                ),
                (
                    403,
                    "IP allow list enabled; your IP address is not permitted",
                    false,
                    403,
                    1,
                ),
                (403, "Resource not accessible", true, 403, 1),
                (500, "Server error", true, 500, 1),
                (200, "OK", true, 200, 1),
            ] {
                let server = Server::http(("127.0.0.1", 0)).unwrap();
                let url = format!("http://{}/repos/o/r/pulls/7/reviews", server.server_addr());
                let expected_method = method.to_string();
                let t = std::thread::spawn(move || {
                    for i in 0..requests {
                        let mut req = server
                            .recv_timeout(Duration::from_secs(5))
                            .unwrap()
                            .unwrap();
                        assert_eq!(req.method().as_str(), expected_method);
                        assert_eq!(
                            header(&req, "authorization"),
                            Some("Bearer fixture-personal")
                        );
                        let body: Value = serde_json::from_reader(req.as_reader()).unwrap();
                        assert_eq!(body, json!({"body":"exact review"}));
                        req.respond(
                            Response::from_string(json!({"message":message}).to_string())
                                .with_status_code(StatusCode(if i == 0 { first } else { last })),
                        )
                        .unwrap();
                    }
                });
                let repairs = std::cell::Cell::new(0);
                let (status, _) = github_json(
                    Client::new()
                        .request(method.clone(), &url)
                        .bearer_auth("fixture-personal")
                        .json(&json!({"body":"exact review"})),
                    |_, _, body| {
                        if !body["message"]
                            .as_str()
                            .unwrap_or_default()
                            .contains("IP allow list enabled")
                        {
                            return false;
                        }
                        repairs.set(repairs.get() + 1);
                        repaired
                    },
                )
                .unwrap();
                assert_eq!(status.as_u16(), last);
                assert_eq!(
                    repairs.get(),
                    usize::from(first == 403 && message.starts_with("IP allow"))
                );
                t.join().unwrap();
            }
        }
    }

    #[test]
    fn route_repair_ignores_other_hosts_and_other_errors() {
        let body = json!({"message":"IP allow list enabled; your IP address is not permitted"});
        assert!(!repair_route(
            "https://api.github.com.evil.test/",
            reqwest::StatusCode::FORBIDDEN,
            &body
        ));
        assert!(!repair_route(
            "https://api.github.com/",
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            &body
        ));
        assert!(!repair_route(
            "https://api.github.com/",
            reqwest::StatusCode::FORBIDDEN,
            &json!({"message":"Resource not accessible"})
        ));
    }

    #[test]
    fn publish_uses_personal_identity_and_exact_edits_and_never_retries_an_unknown_result() {
        for (state, own, head, status) in [
            ("open", false, "abc", 201),
            ("open", false, "new", 0),
            ("closed", false, "abc", 201),
            ("closed", false, "new", 0),
            ("open", true, "abc", 0),
            ("open", false, "abc", 422),
            ("open", false, "abc", 500),
        ] {
            let server = Server::http(("127.0.0.1", 0)).unwrap();
            let origin = format!("http://{}", server.server_addr());
            let t = std::thread::spawn(move || {
                for (i, expected) in ["/user", "/repos/o/r/pulls/7", "/repos/o/r/pulls/7/reviews"]
                    .iter()
                    .enumerate()
                {
                    if i == 2 && status == 0 {
                        break;
                    }
                    let mut req = server
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap()
                        .unwrap();
                    assert_eq!(req.url(), *expected);
                    assert_eq!(
                        header(&req, "authorization"),
                        Some("Bearer fixture-personal")
                    );
                    let mut code = 200;
                    let data = match i {
                        0 => json!({"type":"User","id":1,"login":"reviewer"}),
                        1 => {
                            json!({"head":{"sha":head},"state":state,"user":{"id":if own{1}else{2}}})
                        }
                        _ => {
                            assert_eq!(req.method(), &Method::Post);
                            let body: Value = serde_json::from_reader(req.as_reader()).unwrap();
                            assert_eq!(
                                body,
                                json!({"commit_id":"abc","event":"REQUEST_CHANGES","body":"Edited summary","comments":[{"path":"a.rs","line":4,"side":"RIGHT","body":"Edited inline"}]})
                            );
                            code = status;
                            if status == 422 {
                                json!({"message":"Review is not permitted"})
                            } else {
                                json!({"html_url":"https://github.com/o/r/pull/7#pullrequestreview-11"})
                            }
                        }
                    };
                    req.respond(
                        Response::from_string(data.to_string()).with_status_code(StatusCode(code)),
                    )
                    .unwrap()
                }
            });
            let stage = std::cell::Cell::new("submitted");
            let call = |op: Review| match op {
                Review::View { .. } => Ok(
                    json!({"review":{"stage":stage.get(),"digest":"fixture","version":2,"number":7,"head":"abc","verdict":"REQUEST_CHANGES","body":"Edited summary","comments":[{"anchor":{"path":"a.rs","line":4,"side":"RIGHT"},"body":"Edited inline"}]}}),
                ),
                Review::MarkOutdated { .. } => {
                    stage.set("outdated");
                    Ok(json!({}))
                }
                Review::ClaimPublication { .. } => {
                    assert_eq!(stage.replace("publishing"), "submitted");
                    Ok(json!({}))
                }
                Review::RejectedPublication { .. } => {
                    stage.set("submitted");
                    Ok(json!({}))
                }
                Review::FinishPublication {
                    receipt, author, ..
                } => {
                    assert_eq!(author, "reviewer");
                    assert!(receipt.ends_with("-11"));
                    stage.set("published");
                    Ok(json!({}))
                }
                _ => unreachable!(),
            };
            let r = publish_with(&call, "o/r", 9, 2, "fixture", &origin, "fixture-personal");
            assert_eq!(r.is_ok(), status == 201);
            assert_eq!(
                stage.get(),
                match status {
                    201 => "published",
                    500 => "publishing",
                    _ if head != "abc" => "outdated",
                    _ => "submitted",
                }
            );
            if status == 422 {
                assert!(
                    r.unwrap_err()
                        .to_string()
                        .contains("Review is not permitted")
                );
            }
            if status == 201 {
                assert!(
                    publish_with(&call, "o/r", 9, 2, "fixture", &origin, "fixture-personal")
                        .is_ok()
                )
            }
            if status == 500 {
                assert!(
                    publish_with(&call, "o/r", 9, 2, "fixture", &origin, "fixture-personal")
                        .is_err()
                )
            }
            t.join().unwrap();
        }
    }
}
