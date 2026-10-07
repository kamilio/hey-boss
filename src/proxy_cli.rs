use std::{ffi::OsString, io, os::unix::process::CommandExt, process::Command};

pub fn run(args: impl Iterator<Item = OsString>) -> io::Result<()> {
    let sibling = std::env::current_exe()?.with_file_name("hey-proxy");
    // Older installations can use an existing standalone proxy on PATH until
    // the next package upgrade installs both entry points side by side.
    let executable = if sibling.exists() {
        sibling
    } else {
        "hey-proxy".into()
    };
    let error = Command::new(executable).args(args).exec();
    Err(io::Error::new(
        error.kind(),
        format!(
            "Cannot execute hey-proxy: {error}. Install or upgrade hey-boss to install the proxy binary."
        ),
    ))
}
