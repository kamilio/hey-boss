//! Real /skills assets with controllable fleet responses for browser race and recovery checks.
use super::*;
use std::sync::Mutex;
use tiny_http::{Header, Method, Request, Response, Server};

struct Fleet {
    inventory: Value,
    token: &'static str,
    actor: &'static str,
    failure: &'static str,
    bootstraps: usize,
    reads: usize,
    posts: Vec<(String, String)>,
    applied: Vec<Value>,
    delay: bool,
    pending: Option<(Request, Value)>,
}
fn skill(name: &str, digest: &str, text: &str) -> Value {
    json!({"name":name,"scope":"global","agent":"codex","digest":digest,"text":text,"word_count":4,"path":format!("/fixture/{name}")})
}
fn inventory(versions: &[Value], name: &str) -> Value {
    let machines:Vec<_>=["local","devbox","laptop.local"].into_iter().enumerate().map(|(i,h)|json!({"host":h,"state":"online","scanned_at":1700000000,"copies":[versions[if i==0{0}else{versions.len()-1}]]})).collect();
    json!({"ok":true,"revision":17,"busy":false,"selected":[name],"choices":{},"max_words":400,"message":"Inventory refreshed","machines":machines})
}
impl Fleet {
    fn new(inventory: Value) -> Self {
        Self {
            inventory,
            token: "initial",
            actor: "fixture",
            failure: "",
            bootstraps: 0,
            reads: 0,
            posts: vec![],
            applied: vec![],
            delay: false,
            pending: None,
        }
    }
    fn handle(&mut self, mut req: Request) {
        let (status, value) = if req.url() == "/api/bootstrap" {
            self.bootstraps += 1;
            if self.failure == "bootstrap" {
                (503, json!({"ok":false}))
            } else {
                (
                    200,
                    json!({"ok":true,"csrf":self.token,"actor":{"id":self.actor},"projects":[]}),
                )
            }
        } else if req.method() == &Method::Get {
            self.reads += 1;
            (200, self.inventory.clone())
        } else {
            assert_eq!(req.method(), &Method::Post);
            let mut raw = String::new();
            req.as_reader().read_to_string(&mut raw).unwrap();
            let body: Value = serde_json::from_str(&raw).unwrap();
            let token = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("X-Hey-Boss-CSRF"))
                .unwrap()
                .value
                .to_string();
            let rejected = token != self.token || self.failure == "forbidden";
            self.posts.push((raw, token));
            if self.failure == "network" {
                // End the response before a valid HTTP status has arrived.
                let _ = req.into_writer().write_all(b"HTTP/1.1 invalid\r\n\r\n");
                return;
            }
            if self.failure == "server" {
                (503, json!({"ok":false,"error":{"message":"Unavailable"}}))
            } else if rejected {
                (
                    403,
                    json!({"ok":false,"error":{"code":"forbidden","message":"Reconnect required"}}),
                )
            } else if self.failure == "conflict" {
                (
                    409,
                    json!({"ok":false,"error":{"message":"Inventory changed"}}),
                )
            } else {
                self.applied.push(body.clone());
                if self.delay {
                    let mut reply = self.inventory.clone();
                    reply["choices"] = body["choices"].clone();
                    reply["busy"] = true.into();
                    assert!(
                        self.pending.replace((req, reply)).is_none(),
                        "Overlapping fleet writes"
                    );
                    return;
                }
                (200, self.inventory.clone())
            }
        };
        reply(req, status, &value);
    }
    fn release(&mut self) {
        let (req, value) = self.pending.take().unwrap();
        reply(req, 200, &value);
        self.inventory = value;
    }
}
fn reply(req: Request, status: u16, v: &Value) {
    let _ = req.respond(
        Response::from_string(v.to_string())
            .with_status_code(status)
            .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
            .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap()),
    );
}
fn with_page(data: Value, test: impl FnOnce(&mut Page, &Mutex<Fleet>, &str)) {
    let f = fixture();
    f.init("Skills QA").unwrap();
    let (_web, base) = web(&f, "Skills QA");
    let server = Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}/skills", server.server_addr().to_ip().unwrap());
    let fleet = Mutex::new(Fleet::new(data));
    let (stop, done) = mpsc::channel::<()>();
    thread::scope(|s| {
        let fleet = &fleet;
        s.spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap();
            while matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                let Some(req) = server.recv_timeout(Duration::from_millis(50)).unwrap() else {
                    continue;
                };
                match req.url() {
                    "/api/bootstrap" | "/api/skills" => fleet.lock().unwrap().handle(req),
                    path if !path.starts_with("/api/") && req.method() == &Method::Get => {
                        let response = client.get(format!("{base}{path}")).send().unwrap();
                        let status = response.status().as_u16();
                        let kind = response
                            .headers()
                            .get("Content-Type")
                            .map_or("application/octet-stream", |v| v.to_str().unwrap());
                        let header = Header::from_bytes("Content-Type", kind).unwrap();
                        let _ = req.respond(
                            Response::from_data(response.bytes().unwrap().to_vec())
                                .with_header(header)
                                .with_status_code(status),
                        );
                    }
                    _ => {
                        let _ = req.respond(Response::empty(404));
                    }
                }
            }
        });
        let _stop = stop;
        let mut p = Page::open("about:blank");
        p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error',e=>sessionStorage.setItem('pageError',e.message));addEventListener('unhandledrejection',e=>sessionStorage.setItem('pageError',String(e.reason)))"})).unwrap();
        test(&mut p, fleet, &url);
        assert_eq!(
            p.eval("sessionStorage.getItem('pageError')").unwrap(),
            Value::Null
        );
    });
}
fn click(p: &mut Page, selector: &str) {
    let el = format!("document.querySelector({})", json!(selector));
    p.wait(&format!("{el}?.disabled === false"));
    p.eval(&format!("{el}.click()")).unwrap();
}
fn viewport(p: &mut Page, width: i32, theme: &str) {
    p.command(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":width,"height":1000,"deviceScaleFactor":1,"mobile":false}),
    )
    .unwrap();
    p.command(
        "Emulation.setEmulatedMedia",
        json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
    )
    .unwrap();
    p.wait("document.documentElement.scrollWidth <= innerWidth");
}
fn shot(p: &mut Page, name: &str) {
    if let Ok(dir) = std::env::var("SKILLS_SCREENSHOTS") {
        screenshot(p, &dir, name);
    }
}
const EDITOR: &str =
    "document.querySelector('#skill-codemirror-host')?.shadowRoot?.querySelector('.cm-content')";
