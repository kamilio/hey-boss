use super::*;
fn issue(f: &Fixture, project: &str, args: &[&str]) -> Command {
    let mut c = f.cli(&["issue", "--project", project, "--agent", "human:qa"]);
    c.args(args);
    c
}

pub fn settings(f: &Fixture) {
    let cmd = |args: &[&str]| issue(f, "Settings QA", args);
    f.init("Settings QA");
    run(cmd(&[
        "create",
        "--title",
        "Readable issue",
        "--body",
        "Keep the issue details easy to scan.",
    ]));
    for w in [48, 80, 120] {
        for mode in ["disabled", "prs", "settings"] {
            run(cmd(&[
                "settings",
                "set",
                if mode == "disabled" {
                    "--no-prs"
                } else {
                    "--prs-enabled"
                },
                "--worktree",
                "--prompt",
                "Shared instructions.",
            ]));
            let mut t = Terminal::open(
                cmd(if mode == "settings" {
                    &["settings", "show"]
                } else {
                    &["view", "1"]
                }),
                w,
                24,
            );
            assert_eq!(t.exit(), 0, "{}", t.history());
            let text = t.text();
            assert!(text.contains("Settings QA (named:Settings QA)"), "{text}");
            for absent in ["null", "PRs enabled: false", "Worktrees enabled:", "Chief"] {
                assert!(!text.contains(absent), "{text}")
            }
            if mode == "settings" {
                assert!(
                    text.contains("Worktrees allowed: true")
                        && text.contains("Code Reviews enabled: false")
                        && text.contains("Shared prompt: Shared instructions."),
                    "{text}"
                )
            } else {
                assert!(
                    text.contains("#1 [open] Readable issue")
                        && text.contains("Comments: 0 shown · 0 total")
                        && !text.contains("Worktrees")
                        && !text.contains(": false"),
                    "{text}"
                );
            }
            assert_eq!(
                text.contains("PRs enabled: true"),
                mode != "disabled",
                "{text}"
            );
            f.shot(&mut t, &format!("{mode}-{w}"));
        }
    }
}

pub fn transactions(f: &Fixture, serve: bool) {
    let p = "Transaction recovery QA";
    f.init(p);
    let call = |args: &[&str]| {
        let mut c = issue(f, p, args);
        c.arg("--json");
        json(c)
    };
    call(&["create", "--title", "Prerequisite", "--draft"]);
    let creation = [
        "create",
        "--title",
        "Dependent issue saved exactly once",
        "--body",
        "The original specification survives an uncertain response.\n\n- One issue\n- One prerequisite",
        "--blocked-by",
        "1",
        "--request-id",
        "dependent-once",
    ];
    let created = call(&creation);
    assert_eq!(created["issue"]["number"], 2);
    assert_eq!(created["issue"]["state"], "blocked");
    assert_eq!(call(&creation), created);
    for route in [
        vec!["request", "dependent-once"],
        vec!["request", "dependent-once", "--supervisor"],
    ] {
        assert_eq!(call(&route)["request"]["response"]["issue"]["number"], 2)
    }
    assert_eq!(
        call(&["request", "missing"])["request"]["state"],
        "not_recorded"
    );
    let reject = issue(
        f,
        p,
        &[
            "create",
            "--title",
            "Rejected dependency",
            "--blocked-by",
            "999",
            "--request-id",
            "rejected",
            "--json",
        ],
    )
    .output()
    .unwrap();
    assert!(!reject.status.success());
    assert_eq!(
        call(&["request", "rejected"])["request"]["state"],
        "not_recorded"
    );
    assert_eq!(
        call(&[
            "create",
            "--title",
            "Created after the failed dependency",
            "--draft"
        ])["issue"]["number"],
        3
    );
    assert_eq!(
        call(&["view", "2"])["issue"]["title"],
        created["issue"]["title"]
    );
    assert_eq!(
        call(&["list", "--state", "all", "--all"])["issues"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    for w in [48, 80, 120] {
        for (id, label, body) in [
            ("dependent-once", "Recorded", "Savedresult:#2"),
            ("missing", "No saved receipt", "doesnotprove"),
        ] {
            let mut t = Terminal::open(issue(f, p, &["request", id]), w, 30);
            assert_eq!(t.exit(), 0);
            let text = t.text();
            assert!(
                text.contains(id)
                    && text.contains(label)
                    && !text.contains("undefined")
                    && !text.contains("null")
                    && text.split_whitespace().collect::<String>().contains(body),
                "{text}"
            );
            f.shot(&mut t, &format!("{id}-{w}"));
        }
    }
    if serve {
        let (stop, quit) = std::sync::mpsc::channel();
        ctrlc::set_handler(move || {
            let _ = stop.send(());
        })
        .unwrap();
        let mut server = Service(
            issue(
                f,
                p,
                &["web", "--port", "59705", "--no-discovery", "--json"],
            )
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
        );
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                server.0.try_wait().unwrap().is_none() && Instant::now() < end,
                "fixture web server failed to start"
            );
            let result = f
                .cmd(
                    Path::new("/usr/bin/curl"),
                    &[
                        "-fsS",
                        "--max-time",
                        "2",
                        "http://127.0.0.1:59705/api/bootstrap",
                    ],
                )
                .output()
                .unwrap();
            if result.status.success() {
                let boot: Value = serde_json::from_slice(&result.stdout).unwrap();
                assert_eq!(boot["project"]["id"], "named:Transaction recovery QA");
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        println!("Fixture ready at http://127.0.0.1:59705/; press Ctrl+C to stop.");
        quit.recv().unwrap();
    }
}
