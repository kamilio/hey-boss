use std::process::Command;

#[test]
fn shortcut_preserves_help_version_and_error_output() {
    for arguments in [
        vec!["--version"],
        vec!["--help"],
        vec!["issue", "create", "--help"],
        vec!["alert", "--unknown-option", "spaces and Unicode: café"],
    ] {
        let original = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(&arguments)
            .output()
            .unwrap();
        let shortcut = Command::new(env!("CARGO_BIN_EXE_hb"))
            .args(&arguments)
            .output()
            .unwrap();
        assert_eq!(shortcut.status, original.status, "{arguments:?}");
        assert_eq!(shortcut.stdout, original.stdout, "{arguments:?}");
        assert_eq!(shortcut.stderr, original.stderr, "{arguments:?}");
    }
}
