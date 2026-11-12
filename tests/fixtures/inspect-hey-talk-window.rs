//! Run via packages/hey-talk/test-window-inspection.sh, which owns the named AppKit window.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{io::Write, os::unix::net::UnixStream, path::PathBuf, time::Duration};

fn inspect(target: &str, mode: &str) -> Value {
    let path = PathBuf::from(std::env::var_os("HOME").unwrap())
        .join("Library/Application Support/hey-talk/control.sock");
    let mut socket = UnixStream::connect(path).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    writeln!(
        socket,
        "{}",
        json!({"command":"inspect_window","target":target,"mode":mode})
    )
    .unwrap();
    serde_json::from_reader(std::io::BufReader::new(socket)).unwrap()
}

fn main() {
    let window = "WindowCaptureFixture";
    let ax = inspect(window, "accessibility");
    let check_text = |result: &Value| {
        assert_eq!(result["ok"], true, "{}", result["error"]);
        let text = result["text"].as_str().unwrap();
        assert!(text.contains("Marigold capture test"));
        assert!(!text.contains("must-not-appear"), "Captured a secure field");
    };
    check_text(&ax);
    assert!(ax.get("jpeg").is_none());
    assert_eq!(inspect("No Such Window Fixture", "both")["ok"], false);
    let image = inspect(window, "screenshot");
    let both = inspect(window, "both");
    check_text(&both);
    if image["ok"] == false {
        let reason = image["error"].as_str().unwrap();
        assert!(
            reason.contains("Allow Hey Talk") && reason.contains("Screen & System Audio Recording"),
            "{reason}"
        );
        assert!(both.get("jpeg").is_none());
        assert!(
            both["text"]
                .as_str()
                .unwrap()
                .contains(&format!("Screenshot unavailable: {reason}"))
        );
        println!("PASS: text remains available when Screen Recording is denied");
    } else {
        assert_eq!(image["ok"], true);
        assert!(image.get("text").is_none());
        for result in [image, both] {
            let jpeg = STANDARD.decode(result["jpeg"].as_str().unwrap()).unwrap();
            assert!(
                jpeg.starts_with(&[0xff, 0xd8]) && jpeg.ends_with(&[0xff, 0xd9]),
                "Incomplete JPEG"
            );
        }
        println!("PASS: named window, secure field omission, and JPEG with and without text");
    }
}
