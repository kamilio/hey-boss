use super::*;

#[test]
fn quota_discovery_collapses_store_aliases_but_preserves_distinct_subscriptions() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("config.json");
    let shared = dir.path().join("shared.json");
    std::fs::write(&shared, "{}").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&shared, dir.path().join("alias.json")).unwrap();
    let mut config = Config::default();
    config.accounts = serde_json::from_value(json!({
            "codex-subscription": {"implementation": "codex", "auth": "subscription", "credentials_file": shared},
            "work": {"implementation": "codex", "auth": "subscription", "credentials_file": "work.json"},
            "claude": {"implementation": "claude", "auth": "subscription", "credentials_file": "shared.json"}
    })).unwrap();
    config.codex = Some(codex::ProviderConfig {
        credentials_file: Some("shared.json".into()),
        ..Default::default()
    });
    let mut candidates = vec![Account {
        provider: "codex".into(),
        id: "default".into(),
    }];
    candidates.extend(config.accounts.iter().map(|(id, account)| Account {
        provider: account.implementation().into(),
        id: id.clone(),
    }));
    // Also sanitize discovery from an older running proxy before local merging.
    candidates.push(candidates[0].clone());
    deduplicate_accounts(&mut candidates, &config, Some(&source));
    assert_eq!(
        candidates
            .iter()
            .filter(|a| a.provider == "codex")
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>(),
        ["default", "work"]
    );
    assert!(candidates.iter().any(|a| a.provider == "claude"));
    assert_eq!(
        configured_accounts(&config, Some(&source))
            .iter()
            .filter(|a| a.provider == "codex")
            .count(),
        2
    );
    #[cfg(unix)]
    {
        let mut config = config.clone();
        if let crate::config::accounts::AccountConfig::Codex {
            credentials_file, ..
        } = config.accounts.get_mut("work").unwrap()
        {
            *credentials_file = "alias.json".into();
        }
        deduplicate_accounts(&mut candidates, &config, Some(&source));
        assert_eq!(
            candidates.iter().filter(|a| a.provider == "codex").count(),
            1
        );
    }
    let mut unknown = vec![
        Account {
            provider: "codex".into(),
            id: "remote-a".into(),
        },
        Account {
            provider: "codex".into(),
            id: "remote-b".into(),
        },
    ];
    deduplicate_accounts(&mut unknown, &config, Some(&source));
    assert_eq!(
        unknown.len(),
        2,
        "Never guess identity from quota values or unknown aliases"
    );
}
