//! Explicit project onboarding; existing Markdown defaults remain the source of truth.
use clap::{Args, Subcommand};
use hey_boss::issues::{self, Error, Operation, Request, Result};
use serde_json::{Value, json};
use std::io::{self, IsTerminal, Write};

#[derive(Args)]
pub struct Options {
    #[command(subcommand)]
    action: Action,
    /// Print structured results; requires explicit choices and --yes.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Subcommand)]
enum Action {
    /// Choose a project's workflow and review its default prompts.
    #[command(
        after_help = "Run inside a checkout to detect its project, or select one with --project.\nExisting custom prompts are preserved. Ctrl-C or q cancels before saving.\nFor scripts: --yes --prs <true|false> --worktree <true|false>."
    )]
    Init {
        /// Project name or full ID; defaults to this checkout.
        #[arg(long)]
        project: Option<String>,
        /// Authoritative SSH host (also HEY_BOSS_ISSUE_HOST).
        #[arg(long)]
        host: Option<String>,
        /// Deliver changes through pull requests.
        #[arg(long, action = clap::ArgAction::Set)]
        prs: Option<bool>,
        /// Allow dedicated Git worktrees for tasks.
        #[arg(long, action = clap::ArgAction::Set)]
        worktree: Option<bool>,
        /// Save explicit choices without interactive review.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

pub fn run(options: &Options) -> Result<()> {
    let Action::Init {
        project,
        host,
        prs,
        worktree,
        yes,
    } = &options.action;
    let interactive =
        !yes && !options.json && io::stdin().is_terminal() && io::stdout().is_terminal();
    if !interactive && (!yes || prs.is_none() || worktree.is_none()) {
        return Err(Error::invalid(
            "Run project init in a terminal, or pass --yes --prs <true|false> --worktree <true|false>",
        ));
    }
    let cwd = std::env::current_dir()?.canonicalize()?;
    let machine = issues::identity::machine()?;
    let mut request = Request {
        version: 1,
        project: issues::identity::project(&cwd, &machine)?,
        project_override: project.clone().or_else(|| {
            std::env::var("HEY_BOSS_ISSUE_PROJECT")
                .ok()
                .filter(|v| !v.is_empty())
        }),
        actor: Some(issues::identity::resolve(
            Some("human:boss"),
            &machine,
            &cwd,
        )?),
        operation: Operation::ProjectInit { settings: None },
        request_id: None,
    };
    let host = host.clone().or_else(|| {
        std::env::var("HEY_BOSS_ISSUE_HOST")
            .ok()
            .filter(|v| !v.is_empty())
    });
    let current = crate::cli_request::execute(&request, host.as_deref(), false)?;
    // Pin the resolved destination for the write, including name aliases.
    request.project = serde_json::from_value(current["project"].clone())?;
    request.project_override = Some(request.project.id.clone());
    let existing = current["version"].as_i64().unwrap_or(0) > 0;
    let mut prs = prs.unwrap_or(current["prs_enabled"] == true);
    let mut worktree = worktree.unwrap_or(current["worktree_enabled"] == true);
    if interactive {
        println!();
        line(&format!("Project setup · {}", request.project.name));
        line(&request.project.id);
        if let Some(host) = &host {
            line(&format!("Store: {host}"));
        }
        line(if existing {
            "Existing settings loaded. Custom prompts will be kept."
        } else {
            "Initialize this project to show it in Builder."
        });
        line("Enter keeps the shown choice. Type q to cancel.");
        println!();
        line("1 / 3 · Delivery");
        line("Use pull requests for review, or commit and push to main.");
        let Some(selected) = choice("Use pull requests?", prs)? else {
            return cancelled();
        };
        prs = selected;
        println!();
        line("2 / 3 · Workspace");
        line("Allow a separate Git worktree per task, or use the existing checkout.");
        let Some(selected) = choice("Use worktrees?", worktree)? else {
            return cancelled();
        };
        worktree = selected;
        println!();
        line("3 / 3 · Prompts and review");
        outline(&current, prs, worktree, false);
        println!();
        line(&format!(
            "Pull requests: {} · Worktrees: {}",
            enabled(prs),
            enabled(worktree)
        ));
        line("Workers are configured separately; saving does not start one.");
        loop {
            match answer("Save project? [Y/n/p = all prompts] ")?.as_deref() {
                Some("" | "y" | "yes") => break,
                Some("n" | "no" | "q" | "quit") | None => return cancelled(),
                Some("p") => outline(&current, prs, worktree, true),
                _ => line("Enter y to save, n to cancel, or p to read all prompts."),
            }
        }
    }
    request.operation = serde_json::from_value(json!({
        "action":"project_init", "settings": { "prs_enabled":prs, "worktree_enabled":worktree,
        "if_version":current["version"] },
    }))?;
    let result = crate::cli_request::execute(&request, host.as_deref(), false)?;
    if options.json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!();
        line(&format!("Project saved · {}", request.project.name));
        line(&format!(
            "Pull requests: {} · Worktrees: {}",
            enabled(prs),
            enabled(worktree)
        ));
        line("Review or edit prompts in Builder → Project settings.");
        line("Run hey-boss project init again to change these choices.");
    }
    Ok(())
}

