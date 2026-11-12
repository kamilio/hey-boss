//! Git on the fixture supervisor's PATH. Only clone gets simulated service conditions.
use std::{
    env, fs::OpenOptions, io::Write, os::unix::process::CommandExt, path::Path, process, thread,
    time::Duration,
};

fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.iter().any(|a| a == "clone") {
        let home = env::var_os("HOME").unwrap();
        let url = &args[args.len() - 2];
        writeln!(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(Path::new(&home).join("clone-calls"))
                .unwrap(),
            "{}",
            url.to_string_lossy()
        )
        .unwrap();
        if url.to_string_lossy().starts_with("git@github.com:") {
            eprintln!("Permission denied (publickey).");
            process::exit(128);
        }
        if Path::new(&home).join("slow-clone").exists() {
            thread::sleep(Duration::from_secs(61));
        }
    }
    panic!(
        "{}",
        process::Command::new("/usr/bin/git").args(args).exec()
    );
}
