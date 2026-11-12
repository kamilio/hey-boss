//! Observe native Turbo completion evidence after signalling its process group.
//! See docs/interrupted-validation.md for usage and interpretation.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    os::unix::process::{CommandExt, ExitStatusExt},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

struct Group(Child);
impl Group {
    fn signal(&self, signal: i32) -> io::Result<()> {
        if unsafe { libc::kill(-(self.0.id() as i32), signal) } == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(err)
        }
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        // The task runner can exit before its task processes do.
        let _ = self.signal(libc::SIGKILL);
        let _ = self.0.wait();
    }
}
fn execute(
    mut cmd: Command,
    root: &Path,
    signal: Option<i32>,
    timeout: Duration,
    abort: &AtomicBool,
) -> io::Result<(ExitStatus, bool, bool, String)> {
    let log = root.join("command.log");
    let out = fs::File::create(&log)?;
    let mut group = Group(
        cmd.process_group(0)
            .stdin(Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out)
            .spawn()?,
    );
    let start = Instant::now();
    let marker = root.join("packages/slow/started");
    let mut sent = false;
    let mut timed_out = false;
    let status = loop {
        if abort.load(Ordering::Relaxed) {
            return Err(io::Error::other("Investigation interrupted"));
        }
        if let Some(status) = group.0.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            timed_out = true;
            group.signal(libc::SIGKILL)?;
            break group.0.wait()?;
        }
        if let Some(signal) = signal
            && !sent
            && marker.exists()
        {
            group.signal(signal)?;
            sent = true;
        }
        thread::sleep(Duration::from_millis(20));
    };
    drop(group);
    Ok((
        status,
        sent,
        timed_out,
        String::from_utf8_lossy(&fs::read(log)?).into_owned(),
    ))
}
fn reports(path: &Path) -> io::Result<Vec<Value>> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut reports = vec![];
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let raw: Value = serde_json::from_reader(fs::File::open(path)?)?;
            let tasks = raw["tasks"]
                .as_array()
                .ok_or_else(|| io::Error::other("Invalid task summary"))?
                .iter()
                .map(|t| json!({"taskId":t["taskId"],"execution":t["execution"]}))
                .collect::<Vec<_>>();
            reports.push(json!({"execution":raw["execution"],"tasks":tasks}));
        }
    }
    Ok(reports)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let binary = fs::canonicalize(
        std::env::args_os()
            .nth(1)
            .ok_or("Supply the path to a native Turbo executable")?,
    )?;
    let abort = Arc::new(AtomicBool::new(false));
    let flag = abort.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))?;
    fs::create_dir_all("out")?;
    let fixture = tempfile::Builder::new()
        .prefix("turbo-interrupt-")
        .tempdir_in(fs::canonicalize("out")?)?;
    let root = fixture.path();
    let command = |args: &[&str]| {
        let mut c = Command::new(&binary);
        c.current_dir(root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", root)
            .env("TMPDIR", root)
            .env("CI", "1")
            .env("TURBO_TELEMETRY_DISABLED", "1")
            .args(args);
        c
    };
    let (status, _, _, output) = execute(
        command(&["--version"]),
        root,
        None,
        Duration::from_secs(5),
        &abort,
    )?;
    if !status.success() {
        return Err(format!("Turbo version check failed: {output}").into());
    }
    println!(
        "{}",
        json!({"version":output.trim(),"sha256":format!("{:x}",Sha256::digest(fs::read(&binary)?))})
    );
    fs::write(root.join("package.json"),json!({"name":"interruption-fixture","private":true,"packageManager":"npm@10.9.2","workspaces":["packages/*"]}).to_string())?;
    fs::write(root.join("package-lock.json"),json!({"name":"interruption-fixture","lockfileVersion":3,"packages":{"":{"name":"interruption-fixture","workspaces":["packages/*"]},"packages/fast":{"name":"fast","version":"1.0.0"},"packages/slow":{"name":"slow","version":"1.0.0"}}}).to_string())?;
    fs::write(
        root.join("turbo.json"),
        r#"{"tasks":{"test":{"cache":false}}}"#,
    )?;
    for (name, task) in [
        ("fast", "echo FAST_DONE"),
        ("slow", "echo yes > started && sleep 1.5 && echo SLOW_DONE"),
    ] {
        let dir = root.join("packages").join(name);
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("package.json"),
            json!({"name":name,"version":"1.0.0","scripts":{"test":task}}).to_string(),
        )?;
    }
    for signal in [
        None,
        Some(("SIGINT", libc::SIGINT)),
        Some(("SIGTERM", libc::SIGTERM)),
    ] {
        let marker = root.join("packages/slow/started");
        if marker.exists() {
            fs::remove_file(marker)?;
        }
        let runs = root.join(".turbo/runs");
        if runs.exists() {
            fs::remove_dir_all(&runs)?;
        }
        let (status, sent, timed_out, output) = execute(
            command(&[
                "run",
                "test",
                "--concurrency=1",
                "--summarize",
                "--cache=local:",
            ]),
            root,
            signal.map(|s| s.1),
            Duration::from_secs(15),
            &abort,
        )?;
        let reports = reports(&runs)?;
        let exit_signal = status.signal().map(|s| match s {
            libc::SIGINT => json!("SIGINT"),
            libc::SIGTERM => json!("SIGTERM"),
            libc::SIGKILL => json!("SIGKILL"),
            _ => json!(s),
        });
        println!(
            "{}",
            json!({"requestedSignal":signal.map(|s|s.0),"sent":sent,"timedOut":timed_out,"code":status.code(),"signal":exit_signal,"output":output,"reports":reports})
        );
        if timed_out {
            return Err("Timed out waiting for Turbo".into());
        }
        if signal.is_some() {
            if !sent {
                return Err("Slow task never started; interruption was not tested".into());
            }
        } else {
            if !status.success() || reports.len() != 1 || reports[0]["execution"]["success"] != 2 {
                return Err("Uninterrupted control did not finish both tasks".into());
            }
            let mut ids = reports[0]["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["taskId"].as_str())
                .collect::<Vec<_>>();
            ids.sort();
            if ids != [Some("fast#test"), Some("slow#test")] {
                return Err("Uninterrupted control task set mismatch".into());
            }
        }
    }
    Ok(())
}
