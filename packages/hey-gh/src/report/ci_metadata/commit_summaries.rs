use super::{
    CommitList, ListEvidence, ListVersions, VersionEvidence, late, recent, workflow_versions,
};
use crate::{Client, Error, Freshness, Response, Result, now_ms};
use serde_json::Value;
use std::{collections::BTreeSet, future::Future, sync::Arc, time::Duration};
use tokio::sync::{Notify, OnceCell, watch};

struct Collection {
    repository: String,
    head: String,
    merge: Option<String>,
    closed: bool,
    wait_for_selectors: bool,
    response: OnceCell<Result<Option<Response>>>,
    requested: OnceCell<(Freshness, tokio::time::Instant)>,
    wake: Notify,
    ready: watch::Sender<bool>,
    admission: watch::Sender<bool>,
}

tokio::task_local! { static COLLECTION: Arc<Collection>; }

pub(in crate::report) async fn scope<T>(
    client: &Client,
    repository: &str,
    head: &str,
    merge: Option<&str>,
    closed: bool,
    wait_for_selectors: bool,
    read: impl Future<Output = T>,
) -> T {
    let collection = Arc::new(Collection {
        repository: repository.to_owned(),
        head: head.to_owned(),
        merge: merge.map(str::to_owned),
        closed,
        wait_for_selectors,
        response: OnceCell::new(),
        requested: OnceCell::new(),
        wake: Notify::new(),
        ready: watch::channel(false).0,
        admission: watch::channel(false).0,
    });
    COLLECTION
        .scope(collection.clone(), async {
            let fetch = async {
                collection.wake.notified().await;
                let response = crate::client::CI_SOURCE_ADMISSION
                    .scope(
                        collection.admission.clone(),
                        crate::client::retained_selector_read(client.commit_lists_response(
                            repository,
                            head,
                            merge,
                            collection.requested.get().expect("requested proof").0,
                        )),
                    )
                    .await;
                let response = match response {
                    Ok(response) => Ok(Some(response)),
                    Err(error) if required_error(&error) => Err(error),
                    _ => Ok(None),
                };
                let _ = collection.response.set(response);
                collection.ready.send_replace(true);
            };
            tokio::pin!(read);
            // Keep the query in this collection's task-local identity, generation,
            // priority and deadline scopes. Completion/cancellation drops it; no
            // detached task can spend quota after its consumer has finished.
            tokio::select! {
                biased;
                result = &mut read => result,
                _ = fetch => read.await,
            }
        })
        .await
}

pub(in crate::report) fn ready() -> Option<watch::Receiver<bool>> {
    COLLECTION.try_with(|c| c.ready.subscribe()).ok()
}

pub(super) fn required_error(error: &Error) -> bool {
    matches!(
        error,
        Error::Auth(_)
            | Error::LocalAuth(_)
            | Error::Storage(_)
            | Error::Stopped
            | Error::GraphQL {
                access_denied: true,
                ..
            }
            | Error::GitHub {
                status: 401 | 403,
                ..
            }
    )
}

fn list_empty(commit: &Value, sha: &str, list: CommitList) -> Option<bool> {
    if commit["__typename"] != "Commit" || commit["oid"] != sha {
        return None;
    }
    match list {
        CommitList::Workflows => None,
        CommitList::Statuses => match commit.get("status")? {
            Value::Null => Some(true),
            status if status["id"].as_str().is_some_and(|id| !id.is_empty()) => Some(false),
            _ => None,
        },
        CommitList::Checks => {
            let suites = &commit["checkSuites"];
            let nodes = suites["nodes"].as_array()?;
            if suites["pageInfo"]["hasNextPage"] != false
                || suites["totalCount"].as_u64()? != nodes.len() as u64
                || nodes.len() > 24
            {
                return None;
            }
            let mut ids = BTreeSet::new();
            let mut empty = true;
            for suite in nodes {
                let id = suite["id"].as_str().filter(|id| !id.is_empty())?;
                if !ids.insert(id) {
                    return None;
                }
                empty &= suite["checkRuns"]["totalCount"].as_u64()? == 0;
            }
            Some(empty)
        }
    }
}

