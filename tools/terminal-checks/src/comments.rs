use super::*;
use std::os::unix::fs::PermissionsExt;
pub fn check(f: &Fixture) {
    let gh = f.dir.path().join("bin/gh");
    fs::write(&gh,"#!/bin/sh\ncat > body\nif [ \"$GH_EXIT\" = 1 ]; then echo 'GitHub unavailable' >&2; exit 1; fi\nprintf '%s\\n' 'https://github.com/example/repo/pull/7#issuecomment-1'\n").unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
    let long = "x".repeat(301);
    for w in [48, 80, 120] {
        for (name, args, status, needle) in [
            (
                "help",
                vec!["pr", "comment", "--help"],
                0,
                "Do not sound like a robot.",
            ),
            (
                "rejection",
                vec!["pr", "comment", "7", "--body", &long],
                1,
                "300 characters.",
            ),
            (
                "success",
                vec![
                    "pr",
                    "comment",
                    "7",
                    "--body",
                    "Fixed the retry. Tests pass.",
                ],
                0,
                "#issuecomment-1",
            ),
            (
                "failure",
                vec!["pr", "comment", "7", "--body", "Fixed the retry."],
                1,
                "not retried.",
            ),
        ] {
            let mut cmd = f.cli(&["cached"]);
            cmd.args(args);
            cmd.env("GH_EXIT", if name == "failure" { "1" } else { "0" })
                .env("GH_HOST", "github.com")
                .env("HEY_GH_APP_TOKEN", "synthetic");
            let mut t = Terminal::open(cmd, w, 40);
            let code = t.exit();
            assert_eq!(code, status, "{}", t.history());
            let text = t.text();
            assert!(
                text.split_whitespace()
                    .collect::<String>()
                    .contains(&needle.replace(' ', "")),
                "{name}: {text}"
            );
            if name == "success" {
                assert_eq!(
                    fs::read_to_string(f.cwd.join("body")).unwrap(),
                    "Fixed the retry. Tests pass."
                )
            }
            f.shot(&mut t, &format!("{name}-{w}"));
        }
    }
}
