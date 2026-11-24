use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::current_dir().unwrap().join(format!(
            ".aws-login-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let fixture = Self(path);
        fixture.script("aws", r#"#!/bin/sh
set -eu
test "$1" = --profile
profile=$2
test "$3" = --no-cli-pager
shift 3
printf '%s:%s\n' "$profile" "$*" >> "$TEST_ROOT/aws-calls"
case "$1" in
login)
  read -r answer
  test "$answer" = n
  eval "set -- $BROWSER"
  "$1" "$2" 'https://signin.example.com/authorize?redirect_uri=http%3A%2F%2F127.0.0.1%3A4123%2Foauth%2Fcallback&state=synthetic' || sleep 30
  if test "$MODE" = wait; then sleep 30; fi
  if test "$MODE" != declined; then printf 'Updated profile %s to use fixture credentials.\n' "$profile"; fi
  ;;
sts)
  test "$MODE" != verification-failed
  printf '{"Account":"000000000000","Arn":"arn:aws:sts::000000000000:assumed-role/fixture/session"}\n'
  ;;
*) exit 1;;
esac
"#);
        fixture.script(
            "hey-boss",
            r#"#!/bin/sh
set -eu
printf '%s\n' "$2" >> "$TEST_ROOT/browser-calls"
case "$2" in
browser.authorize)
  test "$1" = action
  test "$3" = --params
  if test "$MODE" = browser-failed; then printf 'Desktop disconnected\n' >&2; exit 1; fi
  printf '%s' "$4" > "$TEST_ROOT/authorization-url"
  printf '{"result":{"opened":true,"session":"fixture-session"}}\n'
  ;;
close) test "$1" = browser; test "$3" = fixture-session;;
*) exit 1;;
esac
"#,
        );
        fixture
    }
    fn script(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        fs::write(&path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn command(&self, mode: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-aws-login"));
        command
            .current_dir(&self.0)
            .env("TEST_ROOT", &self.0)
            .env("MODE", mode)
            .env(
                "PATH",
                format!("{}:{}", self.0.display(), std::env::var("PATH").unwrap()),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).unwrap_or_default()
    }
    fn clean(&self) {
        assert!(!fs::read_dir(&self.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".aws-login-")
        }));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn login_uses_the_requested_profile_and_closes_the_callback() {
    for profile in ["work", "another organization"] {
        let fixture = Fixture::new();
        let output = fixture.command("success").arg(profile).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fixture.read("aws-calls"),
            format!("{profile}:login\n{profile}:sts get-caller-identity --output json\n")
        );
        assert_eq!(fixture.read("browser-calls"), "browser.authorize\nclose\n");
        assert!(fixture.read("authorization-url").contains("redirect_uri="));
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains(&format!("Authenticated profile {profile}:"))
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("redirect_uri"));
        fixture.clean();
    }
}

#[test]
fn omitted_profile_uses_aws_environment_then_default() {
    for profile in [Some("environment-org"), None] {
        let fixture = Fixture::new();
        let mut command = fixture.command("success");
        command.env_remove("AWS_PROFILE");
        if let Some(profile) = profile {
            command.env("AWS_PROFILE", profile);
        }
        assert!(command.output().unwrap().status.success());
        assert!(
            fixture
                .read("aws-calls")
                .starts_with(&format!("{}:login\n", profile.unwrap_or("default")))
        );
        fixture.clean();
    }
}

#[test]
fn declining_a_profile_change_is_failure_even_when_aws_exits_zero() {
    let fixture = Fixture::new();
    let output = fixture.command("declined").arg("work").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("AWS did not update the profile"));
    assert_eq!(fixture.read("aws-calls"), "work:login\n");
    assert_eq!(fixture.read("browser-calls"), "browser.authorize\nclose\n");
    fixture.clean();
}

#[test]
fn disconnected_browser_stops_login_promptly() {
    let fixture = Fixture::new();
    let began = Instant::now();
    let output = fixture
        .command("browser-failed")
        .arg("work")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(began.elapsed() < Duration::from_secs(5));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Desktop disconnected"));
    assert_eq!(fixture.read("aws-calls"), "work:login\n");
    fixture.clean();
}

#[test]
fn verification_failure_never_reports_success() {
    let fixture = Fixture::new();
    let output = fixture
        .command("verification-failed")
        .arg("work")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(fixture.read("browser-calls"), "browser.authorize\nclose\n");
    fixture.clean();
}

#[test]
fn termination_kills_login_and_releases_the_callback() {
    let fixture = Fixture::new();
    let child = fixture.command("wait").arg("work").spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fs::read_dir(&fixture.0)
        .unwrap()
        .any(|entry| entry.unwrap().path().join("session").exists())
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(fixture.read("browser-calls"), "browser.authorize\nclose\n");
    fixture.clean();
}
