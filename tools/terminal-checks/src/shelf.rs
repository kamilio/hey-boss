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
    let open = || Terminal::open(f.cmd(g, &[]), 106, 27);
    let mut empty = open();
    empty.wait("No shelved commits yet");
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
    let mut t = open();
    t.wait("Allow slower connections");
    t.key("\x13");
    t.wait("recommended");
    t.key("1\r");
    t.wait("Shelved");
    // Either commit may sort first when their timestamps match.
    let list: Vec<_> = t
        .text()
        .lines()
        .map(|line| line.chars().take(33).collect::<String>())
        .collect();
    let target = list
        .iter()
        .position(|line| line.contains("Increase search timeout"))
        .unwrap();
    let selected = list.iter().position(|line| line.contains("▸ ")).unwrap();
    if target < selected {
        t.key("\x1b[A")
    } else if target > selected {
        t.key("\x1b[B")
    }
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
    assert_eq!(git(&["log", "-1", "--format=%s"]), "Initialize search");
    t.key("\x1b");
    assert_eq!(t.exit(), 0);
}
