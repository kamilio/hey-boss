use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Web {
    child: Child,
    root: PathBuf,
    authority: String,
    project: String,
    token: String,
}
struct Reply {
    status: u16,
    headers: String,
    body: Vec<u8>,
}
impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}
impl Web {
    fn start() -> Self {
        Self::start_with_args(&[])
    }
    fn start_with_args(args: &[&str]) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("out")
            .join(format!(
                "issues-web-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&root)
            .env("HEY_BOSS_ISSUE_DB", root.join("issues.db"))
            .env("HEY_BOSS_INBOX_SOCKET", root.join("inbox.sock"))
            .env(
                "HEY_BOSS_CODEX",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex-worker.py"),
            )
            .env("HEY_BOSS_TEST_CLI", env!("CARGO_BIN_EXE_hey-boss"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args([
                "issue",
                "web",
                "--no-discovery",
                "--port",
                "0",
                "--agent",
                "human:web-test",
                "--json",
            ])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let ready: Value = serde_json::from_str(&line).unwrap();
        let authority = ready["url"]
            .as_str()
            .unwrap()
            .strip_prefix("http://")
            .unwrap()
            .trim_end_matches('/')
            .into();
        let mut web = Self {
            child,
            root,
            authority,
            project: String::new(),
            token: String::new(),
        };
        let boot = web.http("GET", "/api/bootstrap", &[], b"").json();
        web.project = boot["project"]["id"].as_str().unwrap().into();
        web.token = boot["csrf"].as_str().unwrap().into();
        web
    }
    fn http(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
        let mut stream = TcpStream::connect(&self.authority).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let host = headers
            .iter()
            .find(|h| h.0.eq_ignore_ascii_case("host"))
            .map(|h| h.1)
            .unwrap_or(&self.authority);
        write!(stream,"{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n",body.len()).unwrap();
        for (name, value) in headers {
            if !name.eq_ignore_ascii_case("host") {
                write!(stream, "{name}: {value}\r\n").unwrap();
            }
        }
        stream.write_all(b"\r\n").unwrap();
        stream.write_all(body).unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let end = bytes.windows(4).position(|s| s == b"\r\n\r\n").unwrap();
        let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        let mut body = bytes[end + 4..].to_vec();
        if headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Transfer-Encoding: chunked"))
        {
            let mut decoded = Vec::new();
            let mut position = 0;
            loop {
                let end = body[position..]
                    .windows(2)
                    .position(|s| s == b"\r\n")
                    .unwrap()
                    + position;
                let length = usize::from_str_radix(
                    std::str::from_utf8(&body[position..end])
                        .unwrap()
                        .split(';')
                        .next()
                        .unwrap(),
                    16,
                )
                .unwrap();
                position = end + 2;
                if length == 0 {
                    break;
                }
                decoded.extend_from_slice(&body[position..position + length]);
                position += length;
                assert_eq!(&body[position..position + 2], b"\r\n");
                position += 2;
            }
            body = decoded;
        }
        Reply {
            status,
            headers,
            body,
        }
    }
    fn action(&self, project: &str, operation: Value, key: Option<&str>) -> Reply {
        self.http(
            "POST",
            "/api/action",
            &[
                ("Content-Type", "application/json"),
                ("X-Hey-Boss-CSRF", &self.token),
            ],
            serde_json::to_string(
                &json!({"project":project,"operation":operation,"request_id":key}),
            )
            .unwrap()
            .as_bytes(),
        )
    }
    fn ok(&self, operation: Value) -> Value {
        let r = self.action(&self.project, operation, None);
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        r.json()
    }
}
impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn mobile_proxy_uses_the_authoritative_database_and_preserves_origin_and_csrf_checks() {
    let origin = "https://mac.example.ts.net:8443";
    let host = "mac.example.ts.net:8443";
    let web = Web::start_with_args(&["--mobile-origin", origin]);
    for path in ["/", "/app.js", "/api/bootstrap", "/workers"] {
        let reply = web.http("GET", path, &[("Host", host)], b"");
        assert_eq!(reply.status, 200);
        assert!(
            reply
                .headers
                .to_lowercase()
                .contains("cache-control: no-store")
        );
    }
    let boot = web
        .http("GET", "/api/bootstrap", &[("Host", host)], b"")
        .json();
    assert_eq!(boot["csrf"], web.token);
    assert_eq!(boot["actor"]["id"], "human:boss");
    let body = json!({"project":web.project,"operation":{"action":"create","title":"From phone","body":"Sensitive synthetic body","labels":[]},"request_id":"phone-create"}).to_string();
    let headers = [
        ("Host", host),
        ("Origin", origin),
        ("Sec-Fetch-Site", "same-origin"),
        ("Content-Type", "application/json"),
        ("X-Hey-Boss-CSRF", &web.token),
    ];
    assert_eq!(
        web.http("POST", "/api/action", &headers, body.as_bytes())
            .status,
        200
    );
    // The local browser and CLI see the phone's write in the original store.
    assert_eq!(
        web.ok(json!({"action":"view","number":1}))["issue"]["body"],
        "Sensitive synthetic body"
    );
    let cli = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .current_dir(&web.root)
        .env("HEY_BOSS_ISSUE_DB", web.root.join("issues.db"))
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .args(["issue", "view", "1", "--project", &web.project, "--json"])
        .output()
        .unwrap();
    assert!(cli.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&cli.stdout).unwrap()["issue"]["title"],
        "From phone"
    );
    for (name, value) in [
        ("Host", "other.example.ts.net:8443"),
        ("Origin", "https://evil.example"),
        ("Origin", "http://mac.example.ts.net:8443"),
        ("Origin", "null"),
        ("Origin", &format!("http://{}", web.authority)),
        ("Sec-Fetch-Site", "cross-site"),
        ("X-Hey-Boss-CSRF", "wrong"),
    ] {
        let mut invalid = headers;
        invalid.iter_mut().find(|h| h.0 == name).unwrap().1 = value;
        assert_eq!(
            web.http("POST", "/api/action", &invalid, body.as_bytes())
                .status,
            403,
            "{name}: {value}"
        );
    }
    assert_eq!(
        web.http("POST", "/api/action", &headers[..4], body.as_bytes())
            .status,
        403
    );
    let desktop = Web::start();
    assert_eq!(
        desktop
            .http("GET", "/api/bootstrap", &[("Host", host)], b"")
            .status,
        403
    );
    assert_eq!(
        desktop
            .http(
                "GET",
                "/api/bootstrap",
                &[("X-Forwarded-Host", host), ("X-Forwarded-Proto", "https")],
                b""
            )
            .status,
        200
    );
}

#[test]
fn embedded_assets_and_markdown_are_same_origin_and_script_safe() {
    let web = Web::start();
    for (path, kind) in [
        ("/", "text/html"),
        ("/app.js", "text/javascript"),
        ("/project-settings.js", "text/javascript"),
        ("/tags.js", "text/javascript"),
        ("/app.css", "text/css"),
        ("/workers", "text/html"),
        ("/agents", "text/html"),
        ("/agents/session", "text/html"),
        ("/fleet.js", "text/javascript"),
        ("/fleet.css", "text/css"),
        ("/icon.png", "image/png"),
    ] {
        let r = web.http("GET", path, &[], b"");
        assert_eq!(r.status, 200);
        assert!(r.headers.contains(kind));
        assert!(r.headers.to_lowercase().contains("content-security-policy"));
        assert!(r.headers.contains("frame-ancestors 'none'"));
        assert!(r.headers.contains("no-store"));
        assert!(!r.body.is_empty());
    }
    let body = "# Description\n\n<script>window.pwned=1</script>\n\n[bad](javascript:alert(1))\n\n- [ ] Test\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
    let reply = web.http(
        "POST",
        "/api/preview",
        &[
            ("Content-Type", "application/json"),
            ("X-Hey-Boss-CSRF", &web.token),
        ],
        json!({"body":body}).to_string().as_bytes(),
    );
    assert_eq!(reply.status, 200);
    let data = reply.json();
    let html = data["html"].as_str().unwrap();
    assert!(html.contains("<h1>Description</h1>"));
    assert!(html.contains("<table>"));
    assert!(html.contains("type=\"checkbox\""));
    assert!(!html.contains("<script>"));
    assert!(!html.contains("href=\"javascript:"));
    assert!(html.contains("&lt;script&gt;"));
}

#[test]
fn progress_is_read_only_on_the_web_and_history_is_a_separate_read() {
    let web = Web::start();
    let created = web.ok(json!({"action":"create","title":"Progress","body":"","labels":[]}));
    let number = created["issue"]["number"].as_i64().unwrap();
    let write = web.action(
        &web.project,
        json!({"action":"status","number":number,"level":"green","comment":"Checking the layout."}),
        Some("web-progress"),
    );
    assert_eq!(write.status, 403);
    assert_eq!(write.json()["error"]["code"], "forbidden");
    assert!(
        web.ok(json!({"action":"status_history","number":number,"limit":20,"offset":0}))["updates"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let current = web.ok(json!({"action":"status_view","number":number}));
    assert!(current["status"].is_null());
    assert!(current.get("issue").is_none());
    assert!(current.get("body").is_none());
    let large = web.ok(json!({"action":"create","title":"Large description","body":"x".repeat(1_000_000),"labels":[]}));
    let current = web.ok(json!({"action":"status_view","number":large["issue"]["number"]}));
    assert!(serde_json::to_vec(&current).unwrap().len() < 2000);
    for (path, kind) in [
        ("/status.js", "text/javascript"),
        ("/status.css", "text/css"),
    ] {
        let reply = web.http("GET", path, &[], b"");
        assert_eq!(reply.status, 200);
        assert!(reply.headers.contains(kind));
    }
}

#[test]
fn takeover_requires_csrf_and_json_before_contacting_the_fleet() {
    let web = Web::start();
    let body = br#"{"host":"local","run":"synthetic"}"#;
    assert_eq!(
        web.http(
            "POST",
            "/api/fleet/takeover",
            &[("Content-Type", "application/json")],
            body
        )
        .status,
        403
    );
    assert_eq!(
        web.http(
            "POST",
            "/api/fleet/takeover",
            &[
                ("X-Hey-Boss-CSRF", &web.token),
                ("Content-Type", "text/plain")
            ],
            body
        )
        .status,
        400
    );
    assert_eq!(
        web.http(
            "POST",
            "/api/fleet/takeover",
            &[
                ("X-Hey-Boss-CSRF", &web.token),
                ("Content-Type", "application/json"),
                ("Origin", "https://evil.example")
            ],
            body
        )
        .status,
        403
    );
}

#[test]
fn artifact_diagrams_are_bundled_locally_and_preserve_markdown_source() {
    let web = Web::start();
    let bundle = web.http("GET", "/artifact-diagrams.js", &[], b"");
    assert_eq!(bundle.status, 200);
    assert!(bundle.headers.contains("application/javascript"));
    assert!(bundle.headers.contains("script-src 'self'"));
    assert!(!bundle.body.is_empty());
    let loader = String::from_utf8(bundle.body).unwrap();
    let module = loader
        .split("./diagram-assets/")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let chunk = web.http("GET", &format!("/diagram-assets/{module}"), &[], b"");
    assert_eq!(chunk.status, 200);
    assert!(chunk.headers.contains("application/javascript"));
    assert!(!chunk.body.is_empty());
    assert_eq!(
        web.http("GET", "/diagram-assets/missing.js", &[], b"")
            .status,
        404
    );
    let source = "```mermaid\nflowchart LR\n A[Save <data>] --> B[Recover]\n```";
    let result = web.ok(json!({"action":"artifact","operation":{"command":"create","title":"Recovery diagram","body":source}}));
    let id = result["artifact"]["id"].as_str().unwrap();
    let saved = web.ok(json!({"action":"artifact","operation":{"command":"view","id":id}}));
    assert_eq!(saved["artifact"]["body"], source);
    assert!(
        saved["artifact"]["body_html"]
            .as_str()
            .unwrap()
            .contains("language-mermaid")
    );
}

#[test]
fn comment_resolution_preserves_content_is_reversible_and_scoped_to_issue() {
    let web = Web::start();
    for title in ["First", "Second"] {
        web.ok(json!({"action":"create","title":title,"body":"","labels":[]}));
    }
    let added = web.ok(json!({"action":"comment","number":1,"body":"**Keep this feedback**"}));
    let id = added["comment_id"].as_i64().unwrap();
    let original = web.ok(json!({"action":"view","number":1}));
    assert_eq!(original["comments"][0]["resolved"], false);
    let operation = json!({"action":"resolve_comment","number":1,"comment_id":id,"resolved":true});
    let resolved = web.ok(operation.clone());
    assert_eq!(resolved["changed"], true);
    assert_eq!(web.ok(operation)["changed"], false);
    let view = web.ok(json!({"action":"view","number":1}));
    assert_eq!(view["comments"][0]["resolved"], true);
    assert_eq!(view["comments"][0]["body"], original["comments"][0]["body"]);
    assert_eq!(
        view["comments"][0]["body_html"],
        original["comments"][0]["body_html"]
    );
    assert_eq!(view["comments"][0]["author"], "human:boss");
    assert_eq!(view["issue"]["version"], resolved["issue"]["version"]);
    let wrong = web.action(
        &web.project,
        json!({"action":"resolve_comment","number":2,"comment_id":id,"resolved":false}),
        None,
    );
    assert_eq!(wrong.status, 404);
    web.ok(json!({"action":"resolve_comment","number":1,"comment_id":id,"resolved":false}));
    assert_eq!(
        web.ok(json!({"action":"view","number":1}))["comments"][0]["resolved"],
        false
    );
    let history = web.ok(json!({"action":"history","number":1,"limit":20,"offset":0}));
    let events = history["events"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e["action"] == "comment_resolved")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["action"] == "comment_unresolved")
            .count(),
        1
    );
}

#[test]
fn markdown_preview_issue_and_comment_share_complete_rendering() {
    let web = Web::start();
    let body = include_str!("fixtures/issues-markdown.md");
    let preview = web.http(
        "POST",
        "/api/preview",
        &[
            ("Content-Type", "application/json"),
            ("X-Hey-Boss-CSRF", &web.token),
        ],
        json!({"body":body}).to_string().as_bytes(),
    );
    assert_eq!(preview.status, 200);
    let html = preview.json()["html"].as_str().unwrap().to_owned();
    for expected in [
        "href=\"https://github.com/poe-internal/poe2/pull/14920\"",
        "href=\"https://github.com/poe-internal/poe2/pull/14921\"",
        "href=\"https://example.com/?first=1&amp;second=2\"",
        "<strong>bold</strong>",
        "<em>emphasis</em>",
        "<del>strikethrough</del>",
        "<ol class=\"markdown-short-list\" start=\"3\">",
        "type=\"checkbox\"",
        "<table>",
        "markdown-align-right",
        "markdown-alert-warning",
        "language-typescript",
        "token-keyword",
        "token-insert",
        "footnote-definition",
        "<h6>",
        "<br />",
    ] {
        assert!(html.contains(expected), "Missing {expected}: {html}");
    }
    assert!(!html.contains("style="));
    let created =
        web.ok(json!({"action":"create","title":"Markdown review","body":body,"labels":[]}));
    assert_eq!(created["issue"]["body_html"], html);
    web.ok(json!({"action":"comment","number":1,"body":body}));
    let view = web.ok(json!({"action":"view","number":1}));
    assert_eq!(view["issue"]["body"], body);
    assert_eq!(view["comments"][0]["body"], body);
    assert_eq!(view["issue"]["body_html"], html);
    assert_eq!(view["comments"][0]["body_html"], html);
}

#[test]
fn csrf_cross_origin_and_host_checks_apply_before_operations() {
    let web = Web::start();
    let body=json!({"project":web.project,"operation":{"action":"create","title":"unsafe","body":"","labels":[]},"request_id":null}).to_string();
    assert_eq!(
        web.http(
            "POST",
            "/api/action",
            &[("Content-Type", "application/json")],
            body.as_bytes()
        )
        .status,
        403
    );
    let signal = br#"{"kind":"signal","host":"local","worker":"untrusted","signal":"restart"}"#;
    for headers in [
        vec![("Content-Type", "application/json")],
        vec![
            ("Content-Type", "application/json"),
            ("X-Hey-Boss-CSRF", &web.token),
            ("Origin", "https://attacker.example"),
        ],
    ] {
        assert_eq!(web.http("POST", "/api/fleet", &headers, signal).status, 403);
    }
    assert_eq!(
        web.http(
            "GET",
            "/api/bootstrap",
            &[("Host", "attacker.example")],
            b""
        )
        .status,
        403
    );
    assert_eq!(
        web.http(
            "GET",
            "/api/bootstrap",
            &[("Sec-Fetch-Site", "cross-site")],
            b""
        )
        .status,
        403
    );
    assert_eq!(
        web.http(
            "POST",
            "/api/action",
            &[
                ("Content-Type", "application/json"),
                ("X-Hey-Boss-CSRF", &web.token),
                ("Origin", "https://attacker.example")
            ],
            body.as_bytes()
        )
        .status,
        403
    );
    assert_eq!(
        web.http(
            "POST",
            "/api/action",
            &[
                ("Content-Type", "text/plain"),
                ("X-Hey-Boss-CSRF", &web.token)
            ],
            body.as_bytes()
        )
        .status,
        400
    );
    assert_eq!(
        web.http(
            "POST",
            "/api/action",
            &[
                ("Content-Type", "application/json"),
                ("X-Hey-Boss-CSRF", &web.token)
            ],
            b"not json"
        )
        .status,
        400
    );
    assert_eq!(web.http("GET", "/api/action", &[], b"").status, 404);
    assert_eq!(web.http("GET", "/../../Cargo.toml", &[], b"").status, 404);
    let p = web.ok(json!({"action":"projects"}));
    assert_eq!(p["projects"][0]["open"], 0);
}

#[test]
fn web_and_cli_share_ownership_revisions_and_project_counts() {
    let web = Web::start();
    let create =
        json!({"action":"create","title":"From browser","body":"## Markdown","labels":["bug"]});
    let one = web.action(&web.project, create.clone(), Some("once"));
    assert_eq!(one.status, 200);
    assert_eq!(
        web.action(&web.project, create, Some("once")).json(),
        one.json()
    );
    assert_eq!(one.json()["issue"]["body_html"], "<h2>Markdown</h2>\n");
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .current_dir(&web.root)
        .env("HEY_BOSS_ISSUE_DB", web.root.join("issues.db"))
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .args([
            "issue",
            "claim",
            "1",
            "--project",
            &web.project,
            "--agent",
            "codex:agent-test",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        web.action(
            &web.project,
            json!({"action":"claim","number":1,"force":false}),
            None
        )
        .status,
        409
    );
    let takeover = web.ok(json!({"action":"claim","number":1,"force":true}));
    assert_eq!(takeover["issue"]["assignee"], "human:boss");
    assert_eq!(web.action(&web.project,json!({"action":"edit","number":1,"title":"stale","body":null,"add_labels":[],"remove_labels":[],"if_version":1}),None).status,409);
    web.ok(json!({"action":"comment","number":1,"body":"**Shared** with agents"}));
    let list=web.ok(json!({"action":"list","state":"open","mine":false,"unassigned":false,"labels":[],"search":null,"limit":50,"offset":0}));
    assert_eq!(list["issues"][0]["comment_count"], 1);
    assert!(list["issues"][0].get("body").is_none());
    let v = web.ok(json!({"action":"view","number":1}));
    assert!(
        v["comments"][0]["body_html"]
            .as_str()
            .unwrap()
            .contains("<strong>Shared</strong>")
    );
    web.ok(json!({"action":"close","number":1,"comment":null,"force":false}));
    let p = web.ok(json!({"action":"projects"}));
    assert_eq!(p["projects"][0]["closed"], 1);
    assert_eq!(p["projects"][0]["open"], 0);
    assert_eq!(p["labels"], json!(["bug"]));
    let other = web.action(
        "named:Other",
        json!({"action":"create","title":"Isolated","body":"","labels":[]}),
        None,
    );
    assert_eq!(other.status, 200);
    assert_eq!(other.json()["issue"]["number"], 1);
    assert_eq!(
        web.ok(json!({"action":"projects"}))["projects"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    web.ok(json!({"action":"delete","number":1,"force":false}));
    assert_eq!(
        web.ok(json!({"action":"projects"}))["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == web.project)
            .unwrap()["deleted"],
        1
    );
    let restore = web.ok(json!({"action":"restore","number":1}));
    assert_eq!(restore["issue"]["state"], "closed");
    assert!(restore["issue"]["assignee"].is_null());
}

#[test]
fn opening_web_does_not_start_standalone_workers() {
    let web = Web::start();
    let html = String::from_utf8(web.http("GET", "/", &[], b"").body).unwrap();
    for absent in [
        "worker-dialog",
        "worker-trigger",
        "worker-directory",
        "worker-concurrency",
        "worker-claim-timeout",
        "worker-labels",
        "New worker",
        "Start worker",
        "worker-runs",
    ] {
        assert!(!html.contains(absent), "Unexpected worker UI: {absent}");
    }
    assert!(html.contains("project-instructions-preview"));
    assert_eq!(web.http("GET", "/workers.js", &[], b"").status, 404);
    let script = String::from_utf8(web.http("GET", "/project-settings.js", &[], b"").body).unwrap();
    assert!(!script.contains("SQLite"));
    web.ok(json!({"action":"create","title":"A worker must not be launched by the web service","body":"","labels":[]}));
    fs::write(web.root.join("mode.txt"), "delay-unclaimed").unwrap();
    let config = hey_boss::issues::worker::Settings {
        name: "Legacy managed worker".into(),
        projects: vec![web.project.clone()],
        directory: web.root.to_string_lossy().into(),
        enabled: true,
        ..Default::default()
    };
    let db = rusqlite::Connection::open(web.root.join("issues.db")).unwrap();
    db.execute("INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES('must-not-launch','managed',?1,1,0)",[serde_json::to_string(&config).unwrap()]).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    let count: i64 = db
        .query_row("SELECT count(*) FROM worker_runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count, 0,
        "Opening the web app must never launch a Codex session"
    );
}

#[test]
fn goal_preview_preserves_first_sentence_and_uses_current_project_commands() {
    let web = Web::start();
    for (prompt, use_goal, expected) in [
        (
            None,
            false,
            "Claim and implement `hey-boss issue view <number>`.\n\nWork in the project's existing checkout.\n\nCommit your changes. If a Git remote is configured, push to main.",
        ),
        (
            Some("/goal"),
            true,
            "Claim and implement `hey-boss issue view <number>`.\n\nWork in the project's existing checkout.\n\nCommit your changes. If a Git remote is configured, push to main.",
        ),
        (
            Some("/goal Assign and implement `{{issue_command}}`.\n{{commit_instruction}}"),
            true,
            "Assign and implement `hey-boss issue view <number>`.\n\nWork in the project's existing checkout.\n\nCommit your changes. If a Git remote is configured, push to main.",
        ),
    ] {
        let value = web.ok(json!({"action":"preview_worker","config":{"projects":[web.project],"prompt":prompt},"number":null}));
        assert_eq!(value["use_goal"], use_goal);
        let text = value["prompt"].as_str().unwrap();
        assert_eq!(text, expected);
        assert!(!text.contains("--project"));
        assert_eq!(value["objective"], text);
    }
}

#[test]
fn web_reordering_shares_cli_order_and_rejects_stale_changes() {
    let web = Web::start();
    for title in ["One", "Two", "Three"] {
        web.ok(json!({"action":"create","title":title,"body":"# Preserve","labels":[]}));
    }
    let list = json!({"action":"list","state":"open","mine":false,"unassigned":false,"labels":[],"search":null,"limit":2,"offset":0});
    let current = web.ok(list.clone());
    let version = current["order_version"].clone();
    let moved = web.ok(json!({"action":"move","number":3,"before":1,"if_order_version":version}));
    assert_eq!(moved["changed"], true);
    let sorted = web.ok(list.clone());
    assert_eq!(sorted["issues"][0]["number"], 3);
    assert_eq!(sorted["issues"][1]["number"], 1);
    let stale = web.action(
        &web.project,
        json!({"action":"move","number":2,"before":1,"if_order_version":version}),
        None,
    );
    assert_eq!(stale.status, 409);
    assert_eq!(web.http("GET", "/issue-order.js", &[], b"").status, 200);
}

#[test]
fn ui_creation_uses_selected_end_atomically_and_cli_creation_still_appends() {
    let web = Web::start();
    web.ok(json!({"action":"create","title":"First","body":"","labels":[],"at_top":true}));
    let cli_create = |title: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&web.root)
            .env("HEY_BOSS_ISSUE_DB", web.root.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args([
                "issue",
                "create",
                "--project",
                &web.project,
                "--agent",
                "human:cli-test",
                "--title",
                title,
                "--json",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    cli_create("CLI second");
    web.ok(json!({"action":"move","number":2,"before":1}));
    let list = json!({"action":"list","state":"open","mine":false,"unassigned":false,"labels":[],"search":null,"limit":50,"offset":0});
    let before = web.ok(list.clone());
    let create =
        json!({"action":"create","title":"UI third","body":"# Markdown","labels":[],"at_top":true});
    let created = web.action(&web.project, create.clone(), Some("ui-top-create"));
    assert_eq!(created.status, 200);
    web.ok(json!({"action":"create","title":"UI fourth","body":"","labels":[],"at_top":true}));
    assert_eq!(
        web.action(&web.project, create, Some("ui-top-create"))
            .json(),
        created.json()
    );
    cli_create("CLI fifth");
    let append =
        json!({"action":"create","title":"UI bottom","body":"","labels":[],"at_top":false});
    let appended = web.action(&web.project, append.clone(), Some("ui-bottom-create"));
    assert_eq!(appended.status, 200);
    assert_eq!(
        web.action(&web.project, append, Some("ui-bottom-create"))
            .json(),
        appended.json()
    );
    let result = web.ok(list);
    assert_eq!(
        result["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["number"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![4, 3, 2, 1, 5, 6]
    );
    assert_eq!(
        result["order_version"].as_i64().unwrap(),
        before["order_version"].as_i64().unwrap() + 4
    );
    assert_eq!(
        web.ok(json!({"action":"view","number":3}))["issue"]["body"],
        "# Markdown"
    );
    let db = rusqlite::Connection::open(web.root.join("issues.db")).unwrap();
    let unique: i64 = db
        .query_row("SELECT count(DISTINCT sort_order) FROM issues", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(unique, 6);
}

#[test]
fn complete_issue_lists_ignore_page_offsets_and_preserve_filters_and_order() {
    let web = Web::start();
    for number in 1..=105 {
        web.ok(json!({"action":"create","title":format!("Issue {number}"),"body":"","labels":["ready"]}));
    }
    web.ok(
        json!({"action":"create","title":"Newest UI issue","body":"","labels":[],"at_top":true}),
    );
    let list = json!({"action":"list","state":"open","mine":false,"unassigned":false,"labels":[],"search":null,"limit":1,"offset":100,"all":true});
    let result = web.ok(list.clone());
    assert_eq!(result["issues"].as_array().unwrap().len(), 106);
    assert_eq!(result["issues"][0]["number"], 106);
    assert_eq!(result["issues"][105]["number"], 105);
    assert!(result["next_offset"].is_null());
    let mut filtered = list;
    filtered["labels"] = json!(["ready"]);
    assert_eq!(
        web.ok(filtered.clone())["issues"].as_array().unwrap().len(),
        105
    );
    filtered["search"] = json!("Issue 10");
    assert_eq!(web.ok(filtered)["issues"].as_array().unwrap().len(), 7);
    let html = String::from_utf8(web.http("GET", "/", &[], b"").body).unwrap();
    assert!(!html.contains("previous-page") && !html.contains("next-page"));
    // Defaulted fields must preserve stored payloads for legacy retry IDs.
    let legacy = json!({"action":"create","title":"Legacy","body":"","labels":[]});
    let operation: hey_boss::issues::Operation = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(operation).unwrap(), legacy);
}

#[test]
fn cross_project_create_command_matches_preview_and_claim_and_routes_outside_worker_project() {
    let web = Web::start();
    web.ok(json!({"action":"create","title":"Worker issue","body":"","labels":[]}));
    let prompt = "/goal Implement {{issue_command}}. If safe-bash fails, report using `{{create_issue_command poe-code}}`.";
    web.ok(
        json!({"action":"configure_project","prompt":prompt,"prs_enabled":false,"if_version":null}),
    );
    let preview =
        web.ok(json!({"action":"preview_worker","config":{"projects":[web.project]},"number":1}));
    let claimed = web.ok(json!({"action":"claim","number":1,"force":false}));
    assert_eq!(claimed["instructions"], preview["prompt"]);
    let command =
        "hey-boss issue create --project 'poe-code' --title '<title>' --body '<markdown>'";
    assert!(claimed["instructions"].as_str().unwrap().contains(command));
    let target = web.action(
        "named:poe-code",
        json!({"action":"create","title":"Register target","body":"","labels":[]}),
        None,
    );
    assert_eq!(target.status, 200);
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_hey-boss"));
    let output = Command::new("sh")
        .current_dir(&web.root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                binary.parent().unwrap().display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("HEY_BOSS_ISSUE_DB", web.root.join("issues.db"))
        .env("HEY_BOSS_ISSUE_PROJECT", &web.project)
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .args(["-c", &format!("{command} --agent human:macro-test --json")])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let created: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(created["project"]["id"], "named:poe-code");
    assert_eq!(created["issue"]["number"], 2);
    assert_eq!(created["issue"]["body"], "<markdown>");
    assert_eq!(
        web.ok(json!({"action":"projects"}))["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == web.project)
            .unwrap()["open"],
        1
    );
}

#[test]
fn web_always_acts_as_boss_and_supports_exact_assignee_filters_and_rename() {
    let web = Web::start();
    let boot = web.http("GET", "/api/bootstrap", &[], b"").json();
    assert_eq!(boot["actor"]["id"], "human:boss");
    assert_eq!(boot["boss"]["name"], "Boss");
    let created = web.ok(
        json!({"action":"create","title":"Boss work","body":"Keep markdown","labels":["ready"]}),
    );
    assert_eq!(created["issue"]["created_by"], "human:boss");
    web.ok(json!({"action":"create","title":"Unassigned","body":"","labels":[]}));
    let assigned = web.ok(json!({"action":"assign_boss","number":1,"force":false}));
    assert_eq!(assigned["issue"]["assignee"], "human:boss");
    let filtered = web.ok(json!({"action":"list","state":"open","mine":false,"unassigned":false,"assignee":"human:boss","labels":["ready"],"search":null,"limit":50,"offset":0,"all":true}));
    assert_eq!(filtered["issues"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["issues"][0]["number"], 1);
    assert_eq!(
        web.ok(json!({"action":"projects","include_hidden":true}))["assignees"],
        json!(["human:boss"])
    );
    let settings = web.ok(json!({"action":"configure_project","boss_name":"Alex <Boss>","prompt":null,"prs_enabled":null,"if_version":0}));
    assert_eq!(settings["boss_name"], "Alex <Boss>");
    assert_eq!(web.action(&web.project,json!({"action":"configure_project","boss_name":"Stale","prompt":null,"prs_enabled":null,"if_version":0}),None).status,409);
    let renamed = web.ok(json!({"action":"view","number":1}));
    assert_eq!(renamed["issue"]["assignee_name"], "Alex <Boss>");
    web.ok(json!({"action":"comment","number":1,"body":"Boss comment"}));
    assert_eq!(
        web.ok(json!({"action":"view","number":1}))["comments"][0]["author"],
        "human:boss"
    );
    web.ok(json!({"action":"unassign","number":1,"force":false}));
    assert_eq!(
        web.ok(json!({"action":"view","number":1}))["issue"]["assignee"],
        Value::Null
    );
}

#[test]
fn inbox_api_security_rendering_and_issue_relationships_do_not_mutate_issues() {
    use std::os::unix::net::UnixListener;
    let web = Web::start();
    web.ok(json!({"action":"create","title":"Related issue","body":"Preserve","labels":["ready"]}));
    web.ok(json!({"action":"assign_boss","number":1,"force":false}));
    let before = web.ok(json!({"action":"view","number":1}))["issue"].clone();
    let listener = UnixListener::bind(web.root.join("inbox.sock")).unwrap();
    let worker = std::thread::spawn(move || {
        let mut received = Vec::new();
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut data = Vec::new();
            stream.read_to_end(&mut data).unwrap();
            let request: Value = serde_json::from_slice(&data).unwrap();
            received.push(request.clone());
            let result = if request["command"] == "inbox_list" {
                json!({"tasks":[],"unread":0})
            } else {
                json!({"task":{"taskID":"notice-1","kind":"update","question":"# Report\n\n<script>bad()</script>","comments":[{"text":"**Feedback**"}],"status":"pending","issue":request["issue"]},"changed":true})
            };
            stream
                .write_all(
                    serde_json::to_string(&json!({"status":"ok","result":result.to_string()}))
                        .unwrap()
                        .as_bytes(),
                )
                .unwrap();
        }
        received
    });
    let inbox = |action: Value| {
        web.http(
            "POST",
            "/api/inbox",
            &[
                ("Content-Type", "application/json"),
                ("X-Hey-Boss-CSRF", &web.token),
            ],
            serde_json::to_string(&action).unwrap().as_bytes(),
        )
    };
    assert_eq!(
        web.http(
            "POST",
            "/api/inbox",
            &[("Content-Type", "application/json")],
            br#"{"action":"list"}"#
        )
        .status,
        403
    );
    assert_eq!(inbox(json!({"action":"secret"})).status, 400);
    assert_eq!(inbox(json!({"action":"link","task_id":"notice-1","issue":{"project":web.project,"number":999,"host":null}})).status,404);
    assert_eq!(inbox(json!({"action":"link","task_id":"notice-1","issue":{"project":web.project,"number":0,"host":null}})).status,400);
    assert_eq!(inbox(json!({"action":"list"})).status, 200);
    let linked = inbox(
        json!({"action":"link","task_id":"notice-1","issue":{"project":web.project,"number":1,"host":null}}),
    );
    assert_eq!(linked.status, 200);
    assert_eq!(linked.json()["task"]["issue"]["project"], web.project);
    assert!(
        linked.json()["task"]["body_html"]
            .as_str()
            .unwrap()
            .contains("&lt;script&gt;")
    );
    assert_eq!(
        linked.json()["task"]["comments"][0]["body_html"],
        "<p><strong>Feedback</strong></p>\n"
    );
    assert_eq!(
        inbox(json!({"action":"link","task_id":"notice-1","issue":null})).status,
        200
    );
    assert_eq!(web.ok(json!({"action":"view","number":1}))["issue"], before);
    let received = worker.join().unwrap();
    assert_eq!(received[1]["command"], "inbox_link");
    assert_eq!(received[2]["issue"], Value::Null);
}

#[test]
fn unavailable_inbox_does_not_block_the_issue_service() {
    let web = Web::start();
    let result = web.http(
        "POST",
        "/api/inbox",
        &[
            ("Content-Type", "application/json"),
            ("X-Hey-Boss-CSRF", &web.token),
        ],
        br#"{"action":"list"}"#,
    );
    assert_eq!(result.status, 503);
    assert_eq!(result.json()["error"]["code"], "inbox_unavailable");
    assert_eq!(web.http("GET", "/inbox.js", &[], b"").status, 200);
    web.ok(json!({"action":"create","title":"Issues remain available","body":"","labels":[]}));
}

#[test]
fn inbox_clear_is_protected_and_forwards_only_valid_snapshots() {
    use std::os::unix::net::UnixListener;
    let web = Web::start();
    let listener = UnixListener::bind(web.root.join("inbox.sock")).unwrap();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut data = Vec::new();
        stream.read_to_end(&mut data).unwrap();
        let request: Value = serde_json::from_slice(&data).unwrap();
        stream
            .write_all(br#"{"status":"ok","result":"{\"cleared\":2,\"changed\":true}"}"#)
            .unwrap();
        request
    });
    let send = |body: Value, csrf: bool| {
        let mut headers = vec![("Content-Type", "application/json")];
        if csrf {
            headers.push(("X-Hey-Boss-CSRF", web.token.as_str()));
        }
        web.http(
            "POST",
            "/api/inbox",
            &headers,
            serde_json::to_string(&body).unwrap().as_bytes(),
        )
    };
    assert_eq!(
        send(json!({"action":"clear","task_ids":["one"]}), false).status,
        403
    );
    for ids in [json!([]), json!([""]), json!(["one", "one"])] {
        assert_eq!(
            send(json!({"action":"clear","task_ids":ids}), true).status,
            400
        );
    }
    let result = send(json!({"action":"clear","task_ids":["one","two"]}), true);
    assert_eq!(result.status, 200);
    assert_eq!(result.json()["cleared"], 2);
    let forwarded = worker.join().unwrap();
    assert_eq!(forwarded["command"], "inbox_clear");
    assert_eq!(forwarded["task_ids"], json!(["one", "two"]));
}

#[test]
fn every_page_uses_the_issues_design_library_and_shell() {
    let web = Web::start();
    for path in ["/", "/mm", "/workers"] {
        let html = String::from_utf8(web.http("GET", path, &[], b"").body).unwrap();
        for component in [
            "/components.css",
            "/components.js",
            "class=\"app-header\"",
            "id=\"project-trigger\"",
            "id=\"project-search\"",
            "id=\"project-options\"",
            "class=\"app-navigation\"",
            "class=\"page-heading\"",
            "class=\"page-description\"",
            "class=\"page-footer\"",
            "id=\"main\"",
        ] {
            assert!(
                html.contains(component),
                "Missing shared {component} on {path}"
            );
        }
        assert!(!html.contains("<select id=\"project\""));
        assert!(!html.contains("<!--app-shell-->"));
    }
    for (path, kind) in [
        ("/components.js", "text/javascript"),
        ("/components.css", "text/css"),
    ] {
        let reply = web.http("GET", path, &[], b"");
        assert_eq!(reply.status, 200);
        assert!(reply.headers.contains(kind));
    }
}

#[test]
fn main_navigation_includes_mindmaps_on_every_page() {
    let web = Web::start();
    for path in ["/", "/workers", "/mm"] {
        let reply = web.http("GET", path, &[], b"");
        assert_eq!(reply.status, 200);
        let html = String::from_utf8(reply.body).unwrap();
        let navigation = html
            .split("aria-label=\"Main navigation\"")
            .nth(1)
            .unwrap_or_else(|| panic!("Missing main navigation on {path}"))
            .split("</nav>")
            .next()
            .unwrap();
        for (href, label) in [("/mm", "Mindmaps"), ("/agents", "Agents")] {
            assert!(
                navigation.contains(&format!("href=\"{href}\"")) && navigation.contains(label),
                "Missing {label} in main navigation on {path}"
            );
        }
        assert!(navigation.contains("Inbox") && navigation.contains("Issues"));
        assert_eq!(html.matches("href=\"/mm\"").count(), 1);
        if path != "/" {
            let current = if path == "/workers" { "/agents" } else { path };
            assert!(navigation.contains(&format!("href=\"{current}\" aria-current=\"page\"")));
        }
    }
}

#[test]
fn mindmap_assets_reads_and_authoring_boundary() {
    let web = Web::start();
    for (path, kind) in [
        ("/mm", "text/html"),
        ("/mindmap.js", "text/javascript"),
        ("/mindmap-map.js", "text/javascript"),
        ("/mindmap.css", "text/css"),
    ] {
        let r = web.http("GET", path, &[], b"");
        assert_eq!(r.status, 200);
        assert!(r.headers.contains(kind));
    }
    let show = json!({"action":"mindmap","operation":{"command":"show"}});
    let r = web.action(&web.project, show.clone(), None);
    assert_eq!(r.status, 200);
    assert!(r.json()["nodes"].as_array().unwrap().is_empty());
    let add = json!({"action":"mindmap","operation":{"command":"add","title":"Forbidden","body":"","kind":"text","reference":null,"reference_project":null,"alias":"forbidden","under":null,"if_version":null}});
    let r = web.action(&web.project, add.clone(), None);
    assert_eq!(r.status, 403);
    assert_eq!(r.json()["error"]["code"], "forbidden");
    let headers = [
        ("Content-Type", "application/json"),
        ("X-Hey-Boss-CSRF", web.token.as_str()),
    ];
    for operation in [
        add,
        json!({"action":"create","title":"Forbidden","body":"","labels":[]}),
    ] {
        let body = serde_json::to_vec(
            &json!({"project":web.project,"operation":operation,"request_id":null}),
        )
        .unwrap();
        assert_eq!(web.http("POST", "/api/mm", &headers, &body).status, 403);
    }
    let body =
        serde_json::to_vec(&json!({"project":web.project,"operation":show,"request_id":null}))
            .unwrap();
    assert_eq!(web.http("POST", "/api/mm", &headers, &body).status, 200);
    assert_eq!(
        web.http(
            "POST",
            "/api/mm",
            &[("Content-Type", "application/json")],
            &body
        )
        .status,
        403
    );
}

#[test]
fn native_focus_is_available_without_inline_scripts_and_only_on_mindmaps() {
    let web = Web::start();
    for path in ["/mm?focus=1", "/mm?other=1&focus=1"] {
        let response = web.http("GET", path, &[], b"");
        assert_eq!(response.status, 200);
        let html = String::from_utf8(response.body).unwrap();
        assert!(html.contains("<html lang=\"en\" class=\"mindmap-focus\">"));
        assert!(html.contains("id=\"project-trigger\""));
        assert!(!html.contains("<script>"));
    }
    for path in ["/mm", "/mm?focus=0", "/?focus=1", "/workers?focus=1"] {
        let response = web.http("GET", path, &[], b"");
        assert_eq!(response.status, 200);
        assert!(
            !String::from_utf8(response.body)
                .unwrap()
                .contains("class=\"mindmap-focus\"")
        );
    }
}

#[path = "support/mindmap_inbox.rs"]
mod mindmap_inbox_fixture;

#[test]
fn actual_web_mindmap_notification_completion_recovery_does_not_acknowledge_notices() {
    let web = Web::start();
    for args in [
        vec!["notice", "review", "--id", "review"],
        vec!["add", "Follow-up", "--id", "follow-up", "--under", "review"],
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&web.root)
            .env("HEY_BOSS_ISSUE_DB", web.root.join("issues.db"))
            .env("HEY_BOSS_INBOX_SOCKET", web.root.join("inbox.sock"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .args([
                "mm",
                "--project",
                &web.project,
                "--agent",
                "human:test",
                "--json",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
    }
    let pending = json!([{"taskID":"review","status":"pending","title":"Review rollout","summary":"Pending review"}]);
    let inbox = mindmap_inbox_fixture::Inbox::start(web.root.join("inbox.sock"), pending.clone());
    let show = || web.ok(json!({"action":"mindmap","operation":{"command":"show"}}));
    let g = show();
    assert_eq!(g["nodes"].as_array().unwrap().len(), 2);
    assert!(
        g["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["title"] == "Review rollout" && n["state"] == "pending")
    );
    inbox.tasks(json!([{"taskID":"review","status":"cancelled","title":"Cancelled"}]));
    let g = show();
    assert_eq!(g["nodes"].as_array().unwrap().len(), 1);
    assert!(g["nodes"][0]["parent_id"].is_null());
    inbox.unavailable();
    let g = show();
    assert_eq!(g["notifications"]["available"], false);
    inbox.tasks(pending);
    assert_eq!(show()["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(inbox.requests().len(), 4);
    assert!(
        inbox
            .requests()
            .iter()
            .all(|r| r["command"] == "inbox_list")
    );
    assert!(inbox.path().exists());
}

#[test]
fn mindmap_preview_and_full_node_reads_preserve_markdown_and_web_authoring_boundary() {
    let web = Web::start();
    let body = "## Planning\n\n🧭 ".repeat(2000);
    let db = rusqlite::Connection::open(web.root.join("issues.db")).unwrap();
    db.execute("INSERT INTO mindmap_nodes(id,project_id,alias,parent_id,position,kind,title,body,reference,reference_project,created_at,updated_at) VALUES('n-long-web',?1,'long',NULL,0,'markdown','Long note',?2,NULL,NULL,1,1)",rusqlite::params![web.project,body]).unwrap();
    let preview =
        web.ok(json!({"action":"mindmap","operation":{"command":"show","body_mode":"preview"}}));
    assert_eq!(preview["nodes"][0]["body_truncated"], true);
    assert_eq!(
        preview["nodes"][0]["body"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        512
    );
    let full = web.ok(json!({"action":"mindmap","operation":{"command":"view","node":"long"}}));
    assert_eq!(full["node"]["body"], body);
    assert!(
        full["node"]["body_html"]
            .as_str()
            .unwrap()
            .contains("<h2>Planning</h2>")
    );
    let single_preview = web.ok(json!({"action":"mindmap","operation":{"command":"view","node":"long","body_mode":"preview"}}));
    assert_eq!(single_preview["body_mode"], "preview");
    assert_eq!(
        single_preview["node"]["body"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        512
    );
    assert_eq!(single_preview["node"]["body_truncated"], true);
    let single_omitted = web.ok(
        json!({"action":"mindmap","operation":{"command":"view","node":"long","body_mode":"none"}}),
    );
    assert_eq!(single_omitted["node"]["body"], "");
    assert_eq!(single_omitted["node"]["has_body"], true);
    let edit = json!({"action":"mindmap","operation":{"command":"edit","node":"long","title":"Forbidden","body":null,"if_version":null}});
    assert_eq!(web.action(&web.project, edit, None).status, 403);
    assert_eq!(
        db.query_row(
            "SELECT body FROM mindmap_nodes WHERE id='n-long-web'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        body
    );
}

#[test]
fn web_drafts_respect_settings_and_sync_bound_plans_before_undrafting() {
    let w = Web::start();
    let created =
        w.ok(json!({"action":"create","title":"Plan","body":"Old body","labels":[],"draft":true}));
    assert_eq!(created["issue"]["draft"], true);
    w.ok(json!({"action":"configure_project","drafts_enabled":false}));
    assert_ne!(
        w.action(
            &w.project,
            json!({"action":"create","title":"Disabled","body":"","labels":[],"draft":true}),
            None
        )
        .status,
        200
    );
    let db = rusqlite::Connection::open(w.root.join("issues.db")).unwrap();
    let machine: String = db
        .query_row(
            "SELECT json_extract(metadata,'$.machine') FROM agents WHERE id='human:boss'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let plan =
        json!({"path":"plan.md","checkout":w.root,"machine":machine,"host":"synthetic-owner"});
    db.execute(
        "UPDATE issues SET plan=?1 WHERE number=1",
        [plan.to_string()],
    )
    .unwrap();
    let failed = w.action(&w.project, json!({"action":"undraft","number":1}), None);
    assert_ne!(failed.status, 200);
    let message = failed.json()["error"]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        message.contains("plan.md") && message.contains("retry undrafting"),
        "{message}"
    );
    assert_eq!(
        w.ok(json!({"action":"view","number":1}))["issue"]["draft"],
        true
    );
    fs::write(w.root.join("plan.md"), "# Final title\n\nFinal body\n").unwrap();
    let issue=w.ok(json!({"action":"edit","number":1,"draft":false,"title":"Stale editor","body":"Stale body","add_labels":[],"remove_labels":[]}));
    assert_eq!(issue["issue"]["draft"], false);
    assert_eq!(issue["issue"]["title"], "Final title");
    assert_eq!(issue["issue"]["body"], "Final body");
    assert_ne!(
        w.action(
            &w.project,
            json!({"action":"edit","number":1,"draft":true,"add_labels":[],"remove_labels":[]}),
            None
        )
        .status,
        200
    );
    assert_eq!(
        w.action(&w.project, json!({"action":"read_plan","plan":plan}), None)
            .status,
        403
    );
}

#[test]
fn project_workflow_prompts_select_branches_and_match_claims() {
    let web = Web::start();
    web.ok(json!({"action":"create","title":"Workflow fixture","body":"","labels":[]}));
    let overrides = json!({"worktree":"Isolate issue {{number}}.","checkout":"Use checkout {{number}}.","prs":"Review with PR {{number}}.","main":"Ship directly {{number}}."});
    for worktree in [false, true] {
        for prs in [false, true] {
            let config = json!({"projects":[web.project],"prompt":"/goal Implement {{issue_command}}.","worktree_enabled":worktree,"prs_enabled":prs,"prompt_overrides":overrides});
            let preview = web.ok(json!({"action":"preview_worker","config":config,"number":1}));
            let expected = format!(
                "Implement hey-boss issue view 1.\n\n{}\n\n{}",
                if worktree {
                    "Isolate issue 1."
                } else {
                    "Use checkout 1."
                },
                if prs {
                    "Review with PR 1."
                } else {
                    "Ship directly 1."
                }
            );
            if prs {
                let text = preview["prompt"].as_str().unwrap();
                assert!(text.starts_with(&format!("{expected}\n\nPR handoff:")));
                assert!(text.contains("Keep the issue open until the actual fix PR is merged"));
                assert!(text.contains("hey-boss issue assign-to-boss 1"));
            } else {
                assert_eq!(preview["prompt"], expected);
            }
            assert_eq!(preview["use_goal"], true);
            web.ok(json!({"action":"configure_project","prompt":"/goal Implement {{issue_command}}.","worktree_enabled":worktree,"prs_enabled":prs,"prompt_overrides":overrides}));
            let settings = web.ok(json!({"action":"project_settings"}));
            assert_eq!(settings["worktree_enabled"], worktree);
            assert_eq!(settings["prompt_overrides"], overrides);
            let claim = web.ok(json!({"action":"claim","number":1,"force":false}));
            assert_eq!(claim["instructions"], preview["prompt"]);
        }
    }
    let version = web.ok(json!({"action":"project_settings"}))["version"].clone();
    let bad = web.action(
        &web.project,
        json!({"action":"configure_project","prompt_overrides":{"main":" "},"if_version":version}),
        None,
    );
    assert_eq!(bad.status, 400);
    assert_eq!(
        web.ok(json!({"action":"project_settings"}))["version"],
        version
    );
}

#[test]
fn project_workflow_legacy_templates_reset_and_version_guards() {
    let web = Web::start();
    web.ok(json!({"action":"configure_project","prompt":"/goal Claim {{issue_command}}.\n\n{{ commit_instruction }}", "worktree_enabled":true,"prompt_overrides":{"worktree":"Custom isolated {{number}}."}}));
    let settings = web.ok(json!({"action":"project_settings"}));
    assert_eq!(settings["prompt"], "/goal Claim {{issue_command}}.");
    let preview = web
        .ok(json!({"action":"preview_worker","config":{"projects":[web.project]},"number":null}));
    assert!(
        preview["prompt"]
            .as_str()
            .unwrap()
            .contains("Custom isolated <number>.")
    );
    assert_eq!(
        preview["prompt"]
            .as_str()
            .unwrap()
            .matches("Commit your changes.")
            .count(),
        1
    );
    web.ok(json!({"action":"configure_project","prompt_overrides":{"worktree":null},"if_version":settings["version"]}));
    let stale = web.action(&web.project, json!({"action":"configure_project","worktree_enabled":false,"if_version":settings["version"]}), None);
    assert_eq!(stale.status, 409);
    let preview = web
        .ok(json!({"action":"preview_worker","config":{"projects":[web.project]},"number":null}));
    assert!(
        preview["prompt"]
            .as_str()
            .unwrap()
            .contains("dedicated Git worktree")
    );
    assert!(
        !preview["prompt"]
            .as_str()
            .unwrap()
            .contains("Custom isolated")
    );
}

#[test]
fn agent_conversations_reject_malformed_cursors_and_query_encoding() {
    let web = Web::start();
    for query in ["cursor=-1", "cursor=hello", "host=%ZZ", "host=%FF"] {
        let reply = web.http("GET", &format!("/api/fleet/conversation?{query}"), &[], b"");
        assert_eq!(reply.status, 400, "Query {query}");
    }
    for path in ["/agents", "/agents/session"] {
        let html = String::from_utf8(web.http("GET", path, &[], b"").body).unwrap();
        assert!(html.contains("Agents · Hey Boss"));
        assert!(html.contains("id=\"conversation\""));
        assert!(!html.contains("coordinates workers"));
        assert!(html.contains("href=\"/agents\" aria-current=\"page\""));
    }
}

#[test]
fn file_attachments_share_authenticated_actions_and_all_page_assets() {
    let web = Web::start();
    for path in ["/attachments.js", "/attachments.css"] {
        let response = web.http("GET", path, &[], b"");
        assert_eq!(response.status, 200);
        assert!(!response.body.is_empty());
    }
    for path in ["/", "/mm", "/artifacts"] {
        let response = web.http("GET", path, &[], b"");
        let html = String::from_utf8(response.body).unwrap();
        assert!(html.contains("/attachments.js") && html.contains("/attachments.css"));
    }
    web.ok(json!({"action":"create","title":"Files","body":"","labels":[]}));
    let upload = json!({"action":"attachment","operation":{"command":"upload","target":{"kind":"issue","id":"1"},"name":"notes.txt","data":"aGVsbG8="}});
    let payload = serde_json::to_vec(&json!({"project":web.project,"operation":upload})).unwrap();
    assert_eq!(
        web.http(
            "POST",
            "/api/action",
            &[("Content-Type", "application/json")],
            &payload
        )
        .status,
        403
    );
    let result = web.ok(upload);
    let id = result["attachment"]["id"].as_str().unwrap();
    assert_eq!(
        web.ok(json!({"action":"attachment","operation":{"command":"download","id":id}}))["data"],
        "aGVsbG8="
    );
    web.ok(json!({"action":"attachment","operation":{"command":"remove","id":id}}));
    assert_eq!(
        web.action(
            &web.project,
            json!({"action":"attachment","operation":{"command":"download","id":id}}),
            None
        )
        .status,
        404
    );
}
