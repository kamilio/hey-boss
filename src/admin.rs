//! Read-only review assets and bounded CLI preview transport.
use serde_json::{Value, json};
use std::os::unix::{fs::DirBuilderExt, process::CommandExt};
use std::{
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const HTML: &str = include_str!("issues/web/admin.html");
pub const SCRIPT: &str = include_str!("issues/web/admin.js");
pub const STYLE: &str = include_str!("issues/web/admin.css");

pub struct Temporary(pub PathBuf);
impl Temporary {
    pub fn new() -> io::Result<Self> {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        for _ in 0..100 {
            let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("hey-boss-review-{}-{serial}", std::process::id()));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("Cannot allocate preview directory"))
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn capture(command: &mut Command, input: &[u8]) -> io::Result<Value> {
    let directory = Temporary::new()?;
    let stdout = directory.0.join("stdout");
    let stderr = directory.0.join("stderr");
    let mut child = command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(fs::File::create(&stdout)?)
        .stderr(fs::File::create(&stderr)?)
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take()
        && let Err(error) = stdin.write_all(input)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let start = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status, false);
        }
        if start.elapsed() > Duration::from_secs(30) {
            // Only this preview's process group; never a service or worker.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            break (child.wait()?, true);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let read = |path: &std::path::Path| -> io::Result<(String, bool)> {
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        let truncated = bytes.len() > 1024 * 1024;
        bytes.truncate(1024 * 1024);
        Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
    };
    let (stdout, out_truncated) = read(&stdout)?;
    let (stderr, err_truncated) = read(&stderr)?;
    Ok(
        json!({"stdout":stdout,"stderr":stderr,"exit_code":status.code(),"timed_out":timed_out,"truncated":out_truncated || err_truncated}),
    )
}

pub fn cli(args: &[&str], input: &Value) -> crate::issues::Result<Value> {
    let result = capture(
        Command::new(std::env::current_exe()?).args(args),
        &serde_json::to_vec(input)?,
    )?;
    if result["exit_code"] != 0 {
        return Err(crate::issues::Error::new(
            "preview_failed",
            result["stderr"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("Preview timed out or exited without a result"),
        ));
    }
    Ok(serde_json::from_str(
        result["stdout"].as_str().unwrap_or(""),
    )?)
}
