use super::*;

fn config() -> Config {
    serde_json::from_value(json!({
        "listen":"127.0.0.1:8080",
        "providers":{
            "openai":{"upstream_url":"http://127.0.0.1:9","api_keys":{"default":"PRIVATE_OPENAI_SOURCE"}},
            "gemini":{"api_key":"PRIVATE_GEMINI_SOURCE"}
        },
        "aliases":[
            {"from":"coding","to":"response-target","api_shape":"responses"},
            {"from":"coding","to":"gemini/chat-target","api_shape":"chat_completions"},
            {"from":"legacy-only","to":"legacy-target","api_shape":"completions"},
            {"from":"native-alias","to":"gemini/models/native-target"},
            {"from":"adaptive","to":"openai/primary","reasoning_routes":{"high":{"to":"gemini/deep"},"low":{"to":"fast"}}},
            {"from":"models/native-target","to":"openai-only"}
        ],
        "fallbacks":{"response-target":["backup","gemini/fallback"]}
    })).unwrap()
}
fn api<'a>(catalog: &'a Value, id: &str) -> &'a Value {
    catalog["apis"]
        .as_array()
        .unwrap()
        .iter()
        .find(|api| api["id"] == id)
        .unwrap()
}
fn model<'a>(api: &'a Value, id: &str) -> Option<&'a Value> {
    api["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["id"] == id)
}

#[test]
fn overview_catalog_respects_shape_provider_and_reasoning_routes() {
    let catalog = catalog(&config());
    let responses = api(&catalog, "responses");
    let custom = api(&catalog, "custom-chat");
    let chat = api(&catalog, "chat");
    assert_eq!(
        model(responses, "coding").unwrap()["routes"][0]["target"],
        "response-target"
    );
    assert_eq!(
        model(custom, "coding").unwrap()["routes"][0]["target"],
        "gemini/chat-target"
    );
    assert!(model(chat, "coding").is_none());
    assert!(model(chat, "gemini/chat-target").is_none());
    assert!(model(responses, "legacy-only").is_none());
    assert!(model(api(&catalog, "completions"), "legacy-only").is_some());
    assert_eq!(
        model(custom, "adaptive").unwrap()["routes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let routes = model(chat, "adaptive").unwrap()["routes"]
        .as_array()
        .unwrap();
    assert_eq!(routes.len(), 2);
    assert_eq!(routes[1]["when"], "Reasoning: low");
    assert!(model(responses, "backup").is_some());
    assert!(model(custom, "gemini/fallback").is_some());
    assert!(model(responses, "fast").is_some());
    assert!(model(responses, "invented-model").is_none());
    let json = catalog.to_string();
    for private in [
        "PRIVATE_OPENAI_SOURCE",
        "PRIVATE_GEMINI_SOURCE",
        "127.0.0.1:9",
        "api_key",
        "upstream_url",
    ] {
        assert!(!json.contains(private), "Catalog exposed {private}");
    }
}

#[test]
fn overview_native_names_match_url_routing_and_cross_provider_rejections() {
    let catalog = catalog(&config());
    let native = api(&catalog, "gemini");
    assert_eq!(
        model(native, "native-alias").unwrap()["routes"][0]["target"],
        "native-target"
    );
    assert!(model(native, "chat-target").is_some());
    assert!(model(native, "coding").is_none()); // Chat-scoped aliases do not apply to native URLs.
    assert!(model(native, "native-target").is_none()); // models/native-target rewrites to OpenAI.
    assert!(model(native, "gemini/chat-target").is_none());
    assert!(model(native, "deep").is_some());
    assert!(model(native, "fallback").is_some());
}

#[test]
fn overview_empty_gemini_only_and_relay_configs_do_not_invent_models() {
    let empty = catalog(&Config::default());
    for api in empty["apis"].as_array().unwrap() {
        assert_eq!(api["configured"], false);
        assert_eq!(api["models"], json!([]));
    }
    let mut gemini = config();
    gemini.api_keys.clear();
    let data = catalog(&gemini);
    assert_eq!(api(&data, "chat")["models"], json!([]));
    assert!(model(api(&data, "responses"), "gemini/deep").is_some());
    let relay: Config = serde_json::from_value(json!({"mode":"client","listen":"127.0.0.1:8080","connection":{"url":"http://host.example:8080","api_key":"PRIVATE_HOST_KEY"}})).unwrap();
    let data = catalog(&relay.effective());
    assert_eq!(data["relay"], true);
    assert!(!data.to_string().contains("PRIVATE_HOST_KEY"));
    for api in data["apis"].as_array().unwrap() {
        assert_eq!(api["configured"], false);
        assert_eq!(api["models"], json!([]));
    }
}

async fn serve(config: Config, options: Options) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = router_with(config, options).unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, task)
}

