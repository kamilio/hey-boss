use clap::{Args, CommandFactory, Parser, Subcommand};
use hey_boss::{
    admin::{Temporary, capture},
    issues::{Request, Store},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::Path,
    process::Command,
};

#[derive(Args)]
pub struct Options {
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub action: Action,
}
#[derive(Subcommand)]
pub enum Action {
    /// List every CLI command, its help, aliases and preview coverage.
    Catalog,
    /// Capture text and JSON output in disposable sample state.
    Preview {
        command: String,
        #[arg(long, hide = true)]
        seed_stdin: bool,
    },
    #[command(hide = true)]
    Capture {
        command: String,
        root: std::path::PathBuf,
    },
}
#[derive(Args)]
pub struct SkillOptions {
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub action: SkillAction,
}
#[derive(Subcommand)]
pub enum SkillAction {
    /// Print the canonical Markdown skill.
    Show,
    /// Install the bundled skill and sync selected skills for Codex, Agents and Claude Code.
    Install,
    /// List global and project skills with agent drift and length policy warnings.
    #[command(visible_alias = "audit", visible_alias = "status")]
    List {
        /// Optional project directory to scan for project skills (defaults to current working directory).
        #[arg(long = "project-dir")]
        project_dir: Option<std::path::PathBuf>,
        /// Scan all fleet machines instead of only the local directory.
        #[arg(long)]
        fleet: bool,
    },
    /// Scan all fleet machines for skills and show machine coverage tags.
    #[command(visible_alias = "fleet")]
    Scan,
    /// Distribute and unify selected (or specified) skills across all fleet machines (Codex, Claude, Agents).
    #[command(visible_alias = "unify", visible_alias = "push")]
    Distribute {
        /// Optional skill name(s) to distribute immediately across all machines (defaults to all selected skills).
        names: Vec<String>,
        /// Source machine host (e.g. local, devbox, kamils-macbook-pro) or digest prefix to keep when versions differ.
        #[arg(long = "from")]
        from: Option<String>,
    },
    /// Delete a skill from all connected fleet machines (saving a backup in .hey-boss/skill-backups).
    #[command(visible_alias = "rm", visible_alias = "remove")]
    Delete {
        /// Name of the skill to delete across all machines.
        name: String,
    },
    /// Sync selected (or specified) skills across Codex (.codex), Claude (.claude), Agents (.agents), and optional SSH hosts.
    Sync {
        /// Specific skill names to add to the sync selection and sync immediately.
        names: Vec<String>,
        /// Optional remote SSH host(s) to receive the synced skill bundle.
        #[arg(long = "host")]
        hosts: Vec<String>,
    },
    /// Add skills to the selected sync set and sync them across agents.
    Select {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Remove skills from the selected sync set.
    Unselect {
        #[arg(required = true)]
        names: Vec<String>,
    },
}
pub fn skill(options: &SkillOptions) -> io::Result<()> {
    let home_os = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is missing"))?;
    let home = Path::new(&home_os);
    match &options.action {
        SkillAction::Show if options.json => println!(
            "{}",
            json!({"markdown":hey_boss::skill::MARKDOWN,"source":"skills/hey-boss/SKILL.md","references":hey_boss::skill::references()})
        ),
        SkillAction::Show => print!("{}", hey_boss::skill::MARKDOWN),
        SkillAction::Install => return install_skill(home, options.json),
        SkillAction::List { project_dir, fleet } => {
            if *fleet {
                let report = hey_boss::skill::manager::scan_blocking()
                    .map_err(|e| io::Error::other(e.to_string()))?;
                if options.json {
                    println!("{report}");
                } else {
                    print_fleet_skill_report(&report);
                }
                return Ok(());
            }
            let cwd = project_dir.clone().or_else(|| std::env::current_dir().ok());
            let report = hey_boss::skill::audit_report(home, cwd.as_deref());
            if options.json {
                println!("{report}");
            } else {
                print_skill_audit(&report);
            }
        }
        SkillAction::Scan => {
            let report = hey_boss::skill::manager::scan_blocking()
                .map_err(|e| io::Error::other(e.to_string()))?;
            if options.json {
                println!("{report}");
            } else {
                print_fleet_skill_report(&report);
            }
        }
        SkillAction::Distribute { names, from } => {
            let report = hey_boss::skill::manager::distribute_blocking(names, from.as_deref())
                .map_err(|e| io::Error::other(e.to_string()))?;
            if options.json {
                println!("{report}");
            } else {
                println!(
                    "{}",
                    report["message"]
                        .as_str()
                        .unwrap_or("Distributed skills across fleet.")
                );
                print_fleet_skill_report(&report);
            }
        }
        SkillAction::Delete { name } => {
            let report = hey_boss::skill::manager::delete_blocking(name)
                .map_err(|e| io::Error::other(e.to_string()))?;
            if options.json {
                println!("{report}");
            } else {
                println!(
                    "{}",
                    report["message"]
                        .as_str()
                        .unwrap_or("Deleted skill across fleet.")
                );
            }
        }
        SkillAction::Sync { names, hosts } => {
            let paths = hey_boss::skill::sync_skills(
                home,
                if names.is_empty() {
                    None
                } else {
                    Some(names.as_slice())
                },
            )?;
            let mut synced_hosts = Vec::new();
            for host in hosts {
                push_skill_bundle_ssh(home, host)?;
                synced_hosts.push(host.clone());
            }
            if options.json {
                println!(
                    "{}",
                    json!({"ok":true,"paths":paths,"hosts":synced_hosts,"selected":hey_boss::skill::selected_skills(home)})
                );
            } else {
                println!(
                    "Synced {} skill files across Codex, Claude, and Agents.",
                    paths.len()
                );
                for host in &synced_hosts {
                    println!("Synced skill bundle to host {host}.");
                }
            }
        }
        SkillAction::Select { names } => {
            let mut current: Vec<String> =
                hey_boss::skill::selected_skills(home).into_iter().collect();
            current.extend(names.iter().cloned());
            let updated = hey_boss::skill::set_selected_skills(home, &current)?;
            let paths = hey_boss::skill::sync_skills(home, None)?;
            if options.json {
                println!("{}", json!({"ok":true,"selected":updated,"paths":paths}));
            } else {
                println!(
                    "Selected skills: {}",
                    updated.into_iter().collect::<Vec<_>>().join(", ")
                );
            }
        }
        SkillAction::Unselect { names } => {
            let remove: std::collections::BTreeSet<&str> =
                names.iter().map(String::as_str).collect();
            let remaining: Vec<String> = hey_boss::skill::selected_skills(home)
                .into_iter()
                .filter(|s| !remove.contains(s.as_str()))
                .collect();
            let updated = hey_boss::skill::set_selected_skills(home, &remaining)?;
            if options.json {
                println!("{}", json!({"ok":true,"selected":updated}));
            } else {
                println!(
                    "Selected skills: {}",
                    updated.into_iter().collect::<Vec<_>>().join(", ")
                );
            }
        }
    }
    Ok(())
}

fn push_skill_bundle_ssh(home: &Path, host: &str) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let archive = hey_boss::skill::archive_for_home(Some(home))?;
    let mut child = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            host,
            &hey_boss::skill::remote_install_script(),
        ])
        .stdin(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        std::io::Write::write_all(&mut stdin, &archive)?;
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "SSH skill sync failed for {host}"
        )));
    }
    Ok(())
}

