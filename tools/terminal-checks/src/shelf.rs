use super::*;
pub fn check(f: &Fixture, g: &Path) {
    std::os::unix::fs::symlink(&f.bin, f.dir.path().join("bin/hey-boss")).unwrap();
    let git = |a: &[&str]| run(f.cmd(Path::new("/usr/bin/git"), a));
    git(&["init", "-qb", "main"]);
    git(&["config", "user.name", "Sam"]);
    git(&["config", "user.email", "shelf@example.invalid"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/example/catalog.git",
    ]);
    run(f.cli(&[
        "project",
        "init",
        "--yes",
        "--prs",
        "false",
        "--worktree",
        "true",
    ]));
    run(f.cli(&["issue", "create", "--title", "Tune the search timeout"]));
    let file = f.cwd.join("search.rs");
    fs::write(&file, "const TIMEOUT_MS: u32 = 100;\n").unwrap();
    git(&["add", "search.rs"]);
    git(&["commit", "-qm", "Initialize search"]);
    git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let open = || Terminal::open(f.cmd(g, &[]), 106, 27);
    let mut empty = open();
    empty.wait("No active or saved commits");
    empty.key("\x1b");
    assert_eq!(empty.exit(), 0);
    for (name, ms) in [
        ("Allow slower connections", 200),
        ("Increase search timeout", 250),
    ] {
        fs::write(&file, format!("const TIMEOUT_MS: u32 = {ms};\n")).unwrap();
        git(&["commit", "-qam", name]);
        if ms == 200 {
            run(f.cmd(g, &["stash-commit", &git(&["rev-parse", "HEAD"])]));
        }
    }
    // A newer change to the same line cannot be replayed without the selection.
    fs::write(&file, "const TIMEOUT_MS: u32 = 300;\n").unwrap();
    git(&["commit", "-qam", "Depends on selected change"]);
    let before = git(&["rev-parse", "HEAD"]);
    let mut conflict = open();
    conflict.wait("Depends on selected change");
    conflict.key("\x13");
    conflict.wait("recommended");
    conflict.key("\r");
    conflict.wait("Could not replay");
    assert_eq!(git(&["rev-parse", "HEAD"]), before);
    assert_eq!(git(&["status", "--porcelain"]), "");
    conflict.key("\x1b");
    assert_eq!(conflict.exit(), 0);
    git(&["reset", "--hard", "HEAD^"]);
    let selected_commit = git(&["rev-parse", "HEAD"]);
    fs::write(f.cwd.join("notes"), "keep this\n").unwrap();
    git(&["add", "notes"]);
    git(&["commit", "-qm", "Keep newer work"]);
    let mut t = open();
    t.wait("Keep newer work");
    git(&["commit", "--allow-empty", "-qm", "Branch moved"]);
    t.key("\x13");
    t.wait("recommended");
    t.key("\r");
    t.wait("Branch tip changed");
    assert_eq!(git(&["log", "-1", "--format=%s"]), "Branch moved");
    t.key("\x1b");
    assert_eq!(t.exit(), 0);
    git(&["reset", "--hard", "HEAD^"]);
    let mut t = open();
    t.wait("250;");
    t.wait("Allow slower connections");
    t.key("\x13");
    t.wait("recommended");
    t.key("1\r");
    t.wait("Shelved");
    assert_eq!(git(&["log", "-1", "--format=%s"]), "Keep newer work");
    assert_eq!(
        git(&["rev-parse", "HEAD^"]),
        git(&["rev-parse", "origin/main"])
    );
    assert_eq!(git(&["show", "HEAD:notes"]), "keep this");
    assert!(run(f.cmd(g, &["open", &selected_commit])).contains("250;"));
    // The newly saved review stays selected after the active stack changes.
    t.wait("250;");
    t.key("\r");
    t.key("\x1b[6~");
    t.key("\x03");
    t.wait("Comment on the selected line");
    t.key("Matches the mobile timeout.\r");
    t.wait("Comment saved");
    t.key("\t");
    t.wait("Matches the mobile timeout.");
    f.shot(&mut t, "project-shelf");
    t.resize(48, 23);
    t.key("\x1b[D");
    t.wait("Ctrl+L link");
    assert!(!t.text().contains("Review"));
    f.shot(&mut t, "compact-shelf");
    t.key("\r");
    t.wait("Matches the mobile timeout.");
    f.shot(&mut t, "compact-review");
    t.key("\x1b");
    t.wait("Commits");
    t.key("\x15");
    t.wait("Apply selected commit");
    t.key("\x1b");
    assert_eq!(git(&["log", "-1", "--format=%s"]), "Keep newer work");
    t.key("\x1b");
    assert_eq!(t.exit(), 0);
}
