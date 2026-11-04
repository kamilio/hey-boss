use clap::{Args, Subcommand};
use hey_boss::{
    issues::{self, Error, Result},
    jobs::{
        Operation,
        command::{self, Blob, Spec},
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io::Write, path::PathBuf, time::Duration};

#[derive(Args)]
pub struct Options {
    /// Project selects eligible workers, not the command's checkout or model.
    #[arg(long, global = true)]
    project: Option<String>,
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Submit an isolated command. Reuse request-id to recover an uncertain response.
    Submit {
        #[arg(long)]
        request_id: String,
        #[arg(long, requires = "model")]
        harness: Option<String>,
        #[arg(long, requires = "harness")]
        model: Option<String>,
        #[arg(long, requires = "harness")]
        effort: Option<String>,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Check installation, login state and capacity on eligible workers; makes no model call.
    Check {
        #[arg(long)]
        harness: String,
        #[arg(long)]
        model: String,
    },
    Status {
        execution_id: String,
    },
    Logs {
        execution_id: String,
        #[arg(long)]
        follow: bool,
    },
    Cancel {
        execution_id: String,
    },
    /// Download and verify output files into a new directory.
    Collect {
        execution_id: String,
        #[arg(long)]
        output: PathBuf,
    },
}
fn call(options: &Options, operation: Operation, key: Option<&str>) -> Result<Value> {
    operation.validate()?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let machine = issues::identity::machine()?;
    let request = issues::Request {
        version: 1,
        project: issues::identity::project(&cwd, &machine)?,
        project_override: options.project.clone(),
        actor: operation
            .writes()
            .then(|| issues::identity::resolve(None, &machine, &cwd))
            .transpose()?,
        request_id: key.map(str::to_owned),
        operation: issues::Operation::Job { operation },
    };
    // Jobs own their receipts, including file chunks; do not put file bytes in generic journals.
    issues::Store::open(&issues::database_path()?)?.execute(&request)
}
fn status(options: &Options, id: &str) -> Result<Value> {
    call(options, Operation::Execution { run_id: id.into() }, None)
}
fn terminal(v: &Value) -> bool {
    !matches!(v["run"]["state"].as_str(), Some("pending" | "running"))
}
fn check(options: &Options, harness: &str, model: &str) -> Result<Value> {
    Spec {
        program: "agent-eval".into(),
        args: vec![],
        input: None,
        harness: Some(harness.into()),
        model: Some(model.into()),
        effort: None,
    }
    .validate()?;
    let resolved = call(
        options,
        Operation::List {
            after: None,
            limit: 1,
            include_deleted: false,
        },
        None,
    )?;
    let project = &resolved["project"]["id"];
    let fleet = hey_boss::fleet::call(&json!({"kind":"status"}))?;
    let mut workers = Vec::new();
    for m in fleet["machines"].as_array().into_iter().flatten() {
        for w in m["jobs"]["workers"].as_array().into_iter().flatten() {
            if !w["projects"]
                .as_array()
                .is_some_and(|ps| ps.contains(project))
            {
                continue;
            }
            let ready =
                m["state"] == "connected" && w["readiness"]["harnesses"][harness]["ready"] == true;
            workers.push(json!({"worker":w["id"],"machine":m["node"],"host":m["host"],"ready":ready,"capacity":w["capacity"],"issue_active":w["issue_active"],"evaluator":w["readiness"]["evaluator"],"harness":w["readiness"]["harnesses"][harness],"checked_at":w["readiness"]["checked_at"]}));
        }
    }
    Ok(
        json!({"project":resolved["project"],"harness":harness,"model":model,"model_access":"not_tested","ready":workers.iter().any(|w|w["ready"]==true),"workers":workers}),
    )
}
pub fn run(options: &Options) -> Result<()> {
    let value = match &options.action {
        Action::Check { harness, model } => check(options, harness, model)?,
        Action::Submit {
            request_id,
            harness,
            model,
            effort,
            input,
            command: argv,
        } => {
            let mut spec = Spec {
                program: argv[0].clone(),
                args: argv[1..].to_vec(),
                input: None,
                harness: harness.clone(),
                model: model.clone(),
                effort: effort.clone(),
            };
            spec.validate()?;
            if let (Some(h), Some(m)) = (harness, model) {
                let checked = check(options, h, m)?;
                if checked["ready"] != true {
                    return Err(Error::invalid(format!(
                        "No ready {h} worker for this project; run hey-boss exec check --harness {h} --model {m}"
                    )));
                }
            }
            if let Some(path) = input {
                spec.input = Some(command::upload(path, |op| {
                    call(options, op, Some(request_id))
                })?);
            }
            let id = format!(
                "exec-{:x}",
                Sha256::digest(format!(
                    "{}\n{request_id}",
                    options.project.as_deref().unwrap_or("")
                ))
            );
            call(
                options,
                Operation::Submit {
                    id,
                    execution: spec,
                },
                Some(request_id),
            )?
        }
        Action::Status { execution_id } => status(options, execution_id)?,
        Action::Cancel { execution_id } => call(
            options,
            Operation::CancelExecution {
                run_id: execution_id.clone(),
            },
            Some(&format!("cancel-{execution_id}")),
        )?,
        Action::Collect {
            execution_id,
            output,
        } => {
            if output.exists() {
                return Err(Error::invalid("Output directory already exists"));
            }
            let value = status(options, execution_id)?;
            if !terminal(&value) {
                return Err(Error::invalid("Execution is still running"));
            }
            let blob: Blob = serde_json::from_value(value["result"]["output"].clone())
                .map_err(|_| Error::invalid("Execution has no collected output"))?;
            let archive = output.with_extension(format!("{}.tar.gz", execution_id));
            command::download(&blob, &archive, |op| call(options, op, None))?;
            let result = command::unpack(&archive, output);
            let _ = std::fs::remove_file(&archive);
            result?;
            json!({"execution_id":execution_id,"output":output})
        }
        Action::Logs {
            execution_id,
            follow,
        } => {
            let mut offset = 0;
            let mut printed = 0;
            loop {
                let state = status(options, execution_id)?;
                let job = state["run"]["snapshot"]["job_id"]
                    .as_str()
                    .ok_or_else(|| Error::invalid("Missing execution job"))?;
                let page = call(
                    options,
                    Operation::Log {
                        id: job.into(),
                        run_id: execution_id.clone(),
                        offset,
                    },
                    None,
                )?;
                if let Some(text) = page["log"].as_str() {
                    if text.len() < printed {
                        return Err(Error::invalid("Execution log shrank"));
                    }
                    print!("{}", &text[printed..]);
                    printed = text.len();
                    std::io::stdout().flush()?;
                }
                if let Some(next) = page["next_log_offset"].as_u64() {
                    offset = next;
                    printed = 0;
                    continue;
                }
                if !follow || terminal(&state) {
                    break;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
            return Ok(());
        }
    };
    if options.json {
        println!("{}", serde_json::to_string(&value)?);
    } else if let Some(workers) = value["workers"].as_array() {
        for w in workers {
            println!(
                "{} / {}: {} (login {}, credentials {}, evaluator {})",
                w["host"].as_str().unwrap_or("?"),
                w["worker"].as_str().unwrap_or("?"),
                if w["ready"] == true {
                    "ready"
                } else {
                    "not ready"
                },
                w["harness"]["login"].as_str().unwrap_or("checking"),
                w["harness"]["credentials"].as_str().unwrap_or("unavailable"),
                w["evaluator"].as_str().unwrap_or("missing")
            );
        }
        println!("Model access has not been tested; these checks make no model call.");
    } else if let Some(id) = value["run"]["id"].as_str() {
        println!("{id} {}", value["run"]["state"].as_str().unwrap_or(""));
    } else {
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    Ok(())
}
