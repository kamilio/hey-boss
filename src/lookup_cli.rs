use clap::Args;
use hey_boss::{
    issues::{self, Error, Operation, Request, Result, Store},
    routes::{self, Route},
};
use serde_json::{Value, json};

#[derive(Args)]
pub struct Options {
    /// Complete URL copied from the web app; quote it to protect '&' from the shell.
    url: String,
    /// Authoritative issue SSH host; overrides the URL host and HEY_BOSS_ISSUE_HOST.
    #[arg(long)]
    host: Option<String>,
    /// Project for links without explicit project context.
    #[arg(long)]
    project: Option<String>,
    /// Print the resolved route and complete structured resource result.
    #[arg(long)]
    pub json: bool,
}
fn execute(route: &Route, options: &Options, operation: Operation) -> Result<Value> {
    let cwd = std::env::current_dir()?.canonicalize()?;
    let machine = issues::identity::machine()?;
    let request = Request {
        version: 1,
        project: issues::identity::project(&cwd, &machine)?,
        project_override: route
            .project
            .clone()
            .or_else(|| options.project.clone())
            .or_else(|| {
                std::env::var("HEY_BOSS_ISSUE_PROJECT")
                    .ok()
                    .filter(|v| !v.is_empty())
            }),
        actor: None,
        operation,
        request_id: None,
    };
    match options
        .host
        .clone()
        .or_else(|| route.host.clone())
        .or_else(|| {
            std::env::var("HEY_BOSS_ISSUE_HOST")
                .ok()
                .filter(|v| !v.is_empty())
        }) {
        Some(host) => issues::remote::call(&host, &request),
        None => Store::open(&issues::database_path()?)?.execute(&request),
    }
}
pub fn run(options: &Options) -> Result<()> {
    let route = routes::resolve(&options.url)?;
    let mut value = match route.entity.as_str() {
        "issue" => execute(
            &route,
            options,
            Operation::View {
                number: route.id.parse().unwrap(),
            },
        )?,
        "issues" => {
            let owner = route
                .params
                .get("owner")
                .map(String::as_str)
                .unwrap_or("all");
            execute(
                &route,
                options,
                Operation::List {
                    state: route
                        .params
                        .get("state")
                        .filter(|v| {
                            matches!(
                                v.as_str(),
                                "open" | "blocked" | "closed" | "deleted" | "all"
                            )
                        })
                        .cloned()
                        .unwrap_or_else(|| "open".into()),
                    mine: false,
                    unassigned: owner == "unassigned",
                    assignee: (!matches!(owner, "all" | "unassigned"))
                        .then(|| if owner == "mine" { "human:boss" } else { owner }.into()),
                    labels: route
                        .params
                        .get("label")
                        .filter(|v| !v.is_empty())
                        .cloned()
                        .into_iter()
                        .collect(),
                    search: route
                        .params
                        .get("search")
                        .filter(|v| !v.is_empty())
                        .cloned(),
                    limit: 50,
                    offset: 0,
                    all: true,
                },
            )?
        }
        "artifact" => execute(
            &route,
            options,
            Operation::Artifact {
                operation: hey_boss::artifacts::Operation::View {
                    id: route.id.clone(),
                },
            },
        )?,
        "artifacts" => execute(
            &route,
            options,
            Operation::Artifact {
                operation: hey_boss::artifacts::Operation::List {
                    query: None,
                    archived: false,
                    offset: 0,
                },
            },
        )?,
        "node" => execute(
            &route,
            options,
            Operation::Mindmap {
                operation: hey_boss::mindmap::Operation::View {
                    node: route.id.clone(),
                    body_mode: hey_boss::mindmap::BodyMode::Full,
                },
            },
        )?,
        "mindmap" => execute(
            &route,
            options,
            Operation::Mindmap {
                operation: hey_boss::mindmap::Operation::Show {
                    body_mode: hey_boss::mindmap::BodyMode::Full,
                },
            },
        )?,
        "notice" | "inbox" => {
            // Inbox navigation may retain an issue backend's host. Like the web,
            // read notices from the connected desktop rather than that issue host.
            if options.host.is_some() {
                return Err(Error::invalid(
                    "Inbox belongs to the connected desktop; run lookup there without --host",
                ));
            }
            hey_boss::notices::execute(&if route.entity == "notice" {
                hey_boss::notices::Action::View {
                    task_id: route.id.clone(),
                }
            } else {
                hey_boss::notices::Action::List
            })?
        }
        "agent" => hey_boss::agent_conversations::conversation(
            route.host.as_deref().unwrap(),
            &route.id,
            &hey_boss::agent_conversations::Window::default(),
        )?,
        "agents" => hey_boss::agent_conversations::overview()?,
        _ => return Err(Error::invalid("Unsupported resource")),
    };
    if hey_boss::mindmap::needs_inbox(&value) {
        hey_boss::mindmap::enrich_notifications(
            &mut value,
            hey_boss::notices::execute(&hey_boss::notices::Action::List),
        )?;
    }
    if route.entity == "node" && value.get("node").is_none() {
        return Err(Error::new(
            if value["notifications"]["available"] == false {
                "inbox_unavailable"
            } else {
                "not_found"
            },
            "This notification node is not confirmed pending in Inbox",
        ));
    }
    if route.entity == "agents"
        && let Some(project) = &route.project
    {
        for machine in value["machines"].as_array_mut().into_iter().flatten() {
            for worker in machine["workers"].as_array_mut().into_iter().flatten() {
                if let Some(runs) = worker["runs"].as_array_mut() {
                    runs.retain(|run| run["project_id"] == *project);
                }
            }
        }
    }
    value["ok"] = json!(true);
    value["route"] = serde_json::to_value(&route)?;
    if options.json {
        println!("{}", serde_json::to_string(&value)?);
    } else {
        print_text(&route, &value);
    }
    Ok(())
}
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}
fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
fn print_text(route: &Route, value: &Value) {
    if matches!(route.entity.as_str(), "issue" | "issues") {
        if route.entity == "issue" {
            println!(
                "Resolved {}#{}\n",
                clean(field(&value["project"], "name")),
                route.id
            );
        }
        crate::issue_cli::print_text(value);
        return;
    }
    println!(
        "{}{}",
        route.entity,
        if route.id.is_empty() {
            String::new()
        } else {
            format!(" · {}", clean(&route.id))
        }
    );
    if let Some(resource) = value
        .get("artifact")
        .or_else(|| value.get("node"))
        .or_else(|| value.get("task"))
    {
        println!("{}", clean(field(resource, "title")));
        if let Some(state) = resource["state"]
            .as_str()
            .or_else(|| resource["status"].as_str())
        {
            println!("{}", clean(state));
        }
        if let Some(version) = resource["version"].as_i64() {
            println!("Revision: {version}");
        }
        let body = resource["body"].as_str().unwrap_or_else(|| {
            if matches!(field(resource, "kind"), "update" | "alert") {
                field(resource, "question")
            } else {
                resource["description"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| field(resource, "question"))
            }
        });
        if !body.is_empty() {
            println!("\n{}", clean(body));
        }
        if let Some(reference) = resource["reference"].as_str() {
            println!("\nReference: {}", clean(reference));
        }
        for comment in value["comments"].as_array().into_iter().flatten() {
            println!(
                "\nComment {}{}\n{}",
                comment["id"],
                if comment["resolved"] == true {
                    " (resolved)"
                } else {
                    ""
                },
                clean(field(comment, "body"))
            );
        }
        for backlink in value["backlinks"].as_array().into_iter().flatten() {
            println!("Linked: {}", clean(field(backlink, "title")));
        }
        return;
    }
    match route.entity.as_str() {
        "agent" => {
            for message in value["messages"].as_array().into_iter().flatten() {
                println!(
                    "\n{}\n{}",
                    clean(field(message, "role")),
                    clean(field(message, "text"))
                );
            }
            if value["more"] == true {
                println!("\nMore conversation available; --json includes the page cursor.");
            }
        }
        "agents" => {
            for machine in value["machines"].as_array().into_iter().flatten() {
                for worker in machine["workers"].as_array().into_iter().flatten() {
                    for run in worker["runs"].as_array().into_iter().flatten() {
                        println!(
                            "{} · {} · {} [{}]",
                            clean(field(machine, "host")),
                            clean(field(run, "id")),
                            clean(field(run, "title")),
                            clean(field(run, "state"))
                        );
                    }
                }
            }
        }
        "artifacts" | "mindmap" | "inbox" => {
            let key = match route.entity.as_str() {
                "artifacts" => "artifacts",
                "mindmap" => "nodes",
                _ => "tasks",
            };
            let entries = value[key].as_array().map(Vec::as_slice).unwrap_or_default();
            if entries.is_empty() {
                println!("No matching items.");
            }
            for item in entries {
                println!(
                    "{} · {}",
                    clean(
                        item["id"]
                            .as_str()
                            .or_else(|| item["taskID"].as_str())
                            .unwrap_or("")
                    ),
                    clean(field(item, "title"))
                );
            }
            if value["more"] == true {
                println!("\nMore items available in the web app.");
            }
        }
        _ => {}
    }
}