fn print_skill_audit(report: &serde_json::Value) {
    println!(
        "Global Skills (policy max: {} lines):",
        report["max_lines_policy"]
    );
    if let Some(skills) = report["global_skills"].as_array() {
        for s in skills {
            let mark = if s["selected"] == true { "[x]" } else { "[ ]" };
            let agents: Vec<&str> = ["codex", "claude", "agents"]
                .into_iter()
                .filter(|a| s["agents"][*a] == true)
                .collect();
            println!(
                "  {mark} {} ({} lines) · agents: [{}]",
                s["name"].as_str().unwrap_or(""),
                s["line_count"],
                agents.join(", ")
            );
            if let Some(warnings) = s["warnings"].as_array() {
                for w in warnings {
                    println!(
                        "      Warning [{}]: {}",
                        w["kind"].as_str().unwrap_or(""),
                        w["message"].as_str().unwrap_or("")
                    );
                }
            }
        }
    }
    if let Some(proj) = report["project_skills"]
        .as_array()
        .filter(|p| !p.is_empty())
    {
        println!(
            "
Project Skills:"
        );
        for s in proj {
            println!(
                "  • {} ({} lines)",
                s["name"].as_str().unwrap_or(""),
                s["line_count"]
            );
            if let Some(warnings) = s["warnings"].as_array() {
                for w in warnings {
                    println!(
                        "      Warning [{}]: {}",
                        w["kind"].as_str().unwrap_or(""),
                        w["message"].as_str().unwrap_or("")
                    );
                }
            }
        }
    }
}

