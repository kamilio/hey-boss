//! Takeover runs on the device that owns the selected worker process.
use super::{Result, context::Context, replica::invalid};
use crate::issues::Store;
use serde_json::{Value, json};

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
pub(super) fn command(host: &str, directory: &str, session: &str) -> Result<String> {
    provider_command(host, directory, session, "codex", None)
}
fn provider_command(
    host: &str,
    directory: &str,
    session: &str,
    provider: &str,
    path: Option<&str>,
) -> Result<String> {
    if !directory.starts_with('/')
        || directory.contains(['\n', '\r', '\0'])
        || session.len() != 36
        || !session.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
    {
        return Err(invalid("Resume details are not available for this session"));
    }
    let resume = match provider {
        "codex" => format!(
            "codex resume {} {}",
            crate::codex_permissions::INTERACTIVE_FLAG,
            quote(session)
        ),
        "claude" => format!("claude --resume {}", quote(session)),
        "pi" => {
            let path = path
                .filter(|p| p.starts_with('/') && !p.contains(['\n', '\r', '\0']))
                .ok_or_else(|| invalid("Pi resume requires its saved session file"))?;
            format!("pi --session {}", quote(path))
        }
        _ => return Err(invalid("Unknown agent provider")),
    };
    let local = format!("cd {} && {resume}", quote(directory));
    if host == "local" {
        return Ok(local);
    }
    if !crate::health::remote::valid_host(host) {
        return Err(invalid("Invalid device"));
    }
    // SSH's noninteractive environment may omit npm/nvm's Codex installation.
    let remote = format!(
        "cd {} && exec \"${{SHELL:-/bin/sh}}\" -lic {}",
        quote(directory),
        quote(&resume)
    );
    Ok(format!("ssh -t {} {}", quote(host), quote(&remote)))
}
pub(super) fn saved_command(host: &str, saved: &Value) -> Result<String> {
    let provider = saved["provider"].as_str().unwrap_or("codex");
    let session = saved["session_id"].as_str().unwrap_or("");
    if provider != "codex"
        && (saved["session_ref"]["provider"] != provider || saved["session_ref"]["id"] != session)
    {
        return Err(invalid(
            "Agent resume identity does not match its saved reference",
        ));
    }
    if provider == "codex" {
        return command(host, saved["directory"].as_str().unwrap_or(""), session);
    }
    provider_command(
        host,
        saved["directory"].as_str().unwrap_or(""),
        session,
        provider,
        saved["session_ref"]["path"].as_str(),
    )
}
pub(super) fn steer(ctx: &Context, request: &Value) -> Result<Value> {
    Ok(Store::open(&ctx.path)?.worker_steer(request["run"].as_str().unwrap_or(""), request)?)
}

pub(super) fn apply(ctx: &Context, run: &str) -> Result<Value> {
    let boss = crate::issues::identity::resolve(Some("human:boss"), &ctx.node, &ctx.home)?;
    let mut result = Store::open(&ctx.path)?.worker_takeover(run, &boss)?;
    // Never offer a resume command while the owning worker is still stopping.
    if result["stopped"] == true {
        result["resume_command"] = match saved_command("local", &result) {
            Ok(command) => json!(command),
            Err(_) => Value::Null,
        };
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SESSION: &str = "01234567-89ab-cdef-0123-456789abcdef";
    #[test]
    fn resumes_each_provider_with_its_exact_reference() {
        let mut saved = json!({"directory":"/work","session_id":SESSION,"provider":"claude","session_ref":{"provider":"claude","id":SESSION}});
        assert!(
            saved_command("local", &saved)
                .unwrap()
                .contains("claude --resume")
        );
        saved["provider"] = json!("pi");
        assert!(saved_command("local", &saved).is_err());
        saved["session_ref"] =
            json!({"provider":"pi","id":SESSION,"path":"/work/session's $(no).jsonl"});
        let command = saved_command("devbox", &saved).unwrap();
        assert!(command.contains("pi --session"));
        assert!(command.contains("ssh -t"));
        saved["session_ref"]["path"] = json!("relative.jsonl");
        assert!(saved_command("local", &saved).is_err());
    }
    #[test]
    fn resume_commands_quote_both_shells_and_reject_invalid_metadata() {
        let directory = "/work/Boss's repo $HOME `touch nope`";
        let remote = command("devbox", directory, SESSION).unwrap();
        let output = std::process::Command::new("sh")
            .args([
                "-c",
                &format!("ssh() {{ printf '%s\\n' \"$3\"; }}; {remote}"),
            ])
            .output()
            .unwrap();
        let script = String::from_utf8(output.stdout).unwrap();
        assert!(script.contains("exec \"${SHELL:-/bin/sh}\" -lic"));
        // Decode the remote shell too, without executing a real resume command.
        let script = script.replace("exec \"${SHELL:-/bin/sh}\" -lic", "capture");
        let script = script.replace(&format!("cd {}", quote(directory)), "true");
        let decoded = std::process::Command::new("sh")
            .args([
                "-c",
                &format!("capture() {{ printf '%s\\n' \"$1\"; }}; {script}"),
            ])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(decoded.stdout).unwrap().trim(),
            format!("codex resume --approve-for-me {}", quote(SESSION))
        );
        assert!(
            !command("local", directory, SESSION)
                .unwrap()
                .contains("ssh")
        );
        assert!(
            command("local", directory, SESSION)
                .unwrap()
                .contains("--approve-for-me")
        );
        for (host, dir, session) in [
            ("-bad", "/repo", SESSION),
            ("local", "relative", SESSION),
            ("local", "/repo\nnope", SESSION),
            ("local", "/repo", "invalid"),
        ] {
            assert!(command(host, dir, session).is_err());
        }
    }
}
