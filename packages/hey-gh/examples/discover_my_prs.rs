//! Read-only auth/discovery smoke check in an isolated, temporary cache.
use hey_gh::{Client, Config, Freshness};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let client = Client::from_gh(Config {
        cache_path: directory.path().join("cache.sqlite"),
        ..Config::default()
    })
    .await?;
    let pulls = client
        .all_my_open_pull_requests(Freshness::Revalidate)
        .await?;
    let before = client.status().network_requests;
    let cached = client
        .all_my_open_pull_requests(Freshness::CachedOnly)
        .await?;
    assert_eq!(pulls, cached);
    assert_eq!(before, client.status().network_requests);
    println!(
        "{}",
        serde_json::json!({"openPullRequests":pulls.len(),"cachedOnlyNetworkRequests":0})
    );
    Ok(())
}
