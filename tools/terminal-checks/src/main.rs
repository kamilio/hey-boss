//! Real CLI walkthroughs. All project state and subprocess environments are private.
mod comments;
mod issues;
mod project;
mod shelf;
mod terminal;
use clap::{Parser, Subcommand};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use terminal::Terminal;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    check: Check,
    /// Directory for the captured PNGs.
    #[arg(long, global = true, default_value = "out/terminal-checks")]
    output: PathBuf,
}
#[derive(Subcommand)]
enum Check {
    Comments {
        hey_gh: PathBuf,
    },
    Settings {
        hey_boss: PathBuf,
    },
    Project {
        hey_boss: PathBuf,
    },
    Transactions {
        hey_boss: PathBuf,
        #[arg(long)]
        serve: bool,
    },
    Shelf {
        g: PathBuf,
        hey_boss: PathBuf,
    },
}

struct Service(Child);
impl Drop for Service {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
            let end = Instant::now() + Duration::from_secs(5);
            while self.0.try_wait().is_ok_and(|status| status.is_none()) && Instant::now() < end {
                thread::sleep(Duration::from_millis(20))
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
struct Fixture {
    service: Option<Service>,
    dir: tempfile::TempDir,
    bin: PathBuf,
    cwd: PathBuf,
    output: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(service) = self.service.take() {
            drop(service);
            // Only this disposable database uses this lock identity.
            use sha2::{Digest, Sha256};
            let db = self.dir.path().join("db");
            let Ok(path) = db.canonicalize() else { return };
            let hash = format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()));
            let base = PathBuf::from(format!("/tmp/hey-boss-db-{}", unsafe { libc::getuid() }))
                .join(&hash[..24]);
            if !base.with_extension("sock").exists() {
                for suffix in ["lock", "log", "startup"] {
                    let _ = fs::remove_file(base.with_extension(suffix));
                }
            }
        }
    }
}
impl Fixture {
    fn new(binary: &Path, output: PathBuf) -> Self {
        let parent = std::env::current_dir().unwrap();
        let dir = tempfile::Builder::new()
            .prefix(".qa")
            .tempdir_in(&parent)
            .unwrap();
        let cwd = dir.path().join("checkout");
        for path in [
            &cwd,
            &dir.path().join("home"),
            &dir.path().join("bin"),
            &output,
        ] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(dir.path().join("inventory"), r#"{"ssh_hosts":[]}"#).unwrap();
        fs::write(
            dir.path().join("desired"),
            r#"{"machines":{"local":{"workers":[]}}}"#,
        )
        .unwrap();
        Self {
            service: None,
            dir,
            bin: binary.canonicalize().unwrap(),
            cwd,
            output,
        }
    }
    fn cmd(&self, program: &Path, args: &[&str]) -> Command {
        let root = self.dir.path();
        let mut c = Command::new(program);
        c.args(args)
            .current_dir(&self.cwd)
            .env_clear()
            .env("HOME", root.join("home"))
            .env(
                "PATH",
                std::env::join_paths([
                    root.join("bin"),
                    PathBuf::from("/usr/bin"),
                    PathBuf::from("/bin"),
                ])
                .unwrap(),
            )
            .env("TERM", "xterm-256color")
            .env("CLICOLOR_FORCE", "1")
            .env("LANG", "en_US.UTF-8")
            .env("GIT_CEILING_DIRECTORIES", root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("HEY_BOSS_ISSUE_DB", root.join("db"))
            .env("HEY_BOSS_FLEET_STATE", root.join("f"))
            .env("HEY_BOSS_FLEET_CONFIG", root.join("inventory"))
            .env("HEY_BOSS_FLEET_DESIRED", root.join("desired"))
            .env("HEY_BOSS_FLEET_BINARY", &self.bin)
            .env("HEY_BOSS_AGENT_ID", "human:qa")
            .env("HEY_BOSS_INBOX_SOCKET", root.join("absent.sock"));
        c
    }
    fn cli(&self, args: &[&str]) -> Command {
        self.cmd(&self.bin, args)
    }
    fn start(&mut self) {
        self.service = Some(Service(
            self.cli(&["fleet", "supervisor"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        ));
        let end = Instant::now() + Duration::from_secs(10);
        while !self.dir.path().join("f/fleet.sock").exists() {
            assert!(
                self.service
                    .as_mut()
                    .unwrap()
                    .0
                    .try_wait()
                    .unwrap()
                    .is_none()
                    && Instant::now() < end,
                "private supervisor did not start"
            );
            thread::sleep(Duration::from_millis(30))
        }
    }
    fn init(&self, project: &str) {
        run(self.cli(&[
            "project",
            "init",
            "--project",
            project,
            "--yes",
            "--prs",
            "false",
            "--worktree",
            "false",
            "--drafts",
            "true",
            "--json",
        ]));
    }
    fn screen(&self, args: &[&str], w: usize, h: usize) -> Terminal {
        Terminal::open(self.cli(args), w, h)
    }
    fn shot(&self, t: &mut Terminal, name: &str) {
        t.save(&self.output.join(format!("{name}.png")))
    }
}
fn run(mut c: Command) -> String {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "{c:?}: {} {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap().trim().to_owned()
}
fn json(c: Command) -> Value {
    serde_json::from_str(&run(c)).unwrap()
}
fn main() {
    let args = Args::parse();
    let (binary, name) = match &args.check {
        Check::Comments { hey_gh } => (hey_gh, "comments"),
        Check::Settings { hey_boss } => (hey_boss, "settings"),
        Check::Project { hey_boss } => (hey_boss, "project"),
        Check::Shelf { hey_boss, .. } => (hey_boss, "shelf"),
        Check::Transactions { hey_boss, .. } => (hey_boss, "transactions"),
    };
    let mut f = Fixture::new(binary, args.output.join(name));
    if !matches!(&args.check, Check::Comments { .. }) {
        f.start()
    }
    match args.check {
        Check::Comments { .. } => comments::check(&f),
        Check::Settings { .. } => issues::settings(&f),
        Check::Project { .. } => project::check(&mut f),
        Check::Shelf { g, .. } => shelf::check(&f, &g.canonicalize().unwrap()),
        Check::Transactions { serve, .. } => issues::transactions(&f, serve),
    }
    println!("{name} passed; screens in {}", f.output.display());
}
