//! Embedded web UI. Loopback listener, optional private HTTPS proxy, shared SQLite.
use super::{Actor, Error, Operation, Project, Request, Result, Store, identity};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server, StatusCode};

fn parse_query(url: &str) -> Result<std::collections::HashMap<String, String>> {
    fn decode(input: &str) -> Result<String> {
        let mut bytes = Vec::new();
        let mut chars = input.bytes();
        while let Some(byte) = chars.next() {
            bytes.push(match byte {
                b'+' => b' ',
                b'%' => {
                    let a = chars.next().and_then(|c| (c as char).to_digit(16));
                    let b = chars.next().and_then(|c| (c as char).to_digit(16));
                    match (a, b) {
                        (Some(a), Some(b)) => (a * 16 + b) as u8,
                        _ => return Err(Error::invalid("Invalid query encoding")),
                    }
                }
                other => other,
            });
        }
        String::from_utf8(bytes).map_err(|_| Error::invalid("Invalid query encoding"))
    }
    url.split_once('?')
        .map(|(_, query)| {
            query
                .split('&')
                .map(|part| {
                    let (key, value) = part.split_once('=').unwrap_or((part, ""));
                    Ok((decode(key)?, decode(value)?))
                })
                .collect()
        })
        .unwrap_or_else(|| Ok(std::collections::HashMap::new()))
}

