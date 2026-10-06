//! gh's documented http_unix_socket transport carries plain HTTP over a private
//! socket. No TLS interception, global config edits, or persistent service.
use super::{rewrite_graphql, rewrite_search};
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
    routing::any,
};
use serde_json::Value;
use std::{
    path::PathBuf,
    process::{Command, ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::sync::OnceCell;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn run(mut command: Command, host: String, token: String) -> Result<ExitStatus> {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::Builder::new()
        .prefix("hey-gh-")
        .tempdir_in("/tmp")?;
    let socket = directory.path().join("http.sock");
    let config = directory.path().join("config");
    std::fs::create_dir(&config)?;
    let original = config_directory()?;
    let mut settings: serde_yaml::Value = match std::fs::read(original.join("config.yml")) {
        Ok(data) => serde_yaml::from_slice(&data)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            serde_yaml::Value::Mapping(Default::default())
        }
        Err(e) => return Err(e.into()),
    };
    if settings.is_null() {
        settings = serde_yaml::Value::Mapping(Default::default());
    }
    settings
        .as_mapping_mut()
        .ok_or("invalid gh config.yml")?
        .insert(
            serde_yaml::Value::String("http_unix_socket".into()),
            serde_yaml::Value::String(socket.to_str().ok_or("invalid socket path")?.into()),
        );
    std::fs::write(config.join("config.yml"), serde_yaml::to_string(&settings)?)?;
    match std::fs::read_dir(&original) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if entry.file_name() != "config.yml" {
                    symlink(entry.path(), config.join(entry.file_name()))?;
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    command
        .env("GH_CONFIG_DIR", &config)
        // gh caches requests before the transport resolves @me. Do not reuse
        // another personal account's results under the same installation token.
        .env("GH_CACHE_DIR", directory.path().join("cache"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()?;
    runtime.block_on(async {
        let listener = tokio::net::UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let state = Arc::new(Transport::new(host, token)?);
        let app = Router::new().fallback(any(relay)).with_state(state);
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let result = wait_for_command(command).await;
        server.abort();
        let _ = server.await;
        result
    })
}

fn config_directory() -> Result<PathBuf> {
    let nonempty = |key| std::env::var_os(key).filter(|v| !v.is_empty());
    let path = if let Some(path) = nonempty("GH_CONFIG_DIR") {
        PathBuf::from(path)
    } else if let Some(path) = nonempty("XDG_CONFIG_HOME") {
        PathBuf::from(path).join("gh")
    } else {
        PathBuf::from(nonempty("HOME").ok_or("HOME is unset")?).join(".config/gh")
    };
    Ok(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    })
}

async fn wait_for_command(command: Command) -> Result<ExitStatus> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut hangup = signal(SignalKind::hangup())?;
    let mut child = tokio::process::Command::from(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let pid = child.id().ok_or("gh did not start")?;
    loop {
        let number = tokio::select! {
            status = child.wait() => return Ok(status?),
            _ = interrupt.recv() => libc::SIGINT,
            _ = terminate.recv() => libc::SIGTERM,
            _ = hangup.recv() => libc::SIGHUP,
        };
        unsafe {
            libc::kill(pid as i32, number);
        }
    }
}

struct Transport {
    host: String,
    app_token: String,
    client: reqwest::Client,
    login: OnceCell<String>,
    user_token: OnceCell<String>,
}

impl Transport {
    fn new(host: String, app_token: String) -> Result<Self> {
        Ok(Self {
            host,
            app_token,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            login: OnceCell::new(),
            user_token: OnceCell::new(),
        })
    }
    fn trusted(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(&self.host)
            || ((self.host == "github.com" || self.host.ends_with(".ghe.com"))
                && [
                    format!("api.{}", self.host),
                    format!("uploads.{}", self.host),
                ]
                .iter()
                .any(|allowed| host.eq_ignore_ascii_case(allowed)))
    }
    async fn gh(&self, args: &[&str]) -> std::result::Result<String, String> {
        // The parent keeps the original gh configuration/environment. Only the
        // delegated gh process receives App credentials and the private socket.
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new("gh")
                .args(args)
                .args(["--hostname", &self.host])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| "Personal GitHub identity lookup timed out".to_owned())?
        .map_err(|_| "Cannot run gh to resolve the personal GitHub identity".to_owned())?;
        if !output.status.success() {
            return Err(format!(
                "Cannot resolve the personal GitHub identity for {}; authenticate gh for that host. App authentication is not a personal identity.",
                self.host
            ));
        }
        String::from_utf8(output.stdout)
            .map(|s| s.trim().to_owned())
            .map_err(|_| "Invalid personal GitHub identity response".into())
    }
    async fn login(&self) -> std::result::Result<&str, String> {
        self.login
            .get_or_try_init(|| async {
                let login = self.gh(&["api", "user", "--jq", ".login"]).await?;
                if login.is_empty()
                    || login.len() > 64
                    || !login
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                {
                    return Err("Personal GitHub login is missing or invalid".into());
                }
                Ok(login)
            })
            .await
            .map(String::as_str)
    }
    async fn user_token(&self) -> std::result::Result<&str, String> {
        self.user_token
            .get_or_try_init(|| async {
                let token = self.gh(&["auth", "token"]).await?;
                if !hey_gh::app_auth::valid_cli_token(&token) {
                    return Err("Personal GitHub credentials are invalid".into());
                }
                Ok(token)
            })
            .await
            .map(String::as_str)
    }
}

async fn relay(State(state): State<Arc<Transport>>, request: Request<Body>) -> Response<Body> {
    let result = async {
        let host = request
            .headers()
            .get("host")
            .and_then(|h| h.to_str().ok())
            .ok_or("Missing request host")?;
        let url = url::Url::parse(&format!(
            "https://{host}{}",
            request
                .uri()
                .path_and_query()
                .map(|v| v.as_str())
                .unwrap_or("/")
        ))
        .map_err(|_| "Invalid request URL")?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err("Invalid request host".into());
        }
        let trusted = state.trusted(host);
        forward(&state, request, url, trusted).await
    }
    .await;
    match result {
        Ok(response) => response,
        Err(message) => Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"message":message}).to_string(),
            ))
            .unwrap(),
    }
}