pub(super) async fn evidence(
    client: &Client,
    repository: &str,
    sha: &str,
    path: &str,
    list: CommitList,
    freshness: Freshness,
) -> Result<Option<ListEvidence>> {
    let Some(collection) = COLLECTION.try_with(Arc::clone).ok().filter(|c| {
        c.repository == repository && (c.head == sha || c.merge.as_deref() == Some(sha))
    }) else {
        return Ok(None);
    };
    if !collection.closed && !matches!(list, CommitList::Workflows) {
        return Ok(None);
    }
    let cached = client
        .commit_lists_response(
            repository,
            &collection.head,
            collection.merge.as_deref(),
            Freshness::CachedOnly,
        )
        .await;
    let response = match cached {
        Ok(response)
            if matches!(freshness, Freshness::CachedOnly)
                || matches!(freshness, Freshness::MaxAge(age) if recent(&response, age)) =>
        {
            Some(response)
        }
        Err(error) if !matches!(error, Error::CacheMiss) => return Err(error),
        _ => {
            // Give an already-running selector proof the first opportunity.
            // Merged/ambiguous PR metadata cannot supply one, so immutable
            // commit summaries need not queue behind that unrelated REST read.
            if matches!(freshness, Freshness::CachedOnly)
                || (collection.wait_for_selectors
                    && !matches!(list, CommitList::Workflows)
                    && late::metadata_pending())
            {
                return Ok(None);
            }
            // Do not spend a query when REST already satisfies this read.
            match client.peek_get(path).await {
                Ok(rest) if matches!(freshness, Freshness::MaxAge(age) if recent(&rest, age)) => {
                    return Ok(None);
                }
                Err(error) if !matches!(error, Error::CacheMiss) => return Err(error),
                _ => {}
            }
            if collection
                .requested
                .set((
                    freshness,
                    tokio::time::Instant::now() + Duration::from_secs(2),
                ))
                .is_ok()
            {
                collection.wake.notify_one();
            }
            let mut ready = collection.ready.subscribe();
            // Start the existing REST fallback promptly, but retain the one
            // shared proof while that fallback is pending. Late consumers use
            // its own validation clock, never the old REST payload's clock.
            // All list consumers share one fallback clock; later REST waves
            // must not wait another two seconds for the same pending query.
            let fallback_at = collection.requested.get().expect("requested proof").1;
            let mut admission = collection.admission.subscribe();
            let _ = tokio::time::timeout_at(fallback_at, async {
                let admitted = tokio::select! {
                    biased;
                    _ = ready.wait_for(|done| *done) => false,
                    _ = admission.wait_for(|admitted| *admitted) => true,
                };
                if admitted {
                    // This source joined the collection's shared proof.
                    // Let siblings prepare once that proof owns a queue
                    // slot, without waiting for its HTTP response/fallback.
                    crate::client::ci_source_admitted();
                    let _ = ready.wait_for(|done| *done).await;
                }
            })
            .await;
            collection.response.get().cloned().unwrap_or(Ok(None))?
        }
    };
    let Some(response) = response else {
        return Ok(None);
    };
    if response.validated_at_ms == 0
        || response.validated_at_ms > now_ms()
        || matches!(freshness, Freshness::MaxAge(age) if !recent(&response, age))
    {
        return Ok(None);
    }
    let repo = &response.data["data"]["repository"];
    if !repo["id"].as_str().is_some_and(|id| !id.is_empty())
        || !repo["nameWithOwner"]
            .as_str()
            .is_some_and(|r| r.eq_ignore_ascii_case(repository))
    {
        return Ok(None);
    }
    let commit = &repo[if collection.head == sha {
        "head"
    } else {
        "merge"
    }];
    let (empty, versions) = if matches!(list, CommitList::Workflows) {
        let Some(versions) = workflow_versions::Versions::from_commit(commit, sha) else {
            return Ok(None);
        };
        (versions.is_empty(), Some(ListVersions::Workflows(versions)))
    } else {
        let Some(empty) = list_empty(commit, sha, list) else {
            return Ok(None);
        };
        (empty, None)
    };
    let resource = format!(
        "graphql://{}/{repository}#commit-lists:{}:{sha}",
        client.hostname(),
        list.label()
    );
    let versions = versions.map(|versions| VersionEvidence {
        versions,
        at: response.validated_at_ms,
        resource: resource.clone(),
    });
    Ok(Some(ListEvidence {
        empty,
        at: response.validated_at_ms,
        resource,
        versions,
    }))
}