pub struct Config {
    pub port: u16,
    pub mobile_origin: Option<String>,
    pub discover: bool,
    pub project: Option<String>,
    pub actor: Option<String>,
    pub host: Option<String>,
    pub json: bool,
    pub mindmap: bool,
}
enum Backend {
    Local(std::path::PathBuf),
    Remote(String),
}
struct App {
    backend: Backend,
    project: Project,
    actor: Actor,
    token: String,
    authority: String,
    mobile_origin: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Action {
    project: String,
    operation: Operation,
    request_id: Option<String>,
    #[serde(default)]
    host: Option<String>,
}
impl App {
    fn execute(
        &self,
        project: Option<String>,
        operation: Operation,
        request_id: Option<String>,
    ) -> Result<Value> {
        self.execute_at(project, operation, request_id, None)
    }
    fn execute_at(
        &self,
        project: Option<String>,
        operation: Operation,
        request_id: Option<String>,
        host: Option<&str>,
    ) -> Result<Value> {
        let request = Request {
            version: 1,
            project: self.project.clone(),
            project_override: project,
            actor: Some(self.actor.clone()),
            operation,
            request_id,
        };
        if let Some(host) = host {
            if !crate::health::remote::valid_host(host) {
                return Err(Error::invalid("Invalid SSH host"));
            }
            return super::remote::call(host, &request);
        }
        match &self.backend {
            Backend::Local(path) => Store::open(path)?.execute(&request),
            Backend::Remote(host) => super::remote::call(host, &request),
        }
    }
}

pub fn serve(config: Config) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let mobile_origin = config
        .mobile_origin
        .map(validate_mobile_origin)
        .transpose()?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let original_executable = super::worker::executable_identity(&executable);
    let cwd = std::env::current_dir()?.canonicalize()?;
    let machine = identity::machine()?;
    let project = identity::project(&cwd, &machine)?;
    // The web app always belongs to Boss, regardless of the launching session.
    let actor = Actor {
        id: "human:boss".into(),
        kind: "human".into(),
        session_id: None,
        machine,
        host: identity::host(),
        pid: None,
        process_start: None,
        cwd,
        source: "web interface".into(),
    };
    let backend = match config.host {
        Some(host) => {
            if !crate::health::remote::valid_host(&host) {
                return Err(Error::invalid("Invalid SSH host"));
            }
            Backend::Remote(host)
        }
        None => {
            let path = super::database_path()?;
            Store::open(&path)?;
            Backend::Local(path)
        }
    };
    let server = Server::http(("127.0.0.1", config.port))
        .map_err(|e| Error::new("io_error", e.to_string()))?;
    let authority = server.server_addr().to_ip().unwrap().to_string();
    if let Backend::Local(path) = &backend {
        crate::agent_conversations::start_bridge(path.clone());
    }
    let mut secret = [0_u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut secret)?;
    let mut app = App {
        backend,
        project,
        actor,
        token: secret.iter().map(|b| format!("{b:02x}")).collect(),
        authority,
        mobile_origin,
    };
    let resolved = app.execute(
        config.project,
        Operation::Projects {
            include_hidden: true,
        },
        None,
    )?;
    app.project = serde_json::from_value(resolved["project"].clone())?;
    let url = format!(
        "http://{}/{}",
        app.authority,
        if config.mindmap { "mm" } else { "" }
    );
    if config.json {
        println!(
            "{}",
            json!({"ok":true,"url":url,"mobile_url":app.mobile_origin,"project":app.project,"actor":app.actor.id})
        );
    } else {
        println!(
            "Hey Boss {} · {url}\nPress Ctrl+C to stop.",
            if config.mindmap { "Mindmaps" } else { "Issues" }
        );
        if let Some(origin) = &app.mobile_origin {
            println!("Mobile · {origin}/ (requires private Tailscale Serve)");
        }
    }
    std::io::stdout().flush()?;
    let app = Arc::new(app);
    let stop = Arc::new(AtomicBool::new(false));
    let reload = Arc::new(AtomicBool::new(false));
    super::worker::install_signals_for_upgrade(stop.clone(), Some(reload.clone()))?;
    {
        let stop = stop.clone();
        let reload = reload.clone();
        let executable = executable.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if super::worker::executable_identity(&executable)
                    .is_some_and(|current| Some(current) != original_executable)
                {
                    reload.store(true, Ordering::Relaxed);
                    if stop
                        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                        .is_err()
                    {
                        reload.store(false, Ordering::Relaxed);
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
    }
    if config.discover && matches!(app.backend, Backend::Local(_)) {
        let app = Arc::clone(&app);
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                // Process/Git inspection happens outside the database lock so the UI
                // stays responsive while projects are discovered.
                let projects =
                    super::discovery::projects(&crate::agents::scan(), &app.actor.machine);
                if let Backend::Local(path) = &app.backend {
                    let result = Store::open(path).and_then(|mut store| {
                        store.discover_projects(&projects)?;
                        super::worker::recover(&mut store, &app.actor.machine)?;
                        Ok(())
                    });
                    if let Err(error) = result {
                        eprintln!("Project discovery: {error}");
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(15));
            }
        });
    }
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let app = &app;
            let server = &server;
            let stop = &stop;
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match server.recv_timeout(Duration::from_millis(250)) {
                        Ok(Some(request))
                            if request.url().split('?').next() == Some("/api/fleet/events") =>
                        {
                            let app = Arc::clone(app);
                            std::thread::spawn(move || {
                                if request.method() != &Method::Get || !allowed(&request, &app) {
                                    let _ = request.respond(Response::empty(403));
                                    return;
                                }
                                match crate::fleet::subscribe() {
                                    Ok(mut stream) => {
                                        let mut output = request.into_writer();
                                        if output.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n").and_then(|_| output.flush()).is_err() { return; }
                                        let mut bytes = [0u8; 8192];
                                        loop {
                                            match stream.read(&mut bytes) {
                                                Ok(0) | Err(_) => break,
                                                Ok(n) => if output.write_all(&bytes[..n]).and_then(|_| output.flush()).is_err() { break; }
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        let _ = request.respond(Response::empty(503));
                                    }
                                }
                            });
                        }
                        Ok(Some(request)) => respond(request, app),
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            });
        }
    });
    if reload.load(Ordering::Relaxed) {
        let port = server.server_addr().to_ip().unwrap().port().to_string();
        let mut args: Vec<_> = std::env::args_os().skip(1).collect();
        for index in 0..args.len().saturating_sub(1) {
            if args[index] == "--port" && args[index + 1] == "0" {
                args[index + 1] = port.clone().into();
            }
        }
        for arg in &mut args {
            if arg == "--port=0" {
                *arg = format!("--port={port}").into();
            }
        }
        drop(server);
        drop(app);
        return Err(std::process::Command::new(executable)
            .args(args)
            .exec()
            .into());
    }
    Ok(())
}

