//! Share the large roster lookup, but reread the small current page each time.
use crate::{Client, Error};
use serde_json::Value;
use std::{future::Future, sync::Arc};
use tokio::sync::OnceCell;

#[derive(Clone)]
struct Seed {
    node: Value,
    validated_at: u64,
    after: Option<Value>,
}

struct Lookup {
    repository: String,
    number: u64,
    seed: OnceCell<Option<Seed>>,
}

tokio::task_local! { static LOOKUP: Arc<Lookup>; }

pub(crate) async fn scope<T>(repository: &str, number: u64, future: impl Future<Output = T>) -> T {
    LOOKUP
        .scope(
            Arc::new(Lookup {
                repository: repository.to_owned(),
                number,
                seed: OnceCell::new(),
            }),
            future,
        )
        .await
}

fn same_pr(node: &Value, repository: &str, number: u64) -> bool {
    node["number"] == number
        && node["repository"]["nameWithOwner"]
            .as_str()
            .is_some_and(|repo| repo.eq_ignore_ascii_case(repository))
}

async fn seed(client: &Client, repository: &str, number: u64) -> Option<Seed> {
    let scan = match client.peek_derived(crate::dashboard::DISCOVERY_CACHE).await {
        Ok(Some(scan)) => scan,
        Ok(None) => return None,
        Err(error) => {
            tracing::warn!(
                error_code = error.diagnostic_code(),
                "CI discovery memo unavailable; retaining independent reads"
            );
            return None;
        }
    };
    let mut matches = scan.data["pulls"]
        .as_array()?
        .iter()
        .filter(|node| same_pr(node, repository, number));
    let node = matches.next()?.clone();
    if matches.next().is_some() {
        return None;
    }
    let key = format!("{repository}/{number}");
    let validated_at = scan.data["validatedAtByPr"]
        .as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(&key))?
        .1
        .as_u64()?;
    let after = scan.data["pageAfterByPr"]
        .as_object()
        .and_then(|pages| {
            pages
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(&key))
                .map(|(_, value)| value.clone())
        })
        .filter(|after| after.is_null() || after.as_str().is_some_and(|s| !s.is_empty()));
    Some(Seed {
        node,
        validated_at,
        after,
    })
}

pub(super) async fn read(client: &Client, repository: &str, number: u64) -> Option<(Value, u64)> {
    let lookup = LOOKUP.try_with(Arc::clone).ok().filter(|lookup| {
        lookup.number == number && lookup.repository.eq_ignore_ascii_case(repository)
    });
    let seed = if let Some(lookup) = lookup {
        lookup
            .seed
            .get_or_init(|| seed(client, repository, number))
            .await
            .clone()
    } else {
        seed(client, repository, number).await
    }?;
    let Seed {
        mut node,
        mut validated_at,
        after,
    } = seed;
    if let Some(after) = after {
        match client.cached_discovery_page(after).await {
            Ok(page) if page.validated_at_ms > validated_at => {
                // Page membership can move. Require exactly one matching PR;
                // a partial scan never replaces the authoritative roster.
                let mut matches = page.data["data"]["viewer"]["pullRequests"]["nodes"]
                    .as_array()?
                    .iter()
                    .filter(|node| same_pr(node, repository, number));
                node = matches.next()?.clone();
                if matches.next().is_some() {
                    return None;
                }
                validated_at = page.validated_at_ms;
            }
            Ok(_) | Err(Error::CacheMiss) => {}
            Err(error) => {
                tracing::warn!(
                    error_code = error.diagnostic_code(),
                    "CI discovery page unavailable; retaining independent reads"
                );
                return None;
            }
        }
    }
    Some((node, validated_at))
}
