//! Isolated investigation of nested validation admission; never opens a live pool.
//! rustc --edition 2024 tools/investigate_verification_slots.rs -o /tmp/verification-slots
//! /tmp/verification-slots /absolute/path/to/with-concurrency-slot.sh
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Probe {
    root: PathBuf,
    children: Vec<Child>,
}

impl Drop for Probe {
    fn drop(&mut self) {
        for child in &mut self.children {
            // Only private process groups created by this probe. Reap after signalling.
            if child.try_wait().ok().flatten().is_none() {
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .status();
                let _ = child.wait();
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Probe {
    fn new() -> io::Result<Self> {
        let root = std::env::temp_dir().join(format!("hey-boss-slots-{}", std::process::id()));
        fs::create_dir(&root)?;
        Ok(Self {
            root,
            children: vec![],
        })
    }

    fn spawn(&mut self, mut command: Command) -> io::Result<usize> {
        command.process_group(0).stdin(Stdio::null());
        self.children.push(command.spawn()?);
        Ok(self.children.len() - 1)
    }

    fn completed(&mut self, child: usize) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.children[child].try_wait()? {
                return if status.code() == Some(0) {
                    Ok(())
                } else {
                    Err(io::Error::other(format!("incomplete/failed: {status}")))
                };
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("incomplete: process completion timeout"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn wait_file(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "incomplete: missing {}",
                path.display()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn lock(path: &Path, wait: bool) -> Command {
    let mut command;
    if cfg!(target_os = "macos") {
        command = Command::new("lockf");
        command.args(["-s", "-k", "-t", if wait { "20" } else { "0" }]);
    } else {
        command = Command::new("flock");
        command.args(["-E", "75", "-w", if wait { "20" } else { "0" }]);
    }
    command.arg(path);
    command
}

fn busy(path: &Path) -> io::Result<bool> {
    match lock(path, false).arg("true").status()?.code() {
        Some(0) => Ok(false),
        Some(75) => Ok(true),
        other => Err(io::Error::other(format!("lock probe failed: {other:?}"))),
    }
}

fn require(condition: bool, message: &str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

fn run(wrapper: &Path) -> io::Result<()> {
    let mut probe = Probe::new()?;
    println!("Isolated pool: {}", probe.root.display());
    // Files are arguments, never interpolated into shell syntax.
    for slot in ["slot.1", "slot.2"] {
        fs::write(probe.root.join(slot), "")?;
    }
    let slot1 = probe.root.join("slot.1");
    let slot2 = probe.root.join("slot.2");
    let release = probe.root.join("release");
    let peer_ready = probe.root.join("peer-ready");
    let mut peer = lock(&slot2, true);
    peer.args([
        "sh",
        "-c",
        "touch \"$1\"; while [ ! -f \"$2\" ]; do sleep 0.05; done",
        "peer",
    ])
    .arg(&peer_ready)
    .arg(&release);
    let peer = probe.spawn(peer)?;
    wait_file(&peer_ready)?;
    require(busy(&slot2)?, "peer did not hold slot 2")?;

    // Reproduce the observed explicit two-lock command. The outer marker proves
    // slot 1 admission; absence of the inner marker proves payload non-admission.
    let outer_ready = probe.root.join("outer-ready");
    let payload = probe.root.join("payload");
    let waiter_pid = probe.root.join("waiter-pid");
    let mut nested = lock(&slot1, true);
    let primitive = if cfg!(target_os = "macos") {
        "lockf -s -k -t 20"
    } else {
        "flock -E 75 -w 20"
    };
    nested
        .args([
            "sh",
            "-c",
            &format!("touch \"$1\"; echo $$ > \"$2\"; exec {primitive} \"$3\" touch \"$4\""),
            "outer",
        ])
        .arg(&outer_ready)
        .arg(&waiter_pid)
        .arg(&slot2)
        .arg(&payload);
    let nested = probe.spawn(nested)?;
    wait_file(&waiter_pid)?;
    require(
        busy(&slot1)? && busy(&slot2)? && !payload.exists(),
        "nested reservation was not reproduced",
    )?;
    println!(
        "REPRODUCED holder(slot.1)={} → waiter(slot.2)={}; holder(slot.2)={}; payload=not-admitted",
        probe.children[nested].id(),
        fs::read_to_string(&waiter_pid)?.trim(),
        probe.children[peer].id()
    );
    fs::write(&release, "release peer normally")?;
    probe.completed(peer)?;
    probe.completed(nested)?;
    require(
        payload.exists(),
        "nested payload never completed after peer release",
    )?;
    require(
        !busy(&slot1)? && !busy(&slot2)?,
        "completed reproduction retained a slot",
    )?;
    println!(
        "PASS 1/3: explicit nesting reserved both slots; payload completed only after peer release"
    );

    // Restart a real peer while exercising the repository's actual wrapper,
    // both directly and recursively. Neither control may need the peer's slot.
    for (index, recursive) in [false, true].into_iter().enumerate() {
        fs::remove_file(&release)?;
        fs::remove_file(&peer_ready)?;
        let mut peer = lock(&slot2, true);
        peer.args([
            "sh",
            "-c",
            "touch \"$1\"; while [ ! -f \"$2\" ]; do sleep 0.05; done",
            "peer",
        ])
        .arg(&peer_ready)
        .arg(&release);
        let peer = probe.spawn(peer)?;
        wait_file(&peer_ready)?;
        let ready = probe.root.join(format!("control-{index}-ready"));
        let done = probe.root.join(format!("control-{index}-done"));
        let end = probe.root.join(format!("control-{index}-end"));
        let mut control = Command::new("bash");
        control.arg(wrapper);
        if recursive {
            control.arg("bash").arg(wrapper);
        }
        control
            .args([
                "sh",
                "-c",
                "touch \"$1\"; while [ ! -f \"$2\" ]; do sleep 0.05; done; touch \"$3\"",
                "payload",
            ])
            .arg(&ready)
            .arg(&end)
            .arg(&done)
            .env("POE2_PRE_COMMIT_SEM_DIR", &probe.root)
            .env("POE2_PRE_COMMIT_MAX_CONCURRENCY", "2")
            .env("POE2_PRE_COMMIT_QUEUE_TIMEOUT", "10")
            .env_remove("POE2_PRE_COMMIT_SLOT_HELD");
        let control = probe.spawn(control)?;
        wait_file(&ready)?;
        require(
            busy(&slot1)? && busy(&slot2)?,
            "concurrency cap not preserved",
        )?;
        require(
            probe.children[peer].try_wait()?.is_none(),
            "peer exited early",
        )?;
        fs::write(&end, "complete payload")?;
        probe.completed(control)?;
        require(
            done.exists(),
            "incomplete: missing payload completion evidence",
        )?;
        require(
            !busy(&slot1)? && busy(&slot2)?,
            "control did not release only its own slot",
        )?;
        fs::write(&release, "complete peer")?;
        probe.completed(peer)?;
        require(!busy(&slot1)? && !busy(&slot2)?, "control retained a slot")?;
        println!(
            "PASS {}/3: {} wrapper completed with peer still holding slot.2; cap=2, payload=complete, slots=released",
            index + 2,
            if recursive { "nested" } else { "single" }
        );
    }
    println!(
        "COMPLETE: 3/3 scenarios; all children exited normally; fresh payload markers verified"
    );
    Ok(())
}

fn main() -> io::Result<()> {
    let wrapper = std::env::args_os()
        .nth(1)
        .ok_or_else(|| io::Error::other("supply the repository's with-concurrency-slot.sh path"))?;
    let wrapper = fs::canonicalize(wrapper)?;
    run(&wrapper)
}
