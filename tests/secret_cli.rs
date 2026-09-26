//! Synthetic credentials only. Assert that neither errors nor child output expose them.
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
static SERIAL: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    binary: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-secret-e2e-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let binary = root.join("hey-boss");
        fs::copy(env!("CARGO_BIN_EXE_hey-boss"), &binary).unwrap();
        fs::write(root.join("hey-boss.state"), root.to_str().unwrap()).unwrap();
        Self { root, binary }
    }
    fn reply(&self, values: Option<Vec<String>>) -> std::thread::JoinHandle<()> {
        let reply = match values {
            Some(values) => {
                serde_json::json!({"status":"ok","result":serde_json::to_string(&values).unwrap()})
            }
            None => serde_json::json!({"status":"cancelled"}),
        };
        self.raw_reply(serde_json::to_vec(&reply).unwrap())
    }
    fn raw_reply(&self, reply: Vec<u8>) -> std::thread::JoinHandle<()> {
        let listener = UnixListener::bind(self.root.join("daemon.sock")).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            stream.read_to_end(&mut request).unwrap();
            let request: serde_json::Value = serde_json::from_slice(&request).unwrap();
            assert_eq!(request["command"], "secret");
            assert_eq!(request["sync"], true);
            stream.write_all(&reply).unwrap();
        })
    }
    fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command.current_dir(&self.root);
        command.arg("secret");
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn file_sink_and_long_pair_report_delivery_without_secret_output() {
    let fixture = Fixture::new();
    let long = "synthetic_api_".repeat(2000);
    let server = fixture.reply(Some(vec!["synthetic-login".into(), long.clone()]));
    let output = fixture
        .command()
        .args([
            "--field",
            "LOGIN",
            "--field",
            "PASSWORD",
            "--login",
            "--env-file",
            ".env",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Secret delivery=delivered: credentials saved.\n"
    );
    server.join().unwrap();
    let saved = fs::read_to_string(fixture.root.join(".env")).unwrap();
    assert!(saved.contains(&long));
    assert!(saved.contains("LOGIN='synthetic-login'"));
    assert!(!fixture.root.join("history.db").exists());
}
#[test]
fn stdout_refuses_pipes_and_child_output_is_suppressed() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["--field", "KEY", "--stdout"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let server = fixture.reply(Some(vec!["synthetic_must_not_reach_output".into()]));
    let output=fixture.command().args(["--field","KEY","--","sh","-c","printf '%s' \"$KEY\"; printf '%s' \"$KEY\" >&2; test \"$KEY\" = synthetic_must_not_reach_output"]).output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Secret delivery=delivered: child process succeeded (output suppressed).\n"
    );
    server.join().unwrap();
}
#[test]
fn redirected_private_file_and_cancellation() {
    let fixture = Fixture::new();
    let server = fixture.reply(Some(vec!["synthetic_direct".into()]));
    let file = fs::File::create(fixture.root.join(".env.direct")).unwrap();
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .unwrap();
    let output = fixture
        .command()
        .args(["--field", "KEY", "--stdout"])
        .stdout(Stdio::from(file))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Secret delivery=delivered: credentials saved.\n"
    );
    server.join().unwrap();
    assert!(
        fs::read_to_string(fixture.root.join(".env.direct"))
            .unwrap()
            .contains("synthetic_direct")
    );
    fs::remove_file(fixture.root.join("daemon.sock")).unwrap();
    let server = fixture.reply(None);
    let output = fixture
        .command()
        .args(["--field", "KEY", "--env-file", ".env.cancelled"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    server.join().unwrap();
    assert!(!fixture.root.join(".env.cancelled").exists());
}

#[test]
fn response_failures_report_no_delivery_without_reading_the_destination() {
    for (response, reason) in [
        ("", "response_empty"),
        ("synthetic_must_not_reach_output", "response_malformed"),
        (
            r#"{"status":"ok","result":"synthetic_must_not_reach_output"}"#,
            "response_malformed",
        ),
        (r#"{"status":"ok"}"#, "response_missing"),
        (
            r#"{"status":"cancelled","result":{"private":"synthetic_must_not_reach_output"}}"#,
            "cancelled",
        ),
        (r#"{"status":"expired"}"#, "expired"),
        (r#"{"status":"rejected"}"#, "rejected"),
        (r#"{"status":"busy"}"#, "busy"),
        (
            r#"{"status":"error","error":"synthetic_must_not_reach_output"}"#,
            "unavailable",
        ),
        (
            r#"{"status":"synthetic_must_not_reach_output"}"#,
            "response_malformed",
        ),
        (r#"{"status":"ok","result":"[]"}"#, "values_invalid"),
    ] {
        let fixture = Fixture::new();
        let server = fixture.raw_reply(response.as_bytes().to_vec());
        let output = fixture
            .command()
            .args(["--field", "KEY", "--env-file", ".env"])
            .output()
            .unwrap();
        server.join().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let message = String::from_utf8(output.stderr).unwrap();
        assert!(
            message.contains(&format!("Secret delivery=not_delivered reason={reason}:")),
            "{reason}: {message}"
        );
        assert!(!message.contains("synthetic_must_not_reach_output"));
        assert!(!fixture.root.join(".env").exists());
        assert!(!fixture.root.join("history.db").exists());
    }
}

#[test]
fn child_failure_is_not_reported_as_failed_credential_delivery() {
    for (child, delivery, reason) in [
        (
            vec!["sh", "-c", "printf '%s' \"$KEY\" >&2; exit 7"],
            "delivered",
            "child_failed",
        ),
        (
            vec!["./nonexistent-child"],
            "not_delivered",
            "child_start_failed",
        ),
    ] {
        let fixture = Fixture::new();
        let server = fixture.reply(Some(vec!["synthetic_must_not_reach_output".into()]));
        let output = fixture
            .command()
            .args(["--field", "KEY", "--"])
            .args(child)
            .output()
            .unwrap();
        server.join().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let message = String::from_utf8(output.stderr).unwrap();
        assert!(
            message.contains(&format!("Secret delivery={delivery} reason={reason}:")),
            "{message}"
        );
        assert!(!message.contains("synthetic_must_not_reach_output"));
    }
}

#[test]
fn failed_redirected_output_has_unknown_delivery_and_no_secret_in_diagnostics() {
    let fixture = Fixture::new();
    let path = fixture.root.join(".env.redirected");
    let file = fs::File::create(&path).unwrap();
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .unwrap();
    drop(file);
    let server = fixture.reply(Some(vec!["synthetic_must_not_reach_output".into()]));
    let output = fixture
        .command()
        .args(["--field", "KEY", "--stdout"])
        .stdout(Stdio::from(fs::File::open(path).unwrap()))
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(!output.status.success());
    let message = String::from_utf8(output.stderr).unwrap();
    assert!(message.contains("Secret delivery=unknown reason=destination_failed:"));
    assert!(!message.contains("synthetic_must_not_reach_output"));
}
