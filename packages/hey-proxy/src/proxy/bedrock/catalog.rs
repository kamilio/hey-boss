use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

const TTL: Duration = Duration::from_secs(6 * 3600);

#[derive(Serialize, Deserialize)]
struct Cache {
    provider: Provider,
    updated: SystemTime,
    list: Value,
}

pub(in crate::proxy) fn start(service: &Arc<Service>) {
    let Some(path) = service
        .source
        .as_ref()
        .map(|p| p.with_extension("bedrock-models.json"))
    else {
        return;
    };
    let weak = Arc::downgrade(service);
    tokio::spawn(async move {
        let mut loaded = None;
        let mut refreshed = None;
        loop {
            let Some(service) = weak.upgrade() else { break };
            let proxy = service.snapshot();
            if let Some(provider) = &proxy.config.bedrock {
                if loaded.as_ref() != Some(provider) {
                    loaded = Some(provider.clone());
                    refreshed = None;
                    let bytes = tokio::fs::read(&path).await.unwrap_or_default();
                    if let Ok(cache) = serde_json::from_slice::<Cache>(&bytes)
                        && cache.provider == *provider
                    {
                        publish(&service, provider, &cache.list);
                        refreshed = Some(cache.updated);
                    }
                }
                if !refreshed.is_some_and(|t| t.elapsed().is_ok_and(|age| age < TTL)) {
                    match tokio::time::timeout(Duration::from_secs(45), fetch(&proxy, provider))
                        .await
                    {
                        Ok(Ok(list)) => {
                            publish(&service, provider, &list);
                            let updated = SystemTime::now();
                            refreshed = Some(updated);
                            let cache = Cache {
                                provider: provider.clone(),
                                updated,
                                list,
                            };
                            let path = path.clone();
                            let saved = tokio::task::spawn_blocking(move || -> Result<()> {
                                let mut f = tempfile::NamedTempFile::new_in(
                                    path.parent().context("Missing cache directory")?,
                                )?;
                                serde_json::to_writer(&mut f, &cache)?;
                                f.flush()?;
                                f.persist(path)?;
                                Ok(())
                            })
                            .await;
                            if !matches!(saved, Ok(Ok(()))) {
                                eprintln!("Could not save Bedrock model cache");
                            }
                        }
                        _ => eprintln!(
                            "Bedrock profile refresh failed; retaining last successful catalog"
                        ),
                    }
                }
            } else {
                loaded = None;
            }
            drop(proxy);
            drop(service);
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

fn publish(service: &Service, provider: &Provider, list: &Value) {
    let body = Bytes::from(list.to_string());
    *service.bedrock_catalog.write().unwrap() = Some((provider.clone(), body));
}

pub(super) fn reply(proxy: &Proxy, provider: &Provider) -> Response {
    let cached = proxy.service.bedrock_catalog.read().unwrap();
    if let Some((source, bytes)) = cached.as_ref()
        && source == provider
    {
        return ([(header::CONTENT_TYPE, "application/json")], bytes.clone()).into_response();
    }
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Bedrock profile catalog is not available yet; discovery runs in the background",
    )
}

pub(super) fn collect(page: &Value, models: &mut BTreeMap<String, BTreeSet<String>>) -> Result<()> {
    for p in page["inferenceProfileSummaries"]
        .as_array()
        .context("Invalid Bedrock profile list")?
    {
        let Some(id) = p["inferenceProfileId"].as_str() else {
            continue;
        };
        if p["status"] != "ACTIVE" || family(id).is_none() {
            continue;
        }
        let entry = models.entry(id.into()).or_default();
        for model in p["models"].as_array().into_iter().flatten() {
            if let Some((_, name)) = model["modelArn"]
                .as_str()
                .and_then(|arn| arn.split_once(":foundation-model/"))
            {
                entry.insert(name.into());
            }
        }
    }
    Ok(())
}

async fn fetch(proxy: &Proxy, provider: &Provider) -> Result<Value> {
    let region = match provider {
        Provider::Aws { region } => region,
        Provider::Relay { relay_socket } => {
            let list: Value = relay(
                relay_socket,
                Method::GET,
                "/bedrock/v1/models",
                HeaderMap::new(),
                Bytes::new(),
            )
            .await?
            .error_for_status()?
            .json()
            .await?;
            ensure!(
                list["object"] == "list" && list["data"].is_array(),
                "Invalid model list"
            );
            return Ok(list);
        }
    };
    let mut ids = BTreeMap::new();
    let mut url = url::Url::parse(&format!(
        "https://bedrock.{region}.amazonaws.com/inference-profiles"
    ))?;
    let mut next = None::<String>;
    loop {
        {
            let mut q = url.query_pairs_mut();
            q.clear()
                .append_pair("typeEquals", "SYSTEM_DEFINED")
                .append_pair("maxResults", "100");
            if let Some(token) = &next {
                q.append_pair("nextToken", token);
            }
        }
        let page = signed(
            &proxy.client,
            region,
            Method::GET,
            url.as_str(),
            HeaderMap::new(),
            Bytes::new(),
        )
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
        collect(&page, &mut ids)?;
        next = page["nextToken"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if next.is_none() {
            break;
        }
    }
    Ok(
        json!({"object":"list","data":ids.iter().map(|(id,models)|json!({"object":"model","id":id,"owned_by":family(id),"foundation_models":models})).collect::<Vec<_>>()}),
    )
}