fn personal_read(method: &reqwest::Method, path: &str) -> bool {
    matches!(*method, reqwest::Method::GET | reqwest::Method::HEAD)
        && (path == "/user"
            || path.starts_with("/user/")
            || matches!(path, "/issues" | "/notifications" | "/gists")
            || path.ends_with("/notifications"))
}

async fn forward(
    state: &Transport,
    request: Request<Body>,
    mut url: url::Url,
    trusted: bool,
) -> std::result::Result<Response<Body>, String> {
    let (parts, body) = request.into_parts();
    let path = url
        .path()
        .strip_prefix("/api/v3")
        .unwrap_or(url.path())
        .to_owned();
    let graphql = trusted && matches!(path.as_str(), "/graphql" | "/api/graphql");
    let personal = trusted && personal_read(&parts.method, &path);
    if trusted && !graphql {
        let mut pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        for (name, value) in &mut pairs {
            if path.starts_with("/search/")
                && name == "q"
                && rewrite_search(value, "__heygh_user__") != *value
            {
                *value = rewrite_search(value, state.login().await?);
            } else if matches!(
                name.as_str(),
                "assignee" | "creator" | "mentioned" | "author" | "actor"
            ) && value == "@me"
            {
                *value = state.login().await?.into();
            }
        }
        if url.query().is_some() {
            url.query_pairs_mut().clear().extend_pairs(pairs);
        }
    }
    let body = if graphql && parts.method == reqwest::Method::GET {
        let mut pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let mut input = serde_json::Map::new();
        for (name, value) in &pairs {
            if name == "variables" {
                if let Ok(value) = serde_json::from_str::<Value>(value) {
                    input.insert(name.clone(), value);
                }
            } else if matches!(name.as_str(), "query" | "operationName") {
                input.insert(name.clone(), Value::String(value.clone()));
            }
        }
        let mut input = Value::Object(input);
        let mut probe = input.clone();
        if rewrite_graphql(&mut probe, "__heygh_user__")? {
            rewrite_graphql(&mut input, state.login().await?)?;
            for (name, value) in &mut pairs {
                if matches!(name.as_str(), "query" | "operationName") {
                    *value = input[name.as_str()].as_str().unwrap_or(value).to_owned();
                }
                if name == "variables" {
                    *value = input[name.as_str()].to_string();
                }
            }
            url.query_pairs_mut().clear().extend_pairs(pairs);
        }
        reqwest::Body::wrap_stream(body.into_data_stream())
    } else if graphql && parts.method == reqwest::Method::POST {
        let bytes = axum::body::to_bytes(body, 64 * 1024 * 1024)
            .await
            .map_err(|_| "GraphQL request is too large".to_owned())?;
        if let Ok(mut input) = serde_json::from_slice::<Value>(&bytes) {
            let mut probe = input.clone();
            if rewrite_graphql(&mut probe, "__heygh_user__")? {
                rewrite_graphql(&mut input, state.login().await?)?;
                reqwest::Body::from(
                    serde_json::to_vec(&input).map_err(|_| "Invalid GraphQL request")?,
                )
            } else {
                reqwest::Body::from(bytes)
            }
        } else {
            reqwest::Body::from(bytes)
        }
    } else {
        reqwest::Body::wrap_stream(body.into_data_stream())
    };
    let mut headers = parts.headers;
    for name in [
        "host",
        "authorization",
        "proxy-authorization",
        "connection",
        "transfer-encoding",
        "content-length",
    ] {
        headers.remove(name);
    }
    let mut outgoing = state
        .client
        .request(parts.method, url)
        .headers(headers)
        .body(body);
    if trusted {
        outgoing = outgoing.bearer_auth(if personal {
            state.user_token().await?
        } else {
            &state.app_token
        });
    }
    let response = outgoing
        .send()
        .await
        .map_err(|_| "GitHub request transport failed".to_owned())?;
    let mut result = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        if !matches!(name.as_str(), "connection" | "transfer-encoding") {
            result = result.header(name, value);
        }
    }
    result
        .body(Body::from_stream(response.bytes_stream()))
        .map_err(|_| "Invalid GitHub response".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    #[tokio::test]
    async fn request_routing_keeps_filters_personal_and_mutations_app_authenticated() {
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = calls.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().fallback(any(move |request: Request<Body>| {
            let captured = captured.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
                captured.lock().unwrap().push(json!({"uri":parts.uri.to_string(),"method":parts.method.as_str(),"auth":parts.headers.get("authorization").and_then(|h| h.to_str().ok()),"body":serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)}));
                Response::builder().header("etag", "fixture").body(Body::from("{\"ok\":true}")).unwrap()
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = Transport::new("github.com".into(), "synthetic-app".into()).unwrap();
        state.login.set("octocat".into()).unwrap();
        state.user_token.set("synthetic-personal".into()).unwrap();
        for (method, path, body, trusted) in [
            (
                "POST",
                "/graphql",
                json!({"query":"query Status($q:String!) { viewer { login } search(query:$q,type:ISSUE,first:10) { issueCount } }", "variables":{"q":"author:@me review-requested:@me"}}),
                true,
            ),
            (
                "GET",
                "/search/issues?q=assignee%3A%40me+mentions%3A%40me",
                Value::Null,
                true,
            ),
            (
                "GET",
                "/repos/acme/demo/issues?creator=%40me&assignee=%40me",
                Value::Null,
                true,
            ),
            ("GET", "/user", Value::Null, true),
            ("GET", "/notifications", Value::Null, true),
            ("POST", "/user/keys", json!({"title":"@me"}), true),
            (
                "POST",
                "/graphql",
                json!({"query":"mutation { addComment(input:{subjectId:\"id\",body:\"author:@me\"}) { clientMutationId } }"}),
                true,
            ),
            ("GET", "/download", Value::Null, false),
        ] {
            let request = Request::builder()
                .method(method)
                .header("authorization", "Bearer do-not-forward")
                .body(Body::from(if body.is_null() {
                    String::new()
                } else {
                    body.to_string()
                }))
                .unwrap();
            let response = forward(
                &state,
                request,
                format!("{origin}{path}").parse().unwrap(),
                trusted,
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["etag"], "fixture");
            assert_eq!(
                axum::body::to_bytes(response.into_body(), 100)
                    .await
                    .unwrap(),
                "{\"ok\":true}"
            );
        }
        let mut get = url::Url::parse(&format!("{origin}/api/graphql")).unwrap();
        get.query_pairs_mut()
            .append_pair("query", "query Mine($q:String!){viewer{login} search(query:$q,type:ISSUE,first:1){issueCount}}")
            .append_pair("variables", "{\"q\":\"reviewed-by:@me\"}");
        forward(&state, Request::new(Body::empty()), get, true)
            .await
            .unwrap();
        let explicit = Transport::new("github.com".into(), "synthetic-app".into()).unwrap();
        forward(
            &explicit,
            Request::new(Body::empty()),
            format!("{origin}/search/issues?q=author%3Asomeone")
                .parse()
                .unwrap(),
            true,
        )
        .await
        .unwrap();
        assert!(!explicit.login.initialized());
        assert!(!explicit.user_token.initialized());
        let calls = calls.lock().unwrap();
        assert_eq!(calls[0]["auth"], "Bearer synthetic-app");
        assert!(
            calls[0]["body"]["query"]
                .as_str()
                .unwrap()
                .contains("viewer: user(login: \"octocat\")")
        );
        assert_eq!(
            calls[0]["body"]["variables"]["q"],
            "author:octocat review-requested:octocat"
        );
        assert_eq!(
            calls[1]["uri"],
            "/search/issues?q=assignee%3Aoctocat+mentions%3Aoctocat"
        );
        assert_eq!(
            calls[2]["uri"],
            "/repos/acme/demo/issues?creator=octocat&assignee=octocat"
        );
        for index in [1, 2, 5, 6] {
            assert_eq!(calls[index]["auth"], "Bearer synthetic-app");
        }
        for index in [3, 4] {
            assert_eq!(calls[index]["auth"], "Bearer synthetic-personal");
        }
        assert!(
            calls[6]["body"]["query"]
                .as_str()
                .unwrap()
                .contains("body:\"author:@me\"")
        );
        assert!(calls[7]["auth"].is_null());
        let get =
            url::Url::parse(&format!("{origin}{}", calls[8]["uri"].as_str().unwrap())).unwrap();
        let pairs: std::collections::HashMap<_, _> = get.query_pairs().collect();
        assert!(pairs["query"].contains("viewer: user(login: \"octocat\")"));
        assert_eq!(
            serde_json::from_str::<Value>(&pairs["variables"]).unwrap()["q"],
            "reviewed-by:octocat"
        );
        for index in [8, 9] {
            assert_eq!(calls[index]["auth"], "Bearer synthetic-app");
        }
        server.abort();
    }

    #[test]
    fn auth_scope_is_host_exact_and_personal_fallback_is_read_only() {
        let state = Transport::new("github.com".into(), "synthetic-app".into()).unwrap();
        for host in ["github.com", "api.github.com", "uploads.github.com"] {
            assert!(state.trusted(host));
        }
        for host in [
            "github.com.evil.test",
            "evil.github.com",
            "api.github.com@evil.test",
        ] {
            assert!(!state.trusted(host));
        }
        assert!(personal_read(&reqwest::Method::GET, "/user/repos"));
        for method in [
            reqwest::Method::POST,
            reqwest::Method::PATCH,
            reqwest::Method::DELETE,
        ] {
            assert!(!personal_read(&method, "/user/repos"));
        }
        let enterprise = Transport::new("ghe.example".into(), "synthetic-app".into()).unwrap();
        assert!(enterprise.trusted("ghe.example"));
        assert!(!enterprise.trusted("api.github.com"));
    }
}