fn print_fleet_skill_report(report: &serde_json::Value) {
    use std::collections::{BTreeMap, BTreeSet};
    let machines = report["machines"].as_array().cloned().unwrap_or_default();
    let known_hosts: Vec<String> = machines
        .iter()
        .filter_map(|m| m["host"].as_str().map(str::to_owned))
        .collect();
    println!("Machines ({}):", known_hosts.len());
    for m in &machines {
        let host = m["host"].as_str().unwrap_or("?");
        let state = m["state"].as_str().unwrap_or("?");
        let copies = m["copies"]
            .as_array()
            .map(|a| a.iter().filter(|c| c["scope"] == "global").count())
            .unwrap_or(0);
        println!("  - {:<26} [{}] ({} copies)", host, state, copies);
    }
    let mut by_skill: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();
    for m in &machines {
        let host = m["host"]
            .as_str()
            .unwrap_or("?")
            .trim_end_matches(".local")
            .to_owned();
        for c in m["copies"].as_array().into_iter().flatten() {
            if c["scope"] != "global" {
                continue;
            }
            let name = c["name"].as_str().unwrap_or("?").to_owned();
            let digest = c["digest"].as_str().unwrap_or("").to_owned();
            let entry = by_skill.entry(name).or_default();
            entry.0.insert(host.clone());
            entry.1.insert(digest);
        }
    }
    println!("\nFleet Skill Coverage ({} skills):", by_skill.len());
    for (name, (hosts, digests)) in by_skill {
        let on_all = known_hosts.len() > 1 && hosts.len() >= known_hosts.len();
        let in_sync = digests.len() <= 1;
        let tag = if on_all && in_sync {
            "\x1b[32m[ALL MACHINES]\x1b[0m".to_owned()
        } else if !in_sync {
            format!(
                "\x1b[31m[{} · {} versions]\x1b[0m",
                hosts.into_iter().collect::<Vec<_>>().join(", "),
                digests.len()
            )
        } else if hosts.len() <= 1 && known_hosts.len() > 1 {
            format!(
                "\x1b[31m[{}]\x1b[0m",
                hosts.into_iter().collect::<Vec<_>>().join(", ")
            )
        } else {
            format!(
                "\x1b[33m[{}]\x1b[0m",
                hosts.into_iter().collect::<Vec<_>>().join(", ")
            )
        };
        println!("  {:<28} {}", name, tag);
    }
}

fn install_skill(home: &Path, json: bool) -> io::Result<()> {
    let paths = hey_boss::skill::install(home)?;
    if json {
        println!("{}", json!({"ok":true,"paths":paths}));
    } else {
        for path in paths {
            println!("Installed {}", path.display());
        }
    }
    Ok(())
}

