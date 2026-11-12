use super::*;
pub fn seed(f: &Fixture, s: Scenario, p: &str) -> Result<()> {
    use Scenario::*;
    let issue = |args: &[&str]| run(f.issue(p, args));
    match s {
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
        LongTitle => {}
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
        StatusDot => {
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
                let created = issue(&[
                    "create",
                    "--at-bottom",
                    "--title",
                    title,
                    "--body",
                    "Keep the work easy to follow.",
                ])?;
                if !level.is_empty() {
                    let number = created["issue"]["number"].to_string();
                    issue(&["claim", &number])?;
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
    Ok(())
}
