use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=keychain_reader.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    // The reader has its own executable identity in Keychain's access list.
    // Embed it so standalone CLI installs can initialize a new Mac offline.
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("keychain-reader");
    let status = Command::new(env::var_os("RUSTC").unwrap())
        .args(["--edition=2024", "--target"])
        .arg(env::var_os("TARGET").unwrap())
        .args([
            "-C",
            "opt-level=s",
            "-C",
            "strip=symbols",
            "-C",
            "panic=abort",
            "keychain_reader.rs",
            "-o",
        ])
        .arg(output)
        .status()
        .expect("compile macOS Keychain reader");
    assert!(status.success(), "could not compile macOS Keychain reader");
}
