use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

pub struct Fixture {
    pub root: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "hb-environment-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::copy(
            crate::worker_fixtures::binary("signing-gh"),
            root.join("bin/gh"),
        )
        .unwrap();
        Self { root }
    }
    pub fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(&self.root)
            .env_remove("SSH_AUTH_SOCK")
            .env_remove("SSH_AGENT_PID")
            .env("HOME", &self.root)
            .env("GNUPGHOME", self.root.join(".gnupg"))
            .env("XDG_CONFIG_HOME", self.root.join(".config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join(".gitconfig"))
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_CONFIG_DIR")
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            );
        for (key, _) in std::env::vars() {
            if key.starts_with("GIT_")
                && !["GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_GLOBAL"].contains(&key.as_str())
            {
                c.env_remove(key);
            }
        }
        c
    }
    pub fn cli(&self, action: &str) -> Output {
        self.command(env!("CARGO_BIN_EXE_hey-boss"))
            .args(["environment", action, "--json"])
            .output()
            .unwrap()
    }
    #[allow(dead_code)]
    pub fn git(&self, args: &[&str]) {
        let o = self.command("git").args(args).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    pub fn success(&self, action: &str) -> serde_json::Value {
        let o = self.cli(action);
        assert!(
            o.status.success(),
            "{} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.root.join(".gnupg").exists() {
            let _ = self
                .command("gpgconf")
                .args(["--kill", "gpg-agent"])
                .output();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
