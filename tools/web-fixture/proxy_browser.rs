//! Shipped proxy dashboards in Chrome, with subscription and spend data supplied over HTTP.
use super::*;
use tiny_http::{Header, Request, Response, Server};

struct Api(mpsc::Receiver<Request>);
impl Api {
    #[track_caller]
    fn next(&self) -> Request {
        self.0
            .recv_timeout(Duration::from_secs(5))
            .expect("browser API request")
    }
    #[track_caller]
    fn reply(&self, path: &str, status: u16, value: &Value) {
        let req = self.next();
        assert_eq!(req.url(), path);
        respond(req, status, value);
    }
}
fn respond(req: Request, status: u16, value: &Value) {
    req.respond(
        Response::from_string(value.to_string())
            .with_status_code(status)
            .with_header(Header::from_bytes("Content-Type", "application/json").unwrap()),
    )
    .unwrap();
}
fn with_page(path: &str, test: impl FnOnce(&mut Page, Api)) {
    let server = Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}{path}", server.server_addr().to_ip().unwrap());
    let (send, requests) = mpsc::channel();
    let (stop, done) = mpsc::channel::<()>();
    thread::scope(|s| {
        s.spawn(move || {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/hey-proxy/src/proxy");
            while matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                let Some(req) = server.recv_timeout(Duration::from_millis(50)).unwrap() else {
                    continue;
                };
                let (file, kind) = match req.url() {
                    "/logs" => ("logs.html", "text/html"),
                    "/apis" => ("overview.html", "text/html"),
                    "/logs/dashboard.js" => ("dashboard.js", "text/javascript"),
                    "/overview.js" => ("overview.js", "text/javascript"),
                    _ => {
                        if send.send(req).is_err() {
                            break;
                        }
                        continue;
                    }
                };
                let _ = req.respond(
                    Response::from_data(fs::read(root.join(file)).unwrap()).with_header(
                        Header::from_bytes("Content-Type", format!("{kind}; charset=utf-8"))
                            .unwrap(),
                    ),
                );
            }
        });
        let _stop = stop;
        let mut page = Page::open("about:blank");
        page.command("Page.enable", json!({})).unwrap();
        page.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"window.pageErrors=[]; addEventListener('error',e=>pageErrors.push(e.message)); addEventListener('unhandledrejection',e=>pageErrors.push(String(e.reason)))"})).unwrap();
        page.command("Page.navigate", json!({"url":url})).unwrap();
        test(&mut page, Api(requests));
        assert_eq!(page.eval("window.pageErrors").unwrap(), json!([]));
    });
}
fn click(page: &mut Page, id: &str) {
    page.eval(&format!("document.getElementById({}).click()", json!(id)))
        .unwrap();
}
fn shows(page: &mut Page, id: &str, text: &str) {
    page.wait(&format!(
        "document.getElementById({})?.innerText.includes({})",
        json!(id),
        json!(text)
    ));
}
fn overview(api: &Api, catalog: &Value, accounts: &Value) {
    let mut paths = vec![];
    for _ in 0..2 {
        let req = api.next();
        let path = req.url().to_owned();
        let data = match path.as_str() {
            "/overview/api" => catalog,
            "/providers/v1" => accounts,
            _ => panic!("unexpected overview request {path}"),
        };
        respond(req, 200, data);
        paths.push(path);
    }
    assert_ne!(paths[0], paths[1]);
}
fn catalog(id: &str) -> Value {
    json!({"mode":"standalone","relay":false,"apis":[{"id":id,"name":id,"description":"Native","configured":true,"base_path":"/v1","routes":[],"models":[]}]})
}

