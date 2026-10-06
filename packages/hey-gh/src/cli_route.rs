//! Keep gh's parser and process semantics authoritative. Only inspect enough of
//! argv to select an explicit extension or authentication provider.
use std::ffi::{OsStr, OsString};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    Auto,
    User,
    App,
}

pub struct Invocation {
    pub args: Vec<OsString>,
    pub auth: Auth,
    pub cached: bool,
    pub command: usize,
}

impl Invocation {
    pub fn parse(mut args: Vec<OsString>) -> Result<Self> {
        let mut auth = Auth::Auto;
        let mut index = 0;
        while index < args.len() {
            let arg = args[index].to_string_lossy();
            if arg == "--auth" || arg.starts_with("--auth=") {
                let (value, count) = if let Some(value) = arg.strip_prefix("--auth=") {
                    (value.to_owned(), 1)
                } else {
                    (
                        args.get(index + 1)
                            .ok_or("--auth requires auto, user, or app")?
                            .to_string_lossy()
                            .into_owned(),
                        2,
                    )
                };
                auth = match value.as_str() {
                    "auto" => Auth::Auto,
                    "user" => Auth::User,
                    "app" => Auth::App,
                    _ => return Err("--auth requires auto, user, or app".into()),
                };
                args.drain(index..index + count);
                continue;
            }
            if matches!(
                arg.as_ref(),
                "-R" | "--repo" | "--server" | "--cursor" | "--timeout"
            ) {
                index += 2;
            } else if arg.starts_with('-') && arg != "--" {
                index += 1;
            } else {
                break;
            }
        }
        let command = index;
        let explicit = args.get(index).is_some_and(|a| a == "cached");
        if explicit {
            args.remove(index);
        }
        let root = args.get(index).and_then(|a| a.to_str()).unwrap_or("");
        let positions = positional_indices(&args);
        let sub = positions
            .get(1)
            .and_then(|i| args[*i].to_str())
            .unwrap_or("");
        let internal = matches!(
            root,
            "app"
                | "service"
                | "serve"
                | "logs"
                | "install"
                | "ci"
                | "snapshot"
                | "mine"
                | "watch"
                | "watch-repo"
                | "required-checks"
                | "prs"
                | "watches"
                | "unwatch"
                | "changes"
        );
        let help = flag_enabled(&option_values(&args), &["--help", "-h"]);
        let legacy = (root == "pr"
            && ((sub.is_empty() && !help) || sub.contains('/') || sub == "changes"))
            || (root == "repo" && sub.contains('/'))
            || (root == "release"
                && matches!(
                    sub,
                    "profile" | "add" | "status" | "poll" | "watch" | "remove"
                ));
        // Inspect flags only on read commands. A body, title, jq expression, or
        // extension argument that happens to say --cached-only is never a mode.
        let read = (root == "pr" && matches!(sub, "list" | "view" | "checks" | "status"))
            || root == "status";
        let extensions = read
            && option_values(&args).iter().any(|(name, _)| {
                matches!(
                    name.as_str(),
                    "--server"
                        | "--cursor"
                        | "--timeout"
                        | "--refresh"
                        | "--cached-only"
                        | "--wait"
                )
            });
        let shorthand = root.is_empty()
            && option_values(&args)
                .iter()
                .any(|(name, _)| matches!(name.as_str(), "--cursor" | "--server" | "--timeout"));
        let cached = explicit || internal || legacy || extensions || shorthand;
        if cached && auth != Auth::Auto {
            return Err("--auth selects credentials for native gh commands; cached reads use the daemon's configured providers".into());
        }
        Ok(Self {
            args,
            auth,
            cached,
            command,
        })
    }

    pub fn root(&self) -> &str {
        self.args
            .get(self.command)
            .and_then(|a| a.to_str())
            .unwrap_or("")
    }
    pub fn subcommand(&self) -> &str {
        positional_indices(&self.args)
            .get(1)
            .and_then(|i| self.args[*i].to_str())
            .unwrap_or("")
    }
    pub fn help(&self) -> bool {
        self.root() == "help"
            || flag_enabled(&option_values(&self.args), &["--help", "-h", "--version"])
    }
}

pub fn flag_enabled(options: &[(String, Option<OsString>)], names: &[&str]) -> bool {
    options
        .iter()
        .rev()
        .find(|(name, _)| names.contains(&name.as_str()))
        .is_some_and(|(_, value)| {
            value.as_ref().is_none_or(|value| {
                matches!(
                    value.to_str(),
                    Some("1" | "t" | "T" | "true" | "TRUE" | "True")
                )
            })
        })
}

pub fn positional_indices(args: &[OsString]) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        let value = arg.to_string_lossy();
        if value == "--" {
            positions.extend(index + 1..args.len());
            break;
        }
        if !value.starts_with('-') {
            positions.push(index);
        }
        index += if takes_value(&value) { 2 } else { 1 };
    }
    positions
}

fn takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-R" | "--repo"
            | "-b"
            | "--body"
            | "-F"
            | "--body-file"
            | "-f"
            | "--field"
            | "--raw-field"
            | "-X"
            | "--method"
            | "--hostname"
            | "--input"
            | "-H"
            | "--header"
            | "--jq"
            | "-q"
            | "--template"
            | "-t"
            | "--json"
            | "--server"
            | "--cursor"
            | "--timeout"
            | "--wait"
            | "--title"
            | "--author"
            | "--state"
            | "--limit"
            | "-L"
            | "--cache"
            | "--preview"
            | "-p"
    )
}

/// Preserve raw argv for execution; this inspection only skips known values.
/// In particular, never interpret option-looking body text as a wrapper flag.
pub fn option_values(args: &[OsString]) -> Vec<(String, Option<OsString>)> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].to_string_lossy();
        index += 1;
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') {
            continue;
        }
        if let Some((name, value)) = arg.split_once('=') {
            if name.starts_with("--") || name.len() == 2 {
                result.push((name.into(), Some(value.into())));
                continue;
            }
        }
        if takes_value(&arg) {
            result.push((arg.into_owned(), args.get(index).cloned()));
            index += 1;
        } else if arg.len() > 2
            && ["-R", "-b", "-F", "-f", "-X", "-H", "-q", "-t", "-p"]
                .iter()
                .any(|prefix| arg.starts_with(prefix))
        {
            result.push((arg[..2].into(), Some(OsStr::new(&arg[2..]).to_owned())));
        } else {
            result.push((arg.into_owned(), None));
        }
    }
    result
}
