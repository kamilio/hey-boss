//! Read-only CLI authentication checks. Never return credentials or raw auth output.
use serde_json::{Value, json};
use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn output(program: &str, args: &[&str]) -> Option<(bool, String)> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.take(65536).read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    Some((status.is_some_and(|s| s.success()), reader.join().ok()?))
}
pub fn inspect() -> Value {
    let evaluator = output("agent-eval", &["--version"])
        .filter(|v| v.0)
        .map(|v| v.1.trim().to_owned());
    // A gateway-backed Codex run does not use the CLI's own login. Check source
    // availability without exposing values; model authorization remains untested.
    let proxy = output("hey-proxy", &["check-credentials"]).is_some_and(|v| v.0);
    let mut harnesses = serde_json::Map::new();
    for harness in ["codex", "claude", "gemini"] {
        let version = output(harness, &["--version"])
            .filter(|v| v.0)
            .map(|v| v.1.trim().to_owned());
        let login = if version.is_none() {
            "missing"
        } else {
            match harness {
                "claude" => match output(harness, &["auth", "status", "--json"]) {
                    Some((true, text))
                        if serde_json::from_str::<Value>(&text)
                            .ok()
                            .is_some_and(|v| v["loggedIn"] == true) =>
                    {
                        "logged_in"
                    }
                    Some(_) => "not_logged_in",
                    None => "unknown",
                },
                "codex" => match output(harness, &["login", "status"]) {
                    Some((true, _)) => "logged_in",
                    Some(_) => "not_logged_in",
                    None => "unknown",
                },
                _ => "unknown",
            }
        };
        let credentials = if login == "logged_in" {
            Some("cli_login")
        } else if harness == "codex"
            && std::env::var_os("OPENAI_API_KEY").is_some_and(|v| !v.is_empty())
        {
            Some("environment")
        } else if harness == "codex" && proxy {
            Some("proxy_sources")
        } else {
            None
        };
        harnesses.insert(harness.into(),json!({"version":version,"login":login,"credentials":credentials,"ready":evaluator.is_some() && version.is_some() && credentials.is_some()}));
    }
    json!({"evaluator":evaluator,"harnesses":harnesses,"checked_at":jiff::Timestamp::now().to_string(),"model_access":"not_tested"})
}
