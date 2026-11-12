//! Run the shipped web client in Chrome, including the plain HTTP code path.
//! cargo test --locked -p hey-boss --test web_client -- --ignored
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, sync::mpsc, thread, time::Duration};
use tiny_http::{Header, Response, Server};

#[path = "support/chrome.rs"]
mod chrome;
use chrome::Page;

// Serve the actual frontend files. Only the desktop handoff response is simulated.
fn with_client(f: impl FnOnce(u16, mpsc::Receiver<(String, Value)>)) {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let (stop, done) = mpsc::channel::<()>();
    thread::scope(|scope| {
        scope.spawn(move || serve_client(server, tx, done));
        // Dropping the sender also stops the server if an assertion unwinds.
        let _stop = stop;
        f(port, rx);
    });
}

fn serve_client(server: Server, tx: mpsc::Sender<(String, Value)>, done: mpsc::Receiver<()>) {
    let routes = hey_boss::routes::javascript();
    while matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)) {
        let Some(mut r) = server.recv_timeout(Duration::from_millis(100)).unwrap() else {
            continue;
        };
        let (body, kind) = match r.url() {
            "/" => (
                "<meta charset=utf-8><script src='/routes.js'></script><script src='/components.js'></script><script src='/assignments.js'></script><script src='/artifacts.js'></script>",
                "text/html",
            ),
            "/routes.js" => (routes.as_str(), "text/javascript; charset=utf-8"),
            "/components.js" => (
                include_str!("../src/issues/web/components.js"),
                "text/javascript; charset=utf-8",
            ),
            "/assignments.js" => (
                include_str!("../src/issues/web/assignments.js"),
                "text/javascript; charset=utf-8",
            ),
            "/artifacts.js" => (
                include_str!("../src/issues/web/artifacts.js"),
                "text/javascript; charset=utf-8",
            ),
            "/api/inbox" => {
                assert_eq!(r.method(), &tiny_http::Method::Post);
                let csrf = r
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("X-Hey-Boss-CSRF"))
                    .unwrap()
                    .value
                    .to_string();
                let op: Value = serde_json::from_reader(r.as_reader()).unwrap();
                let reply = match op["id"].as_str() {
                    Some("doc-1") => Response::from_string(r#"{"ok":true}"#),
                    Some("error") => {
                        Response::from_string(r#"{"error":{"message":"Desktop app unavailable"}}"#)
                            .with_status_code(503)
                    }
                    _ => Response::from_string("bad JSON"),
                };
                tx.send((csrf, op)).unwrap();
                r.respond(reply).unwrap();
                continue;
            }
            _ => {
                r.respond(Response::empty(404)).unwrap();
                continue;
            }
        };
        let _ = r.respond(
            Response::from_string(body)
                .with_header(Header::from_bytes("Content-Type", kind).unwrap()),
        );
    }
}

#[test]
#[ignore = "requires Chrome"]
fn web_crypto_on_loopback_and_plain_http() {
    with_client(|port, _| {
        let vectors = [0, 55, 56, 63, 64, 65, 127, 128, 129, 65536, 1048576].map(|n| {
            (0..n)
                .map(|i| (i as u8).wrapping_mul(157))
                .collect::<Vec<_>>()
        });
        for (host, secure) in [("hey-boss.test", false), ("127.0.0.1", true)] {
            let mut page = Page::open(&format!("http://{host}:{port}/"));
            assert_eq!(
                page.eval("Boolean(crypto.subtle && crypto.randomUUID)")
                    .unwrap(),
                secure
            );
            for v in vectors
                .iter()
                .map(Vec::as_slice)
                .chain([b"abc".as_slice(), "Zażółć 🌍".as_bytes()])
            {
                let bytes = STANDARD.encode(v);
                let actual = page
                    .eval(&format!(
                        "HeyBossUI.sha256(Uint8Array.from(atob('{bytes}'), c => c.charCodeAt(0)))"
                    ))
                    .unwrap();
                assert_eq!(actual, format!("{:x}", Sha256::digest(v)));
            }
            let ids = page
                .eval("Array.from({length:1000}, () => HeyBossUI.requestId())")
                .unwrap();
            let ids = ids.as_array().unwrap();
            let unique: HashSet<_> = ids
                .iter()
                .map(|v| {
                    let id = uuid::Uuid::parse_str(v.as_str().unwrap()).unwrap();
                    assert_eq!(v.as_str().unwrap(), id.hyphenated().to_string());
                    assert_eq!(id.get_version_num(), 4);
                    assert_eq!(id.get_variant(), uuid::Variant::RFC4122);
                    id
                })
                .collect();
            assert_eq!(unique.len(), 1000);
        }
    });
}
#[test]
#[ignore = "requires Chrome"]
fn native_artifact_launch_sends_context_and_reports_failures() {
    with_client(|port, requests| {
        let mut page = Page::open(&format!("http://127.0.0.1:{port}/"));
        let open = |page: &mut Page, id: &str| {
            page.eval(&format!("HeyBossArtifacts.openNative({{project:'named:Plan',host:'devbox',csrf:'fixture-csrf'}},{{id:'{id}'}})"))
        };
        assert_eq!(open(&mut page, "doc-1").unwrap(), json!({"ok":true}));
        let (csrf, request) = requests.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(csrf, "fixture-csrf");
        assert_eq!(
            request,
            json!({"action":"open_artifact","project":"named:Plan","host":"devbox","id":"doc-1"})
        );
        assert!(
            open(&mut page, "error")
                .unwrap_err()
                .contains("Desktop app unavailable")
        );
        assert!(
            open(&mut page, "malformed")
                .unwrap_err()
                .contains("Could not confirm native editor launch")
        );
    });
}