fn edit(p: &mut Page, text: &str) {
    click(p, "[data-toggle-editor]");
    p.wait(&format!("!!{EDITOR}"));
    p.eval(&format!("{EDITOR}.focus()")).unwrap();
    p.command(
        "Input.dispatchKeyEvent",
        json!({"type":"keyDown","key":"a","code":"KeyA","commands":["selectAll"]}),
    )
    .unwrap();
    p.command(
        "Input.dispatchKeyEvent",
        json!({"type":"keyUp","key":"a","code":"KeyA"}),
    )
    .unwrap();
    p.command("Input.insertText", json!({"text":text})).unwrap();
    assert_eq!(
        p.eval(&format!("{EDITOR}.innerText"))
            .unwrap()
            .as_str()
            .unwrap()
            .trim_end_matches('\n'),
        text.trim_end_matches('\n')
    );
}
fn key(p: &mut Page, key: &str, code: &str, vk: i32, text: &str) {
    for kind in ["keyDown", "keyUp"] {
        p.command("Input.dispatchKeyEvent",json!({"type":kind,"key":key,"code":code,"windowsVirtualKeyCode":vk,"text":if kind=="keyDown"{text}else{""}})).unwrap();
    }
}

fn navigate(p: &mut Page, url: &str) {
    p.command("Page.navigate", json!({"url":url})).unwrap();
    p.wait(&format!("location.href.startsWith({}) && document.querySelector('#skills-scan')?.disabled === false",json!(url)));
}
fn until(f: &Mutex<Fleet>, condition: impl Fn(&Fleet) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !condition(&f.lock().unwrap()) {
        assert!(
            std::time::Instant::now() < deadline,
            "Expected skills request did not arrive"
        );
        thread::sleep(Duration::from_millis(25));
    }
}
fn chosen(p: &mut Page, digest: &str) {
    p.wait(&format!(
        "document.querySelector('[data-choose={digest}]')?.getAttribute('aria-pressed') === 'true'"
    ));
}

#[path = "skills_editor.rs"]
mod editor;

#[path = "skills_selection.rs"]
mod selection;

#[path = "skills_reconnect.rs"]
mod reconnect;
