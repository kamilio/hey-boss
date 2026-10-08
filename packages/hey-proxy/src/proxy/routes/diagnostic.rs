//! Formats the real routing decision, without generating model traffic.
use super::*;

pub(super) struct Probe<'a> {
    config: Arc<Config>,
    path: &'a str,
    input: &'a Value,
    quota: Vec<String>,
}

impl<'a> Probe<'a> {
    pub(super) fn new(config: &Arc<Config>, path: &'a str, input: &'a Value) -> Self {
        Self {
            config: config.clone(),
            path,
            input,
            quota: Vec::new(),
        }
    }

    pub(super) fn quota(
        &mut self,
        leg: &ResolvedLeg,
        availability: &quota::Availability,
        reading: Option<&Value>,
    ) {
        let provider = &leg.provider;
        if *availability == quota::Availability::Unavailable {
            self.quota.push(format!("{provider}: quota unavailable"));
            return;
        }
        let Some(reading) = reading else {
            self.quota
                .push(format!("{provider}: quota exhausted (upstream cooldown)"));
            return;
        };
        if let Some(windows) = reading["data"]["windows"].as_array() {
            for window in windows.iter().filter(|w| {
                quota::applies(w, leg.implementation, &leg.upstream_model) == Some(true)
            }) {
                let label = match window["id"].as_str().unwrap_or("") {
                    "five_hour" => "5-hour quota",
                    "seven_day" => "Weekly quota",
                    "seven_day_sonnet" => "Weekly Sonnet quota",
                    "seven_day_opus" => "Weekly Opus quota",
                    "seven_day_oauth_apps" => "Weekly OAuth quota",
                    "codex-spark" => "5-hour Spark quota",
                    "codex-spark-weekly" => "Weekly Spark quota",
                    _ => "Included quota",
                };
                if let Some(remaining) = window["remaining_percent"]
                    .as_f64()
                    .or_else(|| window["used_percent"].as_f64().map(|v| 100.0 - v))
                {
                    let mut line = format!("{provider}: {label}: {remaining:.1}% remaining");
                    if let Some(reset) = window["resets_at"]
                        .as_str()
                        .and_then(hey_proxy::usage::parse_timestamp)
                    {
                        line.push_str(&format!(
                            "; resets {}",
                            hey_proxy::usage::format_unix_iso8601(reset)
                        ));
                    }
                    self.quota.push(line);
                }
            }
        }
        if let Some(updated) = reading["updated_at"].as_u64() {
            self.quota.push(format!(
                "{provider}: quota checked {}s ago",
                crate::codex_auth::now().saturating_sub(updated)
            ));
        }
    }

    pub(super) fn reply(&self, leg: Option<&ResolvedLeg>, blocked: Option<&str>) -> Response {
        let mut text = match leg {
            Some(leg) => format!(
                "Using: {} -> {}/{} ({})\nReasoning: {}",
                leg.provider,
                leg.implementation,
                leg.upstream_model,
                if leg.billing_mode == BillingMode::IncludedSubscription {
                    "included subscription"
                } else {
                    "pay per token"
                },
                leg.reasoning.as_deref().unwrap_or("default")
            ),
            None => format!(
                "Using: none — {}",
                blocked.unwrap_or("provider unavailable")
            ),
        };
        if leg.is_some_and(|leg| leg.billing_mode == BillingMode::PayPerToken) {
            text.push_str(
                "\nSelected provider quota: API billing; no included subscription quota.",
            );
        }
        for line in &self.quota {
            text.push('\n');
            text.push_str(line);
        }
        text.push_str("\n\n");
        text.push_str(&probe::routing::describe(
            &self.config,
            self.path,
            self.input,
        ));
        probe::text_reply(self.path, self.input, &text)
    }
}

pub(super) fn failure(
    report: Option<&Probe<'_>>,
    status: StatusCode,
    code: &str,
    message: &str,
) -> Response {
    match report {
        Some(report) => report.reply(None, Some(message)),
        None => super::failure(status, code, message),
    }
}