fn enabled(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
fn cancelled() -> Result<()> {
    println!("\nCancelled. Project settings were not changed.");
    Ok(())
}
fn answer(label: &str) -> Result<Option<String>> {
    print!("{label}");
    io::stdout().flush()?;
    let mut value = String::new();
    if io::stdin().read_line(&mut value)? == 0 {
        return Ok(None);
    }
    Ok(Some(value.trim().to_ascii_lowercase()))
}
fn choice(label: &str, default: bool) -> Result<Option<bool>> {
    loop {
        match answer(&format!(
            "{label} {} ",
            if default { "[Y/n]" } else { "[y/N]" }
        ))?
        .as_deref()
        {
            Some("") => return Ok(Some(default)),
            Some("y" | "yes") => return Ok(Some(true)),
            Some("n" | "no") => return Ok(Some(false)),
            Some("q" | "quit") | None => return Ok(None),
            _ => line("Enter y or n, or q to cancel."),
        }
    }
}
fn outline(settings: &Value, prs: bool, worktree: bool, all: bool) {
    println!();
    line(if settings["prompt"] == issues::worker::DEFAULT_PROMPT {
        "Implementation (default)"
    } else {
        "Implementation (custom)"
    });
    line(settings["prompt"].as_str().unwrap_or_default());
    let workspace = if worktree { "worktree" } else { "checkout" };
    let delivery = if prs { "prs" } else { "main" };
    if let Some(sections) = settings["prompt_sections"].as_array() {
        for section in sections {
            let key = section["key"].as_str().unwrap_or_default();
            if !all && ![workspace, delivery, "plan"].contains(&key) {
                continue;
            }
            let custom = settings["prompt_overrides"][key].as_str();
            println!();
            line(&format!(
                "{}{}",
                section["title"].as_str().unwrap_or(key),
                if custom.is_some() {
                    " (custom)"
                } else {
                    " (default)"
                }
            ));
            line(
                custom
                    .or_else(|| settings["prompt_defaults"][key].as_str())
                    .unwrap_or_default(),
            );
        }
    }
    if all || settings["chief_enabled"] == true {
        println!();
        line(&format!(
            "Chief ({})",
            if settings["chief_enabled"] == true {
                "enabled"
            } else {
                "disabled"
            }
        ));
        line(settings["chief_prompt"].as_str().unwrap_or_default());
    }
}

// Keep previews readable in a narrow terminal and neutralize control characters
// in user-controlled names and saved prompts without modifying their stored text.
fn line(text: &str) {
    let width = crossterm::terminal::size()
        .map(|(cols, _)| usize::from(cols).saturating_sub(1).max(10))
        .unwrap_or(79);
    for paragraph in text.lines() {
        let mut used = 0;
        let mut wrapped = String::new();
        for word in paragraph.split_whitespace() {
            let word: String = word.chars().filter(|c| !c.is_control()).collect();
            let length = word.chars().count();
            if used > 0 && used + 1 + length > width {
                wrapped.push('\n');
                used = 0;
            }
            if used > 0 {
                wrapped.push(' ');
                used += 1;
            }
            for character in word.chars() {
                if used == width {
                    wrapped.push('\n');
                    used = 0;
                }
                wrapped.push(character);
                used += 1;
            }
        }
        println!("{wrapped}");
    }
}
