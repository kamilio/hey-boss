use super::{CommitList, ListEvidence, late, recent};
use crate::{Client, Error, Freshness, Response, Result, now_ms};
use serde_json::Value;
use std::{collections::BTreeSet, future::Future, sync::Arc};
use tokio::sync::OnceCell;

struct Collection {
    repository: String,
    head: String,
    merge: Option<String>,
    wait_for_selectors: bool,
    response: OnceCell<Result<Option<Response>>>,
}

tokio::task_local! { static COLLECTION: Arc<Collection>; }

pub(in crate::report) async fn scope<T>(
    repository: &str,
    head: &str,
    merge: Option<&str>,
    wait_for_selectors: bool,
    read: impl Future<Output = T>,
) -> T {
    COLLECTION
        .scope(
            Arc::new(Collection {
                repository: repository.to_owned(),
                head: head.to_owned(),
                merge: merge.map(str::to_owned),
                wait_for_selectors,
                response: OnceCell::new(),
            }),
            read,
        )
        .await
}

fn list_empty(commit: &Value, sha: &str, list: CommitList) -> Option<bool> {
    if commit["__typename"] != "Commit" || commit["oid"] != sha {
        return None;
    }
    match list {
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
                || (collection.wait_for_selectors && late::metadata_pending())
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
            collection
                .response
                .get_or_init(|| async {
                    match crate::client::optional_selector_read(client.commit_lists_response(
                        repository,
                        &collection.head,
                        collection.merge.as_deref(),
                        freshness,
                    ))
                    .await
                    {
                        Ok(response) => Ok(Some(response)),
                        Err(
                            error @ (Error::Auth(_)
                            | Error::LocalAuth(_)
                            | Error::Storage(_)
                            | Error::Stopped
                            | Error::GraphQL {
                                access_denied: true,
                                ..
                            }
                            | Error::GitHub {
                                status: 401 | 403, ..
                            }),
                        ) => Err(error),
                        _ => Ok(None),
                    }
                })
                .await
                .clone()?
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
    let Some(empty) = list_empty(
        &repo[if collection.head == sha {
            "head"
        } else {
            "merge"
        }],
        sha,
        list,
    ) else {
        return Ok(None);
    };
    Ok(Some(ListEvidence {
        empty,
        at: response.validated_at_ms,
        resource: format!(
            "graphql://{}/{repository}#commit-lists:{}:{sha}",
            client.hostname(),
            list.label()
        ),
        versions: None,
    }))
}
