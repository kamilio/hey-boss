use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
};
use tiny_http::{Header, Method, Request, Response, Server, SslConfig};

const ASSETS: &[(&str, &str, &[u8])] = &[
    (
        "/",
        "text/html; charset=utf-8",
        include_bytes!("conversation_web/index.html"),
    ),
    (
        "/app.js",
        "text/javascript",
        include_bytes!("conversation_web/app.js"),
    ),
    (
        "/sw.js",
        "text/javascript",
        include_bytes!("conversation_web/sw.js"),
    ),
    (
        "/mic.js",
        "text/javascript",
        include_bytes!("conversation_web/mic.js"),
    ),
    (
        "/components.css",
        "text/css",
        include_bytes!("issues/web/components.css"),
    ),
    (
        "/brand.png",
        "image/png",
        include_bytes!("issues/web/icon.png"),
    ),
    (
        "/app.css",
        "text/css",
        include_bytes!("conversation_web/app.css"),
    ),
    (
        "/manifest.webmanifest",
        "application/manifest+json",
        include_bytes!("conversation_web/manifest.webmanifest"),
    ),
    (
        "/icon.png",
        "image/png",
        include_bytes!("conversation_web/icon.png"),
    ),
];
fn header(key: &str, value: &str) -> Header {
    Header::from_bytes(key, value).unwrap()
}
fn reply(
    req: Request,
    code: u16,
    mime: &str,
    data: Vec<u8>,
    cookie: Option<&str>,
) -> io::Result<()> {
    let mut response = Response::from_data(data)
        .with_status_code(code)
        .with_header(header("Content-Type", mime))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header(
            "Content-Security-Policy",
            "default-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ));
    if let Some(key) = cookie {
        response.add_header(header("Set-Cookie", &format!("__Host-conversations={key}; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000")));
    }
    req.respond(response)
}
fn handle(mut req: Request, host: &str, key: &str, socket: &Path) -> io::Result<()> {
    let field = |name: &'static str| {
        req.headers()
            .iter()
            .find(|h| h.field.equiv(name))
            .map(|h| h.value.as_str())
            .unwrap_or("")
    };
    let local = req.remote_addr().is_some_and(|addr| match addr.ip() {
        std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        std::net::IpAddr::V6(_) => false,
    });
    if !local || !field("Host").eq_ignore_ascii_case(host) {
        return reply(req, 403, "text/plain", vec![], None);
    }
    if req.method() == &Method::Get {
        if let Some((_, mime, data)) = ASSETS.iter().find(|(path, _, _)| *path == req.url()) {
            return reply(req, 200, mime, data.to_vec(), None);
        }
    }
    if req.method() != &Method::Post || !matches!(req.url(), "/pair" | "/api") {
        return reply(req, 404, "text/plain", vec![], None);
    }
    let pair = req.url() == "/pair";
    if !field("Origin").eq_ignore_ascii_case(&format!("https://{host}"))
        || (!pair
            && !field("Cookie")
                .split(';')
                .any(|s| s.trim().strip_prefix("__Host-conversations=") == Some(key)))
    {
        return reply(req, 403, "text/plain", vec![], None);
    }
    if req.body_length().is_none_or(|n| n > 65536) {
        return reply(req, 413, "text/plain", vec![], None);
    }
    let mut body = Vec::new();
    req.as_reader().take(65537).read_to_end(&mut body)?;
    if pair {
        let ok = body == key.as_bytes();
        return reply(
            req,
            if ok { 200 } else { 403 },
            "text/plain",
            vec![],
            ok.then_some(key),
        );
    }
    let Ok(p) = serde_json::from_slice::<Value>(&body) else {
        return reply(req, 400, "text/plain", vec![], None);
    };
    // Never forward a browser-supplied daemon command.
    let payload = match p["action"].as_str() {
        Some("list") => json!({"command":"chat_list","sync":false,"chat_id":p["after"]}),
        Some(_) => json!({"command":"chat_phone","sync":false,"phone":p}),
        None => return reply(req, 400, "text/plain", vec![], None),
    };
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    serde_json::to_writer(&mut stream, &payload)?;
    stream.shutdown(Shutdown::Write)?;
    let mut data = Vec::new();
    stream.read_to_end(&mut data)?;
    if p["action"] == "state" {
        let mut result: Value = serde_json::from_slice(&data)?;
        if let Some(messages) = result["messages"].as_array_mut() {
            for message in messages {
                if message["speaker"] != "You"
                    && let Some(text) = message["text"].as_str()
                {
                    message["html"] = hey_boss::markdown::render_fragment(text).into();
                    message.as_object_mut().unwrap().remove("text");
                }
            }
            data = serde_json::to_vec(&result)?;
        }
    }
    reply(req, 200, "application/json", data, None)
}
pub(super) fn run(install: bool, pairing_code: bool) -> io::Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    if !cfg!(target_os = "macos") || exe.with_file_name("hey-boss.companion").exists() {
        return Err(io::Error::other(
            "Run this on the Mac with Hey Boss Conversations",
        ));
    }
    if install {
        let status = std::process::Command::new("/bin/bash")
            .args([
                "-ec",
                include_str!("conversation_web/install.sh"),
                "conversation-web",
            ])
            .arg(exe)
            .status()?;
        return if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("Conversation web setup failed"))
        };
    }
    let state = fs::read_to_string(exe.with_file_name("hey-boss.state"))?;
    let state = Path::new(state.trim());
    let web = state.join("conversation-web");
    let key = fs::read_to_string(web.join("pairing-code")).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            io::Error::other("Run hey-boss agent chats web --install on the Mac first.")
        } else {
            e
        }
    })?;
    let key = key.trim();
    if key.len() != 32 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::other("Invalid pairing code"));
    }
    if pairing_code {
        println!("{key}");
        return Ok(());
    }
    let host = fs::read_to_string(web.join("host"))?;
    let server = Server::https(
        ("0.0.0.0", 4785),
        SslConfig {
            certificate: fs::read(web.join("server.pem"))?,
            private_key: fs::read(web.join("server.key"))?,
        },
    )
    .map_err(io::Error::other)?;
    let socket = state.join("daemon.sock");
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for req in server.incoming_requests() {
                    if let Err(e) = handle(req, host.trim(), key, &socket) {
                        eprintln!("Conversation web: {e}");
                    }
                }
            });
        }
    });
    Ok(())
}
