//! Git identity and signing checks run in the caller's OS user and environment.
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs, io,
    os::fd::AsRawFd,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Command,
};
mod quota;

/// A server-directed preflight retry, preserved through the worker result journal.
pub fn retry_at(error: &io::Error) -> Option<i64> {
    error
        .get_ref()?
        .downcast_ref::<quota::Quota>()
        .map(|q| q.retry_at)
}

type Config = BTreeMap<String, String>;
const SETTINGS: &[&str] = &[
    "user.name",
    "user.email",
    "user.signingkey",
    "user.useconfigonly",
    "commit.gpgsign",
    "tag.gpgsign",
    "gpg.format",
    "gpg.program",
    "gpg.ssh.program",
    "gpg.ssh.defaultkeycommand",
    "gpg.ssh.revocationfile",
    "gpg.openpgp.program",
    "gpg.x509.program",
    "gpg.mintrustlevel",
];

#[derive(Args)]
pub struct Options {
    /// Diagnose effective settings for this checkout (defaults to the current directory).
    #[arg(short = 'C', long, global = true)]
    directory: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Configure Git identity and signing; register the public signing key with GitHub.
    Setup,
    /// Verify signing and GitHub registration without changing configuration or keys.
    Check,
}
#[derive(Serialize)]
pub struct Report {
    ok: bool,
    uid: u32,
    home: PathBuf,
    directory: PathBuf,
    name: String,
    email: String,
    format: String,
    signing_key: String,
    repository_overrides: Config,
}