#[test]
#[ignore = "requires Chrome"]
fn spend_reports_gaps_and_keeps_stale_values_until_recovery() {
    with_page("/logs", |p, api| {
        let totals = json!({"snapshot_ms":1700000000000_i64,"today":{"requests":4,"priced_requests":3,"unpriced_requests":1,"estimated_cost_usd":1.25},"week":{"requests":8,"priced_requests":0,"unpriced_requests":8,"estimated_cost_usd":null},"all_time":{"requests":20,"priced_requests":12,"unpriced_requests":8,"estimated_cost_usd":10.5}});
        let mut data = json!({"source":"host","rpm":1234,"spend":totals,"logging":{"enabled":true,"status":"healthy"}});
        let req = api.next();
        let path = req.url().to_owned();
        let url = reqwest::Url::parse(&format!("http://localhost{path}")).unwrap();
        assert_eq!(url.path(), "/logs/api/dashboard");
        let query: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(query.len(), 2);
        for key in ["day_start_ms", "week_start_ms"] {
            let millis = query[key].parse::<i64>().unwrap();
            let check = if key == "week_start_ms" {
                "d.setDate(d.getDate()-(d.getDay()+6)%7);"
            } else {
                ""
            };
            assert_eq!(
                p.eval(&format!(
                    "(()=>{{const d=new Date(); {check} return d.setHours(0,0,0,0)}})()"
                ))
                .unwrap(),
                millis
            );
        }
        // Exercise the busy guard even when an event bypasses the disabled button.
        p.eval("document.querySelector('#refresh').dispatchEvent(new Event('click'))")
            .unwrap();
        assert!(api.0.recv_timeout(Duration::from_millis(150)).is_err());
        respond(req, 200, &data);
        shows(p, "status", "Live");
        shows(p, "scope", "Connected host");
        shows(p, "today", "$1.25");
        shows(p, "week", "Unavailable");
        shows(p, "all_time", "$10.50");
        shows(p, "today-note", "partial estimate");
        p.eval("Object.defineProperty(document,'hidden',{configurable:true,value:true}); document.dispatchEvent(new Event('visibilitychange'))").unwrap();
        assert!(api.0.recv_timeout(Duration::from_millis(150)).is_err());
        p.eval("delete document.hidden").unwrap();
        data["logging"] =
            json!({"enabled":true,"status":"error","write_errors":3,"dropped_events":0});
        click(p, "refresh");
        api.reply(&path, 200, &data);
        shows(p, "status", "Logging error");
        p.wait("document.querySelector('#status').className === 'warning'");
        data["logging"]["status"] = "healthy".into();
        click(p, "refresh");
        api.reply(&path, 200, &data);
        shows(p, "status", "Live");
        p.wait("document.querySelector('#status').className === ''");
        data["logging"]["status"] = "gaps".into();
        data["logging"]["dropped_events"] = 2.into();
        click(p, "refresh");
        api.reply(&path, 200, &data);
        shows(p, "status", "2 dropped events");
        click(p, "refresh");
        api.reply(&path, 401, &Value::Null);
        shows(p, "status", "Stale · Session expired");
        shows(p, "today", "$1.25");
        p.wait("!document.querySelector('#login').hidden && !document.querySelector('#refresh').disabled");
        data["logging"] = json!({"enabled":false,"status":"disabled"});
        data["spend"] = Value::Null;
        click(p, "refresh");
        api.reply(&path, 200, &data);
        shows(p, "today", "Unavailable");
        shows(p, "today-note", "disabled");
        p.wait("document.querySelector('#login').hidden");
    });
}

#[test]
#[ignore = "requires Chrome"]
fn subscription_usage_distinguishes_unknown_disabled_and_zero() {
    with_page("/apis", |p, api| {
        overview(
            &api,
            &catalog("claude"),
            &json!({"schema_version":1,"connections":[]}),
        );
        let label = "<img src=x onerror=bad()>";
        let mut usage = json!({"state":"stale","updated_at":1790800000,"error":"Waiting before retrying","data":{"windows":[{"label":label,"used_percent":null},{"label":"Weekly","used_percent":112,"resets_at":"bad-date"}],"extra_usage":{"enabled":false}}});
        api.reply("/claude/usage", 200, &usage);
        shows(p, "usage-status", "Stale");
        shows(p, "usage-windows", label);
        shows(p, "usage-windows", "Usage not reported");
        p.wait("document.querySelectorAll('.usage-card').length===2 && !document.querySelector('.usage-card img, .usage-card:first-child progress') && document.querySelector('.usage-card:nth-child(2) progress').value===100");
        p.wait("document.querySelector('.usage-card:nth-child(2) p:last-child').textContent === 'Reset time not reported'");
        shows(p, "extra-usage", "Extra usage is disabled.");
        for (spend, expected) in [
            (
                json!({"enabled":false,"spend":{"currency":"USD","used":12.5,"limit":10,"remaining":0,"over_limit":2.5}}),
                "disabled. Provider-reported monthly extra spend: USD 12.50 · cap: USD 10.00 · remaining: USD 0.00 · above cap: USD 2.50",
            ),
            (
                json!({"enabled":true,"spend":{"currency":"USD","used":0}}),
                "monthly extra spend: USD 0.00 · cap: unknown · remaining: unknown",
            ),
        ] {
            usage["data"]["extra_usage"] = spend;
            click(p, "refresh-usage");
            api.reply("/claude/usage", 200, &usage);
            shows(p, "extra-usage", expected);
        }
        click(p, "refresh-usage");
        api.reply("/claude/usage", 401, &Value::Null);
        shows(p, "usage-error", "session expired");
        shows(p, "usage-status", "stale");
        p.wait("!document.querySelector('#refresh-usage').disabled");
    });
}

