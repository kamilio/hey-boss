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
fn configure_pi_preserves_tuning_and_scopes_compaction_to_gemini() {
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
    for id in ["gemini-test", "gemini/gemini-early-exp", "gemini/direct"] {
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
    for id in ["coding", "unknown-model", "chat-only", "gemini/chat"] {
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