pub fn run(options: &Options) -> io::Result<()> {
    let cwd = options
        .directory
        .clone()
        .unwrap_or(std::env::current_dir()?);
    let result = match options.action {
        Action::Setup => setup(&cwd),
        Action::Check => check(&cwd),
    };
    match result {
        Ok(report) => {
            if options.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!(
                    "Signing verified for {} <{}> (uid {}, HOME={}).\nGitHub registration confirmed; {} signing key: {}",
                    report.name,
                    report.email,
                    report.uid,
                    report.home.display(),
                    report.format,
                    report.signing_key
                );
                for (key, value) in report.repository_overrides {
                    println!("Repository override: {key}={value}");
                }
            }
            Ok(())
        }
        Err(error) => {
            if options.json {
                let retry_at = retry_at(&error);
                println!(
                    "{}",
                    serde_json::json!({"ok":false,"error":error.to_string(),"retry_at":retry_at})
                );
            }
            Err(error)
        }
    }
}
fn fail(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn home() -> io::Result<PathBuf> {
    let home = PathBuf::from(
        std::env::var_os("HOME")
            .ok_or_else(|| fail("HOME is missing; run environment setup as the worker OS user"))?,
    );
    if !home.is_absolute() || !home.is_dir() {
        return Err(fail("HOME must be an existing absolute directory"));
    }
    Ok(home)
}
fn output(command: &mut Command, label: &str) -> io::Result<String> {
    // Bound network, agents, pinentry and credential-helper waits in unattended workers.
    let result = crate::admin::capture(command, &[]).map_err(|e| fail(format!("{label}: {e}")))?;
    if result["timed_out"] == true {
        return Err(fail(format!("{label}: timed out after 30 seconds")));
    }
    if result["exit_code"] != 0 || result["truncated"] == true {
        return Err(fail(format!(
            "{label}: {}",
            result["stderr"].as_str().unwrap_or("command failed").trim()
        )));
    }
    Ok(result["stdout"]
        .as_str()
        .unwrap_or_default()
        .trim_end()
        .to_owned())
}
fn git(cwd: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(cwd);
    c
}
fn isolated_git(cwd: &Path) -> Command {
    let mut c = git(cwd);
    // TMPDIR may itself live inside a checkout; don't discover its parent repository.
    if let Ok(path) = cwd.canonicalize()
        && let Some(parent) = path.parent()
    {
        c.env("GIT_CEILING_DIRECTORIES", parent);
    }
    // Preserve configuration and agent variables, but never write the caller's repository.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
    ] {
        c.env_remove(key);
    }
    c
}
fn config(cwd: &Path) -> io::Result<Config> {
    read_config(git(cwd))
}
fn read_config(mut command: Command) -> io::Result<Config> {
    let text = output(
        command.args(["config", "--null", "--list"]),
        "Read Git configuration",
    )?;
    let mut values = Config::new();
    for entry in text.split('\0') {
        let (key, value) = entry.split_once('\n').unwrap_or((entry, "true"));
        if SETTINGS.contains(&key) {
            values.insert(key.to_owned(), value.to_owned());
        }
    }
    Ok(values)
}
fn value<'a>(config: &'a Config, key: &str) -> &'a str {
    config.get(key).map(String::as_str).unwrap_or("")
}
fn enabled(config: &Config, key: &str) -> bool {
    matches!(
        value(config, key).to_ascii_lowercase().as_str(),
        "true" | "yes" | "on" | "1" | ""
    ) && config.contains_key(key)
}
fn format(config: &Config) -> &str {
    config
        .get("gpg.format")
        .map(String::as_str)
        .unwrap_or("openpgp")
}
fn api(endpoint: &str, fields: &[(&str, &str)], pages: bool) -> io::Result<Value> {
    let mut gate = quota::Gate::open(&quota::scope()?)?;
    // launchd's normal PATH omits Homebrew and ~/.local/bin. Resolve the CLI
    // without changing Git's environment or relying on an interactive shell.
    use std::os::unix::fs::PermissionsExt;
    let mut candidates: Vec<_> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|path| path.join("gh"))
            .collect();
    candidates.extend([
        home()?.join(".local/bin/gh"),
        "/opt/homebrew/bin/gh".into(),
        "/usr/local/bin/gh".into(),
    ]);
    let gh = candidates
        .into_iter()
        .find(|path| {
            fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .ok_or_else(|| {
            fail("GitHub CLI (gh) is missing; install and authenticate it as the worker OS user")
        })?;
    let mut c = Command::new(gh);
    c.args(["api", "--hostname", "github.com", "--include", endpoint]);
    if pages {
        c.arg("--paginate");
    }
    if !fields.is_empty() {
        c.args(["--method", "POST"]);
    }
    for (key, value) in fields {
        c.arg("-f").arg(format!("{key}={value}"));
    }
    let permission = if endpoint == "user/ssh_signing_keys" && !fields.is_empty() {
        "GitHub signing-key registration requires admin:ssh_signing_key permission (or fine-grained SSH signing keys write access). Use gh auth refresh -h github.com -s admin:ssh_signing_key"
    } else {
        "Check the current account's GitHub permissions"
    };
    let result = crate::admin::capture(&mut c, &[])?;
    if result["timed_out"] == true || result["truncated"] == true {
        return Err(fail(format!(
            "GitHub request failed for {endpoint}: response timed out or exceeded the capture limit"
        )));
    }
    let responses = responses(result["stdout"].as_str().unwrap_or_default())?;
    for response in &responses {
        let message = response.body["message"].as_str().unwrap_or_default();
        let lower = message.to_ascii_lowercase();
        if response.status == 429
            || response.status == 403
                && (response
                    .headers
                    .get("x-ratelimit-remaining")
                    .is_some_and(|v| v == "0")
                    || response.headers.contains_key("retry-after")
                    || lower.contains("rate limit")
                    || lower.contains("abuse detection"))
        {
            return Err(gate.exhausted(&response.headers)?);
        }
        if response.status == 401 {
            return Err(fail(format!(
                "GitHub authentication failed for {endpoint} (HTTP 401). Authenticate gh for github.com as the worker OS user"
            )));
        }
        if response.status == 403 {
            return Err(fail(format!(
                "GitHub permission denied for {endpoint} (HTTP 403). {permission}"
            )));
        }
        if response.status >= 400 {
            return Err(fail(format!(
                "GitHub request failed for {endpoint} (HTTP {})",
                response.status
            )));
        }
    }
    if result["exit_code"] != 0 || responses.is_empty() {
        let detail = result["stderr"].as_str().unwrap_or_default().trim();
        if detail.contains("gh auth login") || detail.contains("not logged into") {
            return Err(fail(
                "GitHub authentication failed. Authenticate gh for github.com as the worker OS user",
            ));
        }
        return Err(fail(format!(
            "GitHub request failed for {endpoint}: {detail}"
        )));
    }
    gate.succeeded()?;
    if pages {
        Ok(Value::Array(
            responses.into_iter().map(|r| r.body).collect(),
        ))
    } else {
        Ok(responses.into_iter().last().unwrap().body)
    }
}

#[cfg(test)]
mod response_tests {
    use super::*;
    #[test]
    fn paginated_headers_and_json_are_kept_per_response() {
        let parsed = responses("HTTP/2.0 200 OK\r\nX-RateLimit-Remaining: 1\r\n\r\n[{\"key\":\"first\"}]\nHTTP/2.0 403 Forbidden\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 200\r\n\r\n{\"message\":\"API rate limit exceeded\"}").unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].body[0]["key"], "first");
        assert_eq!(parsed[1].status, 403);
        assert_eq!(parsed[1].headers["x-ratelimit-reset"], "200");
        let parsed = responses("HTTP/1.1 200 OK\n\n[]\nHTTP/1.1 200 OK\n\n[1]").unwrap();
        assert_eq!(parsed[1].body[0], 1);
        assert_eq!(
            responses("HTTP/2.0 429\r\nRetry-After: 60\r\n\r\n<html>Busy</html>").unwrap()[0]
                .headers["retry-after"],
            "60"
        );
    }
}

