use super::*;

fn config() -> Config {
    serde_json::from_value(json!({
        "listen": "0.0.0.0:18080",
        "providers": {"gemini": {"api_key": "upstream-secret"}},
        "aliases": [
            {"from": "openai", "to": "gpt-5"},
            {"from": "coding", "to": "gemini/models/gemini-test", "api_shape": "responses"}
        ]
    }))
    .unwrap()
}

#[test]
fn setup_preserves_settings_and_env_with_private_backups_and_noop_reruns() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.json");
    let env_path = dir.path().join(".env");
    let settings = json!({
        "hooks": {"BeforeAgent": []}, "mcpServers": {"mine": {"command": "local"}},
        "security": {"folderTrust": {"enabled": true}, "auth": {"selectedType": "oauth-personal"}},
        "model": {"name": "old", "maxSessionTurns": 12}, "ui": {"theme": "light"}
    });
    fs::write(&settings_path, settings.to_string()).unwrap();
    let original_env = "# keep this\nOTHER='multi\nGEMINI_API_KEY=inside-other-value\nline'\nexport GEMINI_API_KEY = 'old\nkey'\nGEMINI_MODEL=stale\nGOOGLE_GENAI_USE_GCA=true\nGOOGLE_GENAI_USE_VERTEXAI=true\nGOOGLE_GEMINI_BASE_URL: https://old.example\n";
    fs::write(&env_path, original_env).unwrap();
    configure(&config(), "hp_test", None, Some(dir.path())).unwrap();
    let written: Value = serde_json::from_str(&read(&settings_path).unwrap()).unwrap();
    assert_eq!(
        written["security"]["auth"]["selectedType"],
        "gemini-api-key"
    );
    assert_eq!(written["model"]["name"], "gemini-test");
    assert_eq!(written["hooks"], settings["hooks"]);
    assert_eq!(written["mcpServers"], settings["mcpServers"]);
    assert_eq!(
        written["security"]["folderTrust"],
        settings["security"]["folderTrust"]
    );
    assert_eq!(written["model"]["maxSessionTurns"], 12);
    assert_eq!(written["ui"], settings["ui"]);
    let env = read(&env_path).unwrap();
    assert!(
        env.starts_with("# keep this\nOTHER='multi\nGEMINI_API_KEY=inside-other-value\nline'\n")
    );
    assert!(env.contains("GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:18080\n"));
    assert!(env.contains("GEMINI_API_KEY=hp_test\nGEMINI_API_KEY_AUTH_MECHANISM=bearer\n"));
    assert!(!env.contains("upstream-secret"));
    assert!(!env.contains("stale"));
    assert!(!env.contains("=true"));
    let before = fs::metadata(&settings_path).unwrap().modified().unwrap();
    configure(&config(), "hp_test", None, Some(dir.path())).unwrap();
    assert_eq!(read(&env_path).unwrap(), env);
    assert_eq!(
        fs::metadata(&settings_path).unwrap().modified().unwrap(),
        before
    );
    let paths: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|p| p.unwrap().path())
        .collect();
    assert_eq!(paths.len(), 4); // two configs and exactly one backup of each
    assert!(paths.iter().any(|p| {
        p.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".env.backup-")
            && read(p).unwrap() == original_env
    }));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in paths {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

#[test]
fn broken_settings_or_env_leave_both_files_unchanged() {
    for settings in [
        "{",
        "[]",
        r#"{"security":null}"#,
        r#"{"security":{"auth":false}}"#,
        r#"{"model":"old"}"#,
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.json"), settings).unwrap();
        fs::write(dir.path().join(".env"), "CUSTOM=keep\n").unwrap();
        assert!(configure(&config(), "key", None, Some(dir.path())).is_err());
        assert_eq!(read(&dir.path().join("settings.json")).unwrap(), settings);
        assert_eq!(read(&dir.path().join(".env")).unwrap(), "CUSTOM=keep\n");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(".env"), "OTHER='unterminated\n").unwrap();
    assert!(configure(&config(), "key", None, Some(dir.path())).is_err());
    assert!(!dir.path().join("settings.json").exists());
    assert_eq!(
        read(&dir.path().join(".env")).unwrap(),
        "OTHER='unterminated\n"
    );
}

#[test]
fn native_selection_uses_destinations_and_rejects_cross_provider_aliases() {
    let mut cfg = config();
    assert_eq!(model(&cfg, None).unwrap(), "gemini-test");
    assert_eq!(model(&cfg, Some("gemini/models/custom")).unwrap(), "custom");
    assert!(model(&cfg, Some("openai")).is_err());
    for invalid in [
        "",
        "../bad",
        "a/b",
        "x\nBAD=value",
        "x:generateContent",
        "a?key=b",
    ] {
        assert!(model(&cfg, Some(invalid)).is_err());
    }
    cfg.aliases = serde_json::from_value(json!([
        {"from":"gemini-test", "to":"gpt-5"},
        {"from":"coding", "to":"gemini/gemini-test"},
        {"from":"reasoning", "reasoning_routes":{"high":{"to":"gemini/deep"}}}
    ]))
    .unwrap();
    assert_eq!(model(&cfg, None).unwrap(), "deep");
    cfg.aliases.clear();
    cfg.fallbacks
        .insert("gpt-5".into(), vec!["gemini/backup".into()]);
    assert_eq!(model(&cfg, None).unwrap(), "backup");
    cfg.fallbacks.clear();
    assert!(model(&cfg, None).is_err());
    cfg.model_registry = Some(
        serde_json::from_value(json!({
            "defaults": crate::model_registry::DEFAULT_BUDGET,
            "models": {"gemini/registered": {}}
        }))
        .unwrap(),
    );
    assert_eq!(model(&cfg, None).unwrap(), "registered");
    cfg.gemini = None;
    assert!(model(&cfg, Some("registered")).is_err());
}

#[test]
fn fresh_setup_and_relay_use_local_endpoint_without_upstream_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config();
    cfg.listen = "[::]:18081".parse().unwrap();
    cfg.mode = Mode::Client;
    cfg.gemini = None;
    assert!(configure(&cfg, "hey-proxy", None, Some(dir.path())).is_err());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    configure(&cfg, "hey-proxy", Some("remote-model"), Some(dir.path())).unwrap();
    assert!(
        read(&dir.path().join(".env"))
            .unwrap()
            .contains("http://[::1]:18081\n")
    );
    let settings: Value =
        serde_json::from_str(&read(&dir.path().join("settings.json")).unwrap()).unwrap();
    assert_eq!(settings["model"]["name"], "remote-model");
    assert!(settings["security"].get("folderTrust").is_none());
}

#[test]
fn dotenv_preserves_quoted_records_comments_and_duplicate_override_cleanup() {
    let source = "# GEMINI_API_KEY=comment\nA=keep # comment\nCOLON: 'first=value\nGEMINI_MODEL=keep-in-colon\nend'\nB=\"multi\nGEMINI_MODEL=in-string\nend\"\nC=`another\nvalue`\nexport GEMINI_API_KEY=one\nGEMINI_API_KEY=two\nTAIL=last";
    let result = environment(source, "http://127.0.0.1:8080", "hp_key").unwrap();
    assert!(result.starts_with("# GEMINI_API_KEY=comment\nA=keep # comment\nCOLON: 'first=value\nGEMINI_MODEL=keep-in-colon\nend'\nB=\"multi\nGEMINI_MODEL=in-string\nend\"\nC=`another\nvalue`\nTAIL=last\n"));
    assert_eq!(
        result
            .lines()
            .filter(|l| l.starts_with("GEMINI_API_KEY="))
            .count(),
        1
    );
    assert!(environment(source, "http://127.0.0.1:8080", "bad\nINJECT=yes").is_err());
}
