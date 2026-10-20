//! A data-only bridge: callers never receive or resolve provider credentials.
use anyhow::{Result, bail};
use clap::Args as ClapArgs;
use serde::{Deserialize, Serialize};
use std::{io::Read, path::PathBuf, time::Duration};

#[derive(ClapArgs)]
pub(crate) struct Args {
    /// Total request deadline, including the response body.
    #[arg(long,default_value_t=20000,value_parser=clap::value_parser!(u64).range(100..=30000))]
    timeout_ms: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Input {
    model: String,
    dimensions: usize,
    input: Vec<String>,
}
fn parse(bytes: &[u8]) -> Result<Input> {
    let input: Input = serde_json::from_slice(bytes).map_err(|_| {
        anyhow::anyhow!("Expected embedding JSON with model, dimensions and input strings")
    })?;
    if !matches!(
        input.model.as_str(),
        "text-embedding-3-small" | "text-embedding-3-large"
    ) || input.dimensions == 0
        || input.dimensions > 3072
        || input.input.is_empty()
        || input.input.len() > 64
        || input
            .input
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > 8192)
        || input.input.iter().map(String::len).sum::<usize>() > 128 * 1024
    {
        bail!("Embedding input exceeds model or batch limits");
    }
    Ok(input)
}
pub(crate) async fn run(args: Args, config_path: Option<PathBuf>) -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 {
        bail!("Embedding input exceeds 256 KiB");
    }
    let input = parse(&bytes)?;
    let path = crate::config_path(config_path)?;
    let config = crate::config::load(&path)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(args.timeout_ms))
        .build()?;
    let mut request = client
        .post(format!("http://{}/v1/embeddings", config.local_address()))
        .json(&input);
    if config.mode == crate::config::Mode::Host {
        request = request.bearer_auth(crate::access::read(&path)?.local);
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Embedding proxy unavailable or request timed out"))?;
    if !response.status().is_success() {
        bail!(
            "Embedding proxy returned HTTP {}",
            response.status().as_u16()
        );
    }
    let mut output = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("Embedding response interrupted"))?
    {
        if output.len() + chunk.len() > 4 * 1024 * 1024 {
            bail!("Embedding response too large");
        }
        output.extend_from_slice(&chunk);
    }
    let value: serde_json::Value = serde_json::from_slice(&output)
        .map_err(|_| anyhow::anyhow!("Invalid embedding response"))?;
    if !value["data"].is_array() {
        bail!("Invalid embedding response");
    }
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn data_only_bounded_embedding_contract() {
        assert!(parse(br#"{"model":"text-embedding-3-small","dimensions":512,"input":["sleep reconnect"]}"#).is_ok());
        for text in [
            r#"{"model":"chat-latest","dimensions":512,"input":["hello"]}"#,
            r#"{"model":"text-embedding-3-small","dimensions":512,"input":[""]}"#,
            r#"{"model":"text-embedding-3-small","dimensions":512,"input":["hello"],"api_key":"synthetic"}"#,
        ] {
            assert!(parse(text.as_bytes()).is_err());
        }
    }
}
