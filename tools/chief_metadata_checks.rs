//! Exercise metadata changes over an isolated supervisor/companion connection.
#[path = "../tests/support/fleet_tunnel.rs"]
mod fixture;
use clap::{Parser, Subcommand};
use fixture::Fleet;
use serde_json::{Value, json};
use std::{
    env, fs,
    path::PathBuf,
    process::Stdio,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const PROJECT: &str = "Chief metadata QA";
const ACTOR: &str = "codex:chief-fixture";
#[derive(Parser)]
struct Args {
    /// Keep the companion viewer running on port 59651 for browser checks.
    #[arg(long, global = true)]
    serve: bool,
    /// Optional browser scenario.
    #[command(subcommand)]
    scenario: Option<Scenario>,
}
#[derive(Copy, Clone, Subcommand)]
enum Scenario {
    #[command(long_flag = "reopen")]
    Reopen,
    #[command(long_flag = "dependencies")]
    Dependencies,
    #[command(long_flag = "pr-attachments")]
    PrAttachments,
    #[command(long_flag = "lifecycle")]
    Lifecycle,
    #[command(long_flag = "moves")]
    Moves,
}

fn until(label: &str, mut ready: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(
            start.elapsed() < Duration::from_secs(45),
            "{label} did not converge"
        );
        thread::sleep(Duration::from_millis(100));
    }
}
fn ownership(f: &Fleet) -> Value {
    json!([
        f.sql(
            "main",
            "SELECT number,state,assignee FROM issues ORDER BY number"
        ),
        f.sql("main", "SELECT * FROM fleet_allocations"),
        f.sql("main", "SELECT * FROM worker_runs")
    ])
}
fn receipts(f: &Fleet, filter: &str) {
    assert_eq!(
        f.sql(
            "main",
            &format!("SELECT DISTINCT actor FROM requests WHERE {filter}")
        ),
        json!([[ACTOR]])
    );
    assert_eq!(
        f.sql(
            "peer",
            &format!("SELECT count(*) FROM requests WHERE {filter}")
        ),
        json!([[0]])
    );
}
fn base(f: &Fleet) {
    let main = |args: &[&str]| f.issue("main", args, 0);
    let peer = |args: &[&str]| f.issue("peer", args, 0);
    let error = |args: &[&str], code| f.issue("peer", args, code)["error"]["code"].clone();
    f.database(
        "main",
        json!({"replica":"capture","role":"controller","node":"main"}),
    );
    main(&[
        "create",
        "--title",
        "Delivered fix — label cleanup",
        "--label",
        "rework needed",
        "--body",
        "## Verified delivery\n\nThe fix is delivered. Remove the stale triage label without reopening this issue.",
    ]);
    main(&["close", "1"]);
    main(&[
        "create",
        "--title",
        "Active worker — metadata review",
        "--body",
        "## Worker in progress\n\nReview labels while preserving the running assignment and its reservation.",
    ]);
    f.cli(
        "main",
        &[
            "issue",
            "--project",
            PROJECT,
            "--agent",
            "codex:active-worker",
            "--json",
            "claim",
            "2",
        ],
        0,
    );
    f.sql("main","INSERT OR REPLACE INTO fleet_allocations VALUES('named:Chief metadata QA',2,'active-worker-machine')");
    f.sql("main","INSERT INTO worker_runs(id,project_id,issue_number,actor_id,state,started_at,updated_at,reservation_expires,claimed_at,job,owner_pid,owner_start,machine) VALUES('fixture-live','named:Chief metadata QA',2,'codex:active-worker','running',1,1,9223372036854775807,1,'{}',123,'fixture','active-worker-machine')");
    let before = ownership(f);
    for number in ["1", "2"] {
        let initial = peer(&["--supervisor", "view", number]);
        assert_eq!(initial["store"]["host"], "supervisor");
        let version = initial["issue"]["version"].to_string();
        let key = format!("chief-metadata-{number}");
        let args = [
            "--supervisor",
            "edit",
            number,
            "--if-version",
            &version,
            "--label",
            "reviewed",
            "--remove-label",
            "rework needed",
            "--request-id",
            &key,
        ];
        let saved = peer(&args);
        assert_eq!(saved["issue"]["labels"], json!(["reviewed"]));
        assert_eq!(peer(&args), saved);
        assert_eq!(main(&["view", number])["issue"], saved["issue"]);
        assert_eq!(
            error(
                &[
                    "--supervisor",
                    "edit",
                    number,
                    "--if-version",
                    &version,
                    "--label",
                    "stale",
                    "--request-id",
                    &format!("stale-{number}")
                ],
                4
            ),
            "conflict"
        );
    }
    assert_eq!(ownership(f), before);
    let file = f.root.join("batch.json");
    let path = file.to_str().unwrap();
    let mut batch = json!([{"number":1,"if_version":3,"expected_assignee":null,"add_labels":["qualified"]},{"number":2,"if_version":3,"expected_assignee":"wrong","add_labels":["qualified"]}]);
    fs::write(&file, batch.to_string()).unwrap();
    assert_eq!(
        f.issue(
            "peer",
            &[
                "--supervisor",
                "batch",
                "--file",
                path,
                "--request-id",
                "batch-stale"
            ],
            4
        )["accepted"],
        false
    );
    assert_eq!(main(&["view", "1"])["issue"]["labels"], json!(["reviewed"]));
    batch[1]["expected_assignee"] = "codex:active-worker".into();
    fs::write(&file, batch.to_string()).unwrap();
    assert_eq!(
        peer(&[
            "--supervisor",
            "batch",
            "--file",
            path,
            "--request-id",
            "batch-good"
        ])["accepted"],
        true
    );
    assert_eq!(ownership(f), before);
    let body = "## Corrected guidance\n\nFollow the existing dependency graph.";
    assert_eq!(
        peer(&[
            "--supervisor",
            "edit",
            "2",
            "--body",
            body,
            "--if-version",
            "4",
            "--request-id",
            "body-correction"
        ])["issue"]["body"],
        body
    );
    for args in [
        &["--supervisor", "claim", "2", "--force"][..],
        &["--supervisor", "close", "1", "--force"],
        &[
            "--supervisor",
            "edit",
            "2",
            "--label",
            "invalid-version",
            "--if-version",
            "0",
            "--request-id",
            "invalid-version",
        ],
    ] {
        assert_eq!(error(args, 2), "invalid_input");
    }
    batch[0]["assignment"] = "unassign".into();
    fs::write(&file, batch.to_string()).unwrap();
    assert_eq!(
        error(
            &[
                "--supervisor",
                "batch",
                "--file",
                path,
                "--request-id",
                "assignment"
            ],
            2
        ),
        "invalid_input"
    );
    receipts(
        f,
        "request_id IN ('chief-metadata-1','chief-metadata-2','batch-good','body-correction')",
    );
    until("body correction", || {
        peer(&["list", "--state", "all", "--all"])["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["number"] == 2)
            && peer(&["view", "2"])["issue"]["body"] == body
    });
    assert_eq!(
        error(&["edit", "1", "--label", "offline"], 4),
        "fleet_allocation_missing"
    );
    assert_eq!(ownership(f), before);
    let cap = f.cli("peer", &["fleet", "capabilities"], 0);
    assert_eq!(cap["route"], "supervisor_tunnel");
    for key in ["issue_draft", "issue_metadata"] {
        assert_eq!(cap["capabilities"][key], true);
    }
    main(&[
        "create",
        "--title",
        "Eligible import — paused as a draft",
        "--body",
        "## Imported task\n\nThe user requested that this eligible issue remain a draft. No worker or claim is needed.",
    ]);
    let args = ["edit", "3", "--draft", "--if-version", "1"];
    let saved = peer(&args);
    assert_eq!(saved["issue"]["draft"], true);
    assert_eq!(saved["store"]["host"], "supervisor");
    assert_eq!(peer(&args), saved);
    assert_eq!(main(&["view", "3"])["issue"], saved["issue"]);
    let denied = f.issue(
        "peer",
        &[
            "edit",
            "2",
            "--draft",
            "--if-version",
            "5",
            "--request-id",
            "assigned-draft",
        ],
        4,
    );
    assert_eq!(denied["error"]["code"], "conflict");
    assert!(
        denied["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unassigned, unreserved")
    );
    assert_eq!(main(&["view", "2"])["issue"]["draft"], false);
    assert_eq!(
        error(
            &[
                "edit",
                "3",
                "--draft",
                "--if-version",
                "1",
                "--request-id",
                "stale-draft"
            ],
            4
        ),
        "conflict"
    );
    assert_eq!(peer(&["edit", "3", "--draft"])["issue"]["draft"], true);
    receipts(f, "request_id LIKE 'draft-%'");
    assert_eq!(
        f.sql(
            "main",
            "SELECT count(*) FROM fleet_allocations WHERE issue_number=3"
        ),
        json!([[0]])
    );
    main(&[
        "create",
        "--title",
        "Eligible import — browser draft action",
        "--body",
        "## Ready for triage\n\nMove this unassigned import to a draft from the connected companion.",
    ]);
    until("draft", || {
        let list = peer(&["list", "--all"]);
        let list = list["issues"].as_array().unwrap();
        list.iter().any(|i| i["number"] == 3 && i["draft"] == true)
            && list.iter().any(|i| i["number"] == 4)
    });
    for side in ["main", "peer"] {
        assert_eq!(f.sql(side, "PRAGMA integrity_check"), json!([["ok"]]));
    }
}

fn main() {
    let args = Args::parse();
    let binary = env::var_os("HEY_BOSS_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("hey-boss")
        });
    let (tx, rx) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = tx.send(());
    })
    .unwrap();
    let mut f = Fleet::with_binary(&binary, PROJECT, ACTOR);
    f.start();
    base(&f);
    if let Some(mode) = args.scenario {
        scenario(&f, mode);
    }
    if args.serve {
        let viewer = f
            .command(
                "peer",
                &[
                    "issue",
                    "--project",
                    PROJECT,
                    "--agent",
                    ACTOR,
                    "--json",
                    "web",
                    "--port",
                    "59651",
                    "--no-discovery",
                ],
            )
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        f.services.push(viewer);
        let url = "http://127.0.0.1:59651";
        until("companion viewer", || {
            assert!(
                f.services.last_mut().unwrap().try_wait().unwrap().is_none(),
                "viewer exited"
            );
            reqwest::blocking::get(url).is_ok_and(|r| r.status().is_success())
        });
        println!(
            "{}",
            json!({"ready":true,"pid":std::process::id(),"root":f.root,"url":url})
        );
        rx.recv().unwrap();
    }
    f.stop_services().unwrap();
    let root = f.root.clone();
    drop(f);
    assert!(!root.exists(), "fixture directory survived shutdown");
    println!("Chief metadata checks passed; fixture stopped.");
}

