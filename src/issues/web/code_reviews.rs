use super::*;
use crate::issues::code_reviews::Operation as Review;
use reqwest::blocking::Client;
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
    let repo = project
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
        .ok_or_else(|| Error::invalid("Select a GitHub project to publish"))?;
    let op = |operation| {
        app.execute_at(
            Some(project.clone()),
            Operation::CodeReview { operation },
            None,
            host.as_deref(),
        )
    };
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
    let client = Client::builder()
        .user_agent("hey-gh")
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Error::invalid("Cannot initialize GitHub transport"))?;
    let get = |url: &str| -> Result<Value> {
        let response = client
            .get(url)
            .bearer_auth(token)
            .send()
            .map_err(|_| Error::invalid("Could not reach GitHub to verify this review."))?;
        let status = response.status();
        let body = response.json::<Value>().map_err(|_| {
            Error::invalid(format!(
                "GitHub returned an unreadable response ({status})."
            ))
        })?;
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
    let response = client
        .post(format!("{url}/reviews"))
        .bearer_auth(token)
        .json(&payload)
        .send()
        .map_err(|_| {
            Error::conflict("GitHub did not confirm publication. Check the PR before retrying.")
        })?;
    if matches!(response.status().as_u16(), 401 | 403 | 404 | 422) {
        op(Review::RejectedPublication {
            id,
            version: version + 1,
        })?;
        let status = response.status();
        let body = response.json::<Value>().unwrap_or_default();
        let reason = body["message"].as_str().unwrap_or("No reason provided");
        return Err(Error::invalid(format!(
            "GitHub declined this review ({status}): {reason}. Your edits are saved."
        )));
    }
    let receipt = response
        .error_for_status()
        .and_then(|r| r.json::<Value>())
        .map_err(|_| {
            Error::conflict("No confirmed GitHub receipt. Check the PR before retrying.")
        })?;
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