#[tokio::test]
async fn overview_http_reads_local_config_and_refreshes_without_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let initial = config();
    std::fs::write(&path, serde_json::to_vec(&initial).unwrap()).unwrap();
    let (url, task) = serve(
        initial,
        Options {
            source: Some((path.clone(), config::fingerprint(&path))),
            ..Default::default()
        },
    )
    .await;
    let page = reqwest::get(format!("{url}/apis")).await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.text().await.unwrap().contains("/overview.js"));
    let response = reqwest::get(format!("{url}/overview/api")).await.unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let data: Value = response.json().await.unwrap();
    assert!(model(api(&data, "responses"), "coding").is_some());
    let mut updated = config();
    updated.aliases[0].from = "renamed-config-alias".into();
    std::fs::write(&path, serde_json::to_vec(&updated).unwrap()).unwrap();
    let data = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let data: Value = reqwest::get(format!("{url}/overview/api"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if model(api(&data, "responses"), "renamed-config-alias").is_some() {
                break data;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("background catalog refresh");
    assert!(model(api(&data, "responses"), "coding").is_none());
    // The synthetic upstream is unreachable, so successful catalog responses
    // also prove this page does not depend on /v1/models or provider discovery.
    task.abort();
}

#[tokio::test]
async fn overview_host_login_guards_metadata_and_returns_to_landing_page() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("host.json");
    let keys = crate::access::ensure(&path, &[]).unwrap();
    let (url, task) = serve(
        Config {
            mode: Mode::Host,
            ..config()
        },
        Options {
            access_config: Some(path),
            ..Default::default()
        },
    )
    .await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for path in [
        "/",
        "/apis",
        "/overview/api",
        "/overview.js",
        "/claude/usage",
        "/logs/api/dashboard",
    ] {
        assert_eq!(
            client
                .get(format!("{url}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    let login = client
        .post(format!("{url}/logs/login"))
        .form(&[("api_key", keys.local.as_str()), ("next", "/")])
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 303);
    assert_eq!(login.headers()[header::LOCATION], "/");
    let cookie = login.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.ends_with("Path=/"));
    let cookie = cookie.split(';').next().unwrap();
    for path in ["/", "/apis", "/overview/api", "/logs"] {
        assert_eq!(
            client
                .get(format!("{url}{path}"))
                .header(header::COOKIE, cookie)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    let dashboard: Value = client
        .get(format!("{url}/logs/api/dashboard"))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        dashboard["rpm"], 0,
        "Dashboard authentication attempts are not model traffic"
    );
    let api_login = client
        .post(format!("{url}/logs/login"))
        .form(&[("api_key", keys.local.as_str()), ("next", "/apis")])
        .send()
        .await
        .unwrap();
    assert_eq!(api_login.headers()[header::LOCATION], "/apis");
    let login = client
        .post(format!("{url}/logs/login"))
        .form(&[
            ("api_key", keys.local.as_str()),
            ("next", "https://untrusted.example"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(login.headers()[header::LOCATION], "/logs");
    task.abort();
}

#[test]
fn overview_console_routes_show_account_and_override_without_credentials() {
    let config: Config = serde_json::from_value(json!({"listen":"127.0.0.1:8080","account_schema_version":1,
        "accounts":{"workspace":{"implementation":"claude","auth":"console","workspace_id":"wrkspc_example","credentials_file":"PRIVATE.json"}},
        "routes":[{"model":"workspace-model","api_shape":"messages","legs":[{"provider":"workspace","override":"model"}]}],
        "overrides":{"workspace":{"model":{"from":"workspace-model","to":"claude-test"}}}
    })).unwrap();
    let data = catalog(&config);
    let claude = api(&data, "claude");
    assert_eq!(claude["configured"], true);
    let route = &model(claude, "workspace-model").unwrap()["routes"][0];
    assert_eq!(route["target"], "claude-test");
    assert_eq!(route["when"], "workspace");
    assert!(!data.to_string().contains("PRIVATE"));
    assert!(!data.to_string().contains("wrkspc_example"));
}

#[test]
fn wif_account_has_no_oauth_store_and_uses_workspace_api_billing() {
    let value = json!({"listen":"127.0.0.1:8080","account_schema_version":1,
        "accounts":{"workspace":{"implementation":"claude","auth":"wif","workspace_id":"wrkspc_example",
            "federation":{"organization_id":"org-test","service_account_id":"svac_test","federation_rule_id":"fdrl_test","identity_token":"sh://printf synthetic.jwt.assertion"}}},
        "routes":[{"model":"workspace","api_shape":"messages","legs":[{"provider":"workspace","model":"claude-test"}]}]
    });
    let config: Config = serde_json::from_value(value.clone()).unwrap();
    config.validate().unwrap();
    config
        .validate_account_paths(std::path::Path::new("/tmp/config.json"))
        .unwrap();
    assert_eq!(config.accounts["workspace"].auth(), "wif");
    assert_eq!(
        config.routes[0]
            .resolve(&config, 0, "workspace", None)
            .unwrap()
            .billing_mode,
        config::routes::BillingMode::PayPerToken
    );
    assert_eq!(api(&catalog(&config), "claude")["configured"], true);
    for (key, replacement) in [
        ("credentials_file", json!("wrong.json")),
        ("workspace_id", Value::Null),
        ("federation", Value::Null),
    ] {
        let mut invalid = value.clone();
        invalid["accounts"]["workspace"][key] = replacement;
        assert!(serde_json::from_value::<Config>(invalid).map_or(true, |c| c.validate().is_err()));
    }
}

#[test]
fn overview_distinguishes_subscription_defaults_from_workspace_overrides() {
    let config: Config = serde_json::from_value(json!({"listen":"127.0.0.1:8080","account_schema_version":1,
        "providers":{"claude":{}},
        "accounts":{
            "personal":{"implementation":"codex","auth":"subscription","credentials_file":"PRIVATE-personal.json"},
            "workspace":{"implementation":"claude","auth":"console","workspace_id":"wrkspc_example","credentials_file":"PRIVATE-workspace.json"}
        },
        "routes":[
            {"model":"*","api_shape":"responses","legs":[{"provider":"personal"}]},
            {"model":"workspace-model","api_shape":"messages","legs":[{"provider":"workspace","override":"model"}]}
        ],
        "overrides":{"workspace":{"model":{"from":"workspace-model","to":"claude-test"}}}
    })).unwrap();
    config.validate().unwrap();
    let data = catalog(&config);
    assert_eq!(
        data["routing"]["claude_default"],
        json!({"name":"Claude","auth":"subscription"})
    );
    assert_eq!(
        data["routing"]["accounts"]["personal"],
        json!({"defaults":["responses"],"models":[],"fallback":false})
    );
    assert_eq!(
        data["routing"]["accounts"]["workspace"],
        json!({"defaults":[],"models":["workspace-model"],"fallback":false})
    );
    assert!(!data.to_string().contains("PRIVATE"));
    assert!(!data.to_string().contains("wrkspc_example"));
    let mut without_default = config;
    without_default.claude = None;
    assert!(catalog(&without_default)["routing"]["claude_default"].is_null());
    without_default.routes[1].model = "*".into();
    assert_eq!(
        catalog(&without_default)["routing"]["claude_default"],
        json!({"name":"workspace","auth":"console"})
    );
    without_default.mode = Mode::Client;
    assert!(catalog(&without_default)["routing"].is_null());
}