#[derive(Clone, Deserialize)]
struct Scenario {
    args: Vec<String>,
    setup: Vec<Vec<String>>,
}
fn scenarios() -> BTreeMap<String, Scenario> {
    serde_json::from_str(include_str!("admin/scenarios.json")).expect("valid preview scenarios")
}
pub fn catalog() -> Value {
    fn walk(
        command: &clap::Command,
        path: &[String],
        samples: &BTreeMap<String, Scenario>,
        output: &mut Vec<Value>,
    ) {
        let id = path.join(" ");
        if path.len() == 1
            && (crate::notif_cli::Action::has_subcommand(&id)
                || crate::canonical_agent_command(&id).is_some())
        {
            return;
        }
        if !path.is_empty() && id != "admin capture" {
            let mut help = command.clone();
            let json_supported = command
                .get_arguments()
                .any(|a| a.get_long() == Some("json"));
            let mut aliases: Vec<_> = command.get_all_aliases().map(str::to_owned).collect();
            if path.len() == 2 && path[0] == "notif" {
                aliases.push(format!("hey-boss {}", path[1]));
            }
            for old in ["agents", "overview", "configure-agents", "agent-control"] {
                if crate::canonical_agent_command(old) == Some(id.as_str()) {
                    aliases.push(format!("hey-boss {old}"));
                }
            }
            output.push(json!({"id":id,"group":path[0],"description":command.get_about().map(ToString::to_string).unwrap_or_default(),"aliases":aliases,"help":help.render_long_help().to_string(),"usage":help.render_usage().to_string(),"json_supported":json_supported,"preview":if samples.contains_key(&id) {"sample"} else {"help"}}));
        }
        for child in command
            .get_subcommands()
            .filter(|c| c.get_name() != "help" && c.get_name() != "capture")
        {
            let mut next = path.to_vec();
            next.push(child.get_name().into());
            walk(child, &next, samples, output);
        }
    }
    let mut root = crate::Cli::command();
    root.build();
    let mut commands = Vec::new();
    walk(&root, &[], &scenarios(), &mut commands);
    let mut prompts = [
        ("worker", "Shared task", hey_boss::issues::worker::DEFAULT_PROMPT),
        ("plan", "Planning", hey_boss::issues::worker::DEFAULT_PLAN_PROMPT),
        ("worktree", "Dedicated worktree", hey_boss::issues::worker::DEFAULT_WORKTREE_PROMPT),
        ("checkout", "Existing checkout", hey_boss::issues::worker::DEFAULT_CHECKOUT_PROMPT),
        ("main", "Push to main", hey_boss::issues::worker::DEFAULT_MAIN_PROMPT),
        ("prs", "Pull request", hey_boss::issues::worker::DEFAULT_PRS_PROMPT),
        ("chief", "Chief", include_str!("issues/prompts/chief.md").trim_ascii_end()),
    ].map(|(id,title,text)|json!({"id":id,"title":title,"text":text,"source":format!("src/issues/prompts/{id}.md")})).to_vec();
    let defaults = hey_boss::issues::worker::prompt_defaults();
    for section in hey_boss::issues::worker::prompt_sections()
        .as_array()
        .unwrap()
    {
        let key = section["key"].as_str().unwrap();
        if !prompts.iter().any(|p| p["id"] == key) {
            prompts.push(json!({"id":key,"title":section["title"],"text":defaults[key],"source":format!("src/issues/prompts/{key}.md")}));
        }
    }
    json!({"commands":commands,"prompts":prompts,"chief_wrapper":hey_boss::issues::worker::chief_instructions("{{project}}","{{prompt}}", &Default::default()),"skill":{"text":hey_boss::skill::MARKDOWN,"source":"skills/hey-boss/SKILL.md","install":"hey-boss skill install","references":hey_boss::skill::references()},"guide":{"text":hey_boss::agent_guidance::GUIDE,"source":"src/issues/web/agent-guide.md"},"build":env!("HEY_BOSS_BUILD_ID")})
}