fn scenario(f: &Fleet, mode: Scenario) {
    let main = |args: &[&str]| f.issue("main", args, 0);
    let peer = |args: &[&str]| f.issue("peer", args, 0);
    let cap = match mode {
        Scenario::Reopen => "issue_reopen",
        Scenario::Dependencies => "issue_dependencies",
        Scenario::PrAttachments => "issue_pr_attachments",
        Scenario::Lifecycle => "issue_close",
        Scenario::Moves => "issue_move",
    };
    assert_eq!(
        f.cli("peer", &["fleet", "capabilities"], 0)["capabilities"][cap],
        true
    );
    let guarded = |action, number, key, extra: &[&str], code| {
        let view = peer(&["view", number, "--supervisor"]);
        let version = view["issue"]["version"].to_string();
        let mut args = vec![
            action,
            number,
            "--supervisor",
            "--if-version",
            &version,
            "--request-id",
            key,
        ];
        args.extend_from_slice(extra);
        f.issue("peer", &args, code)
    };
    let states = |expected: &[(i32, &str)]| {
        until("replicated issue states", || {
            let issues = peer(&["list", "--state", "all", "--all"]);
            expected.iter().all(|(n, state)| {
                issues["issues"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|i| i["number"] == *n && (state.is_empty() || i["state"] == *state))
            })
        })
    };
    match mode {
        Scenario::Reopen => {
            for title in [
                "Incomplete delivery — dependency retained",
                "Unfinished prerequisite",
                "Manual hold — dependency retained",
                "Incomplete delivery — reopened",
            ] {
                main(&["create", "--title", title]);
            }
            for n in ["5", "8"] {
                main(&["close", n]);
            }
            main(&["blocked-by", "5", "6"]);
            main(&["close", "6"]);
            main(&["blocked-by", "7", "6"]);
            main(&["block", "7", "--comment", "Investigation required"]);
            main(&["reopen", "6"]);
            let saved = guarded("reopen", "8", "reopen-delivery", &[], 0);
            assert_eq!(saved["issue"]["state"], "open");
            assert_eq!(saved["issue"]["assignee"], Value::Null);
            assert_eq!(
                peer(&[
                    "reopen",
                    "8",
                    "--supervisor",
                    "--if-version",
                    "2",
                    "--request-id",
                    "reopen-delivery"
                ]),
                saved
            );
            let dependent = guarded("reopen", "5", "reopen-dependent", &[], 0);
            assert_eq!(dependent["issue"]["state"], "blocked");
            assert_eq!(dependent["issue"]["manual_blocked"], false);
            assert_eq!(
                guarded("reopen", "7", "reopen-held", &[], 4)["error"]["code"],
                "conflict"
            );
            assert_eq!(main(&["view", "7"])["issue"]["manual_blocked"], true);
            let held = guarded("reopen", "7", "reopen-clear", &["--clear-manual-hold"], 0);
            assert_eq!(held["issue"]["state"], "blocked");
            assert_eq!(held["issue"]["manual_blocked"], false);
            let before = ownership(f);
            assert_eq!(
                guarded("reopen", "2", "reopen-active", &[], 4)["error"]["code"],
                "conflict"
            );
            assert_eq!(ownership(f), before);
            receipts(f, "request_id LIKE 'reopen-%'");
            assert_eq!(
                f.sql(
                    "main",
                    "SELECT count(*) FROM fleet_allocations WHERE issue_number IN (5,7,8)"
                ),
                json!([[0]])
            );
            states(&[(5, "blocked"), (8, "open")]);
        }
        Scenario::Dependencies => {
            for title in [
                "Dependency to reconcile",
                "Upstream contract",
                "Independent task",
            ] {
                main(&["create", "--title", title]);
            }
            let version = peer(&["view", "5", "--supervisor"])["issue"]["version"].to_string();
            let add = [
                "blocked-by",
                "5",
                "6",
                "--supervisor",
                "--if-version",
                &version,
                "--request-id",
                "dependencies-add",
            ];
            let added = peer(&add);
            assert_eq!(added["issue"]["blocker_numbers"], json!([6]));
            assert_eq!(added["issue"]["state"], "blocked");
            assert_eq!(peer(&add), added);
            for key in ["dependencies-stale", "dependencies-add"] {
                assert_eq!(
                    f.issue(
                        "peer",
                        &[
                            "blocked-by",
                            "5",
                            "--supervisor",
                            "--if-version",
                            &version,
                            "--request-id",
                            key
                        ],
                        4
                    )["error"]["code"],
                    "conflict"
                );
            }
            assert_eq!(
                peer(&["view", "5", "--supervisor"])["issue"],
                added["issue"]
            );
            let cleared = guarded("blocked-by", "5", "dependencies-clear", &[], 0);
            assert_eq!(cleared["issue"]["blocker_numbers"], json!([]));
            assert_eq!(cleared["issue"]["state"], "open");
            assert_eq!(cleared["issue"]["assignee"], Value::Null);
            guarded("blocked-by", "5", "dependencies-restore", &["6"], 0);
            guarded("blocked-by", "7", "dependencies-independent-add", &["6"], 0);
            assert_eq!(
                guarded("blocked-by", "7", "dependencies-independent-clear", &[], 0)["issue"]["state"],
                "open"
            );
            let before = ownership(f);
            assert_eq!(
                guarded("blocked-by", "2", "dependencies-live-owner", &["6"], 4)["error"]["code"],
                "conflict"
            );
            assert_eq!(ownership(f), before);
            receipts(f, "request_id LIKE 'dependencies-%'");
            states(&[(5, "blocked"), (7, "open")]);
            assert_eq!(peer(&["view", "5"])["issue"]["blocker_numbers"], json!([6]));
        }
        Scenario::PrAttachments => {
            let before = ownership(f);
            let url = "https://github.com/example/supervisor-tunnel/pull/123";
            let args = [
                "pr",
                "add",
                "2",
                url,
                "--purpose",
                "prerequisite",
                "--supervisor",
                "--request-id",
                "pr-attachment",
            ];
            let saved = peer(&args);
            assert_eq!(saved["changed"], true);
            assert_eq!(saved["store"]["host"], "supervisor");
            assert_eq!(peer(&args), saved);
            let prs = &saved["pull_requests"];
            assert_eq!(
                &peer(&["request", "pr-attachment", "--supervisor"])["request"]["response"]["pull_requests"],
                prs
            );
            assert_eq!(
                &peer(&["pr", "list", "2", "--supervisor"])["pull_requests"],
                prs
            );
            let duplicate = peer(&[
                "pr",
                "add",
                "2",
                url,
                "--purpose",
                "fix",
                "--supervisor",
                "--request-id",
                "pr-duplicate",
            ]);
            assert_eq!(duplicate["changed"], false);
            assert_eq!(&duplicate["pull_requests"], prs);
            for (n, purpose) in [(124, "supporting-evidence"), (125, "fix")] {
                peer(&[
                    "pr",
                    "add",
                    "2",
                    &format!("https://github.com/example/supervisor-tunnel/pull/{n}"),
                    "--purpose",
                    purpose,
                    "--supervisor",
                ]);
            }
            assert_eq!(before, ownership(f));
            receipts(f, "request_id IN ('pr-attachment','pr-duplicate')");
            until("PR attachments", || {
                peer(&["view", "2"])["issue"]["pull_requests"]
                    .as_array()
                    .unwrap()
                    .len()
                    == 3
            });
        }
        Scenario::Lifecycle => {
            for title in [
                "Reviewed development handoff",
                "Verified completed handoff",
                "Idle watched handoff",
                "Browser handoff",
            ] {
                let n=main(&["create","--title",title,"--body","Development usability is recorded separately from CI, merge and production acceptance."])["issue"]["number"].to_string();
                main(&[
                    "pr",
                    "add",
                    &n,
                    "https://github.com/example/lifecycle/pull/1",
                ]);
            }
            let node = f.sql("peer", "SELECT node FROM fleet_meta WHERE id=1");
            let node = node[0][0].as_str().unwrap().replace('\'', "''");
            let allocation = format!(
                "INSERT OR REPLACE INTO fleet_allocations VALUES('named:Chief metadata QA',7,'{node}')"
            );
            f.sql("main", &allocation);
            main(&["assign", "7", "github"]);
            let watch = json!({"event":"reviewed-event","prs":{},"trigger":{"blocking":false,"completed":true}});
            f.sql("main",&format!("INSERT OR REPLACE INTO issue_github_watches VALUES('named:Chief metadata QA',7,'{watch}')"));
            f.sql(
                "main",
                "UPDATE issues SET assignee=NULL,github_ack_event=NULL WHERE number=7",
            );
            f.sql("main", &allocation);
            for n in ["5", "6", "7"] {
                let key = format!("lifecycle-ready-{n}");
                let mut args = vec!["ready", n, "--request-id", &key];
                if n != "7" {
                    args.push("--supervisor");
                }
                let saved = peer(&args);
                assert_eq!(saved["issue"]["state"], "ready");
                assert_eq!(saved["store"]["host"], "supervisor");
                assert_eq!(peer(&["view", n, "--supervisor"])["issue"], saved["issue"]);
                assert_eq!(peer(&args), saved);
            }
            let close = [
                "close",
                "6",
                "--supervisor",
                "--comment",
                "Implementation and installation verified.",
                "--request-id",
                "lifecycle-close",
            ];
            let saved = thread::scope(|s| {
                let first = s.spawn(|| peer(&close));
                let retry = peer(&close);
                assert_eq!(first.join().unwrap(), retry);
                retry
            });
            assert_eq!(saved["issue"]["state"], "closed");
            assert_eq!(
                peer(&["request", "lifecycle-close", "--supervisor"])["request"]["response"]["issue"],
                saved["issue"]
            );
            assert_eq!(
                peer(&["view", "6", "--supervisor"])["issue"],
                saved["issue"]
            );
            let before = ownership(f);
            for a in ["ready", "close"] {
                assert_eq!(
                    f.issue(
                        "peer",
                        &[
                            a,
                            "2",
                            "--supervisor",
                            "--request-id",
                            &format!("lifecycle-denied-{a}")
                        ],
                        4
                    )["error"]["code"],
                    "conflict"
                );
            }
            assert_eq!(ownership(f), before);
            assert_eq!(
                f.sql(
                    "main",
                    "SELECT count(*) FROM events WHERE issue_number IN (5,6,7) AND action='claimed'"
                ),
                json!([[0]])
            );
            receipts(f, "request_id LIKE 'lifecycle-%'");
            states(&[(5, "ready"), (6, "closed"), (7, "ready"), (8, "")]);
        }
        Scenario::Moves => {
            let project = "Queue movement QA";
            f.cli(
                "main",
                &[
                    "project",
                    "init",
                    "--project",
                    project,
                    "--prs",
                    "false",
                    "--worktree",
                    "false",
                    "--yes",
                    "--json",
                ],
                0,
            );
            let queue = |side, args: &[&str], code| {
                let mut command = vec!["issue", "--project", project, "--agent", ACTOR, "--json"];
                command.extend_from_slice(args);
                f.cli(side, &command, code)
            };
            for title in [
                "Browser rendering repair",
                "Dependency prerequisite",
                "Keyboard navigation",
                "Phone layout",
            ] {
                queue(
                    "main",
                    &[
                        "create",
                        "--at-bottom",
                        "--title",
                        title,
                        "--body",
                        "Queue ordering verification.",
                    ],
                    0,
                );
            }
            let args = [
                "move",
                "3",
                "--before",
                "1",
                "--request-id",
                "priority-once",
            ];
            let saved = queue("peer", &args, 0);
            assert_eq!(saved["store"]["host"], "supervisor");
            assert_eq!(
                queue("peer", &["view", "3", "--supervisor"], 0)["issue"],
                saved["issue"]
            );
            queue("main", &["move", "4", "--before", "3"], 0);
            assert_eq!(queue("peer", &args, 0), saved);
            assert_eq!(
                queue(
                    "peer",
                    &[
                        "move",
                        "2",
                        "--before",
                        "1",
                        "--if-order-version",
                        &saved["order_version"].to_string(),
                        "--request-id",
                        "priority-stale"
                    ],
                    4
                )["error"]["code"],
                "conflict"
            );
            until("queue project", || {
                f.sql(
                    "peer",
                    "SELECT count(*) FROM projects WHERE id='named:Queue movement QA'",
                ) == json!([[1]])
            });
            until("queue order", || {
                queue("peer", &["list"], 0)["issues"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i["number"].as_i64().unwrap())
                    .eq([4, 3, 1, 2])
            });
            receipts(f, "request_id='priority-once'");
        }
    }
}
