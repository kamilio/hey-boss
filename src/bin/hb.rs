use std::os::unix::process::CommandExt;

fn main() -> std::io::Result<()> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let error = std::process::Command::new(executable.with_file_name("hey-boss"))
        .args(std::env::args_os().skip(1))
        .exec();
    Err(error)
}
