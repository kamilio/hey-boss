use anyhow::{Context, Result, bail};
use clap::Args as ClapArgs;
use hey_proxy::usage::{AccountUsage, Client, State};
use std::{
    fmt::Write,
    path::PathBuf,
    time::{Duration, UNIX_EPOCH},
};

#[derive(ClapArgs)]
pub(crate) struct Args {
    /// Provider ID (Claude is currently supported; Codex accounts will follow)
    #[arg(long, default_value = "claude")]
    provider: String,
    /// Proxy-local account alias
    #[arg(long, default_value = "default")]
    account: String,
    /// List configured accounts without fetching provider usage
    #[arg(long, conflicts_with_all = ["provider", "account"])]
    accounts: bool,
    /// Print the versioned API response as JSON
    #[arg(long)]
    json: bool,
    /// Proxy root or /v1 URL; otherwise use the local address from --config
    #[arg(long)]
    base_url: Option<String>,
    /// Environment variable containing a proxy access key (never a provider OAuth token)
    #[arg(long, default_value = "HEY_PROXY_TOKEN")]
    token_env: String,
    /// Total request timeout, including the response body
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
    timeout_seconds: u64,
}

pub(crate) async fn run(args: Args, config_path: Option<PathBuf>) -> Result<()> {
    let supplied = match std::env::var(&args.token_env) {
        Ok(token) => Some(token),
        Err(std::env::VarError::NotPresent) if args.token_env == "HEY_PROXY_TOKEN" => None,
        _ => bail!("Cannot read the requested proxy token environment variable"),
    };
    let (base, token) = if let Some(base) = args.base_url {
        // Never send a locally stored access key to an explicitly overridden host.
        (base, supplied)
    } else {
        let path = crate::config_path(config_path)?;
        let config = crate::config::load(&path)?;
        let token = match supplied {
            Some(token) => Some(token),
            None if config.mode == crate::config::Mode::Host => {
                Some(crate::access::read(&path)?.local)
            }
            None => None,
        };
        (format!("http://{}", config.local_address()), token)
    };
    let client = Client::new(&base, token.as_deref())?
        .with_timeout(Duration::from_secs(args.timeout_seconds))?;
    if args.accounts {
        let accounts = client.accounts().await?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&accounts)?);
        } else if accounts.accounts.is_empty() {
            println!("No subscription accounts configured.");
        } else {
            for account in accounts.accounts {
                println!("{}/{}", plain(&account.provider), plain(&account.id));
            }
        }
        return Ok(());
    }
    let usage = client.usage(&args.provider, &args.account).await?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&usage).context("Cannot encode usage")?
        );
    } else {
        print!("{}", render(&usage));
    }
    if usage.reading.state != State::Ok {
        bail!("No fresh subscription reading; see the reported state and retry delay");
    }
    Ok(())
}
fn plain(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect()
}
fn number(value: Option<f64>, suffix: &str) -> String {
    value
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| format!("{v:.1}{suffix}"))
        .unwrap_or_else(|| "unknown".into())
}
fn amount(value: Option<f64>, currency: &str) -> String {
    value
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| format!("{} {v:.2}", plain(currency)))
        .unwrap_or_else(|| "unknown".into())
}
fn render(usage: &AccountUsage) -> String {
    let mut out = String::new();
    let reading = &usage.reading;
    let state = match reading.state {
        State::Ok => "ok",
        State::Stale => "STALE",
        State::Error => "ERROR",
        State::Disabled => "DISABLED",
        State::Unknown => "UNKNOWN",
    };
    let _ = writeln!(
        out,
        "{}/{} · {state}",
        plain(&usage.account.provider),
        plain(&usage.account.id)
    );
    if let Some(at) = reading
        .updated_at
        .and_then(|s| UNIX_EPOCH.checked_add(Duration::from_secs(s)))
    {
        // httpdate supports years through 9999. Guard untrusted server timestamps.
        if reading.updated_at.is_some_and(|s| s < 253_402_300_800) {
            let _ = writeln!(
                out,
                "Last successful reading: {}",
                httpdate::fmt_http_date(at)
            );
        }
    }
    if let Some(data) = &reading.data {
        for window in &data.windows {
            let _ = writeln!(
                out,
                "{}: {} left ({} used) · resets {}",
                plain(&window.label),
                number(window.remaining_percent, "%"),
                number(window.used_percent, "%"),
                window
                    .resets_at
                    .as_deref()
                    .map(plain)
                    .unwrap_or_else(|| "unknown".into())
            );
        }
        if data.windows.is_empty() {
            out.push_str("Subscription quota windows: not reported\n");
        }
        match &data.extra_usage {
            Some(extra) => {
                let _ = writeln!(
                    out,
                    "Extra usage (beyond included subscription): {}",
                    match extra.enabled {
                        Some(true) => "enabled",
                        Some(false) => "disabled",
                        None => "status unknown",
                    }
                );
                let _ = writeln!(
                    out,
                    "Extra budget: {} left ({} used)",
                    number(extra.remaining_percent, "%"),
                    number(extra.used_percent, "%")
                );
                if let Some(spend) = &extra.spend {
                    let _ = writeln!(
                        out,
                        "Provider-reported {} extra spend: {}",
                        plain(&spend.period),
                        amount(spend.used, &spend.currency)
                    );
                    let _ = writeln!(
                        out,
                        "Extra spend cap: {} · remaining: {} · above cap: {}",
                        amount(spend.limit, &spend.currency),
                        amount(spend.remaining, &spend.currency),
                        amount(spend.over_limit, &spend.currency)
                    );
                } else {
                    out.push_str("Extra spend amounts: not reported or units unknown\n");
                }
            }
            None => out.push_str("Extra usage: not reported\n"),
        }
    }
    if let Some(error) = &reading.error {
        let _ = writeln!(out, "Notice: {}", plain(error));
    }
    if let Some(delay) = reading.retry_after_seconds {
        let _ = writeln!(
            out,
            "Next provider refresh in {delay}s (cached until then)."
        );
    }
    out.push_str(
        "Account-wide usage; extra spend is separate from estimated API-equivalent spend.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn cli_defaults_and_account_discovery_parse() {
        assert!(crate::Args::try_parse_from(["hey-proxy", "usage"]).is_ok());
        assert!(
            crate::Args::try_parse_from(["hey-proxy", "usage", "--accounts", "--json"]).is_ok()
        );
        assert!(
            crate::Args::try_parse_from([
                "hey-proxy",
                "usage",
                "--accounts",
                "--provider",
                "claude"
            ])
            .is_err()
        );
        assert!(
            crate::Args::try_parse_from(["hey-proxy", "usage", "--timeout-seconds", "0"]).is_err()
        );
    }
    #[test]
    fn output_marks_stale_unknown_and_disabled_spend_without_terminal_controls() {
        let reading = serde_json::from_value(serde_json::json!({
            "schema_version":1,"account":{"provider":"claude","id":"default"},"state":"stale",
            "updated_at":1790800000,"retry_after_seconds":60,"error":"wait\u{1b}[31m",
            "data":{"windows":[{"id":"session","label":"Session","remaining_percent":null,"used_percent":null}],
            "extra_usage":{"enabled":false,"spend":{"currency":"USD","period":"monthly","used":12.5,"limit":10,"remaining":0,"over_limit":2.5}}}
        })).unwrap();
        let output = render(&reading);
        assert!(output.contains("STALE"));
        assert!(output.contains("Session: unknown left (unknown used)"));
        assert!(output.contains("subscription): disabled"));
        assert!(output.contains("extra spend: USD 12.50"));
        assert!(output.contains("remaining: USD 0.00 · above cap: USD 2.50"));
        assert!(!output.contains('\u{1b}'));
    }
}