#[test]
#[ignore = "requires Chrome"]
fn named_accounts_show_routing_and_limits_across_screen_sizes() {
    with_page("/apis", |p, api| {
        let mut configuration = catalog("responses");
        configuration["routing"] = json!({"claude_default":{"name":"Claude","auth":"subscription"},"accounts":{"codex-personal":{"defaults":["responses"],"models":[],"fallback":false},"workspace":{"defaults":[],"models":["workspace-model"],"fallback":false}}});
        let mut connections:Vec<_>=[
            ("codex-personal","codex","subscription",true,Some("one")),
            ("codex-work","codex","subscription",true,Some("two")),
            ("codex-personal-alias","codex","subscription",true,Some("one")),
            ("claude-personal","claude","subscription",false,None),
            ("ultima","openai","api",true,Some("api")),
            ("workspace","claude","console",true,Some("workspace")),
        ].into_iter().map(|(name,implementation,auth,ready,account_ref)| json!({ "name":name,"implementation":implementation,"auth":auth,"ready":ready,"account_ref":account_ref })).collect();
        let accounts =
            |connections: &[Value]| json!({"schema_version":1,"connections":connections});
        overview(&api, &configuration, &accounts(&connections));
        shows(
            p,
            "default-routing",
            "Claude default: Subscription. Model overrides take precedence.",
        );
        shows(p, "connections", "Default for Responses");
        shows(p, "connections", "Override only: workspace-model");
        p.wait("document.querySelectorAll('.connection-card').length===6 && [...document.querySelectorAll('.connection-card')].find(c=>c.querySelector('h3').textContent==='codex-personal').innerText.includes('Codex')");
        p.wait("!document.querySelector('[aria-label=\"View limits for workspace\"]') && document.querySelector('[aria-label=\"View limits for claude-personal\"]').disabled");
        p.wait("[...document.querySelectorAll('.connection-card p')].filter(n=>n.textContent==='Shared subscription · limits also apply to its other aliases.').length===2");
        p.eval("document.querySelector('[aria-label=\"View limits for codex-work\"]').click()")
            .unwrap();
        let usage = json!({"state":"stale","updated_at":1790800000,"error":"Rate limited; waiting before retrying","data":{"windows":[{"label":"Session · 5 hours","used_percent":93,"remaining_percent":7},{"label":"Weekly","used_percent":null}]}});
        api.reply("/usage/v1/codex/codex-work", 200, &usage);
        shows(p, "claude-usage-title", "codex-work · subscription limits");
        shows(p, "usage-windows", "93% used");
        shows(p, "usage-status", "Stale");
        p.wait("document.querySelectorAll('#usage-windows progress').length===1");
        for width in [1440, 768, 390, 320] {
            p.command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":1080,"deviceScaleFactor":1,"mobile":false}),
            )
            .unwrap();
            for theme in ["light", "dark"] {
                p.eval(&format!(
                    "document.documentElement.dataset.theme={}",
                    json!(theme)
                ))
                .unwrap();
                assert_eq!(
                    p.eval("document.documentElement.scrollWidth<=innerWidth")
                        .unwrap(),
                    true,
                    "overflow at {width} in {theme}"
                );
                if let Ok(dir) = std::env::var("SCREENSHOTS") {
                    let mut clip =
                        p.command("Page.getLayoutMetrics", json!({})).unwrap()["cssContentSize"]
                            .clone();
                    clip["scale"] = 1.into();
                    capture(
                        p,
                        &dir,
                        &format!("accounts-{width}-{theme}"),
                        json!({"format":"png","captureBeyondViewport":true,"clip":clip}),
                    );
                }
            }
        }
        p.eval("document.querySelector('#refresh-accounts').focus()")
            .unwrap();
        p.wait("document.activeElement.id==='refresh-accounts'");
        for kind in ["keyDown", "keyUp"] {
            p.command(
                "Input.dispatchKeyEvent",
                json!({"type":kind,"key":"Enter","code":"Enter","windowsVirtualKeyCode":13,"text":if kind=="keyDown" {"\r"} else {""}}),
            )
            .unwrap();
        }
        api.reply("/providers/v1", 200, &accounts(&connections));
        configuration["routing"]["claude_default"] = Value::Null;
        click(p, "refresh");
        api.reply("/overview/api", 200, &configuration);
        api.reply("/usage/v1/codex/codex-work", 200, &usage);
        shows(
            p,
            "default-routing",
            "Claude default: Not configured. A named workspace does not become the default.",
        );
        connections.truncate(1);
        connections[0]["name"] = "a".repeat(128).into();
        click(p, "refresh-accounts");
        api.reply("/providers/v1", 200, &accounts(&connections));
        shows(p, "connections", &"a".repeat(128));
        p.wait("document.documentElement.scrollWidth<=innerWidth");
        click(p, "refresh-accounts");
        api.reply("/providers/v1", 200, &accounts(&[]));
        shows(
            p,
            "connection-status",
            "No named accounts configured. Default connections remain available below.",
        );
        click(p, "refresh-accounts");
        api.reply("/providers/v1", 503, &Value::Null);
        shows(
            p,
            "connection-status",
            "Account status unavailable. Refresh to try again.",
        );
        p.wait("document.querySelectorAll('.connection-card').length===0");
    });
}
