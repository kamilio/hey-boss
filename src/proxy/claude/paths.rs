//! Resolve the configured credential path once without filesystem work on hot requests.
use super::*;

struct Entry {
    source: Option<PathBuf>,
    configured: Option<PathBuf>,
    resolved: PathBuf,
}

#[derive(Default)]
pub(super) struct Cache(tokio::sync::Mutex<Option<Entry>>);
impl Cache {
    pub async fn resolve(
        &self,
        provider: &ProviderConfig,
        source: Option<&Path>,
    ) -> Result<PathBuf> {
        let mut cache = self.0.lock().await;
        if let Some(entry) = cache.as_ref()
            && entry.source.as_deref() == source
            && entry.configured == provider.credentials_file
        {
            return Ok(entry.resolved.clone());
        }
        let source = source.map(Path::to_owned);
        let (owned_provider, owned_source) = (provider.clone(), source.clone());
        let resolved = tokio::task::spawn_blocking(move || {
            owned_provider.credentials_path(owned_source.as_deref())
        })
        .await
        .context("Claude credential path resolution failed")??;
        *cache = Some(Entry {
            source,
            configured: provider.credentials_file.clone(),
            resolved: resolved.clone(),
        });
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn configuration_changes_reselect_paths_and_cannot_reuse_a_previous_valid_path() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("config.json");
        let alternate = dir.path().join("alternate.json");
        let cache = Cache::default();
        let mut provider = ProviderConfig::default();
        assert_eq!(
            cache.resolve(&provider, Some(&source)).await.unwrap(),
            source.with_extension("claude.json")
        );
        provider.credentials_file = Some("alternate.json".into());
        assert_eq!(
            cache.resolve(&provider, Some(&source)).await.unwrap(),
            alternate
        );
        provider.credentials_file = Some(source.clone());
        assert!(cache.resolve(&provider, Some(&source)).await.is_err());
        provider.credentials_file = None;
        assert_eq!(
            cache.resolve(&provider, Some(&alternate)).await.unwrap(),
            alternate.with_extension("claude.json")
        );
    }
}
