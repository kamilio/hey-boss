use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn html_files_create_edit_and_export_artifacts_and_issue_descriptions_without_changes() {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("hb-html-import-{}", std::process::id())));
    fs::create_dir_all(&fixture.0).unwrap();
    let db = fixture.0.join("issues.db");
    let _owner = hey_boss::database::Owner::host(&db).unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(args)
            .current_dir(&fixture.0)
            .env("HEY_BOSS_ISSUE_DB", &db)
            .env("HEY_BOSS_FLEET_STATE", &fixture.0)
            .env("HEY_BOSS_AGENT_ID", "human:boss")
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_STATE_DIR")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let json = |args: &[&str]| serde_json::from_str::<Value>(&run(args)).unwrap();
    json(&[
        "project",
        "init",
        "--project",
        "HTML import",
        "--prs",
        "false",
        "--worktree",
        "false",
        "--yes",
        "--json",
    ]);
    for name in ["page.html", "page.HTM"] {
        let source = format!(
            "\u{feff}<!doctype html>\n<html><body>\n\n[Literal](missing.csv)\n\n<h1>日本語 🦀</h1><script>document.body.dataset.ready = 'yes';</script>\n</body></html>\n<!-- {name} -->\n"
        );
        fs::write(fixture.0.join(name), &source).unwrap();
        let artifact = json(&[
            "artifact",
            "--project",
            "named:HTML import",
            "create",
            "--title",
            name,
            "--file",
            name,
            "--json",
        ]);
        let id = artifact["artifact"]["id"].as_str().unwrap();
        assert_eq!(artifact["artifact"]["body"], source);
        assert_eq!(
            run(&["artifact", "--project", "named:HTML import", "export", id]),
            source
        );
        let issue = json(&[
            "issue",
            "--project",
            "named:HTML import",
            "create",
            "--title",
            name,
            "--file",
            name,
            "--draft",
            "--json",
        ]);
        let number = issue["issue"]["number"].as_i64().unwrap().to_string();
        assert_eq!(issue["issue"]["body"], source);
        let edited = format!("{source}<!-- edited -->\n");
        fs::write(fixture.0.join(name), &edited).unwrap();
        assert_eq!(
            json(&[
                "issue",
                "--project",
                "named:HTML import",
                "view",
                &number,
                "--json"
            ])["issue"]["body"],
            source
        );
        assert_eq!(
            json(&[
                "artifact",
                "--project",
                "named:HTML import",
                "edit",
                id,
                "--file",
                name,
                "--json"
            ])["artifact"]["body"],
            edited
        );
        assert_eq!(
            json(&[
                "issue",
                "--project",
                "named:HTML import",
                "edit",
                &number,
                "--file",
                name,
                "--json"
            ])["issue"]["body"],
            edited
        );
        assert_eq!(
            run(&["artifact", "--project", "named:HTML import", "export", id]),
            edited
        );
        assert!(hey_boss::markdown::render_fragment(&edited).contains("language-html-render"));
    }
}
