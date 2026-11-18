use crate::*;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    os::unix::process::CommandExt,
    time::Duration,
};
use tiny_http::{Header, Method, Response, Server};

const LIMIT: u64 = 12 * 1024 * 1024;

fn snapshot(sha: &str) -> Result<String> {
    let output = Command::new("git")
        .args([
            "show",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--find-renames",
            "--format=%H%x00%h%x00%an%x00%ae%x00%aI%x00%s%x00%b%x1eGIT_SHELF_META%x1e",
            "--patch",
            &format!("{sha}^{{commit}}"),
            "--",
        ])
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_string()
            .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn open(sha: &str) -> Result<()> {
    let data = serde_json::to_vec(&snapshot(sha)?)?;
    if data.len() as u64 > LIMIT {
        return Err("Commit review exceeds 12 MiB".into());
    }
    let mut child = Command::new(std::env::current_exe()?)
        .arg("serve-review")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let opened = (|| -> Result<()> {
        child.stdin.take().unwrap().write_all(&data)?;
        let mut url = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut url)?;
        if !url.starts_with("http://127.0.0.1:") {
            return Err("Could not start the local commit viewer".into());
        }
        let result = Command::new("hey-boss")
            .args(["browser", "open", url.trim()])
            .output()?;
        let reply: Value = serde_json::from_slice(&result.stdout).unwrap_or(Value::Null);
        if !result.status.success() || !reply["result"]["session"].is_string() {
            return Err(format!(
                "Could not open the commit on the connected Mac: {}",
                reply["error"]["message"]
                    .as_str()
                    .unwrap_or_else(|| std::str::from_utf8(&result.stderr)
                        .unwrap_or("No browser acknowledgment")
                        .trim())
            )
            .into());
        }
        println!("Opened commit {sha} on the connected Mac");
        Ok(())
    })();
    if opened.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    opened
}

pub fn serve() -> Result<()> {
    let mut data = Vec::new();
    std::io::stdin().take(LIMIT + 1).read_to_end(&mut data)?;
    if data.len() as u64 > LIMIT {
        return Err("Commit review exceeds 12 MiB".into());
    }
    let mut raw = serde_json::from_slice::<String>(&data)?;
    let mut sha = raw
        .split('\0')
        .next()
        .ok_or("Missing commit ID")?
        .to_owned();
    let target = crate::edit::target(&sha);
    let (branch, mut head, edit_error) = match target {
        Ok((branch, head)) => (branch, head, None),
        Err(error) => (String::new(), String::new(), Some(error.to_string())),
    };
    data = serde_json::to_vec(
        &json!({"raw":raw,"message":crate::edit::body(&sha)?,"branch":branch,"edit_error":edit_error}),
    )?;
    let mut random = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    let prefix = format!(
        "/{}/",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let server = Server::http("127.0.0.1:0").map_err(|e| e.to_string())?;
    println!("http://{}{prefix}", server.server_addr());
    std::io::stdout().flush()?;
    while let Some(mut request) = server.recv_timeout(Duration::from_secs(30 * 60))? {
        let mut saved = None;
        if request.method() == &Method::Post && request.url() == format!("{prefix}message") {
            let result = (|| -> Result<()> {
                if !request
                    .headers()
                    .iter()
                    .any(|h| h.field.equiv("X-Git-Shelf-Review") && h.value.as_str() == prefix)
                {
                    return Err("Missing review token".into());
                }
                if let Some(error) = &edit_error {
                    return Err(error.clone().into());
                }
                let mut bytes = Vec::new();
                request
                    .as_reader()
                    .take(256 * 1024 + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > 256 * 1024 {
                    return Err("Message request is too large".into());
                }
                let input: Value = serde_json::from_slice(&bytes)?;
                if input["sha"].as_str() != Some(&sha) {
                    return Err("Commit changed; reload this review before saving".into());
                }
                let (next, next_head) =
                    crate::edit::message(&sha, &branch, &head, field(&input, "message")?)?;
                sha = next;
                head = next_head;
                raw = snapshot(&sha)?;
                data = serde_json::to_vec(
                    &json!({"raw":raw,"message":crate::edit::body(&sha)?,"branch":branch,"edit_error":null}),
                )?;
                Ok(())
            })();
            saved = Some(match result {
                Ok(()) => (200, data.clone()),
                Err(error) => (
                    409,
                    serde_json::to_vec(&json!({"error":error.to_string()}))?,
                ),
            });
        }
        let asset = request.url().strip_prefix(&prefix).and_then(asset);
        let (status, mime, body) = if let Some((status, body)) = &saved {
            (*status, "application/json", body.as_slice())
        } else if request.method() != &Method::Get {
            (405, "text/plain", &b"Method not allowed"[..])
        } else if request.url() == format!("{prefix}commit.json") {
            (200, "application/json", data.as_slice())
        } else if let Some((mime, body)) = asset {
            (200, mime, body)
        } else {
            (404, "text/plain", &b"Not found"[..])
        };
        let mut response = Response::from_data(body).with_status_code(status);
        for (name, value) in [
            ("Content-Type", mime),
            ("Cache-Control", "no-store"),
            ("X-Content-Type-Options", "nosniff"),
            ("Referrer-Policy", "no-referrer"),
            (
                "Content-Security-Policy",
                "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'",
            ),
        ] {
            response.add_header(Header::from_bytes(name, value).unwrap());
        }
        let _ = request.respond(response);
    }
    Ok(())
}

fn asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    macro_rules! web {
        ($file:literal) => {
            include_bytes!(concat!("../../../src/issues/web/", $file)).as_slice()
        };
    }
    Some(match path {
        "" => ("text/html; charset=utf-8", include_bytes!("review.html")),
        "review.js" => ("text/javascript", include_bytes!("review.js")),
        "components.css" => ("text/css", web!("components.css")),
        "commit-review.css" => ("text/css", web!("commit-review.css")),
        "diff.js" => ("text/javascript", web!("git-review/diff.js")),
        "git-page.js" => ("text/javascript", web!("git-review/git-page.js")),
        "git-rows.js" => ("text/javascript", web!("git-review/git-rows.js")),
        "git-files.js" => ("text/javascript", web!("git-review/git-files.js")),
        "git-activity.js" => ("text/javascript", web!("git-review/git-activity.js")),
        "git-layout.js" => ("text/javascript", web!("git-review/git-layout.js")),
        _ => return None,
    })
}
