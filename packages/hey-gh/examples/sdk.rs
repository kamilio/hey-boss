//! Run `hey-gh serve`, then `cargo run --example sdk -- OWNER/REPO NUMBER`.
use hey_gh::{ApiClient, Freshness};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: sdk OWNER/REPO NUMBER".into());
    }
    let api = ApiClient::new("http://127.0.0.1:8787/".parse()?)?;
    let report = api
        .ci_for_pr(&args[1], args[2].parse()?, Freshness::Revalidate)
        .await?;
    println!(
        "CI: {} (complete: {})",
        report.data.summary.state, report.complete
    );
    let baseline = api.bootstrap().await?;
    let changes = api
        .changes(Some(&baseline.cursor), 100, Duration::from_secs(1))
        .await?;
    println!(
        "{} snapshots; {} new observations",
        baseline.snapshots.len(),
        changes.changes.len()
    );
    Ok(())
}
