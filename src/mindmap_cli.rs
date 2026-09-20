use clap::{Args, Subcommand};
use hey_boss::{
    issues::{self, Error, Result, Store},
    mindmap::{self, BodyMode, Operation},
};
use serde_json::Value;
use std::{collections::HashMap, io::Read, path::PathBuf};

#[derive(Args)]
#[command(
    after_help = "Selectors: alias, n-ID, PROJECT::alias, issue:123, pr:URL, notice:TASK_ID.\nLinks can add typed references in one step. A depends-on B means A waits for B.\nExamples:\n  hey-boss mm add 'Release' --id release\n  hey-boss mm issue 12 --under release\n  hey-boss mm link issue:12 other-project::plan --description 'Shared rollout'\n  hey-boss mm link pr:https://github.com/org/repo/pull/2 pr:https://github.com/org/repo/pull/1 --kind depends-on\n  hey-boss mm web"
)]
pub struct Options {
    /// Full project ID or unambiguous name; defaults to this repository.
    #[arg(long, global = true)]
    project: Option<String>,
    /// Authoritative SSH issue host (also HEY_BOSS_ISSUE_HOST).
    #[arg(long, global = true)]
    host: Option<String>,
    /// Stable author identity; follows issue command defaults.
    #[arg(long, global = true)]
    agent: Option<String>,
    #[arg(long, global = true)]
    pub json: bool,
    /// Deduplicate an identical mutation retry.
    #[arg(long, global = true)]
    request_id: Option<String>,
    /// Reject mutations if the selected map has changed.
    #[arg(long, global = true)]
    if_version: Option<i64>,
    #[command(subcommand)]
    action: Option<Action>,
}
#[derive(Args)]
struct Body {
    /// Markdown text; '-' reads stdin, up to 1 MiB.
    #[arg(long, conflicts_with = "file")]
    body: Option<String>,
    /// Copy a Markdown file; '-' reads stdin.
    #[arg(long, conflicts_with = "body")]
    file: Option<PathBuf>,
}
impl Body {
    fn read(&self) -> Result<Option<String>> {
        let value = if self.body.as_deref() == Some("-")
            || self.file.as_deref() == Some(std::path::Path::new("-"))
        {
            Some(read(std::io::stdin())?)
        } else if let Some(path) = &self.file {
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)?;
            if !file.metadata()?.is_file() {
                return Err(Error::invalid("Markdown path must be a regular file"));
            }
            Some(read(file)?)
        } else {
            self.body.clone()
        };
        if value.as_ref().is_some_and(|s| s.len() > issues::BODY_LIMIT) {
            return Err(Error::invalid("Markdown exceeds 1 MiB"));
        }
        Ok(value)
    }
}
fn read(reader: impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(issues::BODY_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > issues::BODY_LIMIT {
        return Err(Error::invalid("Markdown exceeds 1 MiB"));
    }
    String::from_utf8(bytes).map_err(|_| Error::invalid("Markdown must be UTF-8"))
}
#[derive(Args)]
struct Placement {
    /// Readable project-local alias for future commands.
    #[arg(long = "id")]
    alias: Option<String>,
    /// Parent node; omit for a top-level node.
    #[arg(long)]
    under: Option<String>,
}
#[derive(Subcommand)]
enum Action {
    /// Show the nested outline, live resources and cross-links (also the default).
    #[command(visible_alias = "list")]
    Show {
        /// Include bodies: default none in the terminal, full in JSON.
        #[arg(long, value_enum)]
        bodies: Option<BodyMode>,
    },
    /// Read one node's live text/Markdown, without loading the whole map.
    View {
        node: String,
        /// Include bodies: default full; preview is up to 512 characters.
        #[arg(long, value_enum, default_value = "full")]
        bodies: BodyMode,
    },
    /// List projects with maps.
    Projects,
    /// Export the nested outline and relationships as Markdown to stdout.
    Export,
    /// Add a text topic, optionally with a Markdown body.
    Add {
        title: String,
        #[command(flatten)]
        body: Body,
        #[command(flatten)]
        placement: Placement,
    },
    /// Add a live issue reference; attached PRs appear automatically.
    Issue {
        number: i64,
        #[arg(long)]
        issue_project: Option<String>,
        /// Map-only label; updates the label if this reference already exists.
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        placement: Placement,
    },
    /// Add a PR that can participate in dependency links.
    Pr {
        url: String,
        /// Readable map label; defaults to the PR URL.
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        placement: Placement,
    },
    /// Add a notification reference; only pending notices appear in the view.
    #[command(visible_alias = "notification")]
    Notice {
        task_id: String,
        #[command(flatten)]
        placement: Placement,
    },
    /// Edit topic text/Markdown or a map-only issue/PR label.
    Edit {
        node: String,
        #[arg(long)]
        title: Option<String>,
        /// Restore a live issue's original title in this map.
        #[arg(long, conflicts_with_all = ["title", "body", "file"])]
        clear_label: bool,
        #[command(flatten)]
        body: Body,
    },
    /// Atomically update labels, aliases, moves and directed links from a JSON array.
    #[command(after_help = BATCH_HELP)]
    Batch {
        /// JSON file; '-' reads stdin, up to 1 MiB.
        #[arg(long)]
        file: PathBuf,
        /// Validate and preview the transaction without saving changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Set or clear a node's readable alias, preserving its identity and links.
    Alias {
        node: String,
        #[arg(required_unless_present = "clear", conflicts_with = "clear")]
        alias: Option<String>,
        #[arg(long)]
        clear: bool,
    },
    /// Move to a parent/root, optionally before or after a sibling.
    Move {
        node: String,
        #[arg(long)]
        under: Option<String>,
        #[arg(long, conflicts_with = "after")]
        before: Option<String>,
        #[arg(long)]
        after: Option<String>,
    },
    /// Remove a map node, preserving its underlying resource.
    #[command(visible_alias = "rm")]
    Remove {
        node: String,
        #[arg(long)]
        recursive: bool,
    },
    /// Create/update a directed link; descriptions are optional.
    Link {
        from: String,
        to: String,
        #[arg(long, default_value = "related")]
        kind: String,
        #[arg(long, visible_alias = "why")]
        description: Option<String>,
    },
    /// Remove one directed link without removing either node.
    Unlink {
        from: String,
        to: String,
        #[arg(long, default_value = "related")]
        kind: String,
    },
    /// Show incoming and outgoing links, including automatic issue→PR links.
    Links { node: Option<String> },
    /// Serve the read-only nested-list viewer on localhost.
    Web {
        #[arg(long, default_value_t = 4781)]
        port: u16,
        #[arg(long)]
        no_discovery: bool,
    },
}

const BATCH_HELP: &str = r#"JSON input format:
  An array of objects. Every object requires "command".
  "command" is exactly "edit", "alias", "move" or "link" (not "action").
  edit, alias and move require "node".
  "node" is an existing selector: alias, n-ID, PROJECT::alias,
  issue:NUMBER, pr:URL or notice:TASK_ID, in the selected map.

  edit:  "title" (string) or "clear_label":true is required.
         "title" changes topic/PR text or an issue's map-only label.
         "clear_label" restores an issue's live title; default false.
         Do not combine "title" with "clear_label":true.
  alias: "alias" (string) sets an alias; null or omission clears it.
  move:  Optional "under", "before", "after" are selector strings.
         Null or omitted "under" moves to the root.
         Use at most one of "before" or "after", in the target parent.
         Null or omitted sibling anchors append to the parent's end.
  link:  Required "from", "to" (selectors) and "kind" (string).
         Adds/updates the directed connection from source to target.
         "description" is optional text, up to 16 KiB (not "why").
         Null, omission or blank text clears an existing description.
         "kind" is nonblank, at most 64 bytes, without control characters;
         "pull-request" is reserved for automatic issue PR connections.
         Self-links are rejected. Cross-project endpoints are supported.
         Missing typed issue/PR/notice references are added as by mm link;
         missing aliases or node IDs are rejected.

  Unknown fields and commands are rejected; body edits are unsupported.
  Limits: 1 MiB of UTF-8 JSON and 10,000 entries.

Example edits.json (all selectors must already exist):
[
  {"command":"edit","node":"issue:1","title":"Keep replies safe"},
  {"command":"edit","node":"issue:2","clear_label":true},
  {"command":"alias","node":"followup","alias":"reply-followup"},
  {"command":"alias","node":"archived","alias":null},
  {"command":"move","node":"followup","under":"existing-topic",
   "before":"existing-sibling"},
  {"command":"move","node":"archived","after":"existing-topic"},
  {"command":"move","node":"loose"},
  {"command":"link","from":"followup","to":"issue:1",
   "kind":"depends-on","description":"Replies need this fix"},
  {"command":"link","from":"existing-topic","to":"issue:2",
   "kind":"related","description":null}
]

Preview and apply (replace 42 with the version from hey-boss mm show --json):
  hey-boss mm batch --file edits.json --dry-run --if-version 42 --json
  hey-boss mm batch --file - --dry-run --if-version 42 --json < edits.json
  hey-boss mm batch --file edits.json --if-version 42 \
    --request-id organize-replies --json

Selectors bind before any edits. Later entries must use the original alias
or stable node ID, not a new alias introduced earlier in the array.
Entries then execute in order. Invalid selectors, descriptions, kinds,
alias/label collisions, cycles or a stale version roll back the whole transaction.
Dry runs save nothing and cannot use --request-id. A changed batch advances
the map version once; empty/net no-op batches preserve it. Retry an identical
commit with the same --request-id to receive its original result.
JSON includes changed_nodes, changed_links (stable from/to/kind and before/after
description metadata; null before means a new link), base_version, version and
affected_projects. Each affected map advances once; a dry run reports proposed
versions. Underlying resources and unrelated links are preserved."#;

impl Options {
    fn operation(&self) -> Result<Operation> {
        let add = |title: String,
                   body: String,
                   kind: &str,
                   reference: Option<String>,
                   reference_project: Option<String>,
                   p: &Placement| Operation::Add {
            title,
            display_label: None,
            body,
            kind: kind.into(),
            reference,
            reference_project,
            alias: p.alias.clone(),
            under: p.under.clone(),
            if_version: self.if_version,
        };
        Ok(match self.action.as_ref() {
            None => Operation::Show {
                body_mode: if self.json {
                    BodyMode::Full
                } else {
                    BodyMode::None
                },
            },
            Some(Action::Export) => Operation::Show {
                body_mode: BodyMode::Full,
            },
            Some(Action::Show { bodies }) => Operation::Show {
                body_mode: bodies.unwrap_or(if self.json {
                    BodyMode::Full
                } else {
                    BodyMode::None
                }),
            },
            Some(Action::View { node, bodies }) => Operation::View {
                node: node.clone(),
                body_mode: *bodies,
            },
            Some(Action::Projects) => Operation::Projects,
            Some(Action::Batch { file, dry_run }) => {
                let input = Body {
                    body: None,
                    file: Some(file.clone()),
                }
                .read()?
                .unwrap();
                Operation::Batch {
                    edits: serde_json::from_str(&input)
                        .map_err(|e| Error::invalid(format!("Invalid batch JSON: {e}")))?,
                    dry_run: *dry_run,
                    if_version: self.if_version,
                }
            }
            Some(Action::Add {
                title,
                body,
                placement,
            }) => {
                let body = body.read()?.unwrap_or_default();
                add(
                    title.clone(),
                    body.clone(),
                    if body.is_empty() { "text" } else { "markdown" },
                    None,
                    None,
                    placement,
                )
            }
            Some(Action::Issue {
                number,
                issue_project,
                title,
                placement,
            }) => Operation::Add {
                title: format!("Issue #{number}"),
                display_label: title.clone(),
                body: String::new(),
                kind: "issue".into(),
                reference: Some(number.to_string()),
                reference_project: issue_project.clone(),
                alias: placement.alias.clone(),
                under: placement.under.clone(),
                if_version: self.if_version,
            },
            Some(Action::Pr {
                url,
                title,
                placement,
            }) => add(
                title.clone().unwrap_or_else(|| url.clone()),
                String::new(),
                "pr",
                Some(url.clone()),
                None,
                placement,
            ),
            Some(Action::Notice { task_id, placement }) => add(
                format!("Notification {task_id}"),
                String::new(),
                "notification",
                Some(task_id.clone()),
                None,
                placement,
            ),
            Some(Action::Edit {
                node,
                title,
                body,
                clear_label,
            }) => Operation::Edit {
                node: node.clone(),
                title: title.clone(),
                body: body.read()?,
                clear_label: *clear_label,
                if_version: self.if_version,
            },
            Some(Action::Alias { node, alias, .. }) => Operation::Alias {
                node: node.clone(),
                alias: alias.clone(),
                if_version: self.if_version,
            },
            Some(Action::Move {
                node,
                under,
                before,
                after,
            }) => Operation::Move {
                node: node.clone(),
                under: under.clone(),
                before: before.clone(),
                after: after.clone(),
                if_version: self.if_version,
            },
            Some(Action::Remove { node, recursive }) => Operation::Remove {
                node: node.clone(),
                recursive: *recursive,
                if_version: self.if_version,
            },
            Some(Action::Link {
                from,
                to,
                kind,
                description,
            }) => Operation::Link {
                from: from.clone(),
                to: to.clone(),
                kind: kind.clone(),
                description: description.clone(),
                if_version: self.if_version,
            },
            Some(Action::Unlink { from, to, kind }) => Operation::Unlink {
                from: from.clone(),
                to: to.clone(),
                kind: kind.clone(),
                if_version: self.if_version,
            },
            Some(Action::Links { node }) => Operation::Links { node: node.clone() },
            Some(Action::Web { .. }) => unreachable!(),
        })
    }
}
pub fn run(options: &Options) -> Result<()> {
    let host = options.host.clone().or_else(|| {
        std::env::var("HEY_BOSS_ISSUE_HOST")
            .ok()
            .filter(|s| !s.is_empty())
    });
    let project_override = options.project.clone().or_else(|| {
        std::env::var("HEY_BOSS_ISSUE_PROJECT")
            .ok()
            .filter(|s| !s.is_empty())
    });
    if let Some(Action::Web { port, no_discovery }) = &options.action {
        return issues::web::serve(issues::web::Config {
            port: *port,
            mobile_origin: None,
            discover: !*no_discovery,
            project: project_override,
            actor: options.agent.clone(),
            host,
            json: options.json,
            mindmap: true,
        });
    }
    let operation = options.operation()?;
    operation.validate()?;
    if !operation.writes() && (options.if_version.is_some() || options.request_id.is_some()) {
        return Err(Error::invalid(
            "--if-version and --request-id apply only to mutations",
        ));
    }
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
    let request = issues::Request {
        version: 1,
        project: issues::identity::project(&cwd, &machine)?,
        project_override,
        actor,
        operation: issues::Operation::Mindmap { operation },
        request_id: options.request_id.clone(),
    };
    let mut graph = match host {
        Some(host) => issues::remote::call(&host, &request)?,
        None => Store::open(&issues::database_path()?)?.execute(&request)?,
    };
    if mindmap::needs_inbox(&graph) {
        mindmap::enrich_notifications(
            &mut graph,
            hey_boss::notices::execute(&hey_boss::notices::Action::List),
        )?;
    }
    if options.json {
        println!("{}", serde_json::to_string(&graph)?);
        return Ok(());
    }
    if matches!(options.action, Some(Action::View { .. })) {
        if let Some(node) = graph.get("node") {
            println!(
                "{} [{}]{}",
                node["title"].as_str().unwrap_or("Untitled"),
                node["kind"].as_str().unwrap_or("text"),
                node["state"]
                    .as_str()
                    .map(|state| format!(" ({state})"))
                    .unwrap_or_default()
            );
            if let Some(body) = node["body"].as_str().filter(|body| !body.is_empty()) {
                println!("\n{body}");
            }
            if node["kind"] == "pr" {
                println!("\n{}", node["reference"].as_str().unwrap());
            } else if node["kind"] == "issue" {
                if let Some(title) = node["original_title"]
                    .as_str()
                    .filter(|_| node["display_label"].is_string())
                {
                    println!("\nOriginal title: {title}");
                }
                if let Some(labels) = node["labels"]
                    .as_array()
                    .filter(|labels| !labels.is_empty())
                {
                    println!(
                        "Labels: {}",
                        labels
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                println!(
                    "\n{} · issue #{} · {}",
                    node["reference_project_name"]
                        .as_str()
                        .or_else(|| node["reference_project"].as_str())
                        .unwrap_or("Unknown project"),
                    node["reference"].as_str().unwrap(),
                    node["reference_project"].as_str().unwrap()
                );
                if let Some(assignee) = node["assignee"].as_str() {
                    if assignee == "human:boss" {
                        let name = graph["boss"]["name"].as_str().unwrap_or("Boss");
                        println!("Assigned to {name} ({assignee})");
                    } else {
                        println!("Assigned to {assignee}");
                    }
                }
            }
        } else {
            println!("This notification is not confirmed pending.");
        }
        if graph["notifications"]["available"] == false {
            eprintln!(
                "Inbox unavailable: {}",
                graph["notifications"]["error"]
                    .as_str()
                    .unwrap_or("Unknown error")
            );
        }
        return Ok(());
    }
    if matches!(options.action, Some(Action::Projects)) {
        for p in graph["projects"].as_array().unwrap() {
            println!(
                "{} · {} nodes · {}",
                p["name"].as_str().unwrap(),
                p["node_count"],
                p["id"].as_str().unwrap()
            );
        }
        return Ok(());
    }
    let export = matches!(options.action, Some(Action::Export));
    let display_text = |text: &str| {
        if export {
            markdown_text(text)
        } else {
            text.to_owned()
        }
    };
    if matches!(options.action, Some(Action::Batch { .. })) {
        println!(
            "{} · map version {}",
            if graph["dry_run"] == true {
                "Preview"
            } else if graph["changed"] == true {
                "Saved"
            } else {
                "Unchanged"
            },
            graph["version"]
        );
        for node in graph["changed_nodes"].as_array().unwrap() {
            println!(
                "{}: {} → {}",
                node["id"].as_str().unwrap(),
                node["before"],
                node["after"]
            );
        }
        for link in graph["changed_links"].as_array().unwrap() {
            println!(
                "{} → {} ({}): {} → {}",
                link["from"].as_str().unwrap(),
                link["to"].as_str().unwrap(),
                link["kind"].as_str().unwrap(),
                link["before"],
                link["after"]
            );
        }
        return Ok(());
    }
    if request.operation.writes() {
        println!(
            "{} · map version {}",
            if graph["changed"] == true {
                "Saved"
            } else {
                "Unchanged"
            },
            graph["version"]
        );
        if let Some(node) = graph.get("node") {
            println!(
                "{}{}",
                node["id"].as_str().unwrap(),
                node["alias"]
                    .as_str()
                    .map(|a| format!(" ({a})"))
                    .unwrap_or_default()
            );
        }
        return Ok(());
    }
    if !matches!(options.action, Some(Action::Links { .. })) {
        println!(
            "{}{}{}",
            if export { "# " } else { "" },
            display_text(graph["project"]["name"].as_str().unwrap()),
            if export {
                String::new()
            } else {
                format!(" · map version {}", graph["version"])
            }
        );
        let include_bodies = export
            || matches!(
                options.action,
                Some(Action::Show {
                    bodies: Some(BodyMode::Full | BodyMode::Preview)
                })
            );
        outline(&graph, None, 0, export, include_bodies);
        if graph["nodes"].as_array().unwrap().is_empty() {
            println!("No nodes yet. Use: hey-boss mm add 'Topic' --id topic");
        }
    }
    let links = graph["links"].as_array().unwrap();
    if !links.is_empty() {
        println!(
            "\n{}",
            if export {
                "## Relationships"
            } else {
                "Relationships"
            }
        );
    }
    let labels = labels(&graph);
    for link in links {
        println!(
            "- {} → {} [{}]{}{}",
            display_text(label(&labels, &link["from"])),
            display_text(label(&labels, &link["to"])),
            display_text(link["kind"].as_str().unwrap()),
            link["description"]
                .as_str()
                .map(|s| format!(" — {}", display_text(s)))
                .unwrap_or_default(),
            if link["automatic"] == true {
                " (automatic)"
            } else {
                ""
            }
        );
    }
    if graph["notifications"]["available"] == false {
        eprintln!(
            "Inbox unavailable: {}",
            graph["notifications"]["error"]
                .as_str()
                .unwrap_or("Unknown error")
        );
    }
    Ok(())
}
fn labels(graph: &Value) -> HashMap<&str, String> {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .chain(graph["external_nodes"].as_array().unwrap())
        .map(|n| {
            let project = if n["project_id"] != graph["project"]["id"] {
                format!("{}::", n["project_id"].as_str().unwrap())
            } else {
                String::new()
            };
            let label = format!(
                "{project}{}",
                n["alias"]
                    .as_str()
                    .or_else(|| n["title"].as_str())
                    .unwrap_or("Unknown")
            );
            (n["id"].as_str().unwrap(), label)
        })
        .collect()
}
fn label<'a>(labels: &'a HashMap<&str, String>, id: &'a Value) -> &'a str {
    let id = id.as_str().unwrap_or("Unknown");
    labels.get(id).map(String::as_str).unwrap_or(id)
}
fn outline(
    graph: &Value,
    parent: Option<&str>,
    depth: usize,
    markdown: bool,
    include_bodies: bool,
) {
    let mut children: HashMap<Option<&str>, Vec<&Value>> = HashMap::new();
    for node in graph["nodes"].as_array().unwrap() {
        children
            .entry(node["parent_id"].as_str())
            .or_default()
            .push(node);
    }
    outline_nodes(&children, parent, depth, markdown, include_bodies);
}
fn outline_nodes(
    children: &HashMap<Option<&str>, Vec<&Value>>,
    parent: Option<&str>,
    depth: usize,
    markdown: bool,
    include_bodies: bool,
) {
    for node in children.get(&parent).into_iter().flatten() {
        let title = node["title"].as_str().unwrap_or("Untitled");
        let title = if markdown && node["kind"] == "pr" {
            format!(
                "[{}](<{}>)",
                markdown_text(title),
                node["reference"]
                    .as_str()
                    .unwrap()
                    .replace('\\', "%5C")
                    .replace('<', "%3C")
                    .replace('>', "%3E")
            )
        } else if markdown {
            markdown_text(title)
        } else {
            title.to_owned()
        };
        println!(
            "{}- {}{}{}{}",
            "  ".repeat(depth),
            title,
            if node["kind"] != "text" {
                format!(" [{}]", node["kind"].as_str().unwrap())
            } else {
                String::new()
            },
            node["state"]
                .as_str()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default(),
            if !markdown {
                format!(
                    " · {}",
                    node["alias"]
                        .as_str()
                        .unwrap_or(node["id"].as_str().unwrap())
                )
            } else {
                String::new()
            }
        );
        if include_bodies {
            for line in node["body"].as_str().unwrap_or("").lines() {
                println!("{}{}", "  ".repeat(depth + 1), line);
            }
        }
        outline_nodes(
            children,
            node["id"].as_str(),
            depth + 1,
            markdown,
            include_bodies,
        );
    }
}
fn markdown_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii_punctuation() {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}
