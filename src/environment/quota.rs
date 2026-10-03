//! Per-credential GitHub preflight gate, shared by threads and worker processes.
use super::{fail, home};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Write},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Quota {
    pub retry_at: i64,
    failures: u32,
}
impl std::fmt::Display for Quota {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let date = UNIX_EPOCH + Duration::from_millis(self.retry_at.max(0) as u64);
        write!(
            f,
            "GitHub quota exhausted. Retry at {}. Automatic pickup will resume after the cooldown; saved work is retained.",
            httpdate::fmt_http_date(date)
        )
    }
}
impl std::error::Error for Quota {}

// Hash credential material only in memory. Neither tokens nor account config
// contents are stored in diagnostics or the cooldown receipt.
pub(super) fn scope() -> io::Result<String> {
    let mut hash = Sha256::new();
    hash.update(b"github.com\0");
    hash.update(unsafe { libc::geteuid() }.to_le_bytes());
    hash.update(home()?.as_os_str().as_encoded_bytes());
    let config = std::env::var_os("GH_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME").map(|p| std::path::PathBuf::from(p).join("gh"))
        })
        .unwrap_or(home()?.join(".config/gh"));
    if let Some(token) = std::env::var_os("GH_TOKEN")
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var_os("GITHUB_TOKEN").filter(|s| !s.is_empty()))
    {
        hash.update(token.as_encoded_bytes());
    } else {
        hash.update(config.as_os_str().as_encoded_bytes());
        match fs::read(config.join("hosts.yml")) {
            Ok(bytes) => hash.update(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) struct Gate {
    _lock: fs::File,
    path: PathBuf,
    previous: Option<Quota>,
}
impl Gate {
    pub fn open(scope: &str) -> io::Result<Self> {
        let directory = home()?.join(".local/share/hey-boss/environment-quota");
        fs::create_dir_all(&directory)?;
        Self::at(&directory.join(format!("{scope}.json")))
    }
    fn at(path: &Path) -> io::Result<Self> {
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path.with_extension("lock"))?;
        // Never keep a reserved worker slot waiting behind another network
        // request indefinitely. admin::capture bounds the lock owner's request.
        let started = std::time::Instant::now();
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(error);
            }
            if started.elapsed() >= Duration::from_secs(35) {
                return Err(fail("GitHub preflight is busy; retry automatically"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let previous: Option<Quota> = if text.is_empty() {
            None
        } else {
            Some(serde_json::from_str(&text).map_err(io::Error::other)?)
        };
        if let Some(quota) = &previous
            && quota.retry_at > now()
        {
            return Err(io::Error::other(quota.clone()));
        }
        Ok(Self {
            _lock: file,
            path: path.to_owned(),
            previous,
        })
    }
    pub fn exhausted(
        &mut self,
        headers: &std::collections::BTreeMap<String, String>,
    ) -> io::Result<io::Error> {
        let failures = self
            .previous
            .as_ref()
            .map_or(1, |q| q.failures.saturating_add(1));
        let quota = Quota {
            retry_at: deadline(headers, now(), failures),
            failures,
        };
        let temporary = self.path.with_extension("tmp");
        let result = (|| -> io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec(&quota)?)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            fs::File::open(self.path.parent().unwrap())?.sync_all()
        })();
        let _ = fs::remove_file(temporary);
        result?;
        Ok(io::Error::other(quota))
    }
    pub fn succeeded(&mut self) -> io::Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => fs::File::open(self.path.parent().unwrap())?.sync_all()?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        self.previous = None;
        Ok(())
    }
}

fn deadline(headers: &std::collections::BTreeMap<String, String>, now: i64, failures: u32) -> i64 {
    let reset = headers
        .get("x-ratelimit-reset")
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(|s| s.checked_mul(1000))
        // Reset headers have whole-second precision. A probe in that second
        // can still see an exhausted bucket; don't mistake it for missing
        // metadata and impose the accumulated five-minute fallback.
        .and_then(|reset| {
            if headers
                .get("x-ratelimit-remaining")
                .is_some_and(|s| s == "0")
            {
                reset.checked_add(1000)
            } else {
                Some(reset)
            }
        });
    let retry = headers.get("retry-after").and_then(|s| {
        s.parse::<i64>()
            .ok()
            .filter(|s| *s >= 0)
            .and_then(|s| s.checked_mul(1000))
            .and_then(|s| now.checked_add(s))
            .or_else(|| {
                httpdate::parse_http_date(s)
                    .ok()?
                    .duration_since(UNIX_EPOCH)
                    .ok()?
                    .as_millis()
                    .try_into()
                    .ok()
            })
    });
    // Never shorten valid server metadata. Missing, stale or malformed hints
    // get a bounded exponential delay, preventing immediate retry loops.
    reset
        .into_iter()
        .chain(retry)
        .filter(|t| *t > now && *t < 253_402_300_800_000)
        .max()
        .unwrap_or_else(|| {
            now.saturating_add((60_000_i64 * (1 << failures.saturating_sub(1).min(3))).min(300_000))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quota_deadlines_use_the_failing_response_and_bound_fallback() {
        let mut h = std::collections::BTreeMap::new();
        h.insert("x-ratelimit-reset".into(), "200".into());
        h.insert("retry-after".into(), "120".into());
        assert_eq!(deadline(&h, 100_000, 1), 220_000);
        h.insert("retry-after".into(), "Thu, 01 Jan 1970 00:05:00 GMT".into());
        assert_eq!(deadline(&h, 100_000, 1), 300_000);
        h.insert("retry-after".into(), "invalid".into());
        assert_eq!(deadline(&h, 100_000, 1), 200_000);
        for n in [1, 2, 3, 10, u32::MAX] {
            assert!((60_000..=300_000).contains(&(deadline(&h, 300_000, n) - 300_000)));
        }
    }

    #[test]
    fn reset_second_does_not_escalate_to_five_minutes() {
        let h = std::collections::BTreeMap::from([
            ("x-ratelimit-reset".into(), "200".into()),
            ("x-ratelimit-remaining".into(), "0".into()),
        ]);
        assert_eq!(deadline(&h, 200_360, 6), 201_000);
        let mut retry = h.clone();
        retry.insert("retry-after".into(), "120".into());
        assert_eq!(deadline(&retry, 200_360, 6), 320_360);
    }

    #[test]
    fn successful_probe_clears_failure_history_under_the_gate_lock() {
        let temporary = crate::admin::Temporary::new().unwrap();
        let path = temporary.0.join("quota.json");
        fs::write(&path, br#"{"retry_at":1,"failures":6}"#).unwrap();
        let mut gate = Gate::at(&path).unwrap();
        gate.succeeded().unwrap();
        assert!(!path.exists());
        let error = gate.exhausted(&Default::default()).unwrap();
        let quota = error.get_ref().unwrap().downcast_ref::<Quota>().unwrap();
        assert_eq!(quota.failures, 1);
        assert!((59_000..=60_000).contains(&(quota.retry_at - now())));
    }
}
