use std::fs;
use std::process::Command;
use toml_edit::DocumentMut;

#[test]
fn configure_codex_updates_approval_defaults_and_selected_profiles_idempotently() {
    for existing in [
        "",
        "approval_policy = 'never'\napprovals_reviewer = 'user'\n",
    ] {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.toml");
        let profile = home.path().join("work.config.toml");
        fs::write(
            &config,
            format!(
                "{existing}sandbox_mode = 'danger-full-access'\nprofile = 'work'\n\
                 [profiles.work]\napproval_policy = 'never'\napprovals_reviewer = 'user'\n\
                 [profiles.manual]\napproval_policy = 'on-request'\napprovals_reviewer = 'user'\n"
            ),
        )
        .unwrap();
        fs::write(&profile, format!("{existing}model = 'existing-model'\n")).unwrap();
        let run = || {
            let output = Command::new(env!("CARGO_BIN_EXE_hey-proxy"))
                .args(["configure-codex", "--base-url", "http://127.0.0.1:8080/v1"])
                .arg("--codex-home")
                .arg(home.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run();
        let contents = [&config, &profile].map(|path| fs::read_to_string(path).unwrap());
        let docs = contents
            .each_ref()
            .map(|text| text.parse::<DocumentMut>().unwrap());
        for settings in [
            docs[0].as_item(),
            &docs[0]["profiles"]["work"],
            docs[1].as_item(),
        ] {
            assert_eq!(
                (
                    settings["approval_policy"].as_str(),
                    settings["approvals_reviewer"].as_str()
                ),
                (Some("on-request"), Some("auto_review"))
            );
        }
        assert_eq!(docs[0]["sandbox_mode"].as_str(), Some("danger-full-access"));
        assert_eq!(docs[1]["model"].as_str(), Some("existing-model"));
        assert_eq!(
            docs[0]["profiles"]["manual"]["approvals_reviewer"].as_str(),
            Some("user")
        );
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 4);
        run();
        assert_eq!(
            [&config, &profile].map(|path| fs::read_to_string(path).unwrap()),
            contents
        );
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 4);
    }
}
