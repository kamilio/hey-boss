//! Clipboard transport tests use a private fake desktop, never the real clipboard.
#![cfg(unix)]

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static SERIAL: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-clipboard-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::hard_link(env!("CARGO_BIN_EXE_hey-boss"), root.join("hey-boss"))
            .or_else(|_| {
                std::fs::copy(env!("CARGO_BIN_EXE_hey-boss"), root.join("hey-boss")).map(|_| ())
            })
            .unwrap();
        std::fs::write(root.join("hey-boss.state"), root.to_str().unwrap()).unwrap();
        Self(root)
    }

    fn run(&self, action: &str, input: &[u8]) -> Output {
        let mut child = Command::new(self.0.join("hey-boss"))
            .args(["utils", action])
            .env_clear()
            .env("HOME", &self.0)
            .env("PATH", &self.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(input);
        child.wait_with_output().unwrap()
    }

    fn call(
        &self,
        action: &str,
        input: &[u8],
        reply: impl FnOnce(Value) -> Value + Send + 'static,
    ) -> Output {
        let listener = UnixListener::bind(self.0.join("daemon.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let peer = std::thread::spawn(move || {
            // Allow cold production CLI launches on heavily loaded CI hosts.
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "CLI never contacted desktop");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            let mut data = Vec::new();
            stream.read_to_end(&mut data).unwrap();
            let request: Value = serde_json::from_slice(&data).unwrap();
            assert_eq!(request["command"], "action");
            assert_eq!(request["sync"], false);
            let envelope: Value =
                serde_json::from_str(request["question"].as_str().unwrap()).unwrap();
            assert_eq!(envelope["version"], 1);
            assert!(!envelope["id"].as_str().unwrap().is_empty());
            let response = reply(envelope);
            stream
                .write_all(&serde_json::to_vec(&response).unwrap())
                .unwrap();
        });
        let output = self.run(action, input);
        // Fail immediately against an older CLI rather than waiting for an unused socket.
        if !String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand") {
            peer.join().unwrap();
        }
        output
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn success(request: &Value, result: Value) -> Value {
    json!({"task_id":request["id"],"status":"ok","result":json!({"version":1,"id":request["id"],"result":result}).to_string()})
}
fn ok(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
}
fn failed(output: &Output) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.starts_with(b"hey-boss: "));
}

#[test]
fn copy_aliases_send_exact_utf8_and_are_silent() {
    for alias in ["copy", "pbcopy"] {
        for text in ["", "a\r\n🌍 e\u{301}\n\0\t'\"\\\n"] {
            let output = Fixture::new().call(alias, text.as_bytes(), move |request| {
                assert_eq!(request["method"], "clipboard.copy");
                assert_eq!(request["params"], json!({"base64":STANDARD.encode(text)}));
                success(&request, json!({"copied":true}))
            });
            ok(&output);
            assert!(output.stdout.is_empty());
        }
    }
}

#[test]
fn paste_aliases_write_only_clipboard_text_without_an_added_newline() {
    for alias in ["paste", "pbpaste"] {
        for text in ["", "trailing spaces  ", "a\r\n🌍 e\u{301}\n\0\t'\"\\\n"] {
            let output = Fixture::new().call(alias, b"ignored stdin", move |request| {
                assert_eq!(request["method"], "clipboard.paste");
                assert_eq!(request["params"], json!({}));
                success(&request, json!({"base64":STANDARD.encode(text)}))
            });
            ok(&output);
            assert_eq!(output.stdout, text.as_bytes());
        }
    }
}

#[test]
fn clipboard_boundary_fits_the_existing_action_envelope() {
    let bytes = vec![0; 128 * 1024];
    let output = Fixture::new().call("copy", &bytes, |request| {
        assert!(request.to_string().len() <= 262144);
        assert_eq!(
            STANDARD
                .decode(request["params"]["base64"].as_str().unwrap())
                .unwrap(),
            vec![0; 128 * 1024]
        );
        success(&request, json!({"copied":true}))
    });
    ok(&output);
    let output = Fixture::new().call("paste", b"", |request| {
        success(
            &request,
            json!({"base64":STANDARD.encode(vec![0; 128 * 1024])}),
        )
    });
    ok(&output);
    assert_eq!(output.stdout, bytes);
}

#[test]
fn invalid_or_oversized_input_fails_before_contacting_desktop() {
    for (input, message) in [
        (vec![255], "UTF-8"),
        (vec![b'a'; 128 * 1024 + 1], "128 KiB"),
    ] {
        let output = Fixture::new().run("copy", &input);
        failed(&output);
        assert!(String::from_utf8_lossy(&output.stderr).contains(message));
    }
}

#[test]
fn disconnected_desktop_fails_without_a_local_clipboard_fallback() {
    for alias in ["copy", "paste"] {
        let fixture = Fixture::new();
        let output = fixture.run(alias, b"private test text");
        failed(&output);
        assert!(!fixture.0.join("queue").exists());
    }
}

#[test]
fn desktop_rejection_is_an_error_not_clipboard_output() {
    for alias in ["copy", "paste"] {
        for structured in [false, true] {
            let output = Fixture::new().call(alias, b"", move |request| {
                if structured {
                    json!({"task_id":request["id"],"status":"error","result":json!({"version":1,"id":request["id"],"error":{"message":"Synthetic desktop error"}}).to_string()})
                } else {
                    json!({"task_id":"action","status":"error","error":"Synthetic desktop error"})
                }
            });
            failed(&output);
            assert!(String::from_utf8_lossy(&output.stderr).contains("Synthetic desktop error"));
        }
    }
}

#[test]
fn malformed_paste_responses_never_reach_stdout() {
    for result in [
        json!({}),
        json!({"base64":42}),
        json!({"base64":"%%%"}),
        json!({"base64":STANDARD.encode([255])}),
        json!({"base64":STANDARD.encode(vec![b'a'; 128 * 1024 + 1])}),
    ] {
        let output = Fixture::new().call("paste", b"", move |request| success(&request, result));
        failed(&output);
    }
    for envelope in [
        json!({"version":2,"result":{"base64":""}}),
        json!({"version":1,"id":"wrong","result":{"base64":""}}),
    ] {
        let output = Fixture::new().call("paste", b"", move |request| {
            json!({"task_id":request["id"],"status":"ok","result":envelope.to_string()})
        });
        failed(&output);
    }
}

#[test]
fn copy_requires_explicit_success() {
    let output = Fixture::new().call("copy", b"text", |request| {
        success(&request, json!({"copied":false}))
    });
    failed(&output);
}
