use clap::Subcommand;
use hey_gh::{
    ApiClient, Freshness,
    release::{Batch, Project, Request, queue::Queue},
};
use serde_json::json;
use std::{path::PathBuf, time::Duration};

#[derive(Subcommand)]
pub enum Action {
    /// Print a project profile to customize and save as JSON.
    Profile {
        #[arg(value_parser=["poe2","poe-code"])]
        project: String,
    },
    /// Add PR numbers or commit SHAs according to the project's target mode.
    Add {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        state: PathBuf,
        #[arg(required = true)]
        targets: Vec<String>,
    },
    /// Read the durable queue without contacting GitHub.
    Status {
        #[arg(long)]
        state: PathBuf,
    },
    /// Observe one fair batch; failures and cancellations remain queued.
    Poll {
        #[arg(long)]
        state: PathBuf,
    },
    /// Poll until interrupted. Explicitly launched; no prompts or assignments.
    Watch {
        #[arg(long)]
        state: PathBuf,
    },
    /// Stop tracking one target; does not affect GitHub.
    Remove {
        #[arg(long)]
        state: PathBuf,
        target: String,
    },
}

pub async fn run(api: &ApiClient, action: Action) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        Action::Profile { project } => println!(
            "{}",
            if project == "poe2" {
                include_str!("release/profiles/poe2.json")
            } else {
                include_str!("release/profiles/poe-code.json")
            }
        ),
        Action::Add {
            config,
            state,
            targets,
        } => {
            let project: Project = serde_json::from_slice(&std::fs::read(config)?)?;
            let mut queue = Queue::open(&state)?;
            queue.add(&project, &targets)?;
            println!("{}", serde_json::to_string_pretty(&queue.entries()?)?);
        }
        Action::Status { state } => println!(
            "{}",
            serde_json::to_string_pretty(&Queue::open(&state)?.entries()?)?
        ),
        Action::Remove { state, target } => {
            Queue::open(&state)?.remove(&target)?;
            println!("{}", json!({"removed":target}));
        }
        Action::Poll { state } => {
            poll(api, &state).await?;
        }
        Action::Watch { state } => {
            let background = api.clone().background();
            loop {
                tokio::select! {
                    result=poll(&background,&state) => { if let Err(error)=result { eprintln!("GitHub Release Watcher: {error}"); } },
                    _=super::shutdown_signal() => break,
                }
                tokio::select! {
                    _=tokio::time::sleep(Duration::from_secs(60)) => {},
                    _=super::shutdown_signal() => break,
                }
            }
        }
    }
    Ok(())
}
async fn poll(api: &ApiClient, state: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    // Cross-process poll exclusion; adding/removing targets still works during I/O.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.with_extension("poll.lock"))?;
    lock.try_lock()
        .map_err(|_| "another process is already polling this release queue")?;
    let mut queue = Queue::open(state)?;
    let project = queue.project()?;
    let targets: Vec<_> = queue
        .entries()?
        .into_iter()
        .take(100)
        .map(|e| e.target)
        .collect();
    if targets.is_empty() {
        println!("[]");
        return Ok(());
    }
    let request = Request { project, targets };
    let result = api
        .release_report(&request, Freshness::MaxAge(Duration::from_secs(60)))
        .await;
    let batch: Batch = match result {
        Ok(batch) => batch,
        Err(error) => {
            queue.record_error(&request.targets, &error.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"error":error.to_string(),"validations":[],"entries":queue.entries()?})
                )?
            );
            return Err(error.into());
        }
    };
    let incomplete = batch.reports.len() != request.targets.len()
        || batch
            .reports
            .iter()
            .any(|r| !r.errors.is_empty() || r.gates.iter().any(|g| !g.history_complete));
    queue.record(&batch)?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"observed_at_ms":batch.observed_at_ms,"validations":batch.validations,"entries":queue.entries()?})
        )?
    );
    if incomplete {
        Err("incomplete release observation; inspect the JSON errors and retry".into())
    } else {
        Ok(())
    }
}
