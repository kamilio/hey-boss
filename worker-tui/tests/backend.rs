#![cfg(unix)]
use hey_boss_worker_tui::backend::{Client, Request};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

fn client(script: &str, label: &str) -> Client {
    let path = std::env::temp_dir().join(format!("hey-boss-tui-{}-{label}", std::process::id()));
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    Client {
        binary: path,
        host: None,
        directory: None,
        timeout: Duration::from_secs(3),
    }
}

#[test]
fn rejects_invalid_payloads_and_bounds_hung_commands() {
    let cancelled = Arc::new(AtomicBool::new(false));
    for (label, script, expected) in [
        ("json", "printf 'bad'", "Invalid worker status"),
        ("schema", "printf '{\"ok\":true}'", "missing workers/runs"),
        ("rejected", "printf '{\"ok\":false}'", "rejected"),
        ("exit", "echo offline >&2; exit 4", "offline"),
        ("timeout", "exec sleep 10", "timed out"),
        (
            "inherited",
            "sleep 10 &\nprintf '{\"ok\":true,\"workers\":[],\"runs\":[]}'",
            "timed out",
        ),
    ] {
        let client = client(script, label);
        let start = Instant::now();
        let error = client
            .execute(&Request::Refresh(None), &cancelled)
            .unwrap_err();
        assert!(error.contains(expected), "{error}");
        assert!(start.elapsed() < Duration::from_secs(5));
        std::fs::remove_file(client.binary).unwrap();
    }
}

#[test]
fn passes_worker_id_as_literal_argument_and_cancels_promptly() {
    let client = client(
        "test \"$1\" = worker && test \"$2\" = --json && test \"$3\" = --id && test \"$4\" = 'a; echo injected' && test \"$5\" = status || exit 2\nprintf '{\"ok\":true,\"workers\":[],\"runs\":[]}'",
        "args",
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let result = client.execute(
        &Request::Refresh(Some("a; echo injected".into())),
        &cancelled,
    );
    assert!(result.is_ok(), "{result:?}");
    cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(
        client
            .execute(&Request::Refresh(None), &cancelled)
            .unwrap_err()
            .contains("Cancelled")
    );
    std::fs::remove_file(client.binary).unwrap();
}
