#![cfg(unix)]
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::net::UnixListener,
    process::{Command, Stdio},
};

#[test]
fn configured_destination_forwards_binary_stdin_arguments_output_and_exit_code() {
    let root = std::env::temp_dir().join(format!("hb-utils-config-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let listener = UnixListener::bind(root.join("fleet.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        for index in 0..3 {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut stream = loop {
                if let Ok((stream, _)) = listener.accept() {
                    break stream;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "CLI did not send request {index}"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            stream.set_nonblocking(false).unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            let response = match index {
                0 => {
                    assert_eq!(request, json!({"kind":"utils_resolve","name":"script"}));
                    json!({"ok":true,"utility":{"command":"/my/script","destination":"macbook"}})
                }
                1 => {
                    assert_eq!(request["kind"], "utils_start");
                    assert_eq!(request["name"], "script");
                    let args: Vec<Vec<u8>> = request["args"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| STANDARD.decode(v.as_str().unwrap()).unwrap())
                        .collect();
                    assert_eq!(
                        args,
                        [
                            b"--help".to_vec(),
                            b"".to_vec(),
                            b"two words".to_vec(),
                            b"$(touch injected)".to_vec(),
                            b"--".to_vec()
                        ]
                    );
                    assert_eq!(
                        STANDARD.decode(request["stdin"].as_str().unwrap()).unwrap(),
                        b"input\0\xff"
                    );
                    json!({"ok":true,"id":"run"})
                }
                _ => {
                    assert_eq!(request, json!({"kind":"utils_poll","id":"run"}));
                    json!({"ok":true,"done":true,"stdout":STANDARD.encode(b"out\0\xff"),"stderr":STANDARD.encode(b"err\n"),"code":37})
                }
            };
            stream.write_all(response.to_string().as_bytes()).unwrap();
        }
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args([
            "utils",
            "script",
            "--help",
            "",
            "two words",
            "$(touch injected)",
            "--",
        ])
        .env("HEY_BOSS_FLEET_STATE", &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"input\0\xff")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(
        output.status.code(),
        Some(37),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"out\0\xff");
    assert_eq!(output.stderr, b"err\n");
}

#[test]
fn configured_builtin_override_runs_locally_with_fixed_and_passthrough_arguments() {
    use std::os::unix::ffi::OsStringExt;
    let root = std::env::temp_dir().join(format!("hb-utils-local-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let listener = UnixListener::bind(root.join("fleet.sock")).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(request["name"], "pbcopy");
        let response = json!({"ok":true,"utility":{"command":"printf '%s\\0' --from noreply@poe.com --from poe-no-reply@quora.com"}});
        stream.write_all(response.to_string().as_bytes()).unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args([
            "utils",
            "pbcopy",
            "--from",
            "extra@example.com",
            "--max-emails",
            "2",
            "",
            "--help",
        ])
        .arg(std::ffi::OsString::from_vec(b"\xff".to_vec()))
        .env("HEY_BOSS_FLEET_STATE", &root)
        .output()
        .unwrap();
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"--from\0noreply@poe.com\0--from\0poe-no-reply@quora.com\0--from\0extra@example.com\0--max-emails\x002\0\0--help\0\xff\0");
}
