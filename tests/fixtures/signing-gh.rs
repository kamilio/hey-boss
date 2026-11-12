//! Offline GitHub CLI responses for the signing and worker environment tests.
use serde_json::{Value, json};
use std::{env, fs, io::Write, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let root = PathBuf::from(env::var_os("HOME").unwrap());
    let args: Vec<_> = env::args().skip(1).collect();
    let endpoint = args
        .iter()
        .find(|s| s.as_str() == "user" || s.starts_with("user/") || s.starts_with("users/"))
        .map(String::as_str);
    writeln!(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("api-log"))
            .unwrap(),
        "{}",
        args.join(" ")
    )
    .unwrap();
    let failure = root.join("github-failure");
    let failure = if failure.exists() {
        Some(serde_json::from_slice::<Value>(&fs::read(failure).unwrap()).unwrap())
    } else if root.join("denied").exists() && endpoint == Some("user/ssh_signing_keys") {
        Some(json!({"status":403,"message":"Resource not accessible by personal access token"}))
    } else {
        None
    };
    if let Some(response) = failure {
        let status = response["status"].as_u64().unwrap();
        let message = response["message"].as_str().unwrap();
        let headers = response["headers"].as_str().unwrap_or("");
        print!("HTTP/2.0 {status}\r\n{headers}\r\n");
        println!("{}", json!({"message":message}));
        eprintln!("gh: {message} (HTTP {status})");
        return ExitCode::FAILURE;
    }
    let file = root.join("github-keys");
    let keys = || -> Vec<Value> {
        if file.exists() {
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap()
        } else {
            Vec::new()
        }
    };
    let body = match endpoint {
        Some("user") => json!({"id":42,"login":"octocat","name":"Octo Cat"}),
        Some("users/octocat/gpg_keys") => {
            json!([{"key_id":fs::read_to_string(root.join("gpg-key-id")).unwrap().trim(),"can_sign":true}])
        }
        Some("user/emails") => {
            json!([{"email":"unverified@example.com","primary":true,"verified":false}])
        }
        Some("user/ssh_signing_keys") if args.iter().any(|arg| arg == "POST") => {
            let key = json!({"key":args.iter().find_map(|s|s.strip_prefix("key=")).unwrap()});
            if !root.join("discard-registration").exists() {
                let mut saved = keys();
                saved.push(key.clone());
                fs::write(file, serde_json::to_vec(&saved).unwrap()).unwrap();
            }
            key
        }
        Some("users/octocat/ssh_signing_keys") => json!(keys()),
        _ => panic!("Unexpected gh call: {args:?}"),
    };
    if args.iter().any(|arg| arg == "--include") {
        print!("HTTP/2.0 200 OK\r\n\r\n");
    }
    println!("{body}");
    ExitCode::SUCCESS
}
