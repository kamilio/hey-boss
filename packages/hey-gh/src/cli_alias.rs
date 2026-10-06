//! Inspect gh aliases for auth selection; gh still performs the actual expansion.
use crate::cli_route::{Auth, Invocation};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    process::{Command, Stdio},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn resolve(invocation: &Invocation) -> Result<Option<Invocation>> {
    if invocation.auth == Auth::User || invocation.help() || builtin(invocation.root()) {
        return Ok(None);
    }
    // Local config only; never contact GitHub or consume the caller's stdin.
    let output = Command::new("gh")
        .args(["alias", "list"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let aliases: BTreeMap<String, String> = match serde_yaml::from_slice(&output.stdout) {
        Ok(aliases) => aliases,
        Err(_) => {
            return Err(
                "cannot inspect gh aliases; select --auth app or --auth user explicitly".into(),
            );
        }
    };
    let mut args = invocation.args.clone();
    for _ in 0..16 {
        let mut current = Invocation::parse(args)?;
        current.auth = invocation.auth;
        let Some(expansion) = aliases.get(current.root()) else {
            return Ok(Some(current));
        };
        if expansion.starts_with('!') {
            if invocation.auth == Auth::App {
                return Ok(None);
            }
            return Err("shell aliases require explicit --auth app or --auth user; their GitHub operations cannot be inferred".into());
        }
        let words =
            shlex::split(expansion).ok_or("invalid gh alias; select authentication explicitly")?;
        let trailing = &current.args[current.command + 1..];
        let mut consumed = 0;
        let mut expanded = Vec::new();
        for word in words {
            let mut word = word;
            // gh's numbered aliases consume the referenced positional args.
            for number in (1..=trailing.len()).rev() {
                let marker = format!("${number}");
                if word.contains(&marker) {
                    let value = trailing[number - 1]
                        .to_str()
                        .ok_or("non-UTF-8 alias parameter requires explicit --auth")?;
                    word = word.replace(&marker, value);
                    consumed = consumed.max(number);
                }
            }
            expanded.push(OsString::from(word));
        }
        args = current.args[..current.command].to_vec();
        args.extend(expanded);
        args.extend_from_slice(&trailing[consumed..]);
    }
    Err("gh alias expansion is recursive; select authentication explicitly".into())
}

fn builtin(root: &str) -> bool {
    matches!(
        root,
        "" | "help"
            | "auth"
            | "browse"
            | "codespace"
            | "gist"
            | "issue"
            | "org"
            | "pr"
            | "project"
            | "release"
            | "repo"
            | "skill"
            | "cache"
            | "run"
            | "workflow"
            | "agent-task"
            | "alias"
            | "api"
            | "attestation"
            | "completion"
            | "config"
            | "copilot"
            | "extension"
            | "extensions"
            | "gpg-key"
            | "label"
            | "licenses"
            | "preview"
            | "ruleset"
            | "search"
            | "secret"
            | "ssh-key"
            | "status"
            | "variable"
    )
}
