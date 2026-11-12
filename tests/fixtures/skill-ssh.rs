//! Run the skills transport on the fixture's second machine, without a network connection.
use std::{
    env,
    os::unix::process::CommandExt,
    process::{Command, ExitCode},
};

fn main() -> ExitCode {
    let args: Vec<_> = env::args().skip_while(|s| s != "--").skip(1).collect();
    let [host, remote] = args.as_slice() else {
        panic!("unexpected ssh call: {args:?}")
    };
    let studio = std::path::PathBuf::from(env::var_os("SKILL_TEST_STUDIO").unwrap());
    if host == "offline" || (host == "studio" && studio.join(".offline").exists()) {
        eprintln!("Offline test machine");
        return ExitCode::from(255);
    }
    assert_eq!(host, "studio");
    let mut cmd = if remote.starts_with("python3 -c ") {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", remote]);
        c
    } else {
        // Skip the login shell so that the remote uses the CLI under test.
        let action = remote
            .strip_prefix("exec \"${SHELL:-/bin/sh}\" -lic 'exec hey-boss skill ")
            .and_then(|s| s.strip_suffix('\''))
            .unwrap();
        assert!(matches!(
            action,
            "apply-copy" | "apply-rename" | "apply-enabled"
        ));
        let mut c = Command::new(env::var_os("SKILL_TEST_CLI").unwrap());
        c.args(["skill", action]);
        c
    };
    panic!("{}", cmd.env("HOME", studio).exec());
}