fn request(root: &Path, operation: Value) -> io::Result<Request> {
    Ok(serde_json::from_value(
        json!({"version":1,"project":{"id":"named:Review sample","name":"Review sample"},"actor":{"id":"human:admin-review","kind":"human","session_id":null,"machine":"review","host":"review","pid":null,"process_start":null,"cwd":root,"source":"admin-preview"},"operation":operation,"request_id":null}),
    )?)
}
fn seed(root: &Path, context: &Value) -> io::Result<Value> {
    let mut store = Store::open(&root.join("issues.db")).map_err(io::Error::other)?;
    let mut call = |op| store.execute(&request(root, op)?).map_err(io::Error::other);
    let title = context["issue"]["title"]
        .as_str()
        .unwrap_or("Simplify agent instructions");
    let body = context["issue"]["body"].as_str().unwrap_or(
        "Review prompts, command output and skill instructions. Preserve the useful context.",
    );
    call(json!({"action":"create","title":title,"body":body,"labels":["review"]}))?;
    call(
        json!({"action":"create","title":"Verify the changes","body":"Check both text and JSON output.","labels":[]}),
    )?;
    call(json!({"action":"comment","number":1,"body":"Keep the next action easy to find."}))?;
    call(json!({"action":"configure_project","prs_enabled":true}))?;
    for (alias, title) in [("review", "Review plan"), ("details", "Output details")] {
        call(
            json!({"action":"mindmap","operation":{"command":"add","kind":"text","title":title,"body":"Review context","alias":alias}}),
        )?;
    }
    let artifact = call(
        json!({"action":"artifact","operation":{"command":"create","title":"Review notes","body":"# Review\n\nInspect the context sent to agents.","issue":1}}),
    )?;
    let attachment = call(
        json!({"action":"attachment","operation":{"command":"upload","target":{"kind":"issue","id":"1"},"name":"sample.md","data":"IyBSZXZpZXcK"}}),
    )?;
    fs::write(root.join("sample.md"), "# Review\n\nA sample attachment.\n")?;
    fs::write(
        root.join("issues.json"),
        serde_json::to_vec(&json!([
            {"number":2,"if_version":1,"expected_assignee":null,"add_labels":["reviewed"],"assignment":"keep"}
        ]))?,
    )?;
    fs::write(
        root.join("map.json"),
        serde_json::to_vec(&json!([
            {"command":"edit","node":"review","title":"Reviewed plan"},
            {"command":"link","from":"review","to":"details","kind":"related","description":"Supporting context"}
        ]))?,
    )?;
    let mut destination = request(
        root,
        json!({"action":"create","title":"Destination context","body":"","labels":[]}),
    )?;
    destination.project.id = "named:Destination review".into();
    destination.project.name = "Destination review".into();
    store.execute(&destination).map_err(io::Error::other)?;
    let variables =
        json!({"artifact":artifact["artifact"]["id"],"attachment":attachment["attachment"]["id"]});
    fs::write(root.join("variables.json"), serde_json::to_vec(&variables)?)?;
    Ok(variables)
}
fn expand(args: &[String], vars: &Value) -> Vec<String> {
    args.iter()
        .map(|s| {
            s.replace("{artifact}", vars["artifact"].as_str().unwrap_or(""))
                .replace("{attachment}", vars["attachment"].as_str().unwrap_or(""))
        })
        .collect()
}
fn command(root: &Path) -> io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .current_dir(root)
        .env("HEY_BOSS_ISSUE_DB", root.join("issues.db"))
        .env("HEY_BOSS_FLEET_STATE", root.join("fleet"))
        .env("HEY_BOSS_FLEET_DESIRED", root.join("desired.json"))
        .env("HEY_BOSS_FLEET_CONFIG", root.join("fleet.json"))
        .env("HEY_BOSS_INBOX_SOCKET", root.join("absent.sock"))
        .env_remove("HEY_BOSS_ISSUE_HOST")
        .env_remove("HEY_BOSS_ISSUE_PROJECT")
        .env_remove("HEY_BOSS_AGENT_ID");
    Ok(command)
}
fn dispatch(args: Vec<String>) -> io::Result<()> {
    let mut args = args;
    let json_output = args.iter().any(|arg| arg == "--json");
    let family = args.first().cloned().unwrap_or_default();
    if matches!(
        args.first().map(String::as_str),
        Some("issue" | "artifact" | "attachment" | "mm")
    ) {
        args.extend([
            "--project".into(),
            "named:Review sample".into(),
            "--agent".into(),
            "human:admin-review".into(),
        ]);
    }
    let cli = crate::Cli::try_parse_from(std::iter::once("hey-boss".to_owned()).chain(args))
        .map_err(io::Error::other)?
        .canonicalize();
    if matches!(
        &cli.command,
        crate::Command::Notif(
            crate::notif_cli::Action::Alert { .. }
                | crate::notif_cli::Action::Update { .. }
                | crate::notif_cli::Action::Ask { .. }
                | crate::notif_cli::Action::Prompt { .. }
                | crate::notif_cli::Action::Approval { .. }
                | crate::notif_cli::Action::Status { .. }
                | crate::notif_cli::Action::Wait { .. }
                | crate::notif_cli::Action::Hide { .. }
        )
    ) {
        let (request, output) = cli.into_request_with_project(|_| {
            Ok(hey_boss::issues::Project {
                id: "named:Review sample".into(),
                name: "Review sample".into(),
            })
        })?;
        let response = json!({"task_id":"review-task-1","status":if matches!(request.command.as_str(),"wait"|"hide") {"ok"} else {"pending"},"result":if request.command=="wait" {Some("Text")} else {None}});
        return crate::print_response(serde_json::from_value(response)?, output.json);
    }
    let result = match cli.command {
        crate::Command::Issue(o) => crate::issue_cli::run(&o),
        crate::Command::Artifact(o) => crate::artifact_cli::run(&o),
        crate::Command::Attachment(o) => crate::attachment_cli::run(&o),
        crate::Command::Mm(o) => crate::mindmap_cli::run(&o),
        crate::Command::Settings(o) => crate::issue_cli::run_global(&o),
        crate::Command::Skill(o) => {
            return match o.action {
                SkillAction::Install => install_skill(&std::env::current_dir()?, o.json),
                _ => skill(&o),
            };
        }
        crate::Command::Admin(o) if matches!(o.action, Action::Catalog) => return run(&o),
        _ => {
            return Err(io::Error::other(
                "Command is not available in isolated previews",
            ));
        }
    };
    if let Err(error) = result {
        if json_output {
            println!("{}", json!({"ok":false,"error":error}));
        } else {
            eprintln!("hey-boss {family}: {error}");
        }
        std::process::exit(error.exit_code());
    }
    Ok(())
}

