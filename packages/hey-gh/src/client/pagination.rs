use super::{Client, Error, Freshness, Result, endpoint_class, next_link};
use crate::Response;
use serde_json::Value;
use std::{collections::HashSet, future::Future, pin::Pin, sync::Arc};
use tokio::sync::Notify;
use url::Url;

struct PageRead<'a> {
    future: Pin<Box<dyn Future<Output = Result<Response>> + Send + 'a>>,
    admitted: Arc<Notify>,
}

tokio::task_local! { static PAGE_ADMISSION: Arc<Notify>; }

// Polling the required future first is insufficient: its cache lookup yields
// before queue admission. Start speculation only after that read owns or joins
// a queue slot, so it cannot steal its predecessor's last available permit.
pub(super) fn admitted() {
    let _ = PAGE_ADMISSION.try_with(|ready| ready.notify_one());
}

struct Prefetch<'a> {
    url: String,
    read: PageRead<'a>,
}

impl Client {
    fn page_read<'a>(
        &'a self,
        url: String,
        freshness: Freshness,
        completed_version: Option<(&'a str, bool)>,
    ) -> PageRead<'a> {
        let admitted = Arc::new(Notify::new());
        let future = PAGE_ADMISSION.scope(admitted.clone(), async move {
            if let Some((version, allow_empty)) = completed_version {
                self.completed_job_page(&url, version, allow_empty, freshness)
                    .await
            } else {
                // Cache the response normally, but record validation only once
                // the preceding page actually includes it in this collection.
                self.request(url, None, freshness).await
            }
        });
        PageRead {
            future: Box::pin(future),
            admitted,
        }
    }

    pub(super) async fn collect_pages(
        &self,
        path: &str,
        field: Option<&str>,
        freshness: Freshness,
        completed_version: Option<(&str, bool)>,
    ) -> Result<Vec<Value>> {
        let first = self.rest_url(path)?.to_string();
        let mut path = first.clone();
        let mut seen = HashSet::new();
        let mut values = Vec::new();
        let mut bytes = 0usize;
        let mut pending: Option<Prefetch<'_>> = None;
        let detail_prefetch = completed_version.is_none()
            && field.is_none()
            && !matches!(freshness, Freshness::CachedOnly)
            && self.0.config.queue_capacity >= 32
            && matches!(
                endpoint_class(&first, false, &self.0.config.rest_url),
                "comments" | "review_comments" | "reviews" | "timeline"
            );
        // A previous first-page link is only a scheduling hint. Revalidate its
        // next page alongside the first, then consume it only if the returned
        // link confirms it. The same two-page window and admission order apply.
        let mut lookahead = if detail_prefetch
            && self.0.config.max_collection_bytes >= self.0.config.max_body_bytes.saturating_mul(2)
        {
            match self.peek_get(&first).await {
                Ok(cached) => cached
                    .link
                    .as_deref()
                    .and_then(|links| self.cached_detail_next(&first, links)),
                Err(Error::CacheMiss) => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        for index in 0..1000 {
            if !seen.insert(path.clone()) {
                return Err(Error::Invalid("pagination link cycle".into()));
            }
            let (mut current, prefetched) = match pending.take() {
                Some(pending) if pending.url == path => (pending.read, true),
                _ => (
                    self.page_read(path.clone(), freshness, completed_version),
                    false,
                ),
            };
            let mut ahead = lookahead.take().map(|url| Prefetch {
                read: self.page_read(url.clone(), freshness, None),
                url,
            });
            let mut response = if let Some(ahead) = &mut ahead {
                tokio::select! {
                    biased;
                    response = &mut current.future => response,
                    response = async {
                        current.admitted.notified().await;
                        (&mut ahead.read.future).await
                    } => {
                        ahead.read.future = Box::pin(async move { response });
                        current.future.await
                    }
                }
            } else {
                current.future.await
            };
            // Speculation can lose admission to its predecessor. Once this
            // page is required, retry normal admission after that page finishes.
            if prefetched && matches!(response, Err(Error::QueueFull)) {
                response = self
                    .page_read(path.clone(), freshness, completed_version)
                    .future
                    .await;
            }
            let response = response?;
            if completed_version.is_none() {
                crate::report::record_validation(&path, &response);
            }
            bytes = bytes.saturating_add(response.data.to_string().len());
            if bytes > self.0.config.max_collection_bytes {
                return Err(Error::Invalid(
                    "pagination exceeds configured collection byte limit".into(),
                ));
            }
            let page = field
                .map_or(&response.data, |field| &response.data[field])
                .as_array()
                .ok_or_else(|| Error::Invalid("expected a paginated GitHub array".into()))?;
            values.extend(page.iter().cloned());
            if values.len() > 100_000 {
                return Err(Error::Invalid("pagination exceeds 100,000 items".into()));
            }
            let Some(next) = response.link.as_deref().and_then(next_link) else {
                return Ok(values);
            };
            path = self.pagination_path(&first, &next)?;
            // A changed chain can discard a speculative result, including its
            // error. Only the newly returned next link authorizes consumption.
            pending = ahead.filter(|ahead| ahead.url == path);
            // Keep at most two page bodies in flight/buffered, within the same
            // collection byte bound. Tiny queues and immutable job retention
            // keep their existing sequential admission/cancellation behavior.
            if detail_prefetch
                && index + 2 < 1000
                && self.0.config.max_collection_bytes.saturating_sub(bytes)
                    >= self.0.config.max_body_bytes.saturating_mul(2)
                && values.len() <= 100_000 - 200
            {
                lookahead =
                    self.detail_page_ahead(&first, &path, response.link.as_deref().unwrap());
            }
        }
        Err(Error::Invalid("pagination exceeds 1000 pages".into()))
    }

    fn cached_detail_next(&self, first: &str, links: &str) -> Option<String> {
        let mut current = self.rest_url(first).ok()?;
        // Only the ordinary first page can use this cached-chain hint.
        if current.query_pairs().any(|(key, _)| key == "page") {
            return None;
        }
        current.query_pairs_mut().append_pair("page", "1");
        let predicted = self.detail_page_ahead(first, current.as_str(), links)?;
        let next = self.pagination_path(first, &next_link(links)?).ok()?;
        (next == predicted).then_some(next)
    }

    // Offset lookahead is limited to familiar personal detail endpoints. A
    // cursor, changed filter, or untrusted last link simply keeps serial reads.
    pub(super) fn detail_page_ahead(&self, first: &str, next: &str, links: &str) -> Option<String> {
        if !matches!(
            endpoint_class(first, false, &self.0.config.rest_url),
            "comments" | "review_comments" | "reviews" | "timeline"
        ) {
            return None;
        }
        let last = links.split(',').find_map(|part| {
            let (target, params) = part.trim().split_once('>')?;
            params
                .split(';')
                .any(|p| p.trim() == "rel=\"last\"")
                .then(|| target.strip_prefix('<'))
                .flatten()
        })?;
        let first = self.rest_url(first).ok()?;
        let mut next = self.rest_url(next).ok()?;
        let last = Url::parse(&self.pagination_path(first.as_str(), last).ok()?).ok()?;
        let filters = |url: &Url| {
            let mut pairs: Vec<_> = url
                .query_pairs()
                .filter(|(k, _)| k != "page")
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            pairs.sort();
            pairs
        };
        if next.path() != first.path()
            || last.path() != first.path()
            || filters(&first) != filters(&next)
            || filters(&first) != filters(&last)
            || next
                .query_pairs()
                .any(|(k, _)| matches!(k.as_ref(), "before" | "after"))
        {
            return None;
        }
        let number = |url: &Url, key: &str| {
            let mut values = url.query_pairs().filter(|(k, _)| k == key);
            let value = values.next()?.1.parse::<u64>().ok()?;
            (value > 0 && values.next().is_none()).then_some(value)
        };
        if number(&next, "per_page")? > 100 {
            return None;
        }
        let page = number(&next, "page")?.checked_add(1)?;
        if page > number(&last, "page")? {
            return None;
        }
        let pairs: Vec<_> = next
            .query_pairs()
            .map(|(k, v)| {
                let value = if k == "page" {
                    page.to_string()
                } else {
                    v.into_owned()
                };
                (k.into_owned(), value)
            })
            .collect();
        next.query_pairs_mut().clear().extend_pairs(pairs);
        Some(next.to_string())
    }
}
