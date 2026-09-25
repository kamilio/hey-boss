use serde_json::Value;
use std::process::Command;

fn cli(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn catalog_contains_nested_commands_help_and_output_support() {
    let catalog = cli(&["admin", "catalog", "--json"]);
    let commands = catalog["commands"].as_array().unwrap();
    for name in [
        "issue view",
        "issue pr add",
        "artifact create",
        "mm show",
        "notif secret",
        "skill install",
    ] {
        let item = commands.iter().find(|v| v["id"] == name).unwrap();
        assert!(item["help"].as_str().unwrap().contains("Usage:"));
        assert!(item["json_supported"].is_boolean());
    }
    assert!(commands.len() > 60);
    assert!(!commands.iter().any(|c| c["id"] == "alert"));
    let alert = commands.iter().find(|c| c["id"] == "notif alert").unwrap();
    assert_eq!(alert["preview"], "sample");
    assert!(
        alert["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "hey-boss alert")
    );
}

#[test]
fn command_capture_uses_fresh_sample_state_for_each_output_format() {
    let preview = cli(&["admin", "preview", "issue create", "--json"]);
    assert_eq!(preview["mode"], "sample");
    assert_eq!(preview["text"]["exit_code"], 0);
    assert_eq!(preview["json"]["exit_code"], 0);
    let json: Value = serde_json::from_str(preview["json"]["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(json["issue"]["number"], 3);
    assert!(
        preview["text"]["stdout"]
            .as_str()
            .unwrap()
            .contains("Review follow-up")
    );
}

#[test]
fn privileged_commands_are_documented_without_running_them() {
    let preview = cli(&["admin", "preview", "upgrade", "--json"]);
    assert_eq!(preview["mode"], "help");
    assert!(preview["reason"].as_str().unwrap().contains("installation"));
    assert!(
        preview["text"]["stdout"]
            .as_str()
            .unwrap()
            .contains("Usage:")
    );
}

#[test]
fn lifecycle_and_batch_previews_include_the_required_context() {
    for command in ["issue ready", "issue transfer", "issue batch", "mm batch"] {
        let preview = cli(&["admin", "preview", command, "--json"]);
        for format in ["text", "json"] {
            assert_eq!(
                preview[format]["exit_code"], 0,
                "{command}: {}",
                preview[format]
            );
        }
        let output: Value =
            serde_json::from_str(preview["json"]["stdout"].as_str().unwrap()).unwrap();
        assert!(output.is_object());
    }
}

#[test]
fn skill_show_returns_the_canonical_document() {
    let value = cli(&["skill", "show", "--json"]);
    assert_eq!(
        value["markdown"],
        include_str!("../skills/hey-boss/SKILL.md")
    );
}

#[test]
fn preview_rejects_arbitrary_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args(["admin", "preview", "issue list; echo injected", "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unknown command"));
}

#[test]
fn notification_samples_use_the_real_output_formatter_without_sending() {
    let preview = cli(&["admin", "preview", "notif alert", "--json"]);
    assert_eq!(preview["text"]["exit_code"], 0);
    assert!(
        preview["text"]["command"]
            .as_str()
            .unwrap()
            .starts_with("hey-boss notif alert")
    );
    let legacy = cli(&["admin", "preview", "alert", "--json"]);
    assert_eq!(legacy["id"], "notif alert");
    assert_eq!(legacy["text"]["stdout"], preview["text"]["stdout"]);
    assert!(
        preview["text"]["stdout"]
            .as_str()
            .unwrap()
            .contains("Task ID: review-task-1")
    );
    assert!(
        preview["reason"]
            .as_str()
            .unwrap()
            .contains("Sample daemon response")
    );
}

#[test]
fn review_http_preview_is_csrf_guarded_and_keeps_live_issues_unchanged() {
    use std::io::BufRead;
    use std::process::Stdio;
    let directory = hey_boss::admin::Temporary::new().unwrap();
    let db = directory.0.join("issues.db");
    let created = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .current_dir(&directory.0)
        .env("HEY_BOSS_ISSUE_DB", &db)
        .env("HEY_BOSS_FLEET_STATE", directory.0.join("fleet"))
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .args([
            "issue",
            "--project",
            "Review API",
            "--agent",
            "human:review-test",
            "create",
            "--title",
            "Keep live issue",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    struct Web(std::process::Child);
    impl Drop for Web {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut web = Web(Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .current_dir(&directory.0)
        .env("HEY_BOSS_ISSUE_DB", &db)
        .env("HEY_BOSS_FLEET_STATE", directory.0.join("fleet"))
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .args([
            "issue",
            "web",
            "--project",
            "Review API",
            "--no-discovery",
            "--port",
            "0",
            "--json",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap());
    let mut ready = String::new();
    std::io::BufReader::new(web.0.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    let ready: Value = serde_json::from_str(&ready).unwrap();
    let base = ready["url"].as_str().unwrap().trim_end_matches('/');
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .unwrap();
    let html = client.get(format!("{base}/admin")).send().unwrap();
    assert_eq!(html.status(), 200);
    assert!(html.text().unwrap().contains("Review pane"));
    let boot: Value = client
        .get(format!("{base}/api/bootstrap"))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let body = serde_json::json!({"command":"issue delete","issue":{"title":"Copied context","body":"Sample"}});
    assert_eq!(
        client
            .post(format!("{base}/api/admin/preview"))
            .json(&body)
            .send()
            .unwrap()
            .status(),
        403
    );
    let response = client
        .post(format!("{base}/api/admin/preview"))
        .header("X-Hey-Boss-CSRF", boot["csrf"].as_str().unwrap())
        .json(&body)
        .send()
        .unwrap();
    assert_eq!(response.status(), 200);
    let preview: Value = response.json().unwrap();
    assert_eq!(preview["text"]["exit_code"], 0);
    let view = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .current_dir(&directory.0)
        .env("HEY_BOSS_ISSUE_DB", &db)
        .env("HEY_BOSS_FLEET_STATE", directory.0.join("fleet"))
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .args(["issue", "view", "1", "--project", "Review API", "--json"])
        .output()
        .unwrap();
    assert!(view.status.success());
    let value: Value = serde_json::from_slice(&view.stdout).unwrap();
    assert_eq!(value["issue"]["title"], "Keep live issue");
    assert!(value["issue"]["deleted_at"].is_null());
}
