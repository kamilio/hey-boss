use super::*;
pub fn seed(f: &Fixture, s: Scenario, p: &str) -> Result<Value> {
    use Scenario::*;
    let issue = |args: &[&str]| run(f.issue(p, args));
    match s {
        Html => {
            let body = include_str!("../../tests/fixtures/html-render.md");
            let page = include_str!("../../tests/fixtures/html-page.html");
            let mut info = desktop::seed(f, s, p)?;
            for (key, title, source) in [
                ("artifact", "HTML preview", body),
                ("full", "A complete HTML page", page),
            ] {
                let result = run(f.command(&[
                    "artifact",
                    "create",
                    "--project",
                    p,
                    "--json",
                    "--title",
                    title,
                    "--body",
                    source,
                ]))?;
                info[key] = result["artifact"]["id"]
                    .as_str()
                    .ok_or("missing artifact ID")?
                    .into();
            }
            info["issue"] =
                issue(&["create", "--title", "HTML issue", "--body", body])?["issue"]["number"]
                    .as_i64()
                    .ok_or("missing issue number")?
                    .into();
            return Ok(info);
        }
        LocalHttp | NativeArtifactEditor => return desktop::seed(f, s, p),
        IssueArchitecture => issue_design::architecture(f, p)?,
        IssueTimeline => issue_design::timeline(f, p)?,
        AssignmentWatch => {
            issue_design::watch(f, p)?;
            return desktop::seed(f, s, p);
        }
        Origin => return origin::seed(f, p),
        Agents => agents::conversations(f)?,
        AutoWorkers => agents::workers(f, p)?,
        WorkerConfig => {
            fs::write(
                f.root.path().join("fleet.yaml"),
                "# All worker instances\nmachines:\n  local:\n    workers:\n      - id: tools\n        intent: pause\n        config:\n          name: Tools\n          concurrency: 2\n",
            )?;
            issue(&["create", "--title", "Synthetic worker configuration check"])?;
        }
        AgentsNavigation => {
            f.init("Atlas")?;
            f.init("Beacon")?;
            fleet::serve(f, json!([]))?;
        }
        AssignedAgent => fleet::assigned_agent(f, p)?,
        ReviewedEvidence => return reviewed_evidence::verify(f, p),
        MarkdownAttachments => return markdown_attachments::seed(f),
        ArtifactDiagrams => {
            run(f.command(&[
                "artifact",
                "--project",
                p,
                "--json",
                "create",
                "--title",
                "Recovery flow",
                "--body",
                include_str!("recovery-flow.md"),
            ]))?;
        }
        PlanPrompt => {
            for (title, label) in [
                ("Plan reconnect improvements", "task:plan"),
                ("Legacy artifact task", "task:research"),
            ] {
                issue(&["create", "--title", title, "--label", label])?;
            }
        }
        LivePrompt => {
            issue(&[
                "create",
                "--title",
                "Keep running agents up to date",
                "--body",
                "Save instructions without restarting active work.",
            ])?;
        }
        ProjectSettings | QuietHours | LongTitle | IssueReopen | ImagePreview | ListPagination => {}
        Archive => archive::seed(f, p)?,
        QueuePlacement => {
            for (actor, title, options) in [
                ("human:boss", "Recovery second", &[][..]),
                ("human:boss", "Recovery first", &[]),
                ("fixture-agent", "Ordinary follow-up", &[]),
                ("fixture-agent", "Explicit MCP priority", &["--at-top"]),
                ("fixture-agent", "Draft follow-up", &["--draft"]),
            ] {
                let mut c = f.issue(p, &["--agent", actor, "create", "--title", title]);
                c.args(options);
                run(c)?;
            }
            let project = format!("named:{p}");
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as i64;
            let event = json!({"worker_id":"fixture-worker","worker_name":"MCP recovery worker",
                "required_tags":["mcp"],"projects":[project],"sort_order":-1,"issue_created_at":now,
                "order_by":["sort_order","created_at","project_id","number"],"rule":"First eligible issue in queue order with a valid checkout."});
            rusqlite::Connection::open(&f.db)?.execute(
                "INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES(?1,2,'fixture-agent','worker_selected',?2,?3)",
                rusqlite::params![project, now, event.to_string()],
            )?;
        }
        ProjectPicker | ProjectNames => {
            let projects = if matches!(s, ProjectPicker) {
                (1..=20)
                    .map(|n| format!("Project {n:02}"))
                    .map(|name| (name, "Review the mobile project menu"))
                    .chain([
                        (
                            "A very long project name for mobile layout testing".into(),
                            "Review the mobile project menu",
                        ),
                        ("Hidden project".into(), "Hidden fixture"),
                    ])
                    .collect::<Vec<_>>()
            } else {
                [
                    ("github.com/poe-internal/poe2", "Original destination"),
                    ("github.com/other/poe2", "Reused destination"),
                    ("named:poe2", "Named alias"),
                    ("Design Team", "Fixture"),
                    (
                        "A very long project name for narrow phone layouts",
                        "Fixture",
                    ),
                    ("Hidden project", "Fixture"),
                ]
                .map(|(p, t)| (p.to_owned(), t))
                .to_vec()
            };
            for (project, title) in projects {
                if project != p {
                    f.init(&project)?;
                }
                run(f.issue(&project, &["create", "--title", title]))?;
            }
            run(f.issue("Hidden project", &["hide-project"]))?;
        }
        AgentLaunches => {
            for title in [
                "Untouched issue",
                "First agent launch",
                "Investigate repeated reconnect failures across devices",
                "Long-running issue with many worker retries",
            ] {
                let created = issue(&[
                    "create",
                    "--title",
                    title,
                    "--body",
                    "Review worker agent launches.",
                ])?;
                assert_eq!(created["issue"]["agent_launch_count"], 0);
            }
            rusqlite::Connection::open(&f.db)?.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1234)
              INSERT INTO issue_agent_launches(run_id, project_id, issue_number, launched_at) SELECT 'fixture-'||issues.number||'-'||n.x,project_id,number,created_at FROM issues JOIN n ON n.x<=CASE number WHEN 2 THEN 1 WHEN 3 THEN 12 WHEN 4 THEN 1234 ELSE 0 END;")?;
        }
        IssueListActions => {}
        JobsUi => fs::write(f.root.path().join("fleet.yaml"), "machines: {}\n")?,
        ScheduledJobs => return controls::jobs(f, p),
        StatusDot | IssueStatus => {
            for (title, level, comment) in [
                (
                    "Reconnect after waking from sleep",
                    "green",
                    "The fix passes tests. Checking the phone layout next.",
                ),
                (
                    "Install on the connected devices",
                    "orange",
                    "One device is offline. Retrying the installation.",
                ),
                (
                    "Restore the deployment connection",
                    "red",
                    "The server is unavailable. Waiting for the connection to return.",
                ),
                ("Plan next week’s improvements", "", ""),
                ("Review a long update", "orange", &"A".repeat(500)),
                (
                    "Keep status text safe",
                    "green",
                    "<img src=x onerror=alert(1)> & \"quoted\" status stays plain text.",
                ),
            ] {
                let mut args = vec!["create", "--at-bottom"];
                if matches!(s, IssueStatus) {
                    args.extend(["--label", "enhancement"]);
                }
                args.extend([
                    "--title",
                    title,
                    "--body",
                    if matches!(s, IssueStatus) { "## What needs to happen\n\nKeep the work easy to follow, with clear updates and lasting findings in comments." } else { "Keep the work easy to follow." },
                ]);
                let created = issue(&args)?;
                if !level.is_empty() {
                    let number = created["issue"]["number"].to_string();
                    issue(&["claim", &number])?;
                    if matches!(s, IssueStatus) && number == "1" {
                        for n in 0..24 {
                            issue(&[
                                "status",
                                "1",
                                if n % 3 == 0 { "orange" } else { "green" },
                                "--comment",
                                &format!("Checking the reconnect behavior, step {}.", n + 1),
                            ])?;
                        }
                    }
                    issue(&["status", &number, level, "--comment", comment])?;
                }
            }
        }
        CreatorModel => {
            for (i, (title, model)) in [
                (
                    "Preserve the model that discovered this issue",
                    Some("gpt-6-astra".into()),
                ),
                (
                    "Review the reconnect implementation",
                    Some("gpt-6-sol".into()),
                ),
                (
                    "A long custom model name stays readable",
                    Some(format!("custom/{}", "m".repeat(220))),
                ),
                ("Older Codex issue without model metadata", None),
            ]
            .into_iter()
            .enumerate()
            {
                let session = format!("aaaaaaaa-bbbb-cccc-dddd-{:012}", i + 1);
                fs::write(
                    f.root
                        .path()
                        .join(format!("codex/sessions/rollout-{session}.jsonl")),
                    format!(
                        "{}\n",
                        json!({"type":"turn_context","payload":{"model":model}})
                    ),
                )?;
                let mut c=f.issue(p,&["create","--title",title,"--body","The creating model is recorded automatically. Session details and conversation links remain available."]);
                c.env_remove("HEY_BOSS_AGENT_ID")
                    .env("CODEX_THREAD_ID", &session);
                let created = run(c)?;
                assert_eq!(created["issue"]["origin"]["model"], json!(model));
            }
            issue(&[
                "create",
                "--agent",
                "human:boss",
                "--title",
                "Human-created issue retains the Boss attribution",
            ])?;
        }
        IssueListOrigins => {
            for title in [
                "Legacy issue with no creation context",
                "Populated origin with two comments",
                "Closed legacy issue",
                "Malformed origin keeps its issue visible",
                "Unsupported origin keeps its issue visible",
            ] {
                issue(&[
                    "create",
                    "--agent",
                    "human:boss",
                    "--title",
                    title,
                    "--body",
                    "Historical issue content remains available.",
                    "--label",
                    "regression",
                ])?;
            }
            for body in ["First finding", "Second finding"] {
                issue(&["comment", "2", "--agent", "human:boss", "--body", body])?;
            }
            issue(&["assign-to-boss", "2"])?;
            issue(&["close", "3"])?;
            rusqlite::Connection::open(&f.db)?.execute_batch(
                "BEGIN;
                UPDATE issues SET origin=NULL WHERE number IN (1,3);
                DROP INDEX issues_origin_session; DROP INDEX issues_origin_run;
                PRAGMA ignore_check_constraints=ON;
                UPDATE issues SET origin='{broken' WHERE number=4;
                UPDATE issues SET origin='[1,2]' WHERE number=5; COMMIT;",
            )?;
        }
    }
    Ok(json!({}))
}
