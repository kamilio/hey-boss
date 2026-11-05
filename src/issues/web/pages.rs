use super::*;
use crate::pages;
use std::path::PathBuf;

fn database(app: &App) -> Result<PathBuf> {
    // Published files are always served by this machine, even while browsing a remote project.
    match &app.backend {
        Backend::Local(path) => Ok(path.clone()),
        Backend::Remote(_) => super::super::database_path(),
    }
}
fn content_key(app: &App, name: &str, revision: &str) -> String {
    hey_boss_sync::digest(format!("{}:{name}:{revision}", app.token).as_bytes())
}
pub(super) fn api(app: &App, url: &str) -> Result<Value> {
    let query = parse_query(url)?;
    let database = database(app)?;
    if let Some(name) = query.get("name") {
        let page = pages::view(&database, name)?;
        let content_url = (!page.deployment.is_empty()).then(|| {
            format!(
                "/page-content/{}/{}/{}/index.html",
                content_key(app, name, &page.deployment),
                name,
                page.deployment
            )
        });
        return Ok(json!({"ok":true,"page":page,"content_url":content_url}));
    }
    let offset = query
        .get("offset")
        .map(|s| {
            s.parse::<usize>()
                .map_err(|_| Error::invalid("Invalid page offset"))
        })
        .transpose()?
        .unwrap_or(0);
    let entries = pages::list(&database, offset)?;
    Ok(json!({"ok":true,"pages":entries,"next_offset":(entries.len()==100).then_some(offset+100)}))
}
pub(super) fn delete(app: &App, bytes: &[u8]) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Delete {
        name: String,
    }
    let request: Delete = serde_json::from_slice(bytes)?;
    pages::delete(&database(app)?, &request.name)?;
    Ok(json!({"ok":true}))
}

pub(super) fn providers() -> Result<Value> {
    let providers: Vec<_> = pages::publishing::providers()?
        .into_iter()
        .map(|(name, p)| json!({"name":name,"audience":p.audience}))
        .collect();
    Ok(json!({"ok":true,"providers":providers}))
}

pub(super) fn share(app: &App, bytes: &[u8]) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Share {
        name: String,
        provider: String,
        link: Option<String>,
        auto_sync: bool,
    }
    let request: Share = serde_json::from_slice(bytes)?;
    let page = pages::publishing::share(
        &database(app)?,
        &request.name,
        &request.provider,
        request.link.as_deref(),
        request.auto_sync,
    )?;
    Ok(json!({"ok":true,"page":page}))
}

pub(super) fn sync(app: &App, bytes: &[u8]) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Sync {
        name: String,
    }
    let request: Sync = serde_json::from_slice(bytes)?;
    Ok(json!({"ok":true,"page":pages::publishing::sync(&database(app)?, &request.name)?}))
}

fn file(app: &App, url: &str) -> Result<pages::Content> {
    let path = url.split('?').next().unwrap_or(url);
    let mut parts = path
        .strip_prefix("/page-content/")
        .unwrap_or("")
        .splitn(4, '/');
    let key = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or("");
    let revision = parts.next().unwrap_or("");
    let raw = parts.next().unwrap_or("");
    // Opaque sandbox requests cannot prove their origin. A per-page capability,
    // issued only by the same-origin API, prevents other websites reading files.
    if key != content_key(app, name, revision) {
        return Err(Error::new(
            "forbidden",
            "Reload the private page to renew content access",
        ));
    }
    // URL decoding must not turn '+' in a filename into a space.
    let decoded = parse_query(&format!(
        "?path={}",
        raw.replace('+', "%2B").replace('&', "%26")
    ))?;
    let path = decoded.get("path").unwrap();
    let path = if path.is_empty() || path.ends_with('/') {
        format!("{path}index.html")
    } else {
        path.clone()
    };
    let database = database(app)?;
    // Check the current name before cache validation, including pinned old deployments.
    let db = super::super::Store::open(&database)?.into_database();
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM published_pages p WHERE name=?1 AND deleted=0 AND (deployment=?2 OR EXISTS(SELECT 1 FROM published_owners o WHERE o.name=p.name AND o.deployment=?2 AND NOT EXISTS(SELECT 1 FROM published_owners birth JOIN published_deletions d ON d.name=birth.name WHERE birth.generation=o.generation AND (birth.clock,birth.origin)<=(d.clock,d.origin)))))",
        [name, revision],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(Error::new("not_found", "Published page not found"));
    }
    pages::content(&database, revision, &path)
}
pub(super) fn respond(request: tiny_http::Request, app: &App) {
    let result = (|| {
        if !allowed(&request, app, true) {
            return Err(Error::new(
                "forbidden",
                "Private published content is unavailable from this address",
            ));
        }
        if !matches!(request.method(), Method::Get | Method::Head) {
            return Err(Error::invalid("Published content is read-only"));
        }
        file(app, request.url())
    })();
    match result {
        Ok(content) => {
            let etag = format!("\"{}\"", content.etag);
            let cached = header(&request, "if-none-match") == Some(etag.as_str());
            let mut response = Response::new(
                StatusCode(if cached { 304 } else { 200 }),
                Vec::new(),
                content.reader,
                Some(if cached { 0 } else { content.size as usize }),
                None,
            );
            for (name, value) in [
                ("Content-Type", content.kind),
                ("Cache-Control", "private, no-cache"),
                ("ETag", etag.as_str()),
                ("Access-Control-Allow-Origin", "*"),
                ("X-Content-Type-Options", "nosniff"),
                ("Referrer-Policy", "no-referrer"),
                (
                    "Content-Security-Policy",
                    "sandbox allow-scripts allow-forms allow-downloads allow-popups allow-popups-to-escape-sandbox; default-src 'none'; script-src http: https: 'unsafe-inline' 'unsafe-eval' blob:; style-src http: https: 'unsafe-inline'; img-src http: https: data: blob:; font-src http: https: data:; media-src http: https: data: blob:; connect-src http: https:; frame-src https:; base-uri 'self'; frame-ancestors 'self'; form-action http: https:",
                ),
            ] {
                response.add_header(Header::from_bytes(name, value).unwrap());
            }
            let _ = request.respond(response);
        }
        Err(error) => {
            let status = match error.code.as_str() {
                "not_found" => 404,
                "forbidden" => 403,
                "invalid_input" => 400,
                _ => 503,
            };
            let mut response =
                Response::from_string(error.message).with_status_code(StatusCode(status));
            for (name, value) in [
                ("Content-Type", "text/plain; charset=utf-8"),
                ("Cache-Control", "no-store"),
                ("Content-Security-Policy", "default-src 'none'; sandbox"),
                ("X-Content-Type-Options", "nosniff"),
            ] {
                response.add_header(Header::from_bytes(name, value).unwrap());
            }
            let _ = request.respond(response);
        }
    }
}
