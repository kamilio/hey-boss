use clap::{Args, Subcommand};
use hey_boss::{
    issues::{self, Error, Result},
    jobs::{Definition, Operation},
};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Args)]
pub struct Options {
    /// Existing project name or full ID; defaults to this checkout.
    #[arg(long, global = true)]
    project: Option<String>,
    /// Stable caller identity used for mutation receipts.
    #[arg(long, global = true)]
    agent: Option<String>,
    /// Reuse with identical content after an uncertain response.
    #[arg(long, global = true)]
    request_id: Option<String>,
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    action: Action,
}
#[derive(Args)]
struct Fields {
    #[arg(long)]
    name: String,
    /// Five fields; quote the expression.
    #[arg(long)]
    cron: String,
    /// Explicit IANA timezone, e.g. America/Chicago.
    #[arg(long)]
    timezone: String,
    #[arg(long, value_enum)]
    harness: hey_boss::agent_runtime::Provider,
    /// Logical model name, preserved in every execution snapshot.
    #[arg(long)]
    model: String,
}
impl Fields {
    fn definition(&self) -> Definition {
        Definition {
            name: self.name.clone(),
            cron: self.cron.clone(),
            timezone: self.timezone.clone(),
            harness: self.harness.name().into(),
            model: self.model.clone(),
        }
    }
}
#[derive(Args)]
struct Window {
    /// Exclusive RFC3339 UTC/offset timestamp (defaults to now).
    #[arg(long)]
    after: Option<String>,
    /// Inclusive RFC3339 timestamp (defaults to one year after --after).
    #[arg(long)]
    through: Option<String>,
    #[arg(long, default_value_t = 10)]
    limit: usize,
}
impl Window {
    fn bounds(&self) -> Result<(i64, i64, usize)> {
        let parse = |s: &str| {
            s.parse::<jiff::Timestamp>()
                .map(|t| t.as_millisecond())
                .map_err(|e| Error::invalid(e.to_string()))
        };
        let after = self
            .after
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or_else(|| jiff::Timestamp::now().as_millisecond());
        let through = self
            .through
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or(after.saturating_add(366 * 86_400_000));
        Ok((after, through, self.limit))
    }
}
#[derive(Subcommand)]
enum Action {
    /// Create a stable job; retry with the same ID and request ID.
    Create {
        #[arg(long)]
        id: String,
        #[command(flatten)]
        fields: Fields,
        /// User-written UTF-8 Markdown file; copied exactly into managed storage.
        #[arg(long)]
        instructions: PathBuf,
        #[arg(long)]
        paused: bool,
    },
    /// Replace definition fields; retain instructions unless a new .md file is supplied.
    Edit {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[command(flatten)]
        fields: Fields,
        #[arg(long)]
        instructions: Option<PathBuf>,
    },
    /// Show the current definition, revision and next scheduled time.
    View { id: String },
    /// List this project's job definitions, with bounded pagination.
    List {
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        include_deleted: bool,
    },
    /// Suppress future occurrences while allowing active work to finish.
    Pause {
        id: String,
        #[arg(long)]
        if_revision: i64,
    },
    /// Schedule forward from now, skipping the paused interval.
    Resume {
        id: String,
        #[arg(long)]
        if_revision: i64,
    },
    /// Disable future scheduling; retain active runs and history.
    Delete {
        id: String,
        #[arg(long)]
        if_revision: i64,
    },
    /// Preview a cron expression without creating tasks or saving a job.
    Preview {
        #[arg(long)]
        cron: String,
        #[arg(long)]
        timezone: String,
        #[command(flatten)]
        window: Window,
    },
    /// Project future occurrences for an enabled job.
    Next {
        id: String,
        #[command(flatten)]
        window: Window,
    },
    /// Read durable run history, newest first.
    History {
        id: String,
        #[arg(long)]
        before: Option<i64>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Read and durably cache an immutable Markdown revision on this machine.
    Revision { id: String, revision: i64 },
    /// Read an execution snapshot and durably cache its required Markdown.
    Run { id: String, run_id: String },
    /// Queue one execution, including when the schedule is paused.
    RunNow { id: String },
    /// Stop this exact owned execution; leaves the schedule unchanged.
    Stop { id: String, run_id: String },
    /// Execute one Jobs operation read as JSON from stdin.
    Rpc,
}
fn markdown(path: &Path) -> Result<String> {
    if path.extension().and_then(|s| s.to_str()) != Some("md") {
        return Err(Error::invalid("Instructions must come from a .md file"));
    }
    String::from_utf8(hey_boss::attachments::read_file(path)?)
        .map_err(|_| Error::invalid("Instructions must be UTF-8"))
}
pub fn run(options: &Options) -> Result<()> {
    let operation = match &options.action {
        Action::Create {
            id,
            fields,
            instructions,
            paused,
        } => Operation::Create {
            id: id.clone(),
            definition: fields.definition(),
            markdown: markdown(instructions)?,
            enabled: !*paused,
        },
        Action::Edit {
            id,
            if_revision,
            fields,
            instructions,
        } => Operation::Edit {
            id: id.clone(),
            if_revision: *if_revision,
            definition: fields.definition(),
            markdown: instructions.as_deref().map(markdown).transpose()?,
        },
        Action::View { id } => Operation::View { id: id.clone() },
        Action::RunNow { id } => Operation::RunNow { id: id.clone() },
        Action::Stop { id, run_id } => Operation::Stop {
            id: id.clone(),
            run_id: run_id.clone(),
        },
        Action::List {
            after,
            limit,
            include_deleted,
        } => Operation::List {
            after: after.clone(),
            limit: *limit,
            include_deleted: *include_deleted,
        },
        Action::Pause { id, if_revision } => Operation::SetEnabled {
            id: id.clone(),
            if_revision: *if_revision,
            enabled: false,
        },
        Action::Resume { id, if_revision } => Operation::SetEnabled {
            id: id.clone(),
            if_revision: *if_revision,
            enabled: true,
        },
        Action::Delete { id, if_revision } => Operation::Delete {
            id: id.clone(),
            if_revision: *if_revision,
        },
        Action::Preview {
            cron,
            timezone,
            window,
        } => {
            let (after, through, limit) = window.bounds()?;
            if options.request_id.is_some() {
                return Err(Error::invalid("--request-id applies only to mutations"));
            }
            let dates = hey_boss::jobs::schedule::Schedule::parse(cron, timezone)?
                .preview(after, through, limit)?;
            return print(&json!({"ok":true,"occurrences":dates}), options.json);
        }
        Action::Next { id, window } => {
            let (after, through, limit) = window.bounds()?;
            Operation::Next {
                id: id.clone(),
                after,
                through,
                limit,
            }
        }
        Action::History { id, before, limit } => Operation::History {
            id: id.clone(),
            before: *before,
            limit: *limit,
        },
        Action::Revision { id, revision } => Operation::Revision {
            id: id.clone(),
            revision: *revision,
        },
        Action::Run { id, run_id } => Operation::Run {
            id: id.clone(),
            run_id: run_id.clone(),
        },
        Action::Rpc => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .take(issues::WIRE_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > issues::WIRE_LIMIT {
                return Err(Error::invalid("Jobs request exceeds 16 MiB"));
            }
            serde_json::from_slice(&bytes)?
        }
    };
    operation.validate()?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let machine = issues::identity::machine()?;
    let actor = if operation.writes() {
        Some(issues::identity::resolve(
            options.agent.as_deref(),
            &machine,
            &cwd,
        )?)
    } else {
        None
    };
    let fresh_run_id =
        if options.request_id.is_none() && matches!(operation, Operation::RunNow { .. }) {
            let mut bytes = [0u8; 16];
            std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
            Some(format!(
                "job-run-{}",
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
            ))
        } else {
            None
        };
    let run_now = matches!(operation, Operation::RunNow { .. });
    let request_id = options.request_id.clone().or(fresh_run_id).or_else(|| {
        use sha2::{Digest, Sha256};
        operation.writes().then(|| {
            format!(
                "job-{:x}",
                Sha256::digest(serde_json::to_vec(&operation).expect("serializable operation"))
            )
        })
    });
    let request = issues::Request {
        version: 1,
        project: issues::identity::project(&cwd, &machine)?,
        project_override: options
            .project
            .clone()
            .or_else(|| std::env::var("HEY_BOSS_ISSUE_PROJECT").ok()),
        actor,
        request_id,
        operation: issues::Operation::Job { operation },
    };
    let value = crate::cli_request::execute(&request, None, false).map_err(|mut error| {
        if run_now {
            error.message.push_str(&format!(
                ". Retry this run with --request-id {}",
                request.request_id.as_deref().unwrap()
            ));
        }
        error
    })?;
    print(
        &value,
        options.json || matches!(options.action, Action::Rpc),
    )
}
fn print(value: &Value, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(value)?);
        return Ok(());
    }
    if let Some(dates) = value["occurrences"].as_array() {
        for date in dates {
            let timestamp = jiff::Timestamp::from_millisecond(date.as_i64().unwrap())
                .map_err(|e| Error::invalid(e.to_string()))?;
            println!("{timestamp}");
        }
        if dates.is_empty() {
            println!("No occurrences in this range.");
        }
    } else if let Some(jobs) = value["jobs"].as_array() {
        for job in jobs {
            print_job(job);
        }
        if jobs.is_empty() {
            println!("No jobs.");
        }
        if let Some(cursor) = value["next_cursor"].as_str() {
            println!("Next page: --after {cursor}");
        }
    } else if value["job"].is_object() {
        print_job(&value["job"]);
    } else if let Some(runs) = value["runs"].as_array() {
        for run in runs {
            println!(
                "{} · {} · {} · {}",
                run["id"].as_str().unwrap_or(""),
                run["trigger"].as_str().unwrap_or(""),
                run["state"].as_str().unwrap_or(""),
                run["scheduled_at"]
                    .as_i64()
                    .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
                    .map(|ts| ts.to_string())
                    .unwrap_or_default()
            );
        }
        if runs.is_empty() {
            println!("No runs.");
        }
        if let Some(cursor) = value["next_cursor"].as_i64() {
            println!("Next page: --before {cursor}");
        }
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}
fn print_job(job: &Value) {
    let d = &job["snapshot"]["definition"];
    println!(
        "{} · {} · revision {} · {}",
        job["id"].as_str().unwrap_or(""),
        d["name"].as_str().unwrap_or(""),
        job["revision"],
        if !job["deleted_at"].is_null() {
            "deleted"
        } else if job["enabled"] == true {
            "enabled"
        } else {
            "paused"
        }
    );
    println!(
        "  {} · {} · {} / {}",
        d["cron"].as_str().unwrap_or(""),
        d["timezone"].as_str().unwrap_or(""),
        d["harness"].as_str().unwrap_or(""),
        d["model"].as_str().unwrap_or("")
    );
    if let Some(next) = job["next_at"]
        .as_i64()
        .and_then(|n| jiff::Timestamp::from_millisecond(n).ok())
    {
        println!("  Next: {next}");
    }
}
