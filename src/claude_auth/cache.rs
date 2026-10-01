//! A bounded token cache with background validation of the proxy-owned files.
use super::*;
use std::sync::Mutex as SyncMutex;

const REFRESH_AFTER: Duration = Duration::from_millis(250);
const MAX_AGE: Duration = Duration::from_secs(2);

struct Entry {
    path: PathBuf,
    checked: Instant,
    tokens: std::result::Result<Arc<Tokens>, String>,
    checking: bool,
}
#[derive(Default)]
struct State {
    version: u64,
    entry: Option<Entry>,
}
#[derive(Default)]
pub(super) struct Cache(SyncMutex<State>);
impl Cache {
    pub fn store(&self, path: PathBuf, tokens: &Tokens) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.version = state.version.wrapping_add(1);
        state.entry = Some(Entry {
            path,
            checked: Instant::now(),
            tokens: Ok(Arc::new(tokens.clone())),
            checking: false,
        });
    }
    pub fn failure(&self, path: PathBuf, error: &anyhow::Error) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.version = state.version.wrapping_add(1);
        state.entry = Some(Entry {
            path,
            checked: Instant::now(),
            tokens: Err(error.to_string()),
            checking: false,
        });
    }
    pub fn get(self: &Arc<Self>, path: &Path, rejected: Option<&str>) -> Option<Result<String>> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let version = state.version;
        let entry = state.entry.as_mut().filter(|entry| entry.path == path)?;
        if rejected.is_some_and(|old| {
            entry
                .tokens
                .as_ref()
                .is_ok_and(|tokens| tokens.access_token == old)
        }) {
            // An explicit rejection invalidates the cached token immediately.
            // A pending file check may not restore it after the refresh publishes.
            state.version = state.version.wrapping_add(1);
            state.entry = None;
            return None;
        }
        // After idle time, revalidate through the async single-flight cold path.
        // A normally idle session must not fail merely because no request was
        // around to schedule background validation.
        if entry.checked.elapsed() >= MAX_AGE {
            return None;
        }
        if entry.checked.elapsed() >= REFRESH_AFTER && !entry.checking {
            entry.checking = true;
            let cache = Arc::downgrade(self);
            let path = path.to_owned();
            tokio::task::spawn_blocking(move || {
                let tokens = load(&path).map(Arc::new).map_err(|error| error.to_string());
                if let Some(cache) = cache.upgrade() {
                    cache.checked(version, path, tokens);
                }
            });
        }
        match &entry.tokens {
            Ok(tokens) if tokens.expires_at > now().saturating_add(60) => {
                Some(Ok(tokens.access_token.clone()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(anyhow::anyhow!(error.clone()))),
        }
    }
    fn checked(
        &self,
        version: u64,
        path: PathBuf,
        tokens: std::result::Result<Arc<Tokens>, String>,
    ) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.version != version || state.entry.as_ref().is_none_or(|entry| entry.path != path) {
            return;
        }
        state.version = state.version.wrapping_add(1);
        state.entry = Some(Entry {
            path,
            checked: Instant::now(),
            tokens,
            checking: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hot_tokens_do_not_read_files_and_background_validation_detects_removal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude.json");
        super::super::tests::fixture(&path, now() + 3600);
        let tokens = load(&path).unwrap();
        let cache = Arc::new(Cache::default());
        cache.store(path.clone(), &tokens);
        std::fs::remove_file(&path).unwrap();
        for _ in 0..100 {
            assert_eq!(
                cache.get(&path, None).unwrap().unwrap(),
                tokens.access_token
            );
        }
        cache.0.lock().unwrap().entry.as_mut().unwrap().checked = Instant::now() - REFRESH_AFTER;
        assert!(cache.get(&path, None).unwrap().is_ok());
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if cache.get(&path, None).unwrap().is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn a_late_file_check_cannot_replace_a_rotated_token() {
        let cache = Cache::default();
        let path = PathBuf::from("synthetic.json");
        let old = Tokens {
            access_token: "sk-ant-oat01-old".into(),
            refresh_token: "old".into(),
            expires_at: now() + 3600,
        };
        cache.store(path.clone(), &old);
        let version = cache.0.lock().unwrap().version;
        let fresh = Tokens {
            access_token: "sk-ant-oat01-new".into(),
            ..old.clone()
        };
        cache.store(path.clone(), &fresh);
        cache.checked(version, path, Ok(Arc::new(old)));
        assert_eq!(
            cache
                .0
                .lock()
                .unwrap()
                .entry
                .as_ref()
                .unwrap()
                .tokens
                .as_ref()
                .unwrap()
                .access_token,
            fresh.access_token
        );
    }

    #[test]
    fn idle_and_explicitly_rejected_tokens_require_revalidation() {
        let cache = Arc::new(Cache::default());
        let path = PathBuf::from("synthetic.json");
        let tokens = Tokens {
            access_token: "sk-ant-oat01-synthetic".into(),
            refresh_token: "synthetic".into(),
            expires_at: now() + 3600,
        };
        cache.store(path.clone(), &tokens);
        cache.0.lock().unwrap().entry.as_mut().unwrap().checked = Instant::now() - MAX_AGE;
        assert!(cache.get(&path, None).is_none());
        cache.store(path.clone(), &tokens);
        let version = cache.0.lock().unwrap().version;
        assert!(cache.get(&path, Some(&tokens.access_token)).is_none());
        cache.checked(version, path.clone(), Ok(Arc::new(tokens)));
        assert!(cache.get(&path, None).is_none());
    }

    #[tokio::test]
    async fn idle_request_reloads_valid_credentials_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude.json");
        super::super::tests::fixture(&path, now() + 3600);
        let manager = TokenManager::default();
        let client = reqwest::Client::new();
        let first = manager.token(&path, &client).await.unwrap();
        manager
            .cache
            .0
            .lock()
            .unwrap()
            .entry
            .as_mut()
            .unwrap()
            .checked = Instant::now() - MAX_AGE;
        let calls = (0..16).map(|_| manager.token(&path, &client));
        for token in futures_util::future::join_all(calls).await {
            assert_eq!(token.unwrap(), first);
        }
    }
}
