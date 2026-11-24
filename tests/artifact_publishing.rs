#[path = "support/projects.rs"]
mod projects;
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn configured_artifact_publishers_track_manual_automatic_failed_and_uncertain_uploads() {
    let dir = tempfile::Builder::new()
        .prefix(".artifact-publishing-test-")
        .tempdir_in(std::env::current_dir().unwrap())
        .unwrap();
    let root = dir.path();
    let database = root.join("issues.db");
    let _owner = hey_boss::database::Owner::host(&database).unwrap();
    projects::seed(&database, &["named:Publishing"]);
    let script = root.join("provider.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
set -eu
root=$(dirname "$0")
printf '%s\n' "$1" >> "$root/calls"
if test "$1" != read; then test -f "$2"; cp "$2" "$root/upload.md"; fi
if test -f "$root/fail"; then echo 'Notion unavailable' >&2; exit 1; fi
printf '{"id":"remote","url":"https://example.com/document","version":"v1"}'
"#,
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let provider = json!({"command":script,"audience":"private","content":"artifact","auto_sync":false,
        "create":["create","{source}"],"read":["read","{id}"],"update":["update","{source}","{id}","{version}"],
        "fields":{"id":"/id","url":"/url","version":"/version"}});
    let mut page = provider.clone();
    page["content"] = json!("page");
    page.as_object_mut().unwrap().remove("auto_sync");
    fs::write(
        root.join("publishing.yaml"),
        serde_json::to_vec(&json!({"notion":provider,"pages":page})).unwrap(),
    )
    .unwrap();
    let call = |operation: Value| {
        use std::io::Write;
        let mut child = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args([
                "artifact",
                "--project",
                "named:Publishing",
                "--agent",
                "human:boss",
                "--json",
                "rpc",
            ])
            .env("HEY_BOSS_ISSUE_DB", &database)
            .env("HEY_BOSS_FLEET_STATE", root)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(operation.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|_| panic!("{stdout}\n{}", String::from_utf8_lossy(&output.stderr)));
        value
    };
    let run = |op: Value| {
        let value = call(op);
        assert_eq!(value["ok"], true, "{value}");
        value
    };
    let providers = run(json!({"command":"providers"}));
    assert_eq!(providers["providers"].as_array().unwrap().len(), 1);
    assert_eq!(providers["providers"][0]["auto_sync"], false);
    let created = run(json!({"command":"create","title":"Plan","body":"First revision"}));
    let id = created["artifact"]["id"].as_str().unwrap();
    assert_eq!(
        run(json!({"command":"share","id":id,"provider":"notion"}))["artifact"]["publications"][0]
            ["auto_sync"],
        false
    );
    assert_eq!(
        fs::read_to_string(root.join("upload.md")).unwrap(),
        "First revision"
    );
    let edited = run(json!({"command":"edit","id":id,"body":"Second revision","if_version":1}));
    assert_eq!(edited["artifact"]["publications"][0]["synced"], false);
    assert_eq!(fs::read_to_string(root.join("calls")).unwrap(), "create\n");
    assert_eq!(
        run(json!({"command":"sync","id":id}))["artifact"]["publications"][0]["synced"],
        true
    );
    run(json!({"command":"share","id":id,"provider":"notion","auto_sync":true}));
    let automatic = run(json!({"command":"edit","id":id,"title":"Renamed","if_version":2}));
    assert_eq!(automatic["artifact"]["publications"][0]["synced"], true);
    assert_eq!(
        fs::read_to_string(root.join("calls")).unwrap(),
        "create\nupdate\nupdate\n"
    );
    run(json!({"command":"edit","id":id,"title":"Renamed","if_version":3}));
    run(json!({"command":"comment","id":id,"body":"Comment only"}));
    assert_eq!(
        fs::read_to_string(root.join("calls")).unwrap(),
        "create\nupdate\nupdate\n"
    );
    fs::write(root.join("fail"), "").unwrap();
    let failed = run(
        json!({"command":"edit","id":id,"body":"Saved even when remote is down","if_version":4}),
    );
    assert_eq!(failed["artifact"]["body"], "Saved even when remote is down");
    assert!(
        failed["artifact"]["publications"][0]["error"]
            .as_str()
            .unwrap()
            .contains("Notion unavailable")
    );
    assert_eq!(
        failed["artifact"]["publications"][0]["url"],
        "https://example.com/document"
    );
    fs::remove_file(root.join("fail")).unwrap();
    assert_eq!(
        run(json!({"command":"sync","id":id}))["artifact"]["publications"][0]["synced"],
        true
    );
    let other = run(json!({"command":"create","title":"Other","body":"Another page"}));
    let other = &other["artifact"]["id"];
    fs::write(root.join("fail"), "").unwrap();
    assert_eq!(
        call(json!({"command":"share","id":other,"provider":"notion"}))["ok"],
        false
    );
    fs::remove_file(root.join("fail")).unwrap();
    let before = fs::read_to_string(root.join("calls")).unwrap();
    assert_eq!(
        call(json!({"command":"share","id":other,"provider":"notion"}))["ok"],
        false
    );
    assert_eq!(fs::read_to_string(root.join("calls")).unwrap(), before);
    assert_eq!(
        run(
            json!({"command":"share","id":other,"provider":"notion","link":"https://example.com/document"})
        )["artifact"]["publications"][0]["synced"],
        true
    );
    let private = run(
        json!({"command":"create","title":"Private","body":"![Private](/attachments/f-secret)"}),
    );
    assert_eq!(
        call(json!({"command":"share","id":private["artifact"]["id"],"provider":"notion"}))["ok"],
        false
    );
    run(json!({"command":"delete","id":id,"if_version":5}));
    let db = rusqlite::Connection::open(database).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM artifact_exports WHERE artifact_id=?1",
            [id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
