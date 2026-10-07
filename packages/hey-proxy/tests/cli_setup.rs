use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

fn cli(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hey-proxy"));
    cmd.env("HOME", home)
        .env("CODEX_HOME", home.join("codex"))
        .stdin(Stdio::null());
    cmd
}

#[cfg(unix)]
#[test]
fn credential_preflight_accepts_piped_config_without_creating_files_or_printing_secrets() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    for input_path in ["/dev/stdin", "/dev/fd/0"] {
        let mut child = cli(dir.path())
            .args(["--config", input_path, "check-credentials"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(
        br#"{"listen":"127.0.0.1:8080","providers":{"openai":{"api_keys":{"default":"synthetic-piped-secret"}}}}"#,
    ).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "Credential sources ready"
        );
        assert!(output.stderr.is_empty());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[test]
fn init_is_minimal_private_and_preserves_existing_proxy_and_codex_files() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let codex = home.join("codex");
    std::fs::create_dir(&codex).unwrap();
    let sentinel = b"model = 'my-existing-model'\n";
    std::fs::write(codex.join("config.toml"), sentinel).unwrap();
    assert!(cli(home).arg("--help").output().unwrap().status.success());
    assert!(!home.join(".hey-proxy").exists());
    assert!(cli(home).arg("--init").output().unwrap().status.success());
    let path = home.join(".hey-proxy/config.json");
    let generated: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let expected: Value =
        serde_json::from_str(include_str!("../examples/minimal.config.json")).unwrap();
    assert_eq!(generated, expected);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let edited = b"{\"listen\":\"127.0.0.1:9090\",\"providers\":{\"openai\":{\"api_keys\":{\"default\":\"synthetic\"}}}}\n";
    std::fs::write(&path, edited).unwrap();
    assert!(cli(home).arg("--init").output().unwrap().status.success());
    assert_eq!(std::fs::read(path).unwrap(), edited);
    assert_eq!(std::fs::read(codex.join("config.toml")).unwrap(), sentinel);
    assert_eq!(std::fs::read_dir(codex).unwrap().count(), 1);
}

#[tokio::test]
async fn first_start_creates_only_proxy_config_and_missing_credentials_return_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = cli(dir.path())
        .args(["--listen", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    struct Stop(std::process::Child);
    impl Drop for Stop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let _stop = Stop(child);
    let first = lines.next().unwrap().unwrap();
    let url = first.strip_prefix("hey-proxy listening on ").unwrap();
    let response = reqwest::Client::new()
        .post(format!("{url}/v1/responses"))
        .json(&json!({"model":"gpt-4.1","input":"test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert!(
        response.json::<Value>().await.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("No OpenAI credential configured")
    );
    assert!(
        reqwest::get(format!("{url}/logs/api?local=true"))
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert!(!dir.path().join("codex").exists());
    assert!(!dir.path().join(".codex").exists());
    assert!(dir.path().join(".hey-proxy/config.json").exists());
}

#[test]
fn all_user_examples_validate_without_resolving_credentials_or_changing_files() {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")).unwrap()
    {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let temporary = dir.path().join(path.file_name().unwrap());
        std::fs::write(&temporary, &bytes).unwrap();
        let result = cli(dir.path())
            .arg("--config")
            .arg(&temporary)
            .arg("--init")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "Invalid example {}: {}",
            path.display(),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(std::fs::read(temporary).unwrap(), bytes);
    }
    assert!(!dir.path().join("codex").exists());
}

#[test]
fn configure_pi_preserves_tuning_and_repairs_unsafe_compaction_for_all_responses_models() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let agent = home.join("pi");
    std::fs::create_dir(&agent).unwrap();
    let config = home.join("proxy.json");
    std::fs::write(&config, json!({
        "listen": "127.0.0.1:18080",
        "aliases": [
            {"from":"gemini-test", "to":"gemini/gemini-early-exp"},
            {"from":"coding", "to":"unknown-model", "reasoning":"high"},
            {"from":"mixed", "to":"unknown-model", "reasoning_routes":{"high":{"to":"gemini/other"}}},
            {"from":"gemini/direct"},
            {"from":"chat-only", "to":"gemini/chat", "api_shape":"chat_completions"}
        ]
    }).to_string()).unwrap();
    let tuning = json!({"id":"gemini-test", "name":"My Gemini", "contextWindow":96000,
        "maxTokens":8192, "reasoning":true, "input":["text","image"],
        "cost":{"input":1,"output":2,"cacheRead":0.1,"cacheWrite":0},
        "compat":{"supportsStore":false}});
    let original_models = json!({"providers":{
        "other":{"models":[{"id":"local"}]},
        "hey-proxy":{"baseUrl":"http://stale", "apiKey":"stale",
            "models":[tuning, {"id":"retired","maxTokens":100}, {"id":"coding","reasoning":false}],
            "modelOverrides":{"gemini-test":{"maxTokens":4096}}}
    }});
    let original_settings = json!({"theme":"light", "compaction":{
        "enabled":true, "keepRecentTokens":100000, "reserveTokens":200000,
        "modelOverrides":{
            "other/gemini-test":{"keepRecentTokens":5000},
            "hey-proxy/mixed":{"keepRecentTokens":12000},
            "hey-proxy/gemini/other":{"reserveTokens":8000}
        }
    }});
    std::fs::write(agent.join("models.json"), original_models.to_string()).unwrap();
    std::fs::write(agent.join("settings.json"), original_settings.to_string()).unwrap();
    let run = || {
        let output = cli(home)
            .env("PI_CODING_AGENT_DIR", &agent)
            .arg("--config")
            .arg(&config)
            .arg("configure-pi")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run();
    let read = |name| -> Value {
        serde_json::from_slice(&std::fs::read(agent.join(name)).unwrap()).unwrap()
    };
    let models = read("models.json");
    let provider = &models["providers"]["hey-proxy"];
    assert_eq!(provider["baseUrl"], "http://127.0.0.1:18080/v1");
    assert_eq!(provider["apiKey"], "hey-proxy");
    assert_eq!(
        provider["modelOverrides"],
        original_models["providers"]["hey-proxy"]["modelOverrides"]
    );
    assert_eq!(
        models["providers"]["other"],
        original_models["providers"]["other"]
    );
    let entries = provider["models"].as_array().unwrap();
    assert_eq!(
        entries.iter().find(|m| m["id"] == "gemini-test").unwrap(),
        &tuning
    );
    assert_eq!(
        entries.iter().find(|m| m["id"] == "coding").unwrap()["reasoning"],
        false
    );
    assert!(!entries.iter().any(|m| m["id"] == "retired"));
    // No guessed upstream limits, even for a Gemini destination.
    assert_eq!(
        entries
            .iter()
            .find(|m| m["id"] == "gemini/gemini-early-exp")
            .unwrap(),
        &json!({"id":"gemini/gemini-early-exp"})
    );
    let settings = read("settings.json");
    assert_eq!(settings["theme"], "light");
    assert_eq!(settings["defaultModel"], "gemini-test");
    let compaction = &settings["compaction"];
    assert_eq!(compaction["keepRecentTokens"], 100000);
    assert_eq!(compaction["reserveTokens"], 200000);
    let overrides = &compaction["modelOverrides"];
    for id in [
        "gemini-test",
        "gemini/gemini-early-exp",
        "gemini/direct",
        "coding",
        "unknown-model",
    ] {
        assert_eq!(
            overrides[format!("hey-proxy/{id}")],
            json!({"keepRecentTokens":20000,"reserveTokens":16384})
        );
    }
    assert_eq!(
        overrides["hey-proxy/mixed"],
        json!({"keepRecentTokens":12000,"reserveTokens":16384})
    );
    assert_eq!(
        overrides["hey-proxy/gemini/other"],
        json!({"keepRecentTokens":20000,"reserveTokens":8000})
    );
    assert_eq!(
        overrides["other/gemini-test"],
        original_settings["compaction"]["modelOverrides"]["other/gemini-test"]
    );
    for id in ["chat-only", "gemini/chat"] {
        assert!(overrides.get(format!("hey-proxy/{id}")).is_none());
    }
    let files_before = std::fs::read_dir(&agent).unwrap().count();
    assert_eq!(files_before, 4); // Original models and settings were backed up.
    run();
    assert_eq!(read("models.json"), models);
    assert_eq!(read("settings.json"), settings);
    assert_eq!(std::fs::read_dir(&agent).unwrap().count(), files_before);
}

#[test]
fn configure_pi_keeps_large_budgets_when_the_resolved_model_window_supports_them() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("proxy.json");
    std::fs::write(
        &config,
        json!({"listen":"127.0.0.1:8080", "aliases":[
            {"from":"large-inline"}, {"from":"large-override"}, {"from":"small"}
        ]})
        .to_string(),
    )
    .unwrap();
    let agent = dir.path().join("pi");
    std::fs::create_dir(&agent).unwrap();
    std::fs::write(agent.join("models.json"), json!({"providers":{"hey-proxy":{
        "models":[{"id":"large-inline","contextWindow":1048576}, {"id":"large-override","contextWindow":128000}],
        "modelOverrides":{"large-override":{"contextWindow":1048576}}
    }}}).to_string()).unwrap();
    std::fs::write(
        agent.join("settings.json"),
        json!({"compaction":{
            "keepRecentTokens":100000,"reserveTokens":200000
        }})
        .to_string(),
    )
    .unwrap();
    let output = cli(dir.path())
        .env("PI_CODING_AGENT_DIR", &agent)
        .arg("--config")
        .arg(config)
        .arg("configure-pi")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let settings: Value =
        serde_json::from_slice(&std::fs::read(agent.join("settings.json")).unwrap()).unwrap();
    let compaction = &settings["compaction"];
    assert_eq!(compaction["keepRecentTokens"], 100000);
    assert_eq!(compaction["reserveTokens"], 200000);
    assert!(
        compaction["modelOverrides"]
            .get("hey-proxy/large-inline")
            .is_none()
    );
    assert!(
        compaction["modelOverrides"]
            .get("hey-proxy/large-override")
            .is_none()
    );
    assert_eq!(
        compaction["modelOverrides"]["hey-proxy/small"],
        json!({"keepRecentTokens":20000,"reserveTokens":16384})
    );
}

#[test]
fn configure_pi_rejects_malformed_tuning_before_writing_either_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let config = home.join("proxy.json");
    std::fs::write(
        &config,
        r#"{"listen":"127.0.0.1:8080","aliases":[{"from":"gemini-test","to":"gemini/unknown"}]}"#,
    )
    .unwrap();
    for (models, settings) in [
        (json!({"providers":{"hey-proxy":{"models":{}}}}), json!({})),
        (
            json!({"providers":{"hey-proxy":{"models":[{"id":"a"},{"id":"a"}]}}}),
            json!({}),
        ),
        (json!({}), json!({"compaction":false})),
        (json!({}), json!({"compaction":{"modelOverrides":[]}})),
        (
            json!({}),
            json!({"compaction":{"modelOverrides":{"hey-proxy/gemini-test":null}}}),
        ),
        (json!({}), json!({"compaction":{"reserveTokens":-1}})),
        (
            json!({}),
            json!({"compaction":{"modelOverrides":{"hey-proxy/gemini-test":{"reserveTokens":200000}}}}),
        ),
    ] {
        let agent = tempfile::tempdir().unwrap();
        let models_path = agent.path().join("models.json");
        let settings_path = agent.path().join("settings.json");
        std::fs::write(&models_path, models.to_string()).unwrap();
        std::fs::write(&settings_path, settings.to_string()).unwrap();
        let output = cli(home)
            .env("PI_CODING_AGENT_DIR", agent.path())
            .arg("--config")
            .arg(&config)
            .arg("configure-pi")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("left unchanged"));
        assert_eq!(
            std::fs::read_to_string(models_path).unwrap(),
            models.to_string()
        );
        assert_eq!(
            std::fs::read_to_string(settings_path).unwrap(),
            settings.to_string()
        );
        assert_eq!(std::fs::read_dir(agent.path()).unwrap().count(), 2);
    }
}

fn registry_fixture() -> Value {
    json!({
        "defaults": {"context_window":128000, "max_tokens":16384,
            "keep_recent_tokens":20000, "reserve_tokens":16384},
        "models": {"gemini/gemini-early-exp": {
            "context_window":1048576, "max_tokens":65536,
            "keep_recent_tokens":100000, "reserve_tokens":200000}}
    })
}

#[test]
fn configure_pi_registry_updates_aliases_routes_fallbacks_and_removes_competing_limits() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let agent = home.join("pi");
    std::fs::create_dir(&agent).unwrap();
    let config_path = home.join("proxy.json");
    let mut config = json!({
        "listen":"127.0.0.1:18080",
        "model_registry":registry_fixture(),
        "aliases":[
            {"from":"gemini-test","to":"gemini/gemini-early-exp"},
            {"from":"second-alias","to":"gemini/gemini-early-exp"},
            {"from":"coding","to":"unknown"},
            {"from":"mixed","to":"gemini/gemini-early-exp","reasoning_routes":{"low":{"to":"unknown"}}},
            {"from":"with-fallback","to":"openai/large"},
            {"from":"direct"}
        ],
        "fallbacks":{"large":["unknown"]}
    });
    config["model_registry"]["models"]["large"] =
        config["model_registry"]["models"]["gemini/gemini-early-exp"].clone();
    // Registry keys describe backends, even if a key is also a frontend alias.
    config["model_registry"]["models"]["coding"] =
        config["model_registry"]["models"]["large"].clone();
    std::fs::write(agent.join("models.json"), json!({"providers":{
        "other":{"models":[{"id":"mine","contextWindow":42}]},
        "hey-proxy":{"models":[{"id":"gemini-test","contextWindow":100,"maxTokens":2,"name":"Mine"}],
            "modelOverrides":{"gemini-test":{"contextWindow":200,"maxTokens":1,"reasoning":true},
                "second-alias":{"contextWindow":10,"maxTokens":1}}}
    }}).to_string()).unwrap();
    std::fs::write(agent.join("settings.json"), json!({"theme":"light","compaction":{
        "enabled":true,"keepRecentTokens":100000,"reserveTokens":200000,
        "modelOverrides":{"hey-proxy/gemini-test":{"reserveTokens":1},"other/model":{"reserveTokens":8}}
    }}).to_string()).unwrap();
    let run = || {
        let result = cli(home)
            .env("PI_CODING_AGENT_DIR", &agent)
            .arg("--config")
            .arg(&config_path)
            .arg("configure-pi")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    for (context, output, keep, reserve) in [
        (1048576, 65536, 100000, 200000),
        (524288, 32768, 50000, 100000),
    ] {
        config["model_registry"]["models"]["gemini/gemini-early-exp"] = json!({
            "context_window":context,"max_tokens":output,"keep_recent_tokens":keep,"reserve_tokens":reserve});
        std::fs::write(&config_path, config.to_string()).unwrap();
        run();
        let catalog: Value =
            serde_json::from_slice(&std::fs::read(agent.join("models.json")).unwrap()).unwrap();
        let settings: Value =
            serde_json::from_slice(&std::fs::read(agent.join("settings.json")).unwrap()).unwrap();
        let provider = &catalog["providers"]["hey-proxy"];
        for id in ["gemini-test", "second-alias", "gemini/gemini-early-exp"] {
            let model = provider["models"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["id"] == id)
                .unwrap();
            assert_eq!(model["contextWindow"], context);
            assert_eq!(model["maxTokens"], output);
            assert_eq!(
                settings["compaction"]["modelOverrides"][format!("hey-proxy/{id}")],
                json!({"keepRecentTokens":keep,"reserveTokens":reserve})
            );
        }
        for id in [
            "coding",
            "unknown",
            "mixed",
            "with-fallback",
            "openai/large",
            "direct",
        ] {
            let model = provider["models"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["id"] == id)
                .unwrap();
            assert_eq!(model["contextWindow"], 128000, "{id}");
            assert_eq!(model["maxTokens"], 16384, "{id}");
            assert_eq!(
                settings["compaction"]["modelOverrides"][format!("hey-proxy/{id}")],
                json!({"keepRecentTokens":20000,"reserveTokens":16384}),
                "{id}"
            );
        }
        assert_eq!(
            provider["modelOverrides"]["gemini-test"],
            json!({"reasoning":true})
        );
        assert!(provider["modelOverrides"].get("second-alias").is_none());
        assert_eq!(
            catalog["providers"]["other"]["models"][0]["contextWindow"],
            42
        );
        assert_eq!(settings["theme"], "light");
        assert_eq!(
            settings["compaction"]["modelOverrides"]["other/model"]["reserveTokens"],
            8
        );
        assert_eq!(settings["compaction"]["reserveTokens"], 200000);
        let before: Vec<_> = ["models.json", "settings.json"]
            .map(|f| std::fs::read(agent.join(f)).unwrap())
            .into();
        let files = std::fs::read_dir(&agent).unwrap().count();
        run();
        for (i, f) in ["models.json", "settings.json"].iter().enumerate() {
            assert_eq!(std::fs::read(agent.join(f)).unwrap(), before[i]);
        }
        assert_eq!(std::fs::read_dir(&agent).unwrap().count(), files);
    }
}

#[test]
fn configure_pi_registry_rejects_invalid_budgets_before_writes() {
    for (field, bad) in [
        ("context_window", json!(0)),
        ("context_window", json!(9007199254740992u64)),
        ("max_tokens", json!(0)),
        ("max_tokens", json!(2000000)),
        ("max_tokens", json!(65536.5)),
        ("keep_recent_tokens", json!(800000)),
        ("reserve_tokens", json!(100)),
        ("reserve_tokens", json!(600000)),
        ("reserve_tokens", json!(-1)),
        ("typo", json!(10)),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let config_path = home.join("proxy.json");
        let mut config = json!({"listen":"127.0.0.1:18080","aliases":[{"from":"gemini-test","to":"gemini/gemini-early-exp"}],"model_registry":registry_fixture()});
        config["model_registry"]["models"]["gemini/gemini-early-exp"][field] = bad;
        std::fs::write(&config_path, config.to_string()).unwrap();
        for f in ["models.json", "settings.json"] {
            std::fs::write(home.join(f), b"{}\n").unwrap();
        }
        let result = cli(home)
            .env("PI_CODING_AGENT_DIR", home)
            .arg("--config")
            .arg(&config_path)
            .arg("configure-pi")
            .output()
            .unwrap();
        assert!(!result.status.success(), "{field}");
        for f in ["models.json", "settings.json"] {
            assert_eq!(std::fs::read(home.join(f)).unwrap(), b"{}\n");
        }
        assert_eq!(std::fs::read_dir(home).unwrap().count(), 3);
    }
}

#[test]
fn routing_diagnostic_is_offline_redacted_and_does_not_rewrite_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routes.json");
    let raw = include_bytes!("../examples/routes.config.json");
    std::fs::write(&path, raw).unwrap();
    let codex = dir.path().join("codex");
    std::fs::create_dir(&codex).unwrap();
    let sentinel = b"model = 'unchanged'\n";
    std::fs::write(codex.join("config.toml"), sentinel).unwrap();
    let output = cli(dir.path())
        .arg("--config")
        .arg(&path)
        .args(["resolve-route", "gpt-6-astra"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["policy"], "routes");
    assert_eq!(value["forwarding"], "staged");
    assert_eq!(value["legs"][0]["provider"], "ultima");
    assert_eq!(value["legs"][0]["upstream_model"], "ultima-alpha");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        !text.contains("op://")
            && !text.contains("example.com")
            && !text.contains("credentials_file")
    );
    assert_eq!(std::fs::read(&path).unwrap(), raw);
    assert_eq!(std::fs::read(codex.join("config.toml")).unwrap(), sentinel);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    let missing = dir.path().join("absent.json");
    assert!(
        !cli(dir.path())
            .arg("--config")
            .arg(&missing)
            .args(["resolve-route", "logical"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!missing.exists());
}
