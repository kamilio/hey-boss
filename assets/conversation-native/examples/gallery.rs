use anyhow::{Context, Result};
use hey_boss_conversation::media;
use serde_json::Value;
use std::path::PathBuf;
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let resources = PathBuf::from(args.next().context("Expected desktop resource directory")?);
    let output = PathBuf::from(args.next().context("Expected fixture directory")?);
    let sources: Vec<Value> = serde_json::from_str(include_str!("gallery.json"))?;
    let http = reqwest::Client::new();
    let mut fixtures = Vec::new();
    for source in sources {
        fixtures.push(media::prepare(&source, &resources, &resources, &resources, &http).await?);
    }
    std::fs::write(output.join("media.json"), serde_json::to_vec(&fixtures)?)?;
    Ok(())
}
