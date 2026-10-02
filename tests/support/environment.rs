use std::{
    fs,
    os::unix::fs::PermissionsExt,
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
        let node = Command::new("which").arg("node").output().unwrap();
        let script = format!(
            "#!{}\n{}",
            String::from_utf8(node.stdout).unwrap().trim(),
            r#"
const fs = require('fs'), path = require('path');
const root = process.env.HOME, args = process.argv.slice(2);
const endpoint = args.find(a => /^users?(?:\/|$)/.test(a));
fs.appendFileSync(path.join(root, 'api-log'), args.join(' ') + '\n');
if (fs.existsSync(path.join(root, 'denied')) && endpoint === 'user/ssh_signing_keys') {
  console.error('HTTP 403 Resource not accessible by personal access token'); process.exit(1);
}
const file = path.join(root, 'github-keys');
let keys = fs.existsSync(file) ? JSON.parse(fs.readFileSync(file)) : [];
if (endpoint === 'user') console.log(JSON.stringify({id: 42, login: 'octocat', name: 'Octo Cat'}));
else if (endpoint === 'users/octocat/gpg_keys') console.log(JSON.stringify([[{key_id:fs.readFileSync(path.join(root,'gpg-key-id'),'utf8').trim(), can_sign:true}]]));
else if (endpoint === 'user/emails') console.log(JSON.stringify([[{email:'unverified@example.com', primary:true, verified:false}]]));
else if (endpoint === 'user/ssh_signing_keys' && args.includes('POST')) {
  const key = args.find(a => a.startsWith('key=')).slice(4);
  if (!fs.existsSync(path.join(root, 'discard-registration'))) { keys.push({key}); fs.writeFileSync(file, JSON.stringify(keys)); }
  console.log(JSON.stringify({key}));
} else if (endpoint === 'users/octocat/ssh_signing_keys') console.log(JSON.stringify([keys]));
else { console.error('Unexpected gh call: ' + args.join(' ')); process.exit(1); }
"#
        );
        fs::write(root.join("bin/gh"), script).unwrap();
        fs::set_permissions(root.join("bin/gh"), fs::Permissions::from_mode(0o755)).unwrap();
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
