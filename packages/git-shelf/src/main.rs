mod checkout;
mod tui;
use base64::{Engine, engine::general_purpose::STANDARD};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::{
    error::Error,
    io::{IsTerminal, Write},
    process::{Command, Stdio},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
#[derive(Parser)]
#[command(
    name = "g", version=env!("GIT_SHELF_VERSION"),
    about = "Browse, review and restore this project's commit shelf"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    /// Browse the project shelf without opening the terminal UI.
    List,
    /// Inspect a stored diff and its review comments.
    Open { sha: String },
    /// Shelve the current branch tip. Linking its task is recommended.
    #[command(alias = "stash")]
    StashCommit {
        /// Refuse to shelve if the displayed branch tip has changed.
        sha: Option<String>,
        #[arg(long)]
        task: Option<i64>,
    },
    /// Cherry-pick a shelf entry into this checkout; keep the saved review.
    #[command(alias = "unstash")]
    ApplyCommit { sha: String },
    /// Attach an existing shelf entry and its discussion to a task.
    Link {
        sha: String,
        #[arg(long)]
        task: i64,
    },
    /// Leave an inline review comment (use --old for a removed line).
    Comment {
        sha: String,
        #[arg(long)]
        file: String,
        #[arg(long)]
        line: u32,
        #[arg(long)]
        old: bool,
        #[arg(long)]
        body: String,
    },
}

fn rpc(operation: Value) -> Result<Value> {
    let mut command = Command::new("hey-boss");
    if std::io::stdin().is_terminal()
        && std::env::var_os("HEY_BOSS_AGENT_ID").is_none()
        && std::env::var_os("CODEX_THREAD_ID").is_none()
        && std::env::var_os("CLAUDECODE").is_none()
    {
        let user = std::env::var("USER")?;
        command.env("HEY_BOSS_AGENT_ID", format!("human:{user}"));
    }
    let mut child = command
        .args(["issue", "shelf", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot start hey-boss: {e}"))?;
    let sent = serde_json::to_writer(child.stdin.take().unwrap(), &operation);
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let error = serde_json::from_slice::<Value>(&out.stdout)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).trim().to_string());
        return Err(format!("Hey Boss: {error}").into());
    }
    sent?;
    let data: Value = serde_json::from_slice(&out.stdout)?;
    if data["ok"] != true {
        return Err(format!("Hey Boss: {}", data["error"]).into());
    }
    Ok(data)
}
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| format!("Incomplete shelf response: missing {key}").into())
}
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '�'
            } else {
                c
            }
        })
        .collect()
}
fn entries() -> Result<Vec<Value>> {
    rpc(json!({"command":"list"}))?["entries"]
        .as_array()
        .cloned()
        .ok_or("Missing project shelf".into())
}
fn open(sha: &str) -> Result<Value> {
    rpc(json!({"command":"open","sha":sha}))
}
fn review(v: &Value) -> Result<String> {
    let r = &v["review"];
    let mut text = format!(
        "{}\n{}  ·  {}\n\n{}",
        field(r, "message")?,
        field(r, "author")?,
        field(v, "sha")?,
        field(r, "patch")?
    );
    for c in v["comments"].as_array().into_iter().flatten() {
        let loc = &c["location"];
        text += &format!(
            "\n{}:{} ({})  {}\n  {}\n",
            field(loc, "file")?,
            loc["line"],
            field(loc, "side")?,
            field(c, "author")?,
            field(c, "body")?
        );
    }
    Ok(clean(&text))
}
fn run() -> Result<()> {
    let cli = Cli::parse();
    checkout::to_root()?;
    match cli.command {
        None if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() => tui::browse()?,
        Some(Action::List) | None => {
            let rows = entries()?;
            println!("Checkout: {}", checkout::tip()?);
            if !rows.is_empty() {
                println!("Shelf:");
            }

            if rows.is_empty() {
                println!("Shelf is empty. Save a commit with g stash-commit SHA --task NUMBER.");
            }
            for e in rows {
                let task = e["task"]
                    .as_i64()
                    .filter(|v| *v > 0)
                    .map(|v| format!("   #{v}"))
                    .unwrap_or_default();
                println!(
                    "{}  {}{}",
                    field(&e, "sha")?.chars().take(10).collect::<String>(),
                    clean(field(&e, "title")?),
                    task
                );
            }
        }
        Some(Action::Open { sha }) => {
            if sha != "HEAD" && !(sha.len() >= 4 && sha.bytes().all(|c| c.is_ascii_hexdigit())) {
                return Err("Use HEAD or a commit SHA".into());
            }
            let prefix = sha.to_ascii_lowercase();
            if entries()?
                .iter()
                .any(|e| e["sha"].as_str().is_some_and(|s| s.starts_with(&prefix)))
            {
                if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
                    tui::inspect(&sha)?;
                } else {
                    println!("{}", review(&open(&sha)?)?);
                }
            } else if !Command::new("git")
                .args([
                    "show",
                    "--no-ext-diff",
                    "--no-textconv",
                    &format!("{sha}^{{commit}}"),
                    "--",
                ])
                .status()?
                .success()
            {
                return Err("Commit is not available in the shelf or checkout".into());
            }
        }
        Some(Action::StashCommit { sha, task }) => {
            println!("{}", checkout::stash(sha.as_deref(), task)?)
        }
        Some(Action::ApplyCommit { sha }) => println!("{}", checkout::unstash(&sha)?),
        Some(Action::Link { sha, task }) => {
            rpc(json!({"command":"link","sha":sha,"task":task}))?;
            println!("Linked to task #{task}");
        }
        Some(Action::Comment {
            sha,
            file,
            line,
            old,
            body,
        }) => {
            rpc(
                json!({"command":"comment","sha":sha,"location":{"file":file,"side":if old {"old"}else{"new"},"line":line},"body":body}),
            )?;
            println!("Comment saved");
        }
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("g: {e}");
        std::process::exit(1);
    }
}