struct Response {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Value,
}
fn responses(mut text: &str) -> io::Result<Vec<Response>> {
    let mut responses = vec![];
    while !text.trim().is_empty() {
        text = text.trim_start();
        let (headers, body) = text
            .split_once("\r\n\r\n")
            .or_else(|| text.split_once("\n\n"))
            .ok_or_else(|| fail("GitHub request failed: missing response headers"))?;
        let mut lines = headers.lines();
        let status = lines
            .next()
            .and_then(|l| l.strip_prefix("HTTP/"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| fail("GitHub request failed: invalid response status"))?;
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        if status >= 400 {
            responses.push(Response {
                status,
                headers,
                body: serde_json::from_str(body).unwrap_or(Value::Null),
            });
            break;
        }
        let mut stream = serde_json::Deserializer::from_str(body).into_iter::<Value>();
        let value = stream
            .next()
            .transpose()
            .map_err(io::Error::other)?
            .unwrap_or(Value::Null);
        text = &body[stream.byte_offset()..];
        responses.push(Response {
            status,
            headers,
            body: value,
        });
    }
    Ok(responses)
}
fn items(value: &Value) -> impl Iterator<Item = &Value> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|page| page.as_array().into_iter().flatten())
}
struct Identity {
    login: String,
    name: String,
    email: String,
    verified: Vec<String>,
}
fn identity() -> io::Result<Identity> {
    let user = api("user", &[], false)?;
    let login = user["login"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fail("GitHub account login missing"))?;
    let id = user["id"]
        .as_u64()
        .ok_or_else(|| fail("GitHub account ID missing"))?;
    let name = user["name"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(login)
        .to_owned();
    let emails = match api("user/emails", &[], true) {
        Err(e) if retry_at(&e).is_some() => return Err(e),
        result => result.unwrap_or(Value::Null),
    };
    let verified: Vec<_> = items(&emails).filter(|e| e["verified"] == true).collect();
    let noreply = format!("{id}+{login}@users.noreply.github.com");
    let email = verified
        .iter()
        .find(|e| e["primary"] == true)
        .or_else(|| verified.first())
        .and_then(|e| e["email"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| noreply.clone());
    let mut verified: Vec<String> = verified
        .iter()
        .filter_map(|e| e["email"].as_str().map(str::to_owned))
        .collect();
    verified.push(noreply);
    Ok(Identity {
        login: login.to_owned(),
        name,
        email,
        verified,
    })
}
fn public_key(text: &str) -> Option<String> {
    let mut parts = text
        .trim()
        .strip_prefix("key::")
        .unwrap_or(text.trim())
        .split_whitespace();
    let kind = parts.next()?;
    let key = parts.next()?;
    if kind.starts_with("ssh-") || kind.starts_with("ecdsa-") || kind.starts_with("sk-") {
        Some(format!("{kind} {key}"))
    } else {
        None
    }
}
fn key_path(text: &str, cwd: &Path) -> io::Result<PathBuf> {
    if let Some(rest) = text.strip_prefix("~/") {
        Ok(home()?.join(rest))
    } else {
        Ok(cwd.join(text))
    }
}
fn ssh_public(config: &Config, cwd: &Path) -> io::Result<String> {
    let key = value(config, "user.signingkey");
    if let Some(key) = public_key(key) {
        return Ok(key);
    }
    if key.is_empty() {
        let default = value(config, "gpg.ssh.defaultkeycommand");
        if !default.is_empty() {
            let text = output(
                Command::new("sh").args(["-c", default]).current_dir(cwd),
                "Resolve default SSH signing key",
            )?;
            return text
                .lines()
                .next()
                .and_then(public_key)
                .ok_or_else(|| fail("gpg.ssh.defaultKeyCommand did not return a public key"));
        }
        return Err(fail("user.signingkey is missing"));
    }
    let path = key_path(key, cwd)?;
    // Only read public files. Private material stays inside ssh-keygen/ssh-agent.
    let pub_path = if path.extension().is_some_and(|e| e == "pub") {
        path.clone()
    } else {
        PathBuf::from(format!("{}.pub", path.display()))
    };
    if let Ok(text) = fs::read_to_string(pub_path)
        && let Some(key) = public_key(&text)
    {
        return Ok(key);
    }
    let text = output(
        Command::new("ssh-keygen")
            .args(["-y", "-P", "", "-f"])
            .arg(path),
        "Read SSH public key",
    )?;
    public_key(&text).ok_or_else(|| fail("Invalid SSH public signing key"))
}
fn registered_keys(login: &str) -> io::Result<Vec<String>> {
    // Public account keys are authoritative and do not need a write-capable token.
    Ok(
        items(&api(&format!("users/{login}/ssh_signing_keys"), &[], true)?)
            .filter_map(|entry| entry["key"].as_str().and_then(public_key))
            .collect(),
    )
}
fn registered(key: &str, login: &str) -> io::Result<bool> {
    Ok(registered_keys(login)?
        .iter()
        .any(|registered| registered == key))
}
fn ensure_registration(key: &str, login: &str, setup: bool) -> io::Result<()> {
    if registered(key, login)? {
        return Ok(());
    }
    if !setup {
        return Err(fail(
            "SSH key is not registered with GitHub for signing; run hey-boss environment setup",
        ));
    }
    let hostname = output(&mut Command::new("hostname"), "Read machine name")?;
    let title = format!("Commit signing on {hostname}");
    if let Err(error) = api(
        "user/ssh_signing_keys",
        &[("title", &title), ("key", key)],
        false,
    ) {
        // Another machine may register a shared agent key between the GET and POST.
        if registered(key, login).unwrap_or(false) {
            return Ok(());
        }
        return Err(error);
    }
    if !registered(key, login)? {
        return Err(fail(
            "GitHub signing key registration was not confirmed by readback",
        ));
    }
    Ok(())
}
fn check_flags(config: &Config) -> io::Result<()> {
    for key in ["commit.gpgsign", "tag.gpgsign"] {
        if !enabled(config, key) {
            return Err(fail(format!(
                "{key} is not true; check repository overrides and run hey-boss environment setup"
            )));
        }
    }
    Ok(())
}
struct Proof {
    signature: String,
    emails: Vec<String>,
}
impl Proof {
    fn identity(&self, identity: &Identity) -> io::Result<()> {
        for email in &self.emails {
            if !identity
                .verified
                .iter()
                .any(|verified| verified.eq_ignore_ascii_case(email))
            {
                return Err(fail(format!(
                    "Commit email {email} is not verified for the authenticated GitHub account; check user.email, repository overrides, GIT_AUTHOR_EMAIL and GIT_COMMITTER_EMAIL. Reading private verified emails requires GitHub user:email permission"
                )));
            }
        }
        Ok(())
    }
}
fn probe(config: &Config, cwd: &Path) -> io::Result<Proof> {
    check_flags(config)?;
    let temp = crate::admin::Temporary::new()?;
    let command = || {
        let mut c = isolated_git(&temp.0);
        for (key, val) in config {
            let val =
                if key == "user.signingkey" && format(config) == "ssh" && public_key(val).is_none()
                {
                    key_path(val, cwd)?.to_string_lossy().into_owned()
                } else {
                    val.clone()
                };
            c.arg("-c").arg(format!("{key}={val}"));
        }
        c.args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ]);
        Ok::<_, io::Error>(c)
    };
    output(
        command()?.args(["init", "--quiet", "--template="]),
        "Create signing probe",
    )?;
    output(
        command()?.args([
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "Verify commit signing",
        ]),
        "Commit signing failed (without -S)",
    )?;
    let emails = output(
        command()?.args(["log", "-1", "--format=%ae%n%ce"]),
        "Read probe identity",
    )?
    .lines()
    .map(str::to_owned)
    .collect();
    let signature = match format(config) {
        "ssh" => {
            let key = ssh_public(config, cwd)?;
            let allowed = temp.0.join("allowed-signers");
            fs::write(&allowed, format!("* {key}\n"))?;
            output(
                command()?
                    .arg("-c")
                    .arg(format!("gpg.ssh.allowedSignersFile={}", allowed.display()))
                    .args(["verify-commit", "HEAD"]),
                "Verify SSH commit signature",
            )?;
            Ok(key)
        }
        "openpgp" => {
            output(
                command()?.args(["verify-commit", "HEAD"]),
                "Verify OpenPGP commit signature",
            )?;
            output(
                command()?.args(["log", "-1", "--format=%GF"]),
                "Read signing fingerprint",
            )
        }
        other => Err(fail(format!(
            "Preserving existing {other} signing configuration; GitHub verification supports SSH and OpenPGP"
        ))),
    }?;
    Ok(Proof { signature, emails })
}
fn github(config: &Config, signature: &str, login: &str, setup: bool) -> io::Result<()> {
    if format(config) == "ssh" {
        return ensure_registration(signature, login, setup);
    }
    let keys = api(&format!("users/{login}/gpg_keys"), &[], true)?;
    let matches = |key: &Value| {
        key["key_id"].as_str().is_some_and(|id| {
            id.len() >= 16
                && signature
                    .to_ascii_uppercase()
                    .ends_with(&id.to_ascii_uppercase())
        }) && key["can_sign"] == true
    };
    if items(&keys)
        .any(|key| matches(key) || key["subkeys"].as_array().into_iter().flatten().any(matches))
    {
        Ok(())
    } else {
        Err(fail(
            "Existing OpenPGP signing key is not registered with GitHub; preserving configuration. Register its public key with GitHub and retry",
        ))
    }
}
fn report(cwd: &Path, config: Config, global: &Config) -> io::Result<Report> {
    let repository_overrides = config
        .iter()
        .filter(|(key, v)| global.get(*key) != Some(*v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Ok(Report {
        ok: true,
        uid: unsafe { libc::geteuid() },
        home: home()?,
        directory: cwd.canonicalize()?,
        name: value(&config, "user.name").into(),
        email: value(&config, "user.email").into(),
        format: format(&config).into(),
        signing_key: value(&config, "user.signingkey").into(),
        repository_overrides,
    })
}
pub fn check(cwd: &Path) -> io::Result<Report> {
    check_inner(cwd, false)
}
fn check_inner(cwd: &Path, worker: bool) -> io::Result<Report> {
    home()?;
    let effective = config(cwd)?;
    let temp = crate::admin::Temporary::new()?;
    let global = read_config(isolated_git(&temp.0))?;
    let verified = || -> io::Result<()> {
        let proof = probe(&effective, cwd)?;
        if worker {
            use std::sync::{Mutex, OnceLock};
            use std::time::{Duration, Instant};
            static VERIFIED: OnceLock<Mutex<BTreeMap<String, Instant>>> = OnceLock::new();
            let key = format!(
                "{}:{:?}:{}:{:?}:{}",
                unsafe { libc::geteuid() },
                home()?,
                proof.signature,
                proof.emails,
                quota::scope()?
            );
            let mut cache = VERIFIED
                .get_or_init(Default::default)
                .lock()
                .map_err(|_| fail("Environment check cache poisoned"))?;
            cache.retain(|_, checked| checked.elapsed() < Duration::from_secs(300));
            if cache.contains_key(&key) {
                return Ok(());
            }
            let identity = identity()?;
            proof.identity(&identity)?;
            github(&effective, &proof.signature, &identity.login, false)?;
            if cache.len() >= 64 {
                cache.clear();
            }
            cache.insert(key, Instant::now());
        } else {
            let identity = identity()?;
            proof.identity(&identity)?;
            github(&effective, &proof.signature, &identity.login, false)?;
        }
        Ok(())
    };
    if let Err(error) = verified() {
        let overrides: Vec<_> = effective
            .iter()
            .filter(|(key, v)| global.get(*key) != Some(*v))
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        return Err(
            if overrides.is_empty()
                || retry_at(&error).is_some()
                || error.to_string().starts_with("GitHub ")
            {
                error
            } else {
                fail(format!(
                    "{error}. Repository overrides: {}",
                    overrides.join(", ")
                ))
            },
        );
    }
    report(cwd, effective, &global)
}
pub fn setup(cwd: &Path) -> io::Result<Report> {
    let home = home()?;
    // Serialize this user's key generation and Git changes across simultaneous onboarding.
    let state = home.join(".local/share/hey-boss");
    fs::create_dir_all(&state)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(state.join("environment.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let temp = crate::admin::Temporary::new()?;
    let original = read_config(isolated_git(&temp.0))?;
    let mut candidate = original.clone();
    let identity = identity()?;
    if value(&candidate, "user.name").is_empty() {
        candidate.insert("user.name".into(), identity.name.clone());
    }
    if !identity
        .verified
        .iter()
        .any(|email| email.eq_ignore_ascii_case(value(&candidate, "user.email")))
    {
        candidate.insert("user.email".into(), identity.email.clone());
    }
    candidate.insert("commit.gpgsign".into(), "true".into());
    candidate.insert("tag.gpgsign".into(), "true".into());
    let existing = if value(&original, "user.signingkey").is_empty()
        && !original.contains_key("gpg.format")
        && !enabled(&original, "commit.gpgsign")
    {
        Err(fail("No existing signing configuration"))
    } else {
        probe(&candidate, &temp.0)
    };
    let signature = match existing {
        Ok(signature) => signature,
        Err(error)
            if !value(&original, "user.signingkey").is_empty() && format(&original) != "ssh" =>
        {
            return Err(fail(format!(
                "Existing signing configuration was preserved: {error}"
            )));
        }
        Err(_) => {
            candidate.insert("gpg.format".into(), "ssh".into());
            candidate.remove("gpg.ssh.defaultkeycommand");
            let ssh = home.join(".ssh");
            let mut choices = vec![
                ssh.join("github_commit_signing"),
                ssh.join("id_ed25519"),
                ssh.join("id_rsa"),
            ];
            if let Ok(entries) = fs::read_dir(&ssh) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "pub") {
                        choices.push(path);
                    }
                }
            }
            let registered = registered_keys(&identity.login)?;
            let mut choices: Vec<String> = choices
                .into_iter()
                .filter(|key| key.is_file())
                .map(|key| key.to_string_lossy().into_owned())
                .collect();
            if let Ok(keys) = output(
                Command::new("ssh-add").arg("-L"),
                "List SSH agent public keys",
            ) {
                choices.extend(
                    keys.lines()
                        .filter_map(public_key)
                        .map(|key| format!("key::{key}")),
                );
            }
            let mut ranked = Vec::new();
            for key in choices {
                candidate.insert("user.signingkey".into(), key.clone());
                if let Ok(public) = ssh_public(&candidate, &temp.0) {
                    ranked.push((!registered.contains(&public), key));
                }
            }
            // Stable ordering keeps local files ahead of agent-only keys when both work.
            ranked.sort_by_key(|(unregistered, _)| *unregistered);
            let mut usable = None;
            for (_, key) in ranked {
                candidate.insert("user.signingkey".into(), key);
                if let Ok(signature) = probe(&candidate, &temp.0) {
                    usable = Some(signature);
                    break;
                }
            }
            match usable {
                Some(signature) => signature,
                None => {
                    if !ssh.exists() {
                        fs::DirBuilder::new().mode(0o700).create(&ssh)?;
                    }
                    let key = ssh.join("github_commit_signing");
                    if key.exists() || key.with_extension("pub").exists() {
                        return Err(fail(
                            "Existing ~/.ssh/github_commit_signing cannot sign unattended; unlock or repair it. No key was overwritten",
                        ));
                    }
                    output(
                        Command::new("ssh-keygen")
                            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                            .arg(&key),
                        "Generate local SSH signing key",
                    )?;
                    candidate.insert("user.signingkey".into(), key.to_string_lossy().into_owned());
                    probe(&candidate, &temp.0)?
                }
            }
        }
    };
    signature.identity(&identity)?;
    github(&candidate, &signature.signature, &identity.login, true)?;
    for (key, val) in &candidate {
        if original.get(key) != Some(val) {
            output(
                isolated_git(&temp.0).args(["config", "--global", key, val]),
                "Configure Git signing",
            )?;
        }
    }
    // Read again: includes, environment and repository settings can override those writes.
    check(cwd)
}

/// Called inside the worker, so service HOME, PATH and SSH_AUTH_SOCK are tested too.
pub fn check_worker(cwd: &Path) -> io::Result<()> {
    // Named non-Git projects can still run document-only workers.
    let repository = crate::admin::capture(
        git(cwd).env("LC_ALL", "C").args(["rev-parse", "--git-dir"]),
        &[],
    )?;
    if repository["exit_code"] != 0 {
        let error = repository["stderr"].as_str().unwrap_or_default();
        if error.contains("not a git repository") {
            return Ok(());
        }
        return Err(fail(format!(
            "Worker environment repository check failed: {error}"
        )));
    }
    check_inner(cwd, true).map(|_| ()).map_err(|e| {
        if retry_at(&e).is_some() || e.to_string().starts_with("GitHub ") {
            e
        } else {
            fail(format!(
                "Worker environment check failed in {}: {e}",
                cwd.display()
            ))
        }
    })
}