fn header<'a>(request: &'a tiny_http::Request, name: &str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.to_string().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}
// This is an explicit trusted proxy origin, never an arbitrary forwarded header.
// Tailscale provides authentication/TLS; this server stays on IPv4 loopback.
fn validate_mobile_origin(origin: String) -> Result<String> {
    let invalid = || {
        Error::invalid(
            "Mobile origin must be an HTTPS Tailscale hostname (https://machine.tailnet.ts.net[:port]), without a path",
        )
    };
    let authority = origin.strip_prefix("https://").ok_or_else(invalid)?;
    let hostname = if let Some((hostname, port)) = authority.split_once(':') {
        if !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        port.parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(invalid)?;
        if port == "443" || port.starts_with('0') {
            return Err(invalid());
        }
        hostname
    } else {
        authority
    };
    let labels: Vec<_> = hostname.split('.').collect();
    if hostname.len() > 253
        || labels.len() < 4
        || !hostname.ends_with(".ts.net")
        || labels.iter().any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
    {
        return Err(invalid());
    }
    Ok(origin)
}
fn allowed(request: &tiny_http::Request, app: &App) -> bool {
    let local = app.authority.replace("127.0.0.1", "localhost");
    let origin = match header(request, "host") {
        Some(host) if host == app.authority || host == local => format!("http://{host}"),
        Some(host)
            if app
                .mobile_origin
                .as_deref()
                .is_some_and(|origin| origin.strip_prefix("https://") == Some(host)) =>
        {
            app.mobile_origin.clone().unwrap()
        }
        _ => return false,
    };
    if header(request, "sec-fetch-site").is_some_and(|s| !["same-origin", "none"].contains(&s)) {
        return false;
    }
    if header(request, "origin").is_some_and(|value| value != origin) {
        return false;
    }
    true
}

