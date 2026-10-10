use base64::{Engine, engine::general_purpose::STANDARD};
use clap::Subcommand;
use serde_json::json;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::process::Command;

// Intercept before clap so even --help and a leading -- belong to the script.
pub fn configured() -> Option<std::io::Result<()>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1)? != "utils" {
        return None;
    }
    let name = args.get(2)?.to_str()?;
    if name.starts_with('-') || name == "help" {
        return None;
    }
    let builtin = matches!(
        name,
        "gcn" | "gpn" | "copy" | "paste" | "pbcopy" | "pbpaste"
    );
    // Preserve installation-free built-ins when there is no fleet connection.
    let socket = match hey_boss::fleet::socket_path() {
        Ok(socket) => socket,
        Err(error) => return Some(Err(error)),
    };
    if builtin && !socket.exists() && !socket.with_file_name("fleet-authority.sock").exists() {
        return None;
    }
    let result = (|| -> std::io::Result<Option<hey_boss::utilities::Definition>> {
        let result = hey_boss::fleet::call(&json!({"kind":"utils_resolve","name":name}))
            .map_err(std::io::Error::other)?;
        if result["utility"].is_null() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(result["utility"].clone())?))
    })();
    match result {
        Ok(Some(definition)) => Some(run_configured(name, &definition, &args[3..])),
        Ok(None) if builtin => None,
        Ok(None) => Some(Err(std::io::Error::other(format!(
            "Unknown utility {name}; add it to utils in fleet.yaml"
        )))),
        Err(error) => Some(Err(error)),
    }
}

fn run_configured(
    name: &str,
    definition: &hey_boss::utilities::Definition,
    args: &[OsString],
) -> std::io::Result<()> {
    if definition.destination.is_none() {
        return Err(definition.launch(args)?.exec());
    }
    let mut input = Vec::new();
    if !std::io::stdin().is_terminal() {
        std::io::stdin()
            .lock()
            .take(hey_boss::utilities::LIMIT as u64 + 1)
            .read_to_end(&mut input)?;
    }
    if input.len() > hey_boss::utilities::LIMIT {
        return Err(std::io::Error::other("Utility stdin exceeds 1 MiB"));
    }
    let args: Vec<_> = args
        .iter()
        .map(|arg| STANDARD.encode(arg.as_bytes()))
        .collect();
    let call = |request| hey_boss::fleet::call(&request).map_err(std::io::Error::other);
    let started =
        call(json!({"kind":"utils_start","name":name,"args":args,"stdin":STANDARD.encode(input)}))?;
    let id = started["id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("Invalid utility run response"))?;
    let deadline = std::time::Instant::now()
        + hey_boss::utilities::TIMEOUT
        + std::time::Duration::from_secs(10);
    loop {
        let result = call(json!({"kind":"utils_poll","id":id}))?;
        if result["done"] == true {
            let stdout = hey_boss::utilities::decode(&result["stdout"])?;
            let stderr = hey_boss::utilities::decode(&result["stderr"])?;
            let code = result["code"]
                .as_i64()
                .filter(|code| (0..=255).contains(code))
                .ok_or_else(|| std::io::Error::other("Invalid utility exit status"))?;
            std::io::stdout().lock().write_all(&stdout)?;
            std::io::stderr().lock().write_all(&stderr)?;
            std::process::exit(code as i32);
        }
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::other(
                "Utility result timed out; execution was not retried",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[derive(Subcommand)]
pub enum Action {
    /// Run git commit --no-verify, forwarding all further arguments unchanged.
    Gcn,
    /// Run git push --no-verify, forwarding all further arguments unchanged.
    Gpn,
    /// Copy stdin (UTF-8, up to 128 KiB) to your connected main Mac's clipboard.
    #[command(visible_alias = "pbcopy")]
    Copy,
    /// Write your connected main Mac's clipboard to stdout, without an added newline.
    #[command(visible_alias = "pbpaste")]
    Paste,
}

pub fn cli_args() -> (Vec<OsString>, Vec<OsString>) {
    let mut args: Vec<_> = std::env::args_os().collect();
    // Keep Git's arguments out of clap: even an initial `--` or `--help` is literal.
    let git_args = match args.as_slice() {
        [_, group, action, ..] if group == "utils" && (action == "gcn" || action == "gpn") => {
            args.split_off(3)
        }
        _ => Vec::new(),
    };
    (args, git_args)
}

pub fn run(action: &Action, args: &[OsString]) -> std::io::Result<()> {
    let command = match action {
        Action::Gcn => "commit",
        Action::Gpn => "push",
        Action::Copy => return clipboard(true),
        Action::Paste => return clipboard(false),
    };
    Err(Command::new("git")
        .arg(command)
        .arg("--no-verify")
        .args(args)
        .exec())
}

const CLIPBOARD_LIMIT: usize = 128 * 1024;

fn clipboard(copy: bool) -> std::io::Result<()> {
    let params = if copy {
        let mut bytes = Vec::new();
        std::io::stdin()
            .lock()
            .take(CLIPBOARD_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        validate_text(&bytes)?;
        serde_json::json!({"base64": STANDARD.encode(bytes)})
    } else {
        serde_json::json!({})
    };
    // Use the installed desktop/companion socket, never this remote host's clipboard.
    // No setup, logging, disk queue, or retries: clipboard contents are ephemeral.
    let executable = std::env::current_exe()?.canonicalize()?;
    let state = std::fs::read_to_string(executable.with_file_name("hey-boss.state"))?;
    let client = hey_boss::Client::new(std::path::Path::new(&state).join("daemon.sock"));
    let id = format!(
        "clipboard-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let response = client.try_action(
        &id,
        if copy {
            "clipboard.copy"
        } else {
            "clipboard.paste"
        },
        params,
    )?;
    let envelope: serde_json::Value =
        serde_json::from_str(response.result.as_deref().unwrap_or("null"))
            .map_err(|_| invalid_response())?;
    if response.status.as_deref() != Some("ok") || envelope.get("error").is_some() {
        return Err(std::io::Error::other(
            envelope["error"]["message"]
                .as_str()
                .unwrap_or("Desktop clipboard request failed"),
        ));
    }
    if envelope["version"] != 1 || envelope["id"] != id {
        return Err(invalid_response());
    }
    if copy {
        if envelope["result"]["copied"] != true {
            return Err(invalid_response());
        }
    } else {
        let encoded = envelope["result"]["base64"]
            .as_str()
            .ok_or_else(invalid_response)?;
        if encoded.len() > CLIPBOARD_LIMIT.div_ceil(3) * 4 {
            return Err(invalid_response());
        }
        let bytes = STANDARD.decode(encoded).map_err(|_| invalid_response())?;
        validate_text(&bytes)?;
        std::io::stdout().lock().write_all(&bytes)?;
    }
    Ok(())
}

fn validate_text(bytes: &[u8]) -> std::io::Result<()> {
    if bytes.len() > CLIPBOARD_LIMIT {
        return Err(std::io::Error::other("Clipboard text exceeds 128 KiB"));
    }
    std::str::from_utf8(bytes)
        .map(|_| ())
        .map_err(|_| std::io::Error::other("Clipboard text must be valid UTF-8"))
}

fn invalid_response() -> std::io::Error {
    std::io::Error::other("Invalid desktop clipboard response")
}
