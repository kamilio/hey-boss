use base64::{Engine, engine::general_purpose::STANDARD};
use hey_boss::issues::{Request, Store};
use serde_json::{Value, json};

fn request(op: Value) -> Request {
    serde_json::from_value(json!({"version":1,"project":{"id":"named:Files","name":"Files"},
        "actor":{"id":"human:boss","kind":"human","session_id":null,"machine":"test","host":"test","pid":null,"process_start":null,"cwd":"/tmp","source":"test"},
        "operation":op,"request_id":null})).unwrap()
}
fn call(store: &mut Store, op: Value) -> Value {
    store.execute(&request(op)).unwrap()
}
fn files(store: &mut Store, op: Value) -> Value {
    call(store, json!({"action":"attachment","operation":op}))
}
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hey-boss-attachments-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn store(&self) -> Store {
        Store::open(&self.0.join("issues.db")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn files_are_disk_backed_project_scoped_and_survive_restart_for_all_targets() {
    let f = Fixture::new("targets");
    let mut s = f.store();
    call(
        &mut s,
        json!({"action":"create","title":"Issue","body":"","labels":[]}),
    );
    call(
        &mut s,
        json!({"action":"mindmap","operation":{"command":"add","title":"Topic","alias":"topic","body":"","kind":"text"}}),
    );
    let doc = call(
        &mut s,
        json!({"action":"artifact","operation":{"command":"create","title":"Doc","body":""}}),
    );
    let binary = vec![0, 255, 10, 13, 128, 42];
    for target in [
        json!({"kind":"issue","id":"1"}),
        json!({"kind":"node","id":"topic"}),
        json!({"kind":"artifact","id":doc["artifact"]["id"]}),
    ] {
        let mut req = request(
            json!({"action":"attachment","operation":{"command":"upload","target":target,"name":"design notes.bin","data":STANDARD.encode(&binary)}}),
        );
        req.request_id = Some(format!("upload-{}", target["kind"]));
        let upload = s.execute(&req).unwrap();
        assert_eq!(s.execute(&req).unwrap()["attachment"], upload["attachment"]);
        let id = upload["attachment"]["id"].as_str().unwrap();
        assert_eq!(upload["attachment"]["size"], binary.len());
        let listed = files(&mut s, json!({"command":"list","target":target}));
        assert_eq!(listed["attachments"].as_array().unwrap().len(), 1);
        drop(s);
        s = f.store();
        let downloaded = files(&mut s, json!({"command":"download","id":id}));
        assert_eq!(
            STANDARD
                .decode(downloaded["data"].as_str().unwrap())
                .unwrap(),
            binary
        );
        let mut other =
            request(json!({"action":"attachment","operation":{"command":"download","id":id}}));
        other.project_override = Some("Other".into());
        assert_eq!(s.execute(&other).unwrap_err().code, "not_found");
        files(&mut s, json!({"command":"remove","id":id}));
        assert!(
            files(&mut s, json!({"command":"list","target":target}))["attachments"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            s.execute(&request(
                json!({"action":"attachment","operation":{"command":"download","id":id}})
            ))
            .unwrap_err()
            .code,
            "not_found"
        );
    }
    let db = rusqlite::Connection::open(f.0.join("issues.db")).unwrap();
    let payload: String = db
        .query_row("SELECT payload FROM requests LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert!(
        !payload.contains("\"data\""),
        "file bytes must not be persisted in SQLite request history"
    );
    assert!(
        std::fs::read_dir(f.0.join("issues.attachments"))
            .unwrap()
            .next()
            .is_none(),
        "removal cleans disk contents"
    );
}

#[test]
fn uploads_reject_bad_names_missing_targets_invalid_bytes_and_oversized_files() {
    let f = Fixture::new("validation");
    let mut s = f.store();
    call(
        &mut s,
        json!({"action":"create","title":"Issue","body":"","labels":[]}),
    );
    for name in [
        "../escape",
        "/absolute",
        "back\\slash",
        "line\nbreak",
        ".",
        "..",
        "",
    ] {
        assert!(s.execute(&request(json!({"action":"attachment","operation":{"command":"upload","target":{"kind":"issue","id":"1"},"name":name,"data":""}}))).is_err(),"{name}");
    }
    for target in [
        json!({"kind":"issue","id":"999"}),
        json!({"kind":"node","id":"absent"}),
        json!({"kind":"artifact","id":"absent"}),
    ] {
        assert_eq!(s.execute(&request(json!({"action":"attachment","operation":{"command":"upload","target":target,"name":"file.txt","data":""}}))).unwrap_err().code,"not_found");
    }
    assert!(s.execute(&request(json!({"action":"attachment","operation":{"command":"upload","target":{"kind":"issue","id":"1"},"name":"file","data":"!"}}))).is_err());
    let oversized = STANDARD.encode(vec![0; 10 * 1024 * 1024 + 1]);
    assert!(s.execute(&request(json!({"action":"attachment","operation":{"command":"upload","target":{"kind":"issue","id":"1"},"name":"file","data":oversized}}))).is_err());
    assert_eq!(
        files(
            &mut s,
            json!({"command":"list","target":{"kind":"issue","id":"1"}})
        )["attachments"],
        json!([])
    );
}

#[test]
fn local_and_ssh_cli_materialize_bytes_without_overwriting_existing_files() {
    use std::{os::unix::fs::PermissionsExt, process::Command};
    let f = Fixture::new("cli");
    let binary = env!("CARGO_BIN_EXE_hey-boss");
    let cli = |args: &[&str], remote: bool| {
        let mut cmd = Command::new(binary);
        cmd.args(args)
            .env("HEY_BOSS_ISSUE_DB", f.0.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env("HEY_BOSS_ISSUE_PROJECT", "named:Files")
            .env("ATTACHMENT_TEST_BINARY", binary);
        if remote {
            cmd.env(
                "PATH",
                format!("{}:{}", f.0.display(), std::env::var("PATH").unwrap()),
            );
        }
        cmd.output().unwrap()
    };
    assert!(
        cli(
            &[
                "issue",
                "create",
                "--title",
                "Issue",
                "--agent",
                "human:boss"
            ],
            false
        )
        .status
        .success()
    );
    let original = f.0.join("binary ' notes.bin");
    let bytes = [0u8, 10, 128, 255, 42];
    std::fs::write(&original, bytes).unwrap();
    std::fs::write(
        f.0.join("ssh"),
        "#!/bin/sh\nexec \"$ATTACHMENT_TEST_BINARY\" issue rpc\n",
    )
    .unwrap();
    std::fs::set_permissions(f.0.join("ssh"), std::fs::Permissions::from_mode(0o700)).unwrap();
    let uploaded = cli(
        &[
            "attachment",
            "upload",
            original.to_str().unwrap(),
            "--issue",
            "1",
            "--host",
            "files.test",
            "--agent",
            "human:boss",
            "--request-id",
            "remote-upload",
            "--json",
        ],
        true,
    );
    assert!(
        uploaded.status.success(),
        "{}",
        String::from_utf8_lossy(&uploaded.stderr)
    );
    let value: Value = serde_json::from_slice(&uploaded.stdout).unwrap();
    let id = value["attachment"]["id"].as_str().unwrap();
    let dest = f.0.join("retrieved.bin");
    let downloaded = cli(
        &[
            "attachment",
            "download",
            id,
            "--host",
            "files.test",
            "--output",
            dest.to_str().unwrap(),
            "--json",
        ],
        true,
    );
    assert!(
        downloaded.status.success(),
        "{}",
        String::from_utf8_lossy(&downloaded.stderr)
    );
    assert_eq!(std::fs::read(&dest).unwrap(), bytes);
    assert!(!String::from_utf8_lossy(&downloaded.stdout).contains("\"data\""));
    let refused = cli(
        &[
            "attachment",
            "download",
            id,
            "--output",
            dest.to_str().unwrap(),
        ],
        false,
    );
    assert!(!refused.status.success());
    assert_eq!(std::fs::read(&dest).unwrap(), bytes);
    let default = cli(&["attachment", "download", id, "--json"], false);
    assert!(default.status.success());
    let downloaded: Value = serde_json::from_slice(&default.stdout).unwrap();
    let path = std::path::PathBuf::from(downloaded["path"].as_str().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    assert!(
        cli(
            &[
                "attachment",
                "remove",
                id,
                "--host",
                "files.test",
                "--agent",
                "human:boss"
            ],
            true
        )
        .status
        .success()
    );
}

#[test]
fn failed_metadata_write_cleans_disk_and_download_detects_tampering() {
    let f = Fixture::new("integrity");
    let mut s = f.store();
    call(
        &mut s,
        json!({"action":"create","title":"Issue","body":"","labels":[]}),
    );
    let db = rusqlite::Connection::open(f.0.join("issues.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_attachment BEFORE INSERT ON file_attachments BEGIN SELECT RAISE(ABORT,'test write failure'); END;").unwrap();
    let upload = json!({"action":"attachment","operation":{"command":"upload","target":{"kind":"issue","id":"1"},"name":"file.bin","data":"AA=="}});
    assert!(s.execute(&request(upload.clone())).is_err());
    assert!(
        std::fs::read_dir(f.0.join("issues.attachments"))
            .unwrap()
            .next()
            .is_none()
    );
    db.execute_batch("DROP TRIGGER reject_attachment;").unwrap();
    let value = call(&mut s, upload);
    let id = value["attachment"]["id"].as_str().unwrap();
    std::fs::write(f.0.join("issues.attachments").join(id), [1u8]).unwrap();
    assert_eq!(
        s.execute(&request(
            json!({"action":"attachment","operation":{"command":"download","id":id}})
        ))
        .unwrap_err()
        .code,
        "io_error"
    );
}

#[test]
fn companions_require_authority_and_symlink_stores_are_refused() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new("authority");
    let mut s = f.store();
    call(
        &mut s,
        json!({"action":"create","title":"Issue","body":"","labels":[]}),
    );
    let db = rusqlite::Connection::open(f.0.join("issues.db")).unwrap();
    let upload = json!({"action":"attachment","operation":{"command":"upload","target":{"kind":"issue","id":"1"},"name":"file.bin","data":"AA=="}});
    db.execute("UPDATE fleet_meta SET role='agent' WHERE id=1", [])
        .unwrap();
    assert!(
        s.execute(&request(upload.clone()))
            .unwrap_err()
            .message
            .contains("--host SUPERVISOR")
    );
    assert!(!f.0.join("issues.attachments").exists());
    db.execute("UPDATE fleet_meta SET role='controller' WHERE id=1", [])
        .unwrap();
    let outside = f.0.join("outside");
    std::fs::create_dir(&outside).unwrap();
    symlink(&outside, f.0.join("issues.attachments")).unwrap();
    assert!(s.execute(&request(upload)).is_err());
    assert!(std::fs::read_dir(outside).unwrap().next().is_none());
}

#[test]
fn transferring_an_issue_moves_file_access_to_the_destination_project() {
    let f = Fixture::new("transfer");
    let mut s = f.store();
    call(
        &mut s,
        json!({"action":"create","title":"Issue","body":"","labels":[]}),
    );
    let file = files(
        &mut s,
        json!({"command":"upload","target":{"kind":"issue","id":"1"},"name":"notes.txt","data":"aGVsbG8="}),
    );
    let id = file["attachment"]["id"].as_str().unwrap();
    let mut create = request(json!({"action":"create","title":"Existing","body":"","labels":[]}));
    create.project_override = Some("Destination".into());
    s.execute(&create).unwrap();
    call(
        &mut s,
        json!({"action":"transfer","number":1,"destination":"Destination","if_version":1}),
    );
    let mut list = request(
        json!({"action":"attachment","operation":{"command":"list","target":{"kind":"issue","id":"2"}}}),
    );
    list.project_override = Some("Destination".into());
    assert_eq!(s.execute(&list).unwrap()["attachments"][0]["id"], id);
    assert_eq!(
        s.execute(&request(
            json!({"action":"attachment","operation":{"command":"download","id":id}})
        ))
        .unwrap_err()
        .code,
        "not_found"
    );
    let mut download =
        request(json!({"action":"attachment","operation":{"command":"download","id":id}}));
    download.project_override = Some("Destination".into());
    assert_eq!(s.execute(&download).unwrap()["data"], "aGVsbG8=");
}
