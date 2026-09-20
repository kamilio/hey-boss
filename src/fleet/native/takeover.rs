//! Takeover runs on the device that owns the selected worker process.
use super::{Result, context::Context, replica::invalid};
use crate::issues::Store;
use serde_json::{Value, json};

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
pub(super) fn command(host: &str, directory: &str, session: &str) -> Result<String> {
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
    let local = format!("cd {} && codex resume {}", quote(directory), quote(session));
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
        quote(&format!("codex resume {}", quote(session)))
    );
    Ok(format!("ssh -t {} {}", quote(host), quote(&remote)))
}
pub(super) fn apply(ctx: &Context, run: &str) -> Result<Value> {
    let boss = crate::issues::identity::resolve(Some("human:boss"), &ctx.node, &ctx.home)?;
    let mut result = Store::open(&ctx.path)?.worker_takeover(run, &boss)?;
    // Never offer a resume command while the owning worker is still stopping.
    if result["stopped"] == true {
        result["resume_command"] = match command(
            "local",
            result["directory"].as_str().unwrap_or(""),
            result["session_id"].as_str().unwrap_or(""),
        ) {
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
            format!("codex resume {}", quote(SESSION))
        );
        assert!(
            !command("local", directory, SESSION)
                .unwrap()
                .contains("ssh")
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
