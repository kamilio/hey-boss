use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[test]
fn every_web_page_advertises_the_same_cli_guide_without_resource_data() {
    let root = std::env::temp_dir().join(format!("hey-boss-guide-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args(["issue", "web", "--no-discovery", "--port", "0", "--json"])
        .env("HEY_BOSS_ISSUE_DB", root.join("issues.db"))
        .env("HEY_BOSS_INBOX_SOCKET", root.join("absent.sock"))
        .env("HEY_BOSS_FLEET_STATE", &root)
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .env_remove("HEY_BOSS_ISSUE_PROJECT")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // Always stop the server, including on failed assertions.
    struct Cleanup(std::process::Child, std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let _cleanup = Cleanup(child, root);
    let ready: serde_json::Value = serde_json::from_str(&line).unwrap();
    let base = ready["url"].as_str().unwrap().trim_end_matches('/');
    let client = reqwest::blocking::Client::new();
    for page in [
        "/",
        "/issues",
        "/artifacts",
        "/mm",
        "/workers",
        "/agents",
        "/agents/session",
    ] {
        let reply = client.get(format!("{base}{page}")).send().unwrap();
        assert_eq!(reply.status(), 200);
        let html = reply.text().unwrap();
        assert_eq!(
            html.matches("id=\"hey-boss-agent-guide\"").count(),
            1,
            "{page}"
        );
        assert!(html.contains("hidden"));
        assert!(html.contains("hey-boss lookup"));
        assert!(html.contains("href=\"/llms.txt\""));
        assert!(html.contains("src=\"/agent-guide.js\""));
        assert!(html.find("Agent guidance").unwrap() < html.find("</head>").unwrap());
    }
    let reply = client.get(format!("{base}/llms.txt")).send().unwrap();
    assert_eq!(reply.status(), 200);
    assert!(
        reply.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    assert_eq!(
        reply.text().unwrap(),
        include_str!("../src/issues/web/agent-guide.md")
    );
    let script = client.get(format!("{base}/agent-guide.js")).send().unwrap();
    assert_eq!(script.status(), 200);
    assert!(
        script.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("javascript")
    );
}
