//! Synthetic-daemon fixture used only by the Node binding integration tests.
//! Arguments: CACHE_PATH UPSTREAM_URL LISTEN_ADDRESS. EOF/newline shuts it down.
use hey_gh::{Client, Config, api::Api, local_auth};
use std::{path::PathBuf, time::Duration};
use tokio::io::AsyncReadExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: node_test_daemon CACHE UPSTREAM LISTEN".into());
    }
    let client = Client::with_token(
        Config {
            cache_path: PathBuf::from(&args[1]),
            rest_url: args[2].parse()?,
            graphql_url: format!("{}graphql", args[2]).parse()?,
            min_spacing: Duration::ZERO,
            max_change_events: 32,
            ..Config::default()
        },
        "synthetic-node-test-token".into(),
    )?;
    let api = Api::new(client).await?;
    let listener = tokio::net::TcpListener::bind(&args[3]).await?;
    let address = listener.local_addr()?;
    let (token, _registration) = local_auth::register(address)?;
    println!(
        "{}",
        serde_json::json!({"server":format!("http://{address}/")})
    );
    axum::serve(listener, api.router_with_auth(Some(token)))
        .with_graceful_shutdown(async {
            let mut byte = [0];
            let _ = tokio::io::stdin().read(&mut byte).await;
        })
        .await?;
    api.stop().await;
    Ok(())
}
