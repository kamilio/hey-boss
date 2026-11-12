use super::*;

fn answer(t: &mut Terminal, label: &str, input: &str) {
    t.wait(label);
    t.key(input);
    t.key("\r")
}
fn settings(f: &Fixture, p: &str) -> Value {
    json(f.cli(&["issue", "--project", p, "--json", "settings", "show"]))
}
fn wizard(f: &Fixture, p: &str, w: usize) -> Terminal {
    f.screen(&["project", "init", "--project", p], w, 28)
}

pub fn check(f: &mut Fixture) {
    for w in [48, 80, 120] {
        let p = format!("Terminal {w}");
        let mut t = wizard(f, &p, w);
        t.wait("Use pull requests?");
        f.shot(&mut t, &format!("start-{w}"));
        answer(&mut t, "Use pull requests?", "maybe");
        answer(&mut t, "Enter y or n", "y");
        answer(&mut t, "Use worktrees?", "n");
        answer(&mut t, "Allow draft issues?", "n");
        answer(&mut t, "Allow artifacts?", "n");
        t.wait("Save project?");
        let h = t.history();
        assert!(
            h.contains("Pull request (default)")
                && h.contains("Existing checkout (default)")
                && !h.contains("Dedicated worktree (default)"),
            "{h}"
        );
        f.shot(&mut t, &format!("review-{w}"));
        t.key("p\r");
        t.wait("Save project?");
        // The expanded prompt can scroll off the screen at narrow widths.
        t.key("\r");
        assert_eq!(t.exit(), 0);
        assert!(t.history().contains("Dedicated worktree (default)"));
        f.shot(&mut t, &format!("saved-{w}"));
        let saved = settings(f, &p);
        assert_eq!(saved["prs_enabled"], true);
        for k in ["worktree_enabled", "drafts_enabled", "artifacts_enabled"] {
            assert_eq!(saved[k], false)
        }
        let mut t = wizard(f, &p, w);
        answer(&mut t, "Use pull requests? [Y/n]", "");
        answer(&mut t, "Use worktrees? [y/N]", "");
        answer(&mut t, "Allow draft issues? [y/N]", "");
        answer(&mut t, "Allow artifacts?", "n");
        answer(&mut t, "Save project?", "n");
        assert_eq!(t.exit(), 0);
        assert_eq!(settings(f, &p)["version"], saved["version"]);
    }
    for (name, cancel) in [("Quit", "q\r"), ("EOF", "\x04"), ("Interrupt", "\x03")] {
        let mut t = wizard(f, name, 80);
        t.wait("Use pull requests?");
        t.key(cancel);
        t.exit();
        let rejected = f
            .cli(&["issue", "--project", name, "settings", "show"])
            .output()
            .unwrap();
        assert!(!rejected.status.success());
        let err = String::from_utf8_lossy(&rejected.stderr);
        assert!(
            err.contains("--project <name-or-id>") && err.contains("hey-boss project init"),
            "{err}"
        );
        let list = json(f.cli(&["issue", "--project", name, "--json", "projects"]));
        assert!(
            !list["projects"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"] == name)
        );
        assert_eq!(
            run(f.cmd(
                Path::new("/usr/bin/sqlite3"),
                &[
                    f.dir.path().join("db").to_str().unwrap(),
                    &format!("SELECT count(*) FROM projects WHERE name='{name}'")
                ]
            )),
            "0"
        );
    }
    let original = f.cwd.clone();
    let parent = f.dir.path().join("parent");
    let nested = parent.join("nested/deep");
    fs::create_dir_all(&nested).unwrap();
    f.cwd = parent.clone();
    run(f.cmd(Path::new("/usr/bin/git"), &["init", "-q"]));
    f.cwd = nested.clone();
    let mut t = f.screen(&["issue", "list"], 80, 24);
    assert_ne!(t.exit(), 0);
    let h = t.history();
    assert!(
        h.contains("hey-boss project init") && h.contains("--project <name-or-id>"),
        "{h}"
    );
    f.shot(&mut t, "uninitialized");
    f.cwd = parent;
    let initialized = json(f.cli(&[
        "project",
        "init",
        "--yes",
        "--prs",
        "false",
        "--worktree",
        "false",
        "--json",
    ]));
    f.cwd = nested;
    let mut reused = f.screen(&["issue", "list", "--json"], 80, 24);
    assert_eq!(reused.exit(), 0);
    assert!(reused.history().contains("parent"));
    assert_eq!(
        json(f.cli(&["issue", "list", "--json"]))["project"],
        initialized["project"]
    );
    f.cwd = original;
    assert_eq!(
        json(f.cli(&["issue", "--project", "Terminal 80", "list", "--json"]))["project"]["name"],
        "Terminal 80"
    );
    let mut t = wizard(f, "Terminal 80", 80);
    answer(&mut t, "Use pull requests?", "n");
    answer(&mut t, "Use worktrees?", "y");
    answer(&mut t, "Allow draft issues?", "");
    answer(&mut t, "Allow artifacts?", "n");
    t.wait("Save project?");
    run(f.cli(&[
        "issue",
        "--project",
        "Terminal 80",
        "settings",
        "set",
        "--prompt",
        "Concurrent custom prompt.",
    ]));
    t.key("y\r");
    assert_ne!(t.exit(), 0);
    assert_eq!(
        settings(f, "Terminal 80")["prompt"],
        "Concurrent custom prompt."
    );
    for w in [48, 80, 120] {
        f.cwd = f.dir.path().join(format!("naming-{w}"));
        fs::create_dir(&f.cwd).unwrap();
        run(f.cmd(Path::new("/usr/bin/git"), &["init", "-q"]));
        run(f.cmd(
            Path::new("/usr/bin/git"),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/example/poe2.git",
            ],
        ));
        let mut t = f.screen(&["project", "init"], w, 28);
        t.wait("Project name [poe2]:");
        f.shot(&mut t, &format!("name-{w}"));
        let name = format!("Poe2 Legacy {w}");
        answer(&mut t, "Project name [poe2]:", &name);
        answer(&mut t, "Use pull requests?", "n");
        answer(&mut t, "Use worktrees?", "y");
        answer(&mut t, "Allow draft issues?", "n");
        answer(&mut t, "Allow artifacts?", "n");
        t.wait("Save project?");
        f.shot(&mut t, &format!("named-review-{w}"));
        t.key("y\r");
        assert_eq!(t.exit(), 0);
        let marker = fs::read(f.cwd.join(".hey-boss")).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&marker).unwrap(),
            serde_json::json!({"id":format!("named:{name}"),"name":name})
        );
        let found = json(f.cli(&["issue", "settings", "show", "--json"]));
        assert_eq!(found["project"]["name"], name);
        assert_eq!(found["prs_enabled"], false);
        assert_eq!(found["drafts_enabled"], false);
        let mut t = f.screen(
            &["issue", "create", "--title", "Rejected draft", "--draft"],
            w,
            28,
        );
        assert_eq!(t.exit(), 2);
        assert!(
            t.history()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains(&format!(
                    "Project \"{name}\" is not using drafts. Use regular tasks."
                ))
        );
        f.shot(&mut t, &format!("draft-disabled-{w}"));
        let mut cancel = wizard(f, &format!("Cancelled {w}"), w);
        answer(&mut cancel, "Use pull requests?", "q");
        assert_eq!(cancel.exit(), 0);
        assert_eq!(fs::read(f.cwd.join(".hey-boss")).unwrap(), marker);
    }
}