#[test]
#[ignore = "requires Chrome"]
fn lookup_routes_follow_the_shared_contract() {
    with_client(|port, _| {
        let mut page = Page::open(&format!("http://127.0.0.1:{port}/"));
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("fixtures/lookup-routes.json")).unwrap();
        for case in cases {
            let route = page
                .eval(&format!("HeyBossRoutes.resolve({})", case["url"]))
                .unwrap();
            for key in ["entity", "id", "project", "host"] {
                assert_eq!(route[key], case[key], "{}: {key}", case["url"]);
            }
        }
        for n in ["-1", "0", "01", "1x", "9007199254740992"] {
            assert_eq!(
                page.eval(&format!("HeyBossRoutes.issueNumber({})", json!(n)))
                    .unwrap(),
                false
            );
        }
        assert!(
            page.eval("HeyBossRoutes.resolve('http://localhost/unknown')")
                .unwrap()
                .is_null()
        );
    });
}

#[test]
#[ignore = "requires Chrome"]
fn actor_labels_preserve_the_recorded_model() {
    with_client(|port, _| {
        let mut page = Page::open(&format!("http://127.0.0.1:{port}/"));
        let label = |page: &mut Page, args: Value| {
            page.eval(&format!("HeyBossUI.actorLabel(...{args})"))
                .unwrap()
        };
        let codex = "codex:internal-session";
        for (args, expected) in [
            (json!([codex, "gpt-6-astra"]), "Codex · gpt-6-astra"),
            (
                json!(["claude:internal-session", "claude-opus-4-6"]),
                "Claude · claude-opus-4-6",
            ),
            (
                json!(["worker:internal-run", "gpt-6-sol"]),
                "Agent · gpt-6-sol",
            ),
            (json!([codex, " custom/<model> "]), "Codex · custom/<model>"),
            (json!(["human:boss", "gpt-6-astra", "Kamil"]), "Kamil"),
            (json!(["human:alice"]), "alice"),
            (json!(["watcher:github"]), "GitHub PR watcher"),
            (json!(["job:internal-run"]), "Scheduled job"),
            (json!([null]), "Unassigned"),
        ] {
            assert_eq!(label(&mut page, args), expected);
        }
        for model in [
            Value::Null,
            json!(""),
            json!({}),
            json!("x".repeat(257)),
            json!("\u{1b}model"),
        ] {
            assert_eq!(
                label(&mut page, json!([codex, model])),
                "Codex · model unknown"
            );
        }
        let remember =
            "HeyBossUI.rememberActors({actor_models:{'codex:internal-session':'gpt-6-sol'}})";
        assert_eq!(page.eval(remember).unwrap(), true);
        assert_eq!(label(&mut page, json!([codex])), "Codex · gpt-6-sol");
        assert_eq!(
            label(&mut page, json!([codex, "gpt-6-astra"])),
            "Codex · gpt-6-astra"
        );
        assert_eq!(page.eval(remember).unwrap(), false);
    });
}

#[test]
#[ignore = "requires Chrome"]
fn assignment_runtime_uses_live_time_only_while_connected() {
    with_client(|port, _| {
        let mut page = Page::open(&format!("http://127.0.0.1:{port}/"));
        let now = 2_000_000_i64;
        let entry = json!({"online":true,"machine":{"state":"connected","heartbeat":now/1000},
            "run":{"state":"running","started_at":now-125000,"finished_at":null}});
        let mut runtime = |entry: &Value, at: i64| {
            page.eval(&format!("IssueAssignments.runtime({entry}, {at})"))
                .unwrap()
        };
        for (elapsed, clock, duration) in [
            (125000, now, "2m 05s"),
            (125000, now + 1000, "2m 06s"),
            (3661000, now, "1h 01m"),
            (90000000, now, "1d 01h"),
        ] {
            let mut active = entry.clone();
            active["run"]["started_at"] = json!(now - elapsed);
            let value = runtime(&active, clock);
            assert_eq!(value["duration"], duration);
            assert_eq!(value["live"], true);
        }
        let mut finished = entry.clone();
        finished["run"]["finished_at"] = json!(now - 5000);
        finished["run"]["state"] = json!("completed");
        let result = runtime(&finished, now + 100000);
        assert_eq!(result["duration"], "2m 00s");
        assert_eq!(result["label"], "Completed");
        assert_eq!(result["live"], false);

        let mut offline = entry.clone();
        offline["online"] = json!(false);
        for result in [
            runtime(&offline, now + 100000),
            runtime(&entry, now + 16000),
        ] {
            assert_eq!(result["duration"], "2m 05s");
            assert_eq!(result["label"], "Last seen");
            assert_eq!(result["live"], false);
        }
        let mut missing_heartbeat = entry.clone();
        missing_heartbeat["machine"] = json!({"state":"disconnected"});
        assert_eq!(runtime(&missing_heartbeat, now)["duration"], "Unavailable");
        let mut approval = entry.clone();
        approval["run"]["state"] = json!("approval_required");
        assert_eq!(runtime(&approval, now)["label"], "Needs approval");

        // NaN, Infinity, and undefined are browser inputs that JSON cannot represent.
        for start in ["null", "undefined", "NaN", "Infinity", "'123'", "2000001"] {
            let expr = format!(
                "IssueAssignments.runtime({{...{entry},run:{{started_at:{start}}}}},{now}).duration"
            );
            assert_eq!(page.eval(&expr).unwrap(), "Unavailable", "{start}");
        }
    });
}
