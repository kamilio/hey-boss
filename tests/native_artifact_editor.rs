use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};

#[test]
fn native_open_routes_project_and_host_without_mutating_the_artifact() {
    let root = std::env::temp_dir().join(format!("hb-native-open-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let owner = hey_boss::database::Owner::host(&root.join("issues.db")).unwrap();
    let socket = root.join("desktop.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let receiver = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let payload: Value = serde_json::from_slice(&bytes).unwrap();
        stream
            .write_all(br#"{"status":"ok","result":"{\"ok\":true}"}"#)
            .unwrap();
        payload
    });
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args([
            "artifact",
            "--project",
            "named:Focus",
            "--host",
            "devbox",
            "edit",
            "doc-1",
            "--json",
        ])
        .env("HEY_BOSS_INBOX_SOCKET", &socket)
        .env("HEY_BOSS_ISSUE_DB", root.join("issues.db"))
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = receiver.join().unwrap();
    assert_eq!(payload["command"], "artifact_editor");
    let context: Value = serde_json::from_str(payload["question"].as_str().unwrap()).unwrap();
    assert_eq!(context["id"], "doc-1");
    assert_eq!(context["project"], "named:Focus");
    assert_eq!(context["host"], "devbox");
    assert!(context["body"].is_null());
    drop(owner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn editor_rpc_preserves_markdown_and_deduplicates_uncertain_saves() {
    let root = std::env::temp_dir().join(format!("hb-editor-rpc-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let owner = hey_boss::database::Owner::host(&root.join("issues.db")).unwrap();
    let rpc = |request_id: &str, operation: Value| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args([
                "artifact",
                "--project",
                "named:Focus",
                "--agent",
                "human:boss",
                "--request-id",
                request_id,
                "--json",
                "rpc",
            ])
            .env("HEY_BOSS_ISSUE_DB", root.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(operation.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let create = json!({"command":"create","title":"Focus","body":"# 🦀\n\n[Local reference](/tmp/nonexistent.md)"});
    let first = rpc("create-focus", create.clone());
    let retry = rpc("create-focus", create);
    assert_eq!(first["artifact"]["id"], retry["artifact"]["id"]);
    let edit = json!({"command":"edit","id":first["artifact"]["id"],"title":"Focus","body":"Auto-saved 🦀","if_version":1});
    let saved = rpc("save-focus", edit.clone());
    let retried = rpc("save-focus", edit);
    assert_eq!(saved["artifact"]["version"], 2);
    assert_eq!(retried["artifact"]["version"], 2);
    assert_eq!(retried["artifact"]["body"], "Auto-saved 🦀");
    drop(owner);
    std::fs::remove_dir_all(root).unwrap();
}