fn respond(mut request: tiny_http::Request, app: &App) {
    let start = std::time::Instant::now();
    // Mermaid's sanitized SVG includes scoped styles. Only its document page
    // permits those styles; script execution remains restricted to local assets.
    let artifact_page = request.url().split('?').next() == Some("/artifacts");
    let reply = route(&mut request, app);
    let (status, kind, bytes) = match reply {
        Ok(value) => value,
        Err(error) => {
            let status = match error.code.as_str() {
                "forbidden" => 403,
                "not_found" => 404,
                "conflict" => 409,
                "invalid_input" | "identity_unavailable" => 400,
                _ => 503,
            };
            (
                status,
                "application/json; charset=utf-8",
                serde_json::to_vec(&json!({"ok":false,"error":error})).unwrap(),
            )
        }
    };
    let mut response = Response::from_data(bytes).with_status_code(StatusCode(status));
    for (name, value) in [
        ("Content-Type", kind),
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("X-Frame-Options", "DENY"),
        (
            "Content-Security-Policy",
            if artifact_page {
                "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
            } else {
                "default-src 'none'; script-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
            },
        ),
    ] {
        response.add_header(Header::from_bytes(name, value).unwrap());
    }
    response.add_header(
        Header::from_bytes(
            "Server-Timing",
            format!("app;dur={:.2}", start.elapsed().as_secs_f64() * 1000.0),
        )
        .unwrap(),
    );
    let _ = request.respond(response);
}
fn route(request: &mut tiny_http::Request, app: &App) -> Result<(u16, &'static str, Vec<u8>)> {
    if !allowed(request, app) {
        return Err(Error::new(
            "forbidden",
            "This interface only accepts its configured same-origin requests",
        ));
    }
    let path = request.url().split('?').next().unwrap_or("/").to_owned();
    if request.method() == &Method::Get {
        let asset: Option<(&str, &[u8])> = match path.as_str() {
            "/" => Some(("text/html; charset=utf-8", include_bytes!("web/index.html"))),
            "/artifacts" => Some((
                "text/html; charset=utf-8",
                include_bytes!("web/artifacts.html"),
            )),
            "/artifact-editor.js" => Some((
                "application/javascript; charset=utf-8",
                include_bytes!("web/artifact-editor.js"),
            )),
            "/artifact-diagrams.js" => Some((
                "application/javascript; charset=utf-8",
                include_bytes!("web/artifact-diagrams.js"),
            )),
            "/artifacts.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/artifacts.js"),
            )),
            "/artifacts.css" => Some((
                "text/css; charset=utf-8",
                include_bytes!("web/artifacts.css"),
            )),
            "/mm" => Some((
                "text/html; charset=utf-8",
                include_bytes!("web/mindmap.html"),
            )),
            "/mindmap.css" => Some(("text/css; charset=utf-8", include_bytes!("web/mindmap.css"))),
            "/mindmap.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/mindmap.js"),
            )),
            "/mindmap-map.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/mindmap-map.js"),
            )),
            "/agents" | "/agents/session" | "/workers" => {
                Some(("text/html; charset=utf-8", include_bytes!("web/fleet.html")))
            }
            "/fleet.css" => Some(("text/css; charset=utf-8", include_bytes!("web/fleet.css"))),
            "/fleet.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/fleet.js"),
            )),
            "/app.css" => Some(("text/css; charset=utf-8", include_bytes!("web/app.css"))),
            "/components.css" => Some((
                "text/css; charset=utf-8",
                include_bytes!("web/components.css"),
            )),
            "/components.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/components.js"),
            )),
            "/quick-issue.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/quick-issue.js"),
            )),
            "/app.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/app.js"),
            )),
            "/inbox.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/inbox.js"),
            )),
            "/tags.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/tags.js"),
            )),
            "/subtasks.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/subtasks.js"),
            )),
            "/issue-order.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/issue-order.js"),
            )),
            "/global-settings.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/global-settings.js"),
            )),
            "/project-settings.js" => Some((
                "text/javascript; charset=utf-8",
                include_bytes!("web/project-settings.js"),
            )),
            "/icon.png" => Some(("image/png", include_bytes!("web/icon.png"))),
            _ if path.starts_with("/diagram-assets/") => {
                let diagrams: &[(&str, &[u8])] = include!("web/diagram-assets.rs");
                diagrams
                    .iter()
                    .find(|(name, _)| *name == path)
                    .map(|(_, bytes)| ("application/javascript; charset=utf-8", *bytes))
            }
            _ => None,
        };
        if let Some((kind, data)) = asset {
            if matches!(
                path.as_str(),
                "/" | "/mm" | "/workers" | "/agents" | "/agents/session" | "/artifacts"
            ) {
                let issues = path == "/";
                let shell = include_str!("web/app-shell.html")
                    .replace(
                        "<!--header-actions-->",
                        if issues {
                            include_str!("web/issue-header-actions.html")
                        } else {
                            ""
                        },
                    )
                    .replace(
                        "<!--profile-->",
                        if issues {
                            include_str!("web/issue-profile.html")
                        } else {
                            ""
                        },
                    )
                    .replace(
                        "<!--project-action-->",
                        if issues {
                            include_str!("web/issue-project-action.html")
                        } else {
                            ""
                        },
                    )
                    .replace(
                        "<!--mindmap-current-->",
                        if path == "/mm" {
                            " aria-current=\"page\""
                        } else {
                            ""
                        },
                    )
                    .replace(
                        "<!--workers-current-->",
                        if matches!(path.as_str(), "/workers" | "/agents" | "/agents/session") {
                            " aria-current=\"page\""
                        } else {
                            ""
                        },
                    );
                let mut html = std::str::from_utf8(data)
                    .unwrap()
                    .replace("<!--app-shell-->", &shell);
                if path == "/mm"
                    && request.url().split_once('?').is_some_and(|(_, query)| {
                        query.split('&').any(|parameter| parameter == "focus=1")
                    })
                {
                    html = html.replace(
                        "<html lang=\"en\">",
                        "<html lang=\"en\" class=\"mindmap-focus\">",
                    );
                }
                return Ok((200, kind, html.into_bytes()));
            }
            return Ok((200, kind, data.to_vec()));
        }
        if path == "/api/fleet/conversation" {
            let query = parse_query(request.url())?;
            let cursor = query
                .get("cursor")
                .map(|s| s.parse::<u64>())
                .transpose()
                .map_err(|_| Error::invalid("Invalid conversation cursor"))?
                .unwrap_or(0);
            let result = crate::agent_conversations::conversation(
                query.get("host").map(String::as_str).unwrap_or(""),
                query.get("run").map(String::as_str).unwrap_or(""),
                &crate::agent_conversations::Window {
                    cursor,
                    before: query
                        .get("before")
                        .map(|s| s.parse::<u64>())
                        .transpose()
                        .map_err(|_| Error::invalid("Invalid earlier-history cursor"))?,
                    latest: query.get("latest").is_some_and(|v| v == "1"),
                },
            )?;
            return json_response(result);
        }
        if path == "/api/fleet/status" {
            return json_response(crate::agent_conversations::overview()?);
        }
        if path == "/api/bootstrap" {
            let mut result = app.execute(
                None,
                Operation::Projects {
                    include_hidden: true,
                },
                None,
            )?;
            result["backend_host"] = match &app.backend {
                Backend::Local(_) => Value::Null,
                Backend::Remote(host) => json!(host),
            };
            result["csrf"] = json!(app.token);
            result["actor"] = json!({"id":app.actor.id,"kind":app.actor.kind});
            return json_response(result);
        }
    }
    if request.method() == &Method::Post
        && [
            "/api/action",
            "/api/preview",
            "/api/inbox",
            "/api/fleet",
            "/api/mm",
        ]
        .contains(&path.as_str())
    {
        if header(request, "x-hey-boss-csrf") != Some(app.token.as_str()) {
            return Err(Error::new(
                "forbidden",
                "Reload the page to reconnect to this server",
            ));
        }
        if !header(request, "content-type").is_some_and(|s| {
            s.split(';')
                .next()
                .is_some_and(|s| s.trim() == "application/json")
        }) {
            return Err(Error::invalid("Expected application/json"));
        }
        let mut bytes = Vec::new();
        request
            .as_reader()
            .take(super::WIRE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > super::WIRE_LIMIT {
            return Err(Error::invalid("Request exceeds 16 MiB"));
        }
        if path == "/api/fleet" {
            let value: Value = serde_json::from_slice(&bytes)?;
            if value["kind"] != "signal" {
                return Err(Error::invalid("Expected a worker signal"));
            }
            return json_response(crate::fleet::call(&value)?);
        }
        if path == "/api/preview" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Preview {
                body: String,
            }
            let input: Preview = serde_json::from_slice(&bytes)?;
            if input.body.len() > super::BODY_LIMIT {
                return Err(Error::invalid("Markdown exceeds 1 MiB"));
            }
            return json_response(
                json!({"ok":true,"html":crate::markdown::render_fragment(&input.body)}),
            );
        }
        if path == "/api/inbox" {
            let mut action: crate::notices::Action = serde_json::from_slice(&bytes)?;
            if let crate::notices::Action::Link {
                issue: Some(reference),
                ..
            } = &mut action
            {
                reference.validate()?;
                let issue = app.execute_at(
                    Some(reference.project.clone()),
                    Operation::View {
                        number: reference.number,
                    },
                    None,
                    reference.host.as_deref(),
                )?;
                reference.project = issue["project"]["id"].as_str().unwrap().into();
            }
            return json_response(crate::notices::execute(&action)?);
        }
        let action: Action = serde_json::from_slice(&bytes)?;
        if matches!(
            action.operation,
            Operation::ReadPlan { .. } | Operation::BindPlan { .. }
        ) {
            return Err(Error::new(
                "forbidden",
                "Plan files are bound and read through the terminal workflow",
            ));
        }
        if let Operation::Mindmap { operation } = &action.operation {
            if operation.writes() {
                return Err(Error::new(
                    "forbidden",
                    "Mindmaps are read-only on the web; author with hey-boss mm",
                ));
            }
        } else if path == "/api/mm" {
            return Err(Error::new(
                "forbidden",
                "The mindmap viewer only accepts mindmap reads",
            ));
        }

        let mut result = app.execute_at(
            Some(action.project),
            action.operation,
            action.request_id,
            action.host.as_deref(),
        )?;
        if crate::mindmap::needs_inbox(&result) {
            crate::mindmap::enrich_notifications(
                &mut result,
                crate::notices::execute(&crate::notices::Action::List),
            )?;
        }
        if let Some(issue) = result.get_mut("issue")
            && let Some(body) = issue["body"].as_str()
        {
            issue["body_html"] = json!(crate::markdown::render_fragment(body));
        }
        if let Some(comments) = result["comments"].as_array_mut() {
            for comment in comments {
                if let Some(body) = comment["body"].as_str() {
                    comment["body_html"] = json!(crate::markdown::render_fragment(body));
                }
            }
        }
        json_response(result)
    } else {
        Err(Error::new("not_found", "Route not found"))
    }
}
fn json_response(value: Value) -> Result<(u16, &'static str, Vec<u8>)> {
    Ok((
        200,
        "application/json; charset=utf-8",
        serde_json::to_vec(&value)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::validate_mobile_origin;

    #[test]
    fn mobile_origins_are_private_https_origins_without_url_components() {
        for origin in [
            "https://mac.example.ts.net",
            "https://dev-box.example.ts.net:8443",
        ] {
            assert_eq!(validate_mobile_origin(origin.into()).unwrap(), origin);
        }
        for origin in [
            "http://mac.example.ts.net",
            "https://example.com",
            "https://ts.net",
            "https://example.ts.net",
            "https://mac.example.ts.net/",
            "https://mac.example.ts.net/issues",
            "https://mac.example.ts.net?x=1",
            "https://mac.example.ts.net#fragment",
            "https://user@mac.example.ts.net",
            "https://mac.example.ts.net.evil.com",
            "https://mac..ts.net",
            "https://-mac.example.ts.net",
            "https://mac-.example.ts.net",
            "https://MAC.example.ts.net",
            "https://mac.example.ts.net:0",
            "https://mac.example.ts.net:65536",
            "https://mac.example.ts.net:+8443",
            "https://mac.example.ts.net:08443",
            "https://mac.example.ts.net:443",
            "https://mac.example.ts.net:8443:1",
            "https://mac.example.ts.net\r\n",
        ] {
            assert!(validate_mobile_origin(origin.into()).is_err(), "{origin:?}");
        }
    }
}