pub fn run(options: &Options) -> io::Result<()> {
    let value = match &options.action {
        Action::Catalog => catalog(),
        Action::Capture { command: id, root } => {
            let scenario = scenarios()
                .remove(id)
                .ok_or_else(|| io::Error::other("Unknown preview"))?;
            let vars: Value = serde_json::from_slice(&fs::read(root.join("variables.json"))?)?;
            let mut args: Vec<String> = id.split_whitespace().map(str::to_owned).collect();
            args.extend(expand(&scenario.args, &vars));
            if options.json {
                args.push("--json".into());
            }
            return dispatch(args);
        }
        Action::Preview {
            command: id,
            seed_stdin,
        } => {
            let canonical = if crate::notif_cli::Action::has_subcommand(id) {
                format!("notif {id}")
            } else if let Some(grouped) = crate::canonical_agent_command(id) {
                grouped.into()
            } else {
                id.clone()
            };
            let id = &canonical;
            let all = catalog();
            let entry = all["commands"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["id"] == *id)
                .ok_or_else(|| io::Error::other("Unknown command"))?;
            if let Some(scenario) = scenarios().get(id) {
                let context: Value = if *seed_stdin {
                    let mut bytes = Vec::new();
                    std::io::stdin()
                        .take(256 * 1024 + 1)
                        .read_to_end(&mut bytes)?;
                    if bytes.len() > 256 * 1024 {
                        return Err(io::Error::other("Preview context is too large"));
                    }
                    serde_json::from_slice(&bytes)?
                } else {
                    json!({})
                };
                let mut result = json!({"mode":"sample","id":id,"reason":if id.starts_with("notif ") {"Sample daemon response rendered by the actual CLI formatter. No notification is sent and no answer is requested."} else {"Actual CLI output from disposable sample state. Text and JSON each start from a fresh copy."}});
                for (format, json_mode) in [("text", false), ("json", true)] {
                    if json_mode && entry["json_supported"] != true {
                        result[format] = Value::Null;
                        continue;
                    }
                    let root = Temporary::new()?;
                    let vars = seed(&root.0, &context)?;
                    // Setup is restricted to the same fixed scenario catalog.
                    for setup in &scenario.setup {
                        let mut db =
                            Store::open(&root.0.join("issues.db")).map_err(io::Error::other)?;
                        setup_operation(&mut db, &root.0, setup, &vars)?;
                    }
                    let mut cmd = command(&root.0)?;
                    cmd.args(["admin", "capture", id]).arg(&root.0);
                    if json_mode {
                        cmd.arg("--json");
                    }
                    result[format] = capture(&mut cmd, &[])?;
                    result[format]["command"] = json!(format!(
                        "hey-boss {}{}{}",
                        id,
                        expand(&scenario.args, &vars)
                            .iter()
                            .map(|s| format!(" '{}'", s.replace('\'', "'\\''")))
                            .collect::<String>(),
                        if json_mode { " --json" } else { "" }
                    ));
                }
                result
            } else {
                json!({"id":id,"mode":"help","reason":"This command uses interactive UI, installation, remote services, process control, or an external integration. Its help is shown; it is never run by the review page.","text":{"command":format!("hey-boss {id} --help"),"stdout":entry["help"],"stderr":"","exit_code":0},"json":null})
            }
        }
    };
    if options.json {
        println!("{value}");
    } else {
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    Ok(())
}

fn setup_operation(
    store: &mut Store,
    root: &Path,
    args: &[String],
    vars: &Value,
) -> io::Result<()> {
    let a = expand(args, vars);
    let op = match a.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["issue", "claim", _] => json!({"action":"claim","number":1,"force":false}),
        ["issue", "close", _] => json!({"action":"close","number":1,"force":false}),
        ["issue", "delete", _] => json!({"action":"delete","number":1,"force":false}),
        ["issue", "hide-project"] => json!({"action":"hide_project"}),
        ["issue", "create", ..] => {
            json!({"action":"create","title":"Draft follow-up","body":"","labels":[],"draft":true})
        }
        ["issue", "resolve-comment", ..] => {
            json!({"action":"resolve_comment","number":1,"comment_id":1,"resolved":true})
        }
        ["issue", "pr", "add", _, url] => json!({"action":"add_pull_request","number":1,"url":url}),
        ["issue", "subtask", "add", ..] => json!({"action":"add_subtask","number":1,"child":2}),
        ["artifact", "archive", id, ..] => {
            json!({"action":"artifact","operation":{"command":"archive","id":id,"archived":true,"if_version":1}})
        }
        ["artifact", "comment", id, ..] => {
            json!({"action":"artifact","operation":{"command":"comment","id":id,"body":"Review comment"}})
        }
        ["mm", "link", from, to] => {
            json!({"action":"mindmap","operation":{"command":"link","from":from,"to":to,"kind":"related"}})
        }
        _ => return Err(io::Error::other("Unknown preview setup")),
    };
    store
        .execute(&request(root, op)?)
        .map_err(io::Error::other)?;
    Ok(())
}
