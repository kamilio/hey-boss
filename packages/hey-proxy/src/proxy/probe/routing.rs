use super::*;

// Only the endpoint is diagnostic. Never include URL credentials, queries,
// fragments, resolved keys, credential sources, or unrelated configuration.
fn endpoint(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw) else {
        return "invalid endpoint".into();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

pub(super) fn describe(config: &Config, path: &str, input: &Value) -> String {
    let path = path.trim_end_matches('/');
    let native = gemini::native_path(path);
    let requested = if native {
        path.split_once("/models/")
            .and_then(|(_, rest)| rest.split_once(':'))
            .map(|(m, _)| m)
    } else {
        input["model"].as_str()
    };
    let Some(requested) = requested else {
        return "Routing unavailable: request has no model. No upstream request was made.".into();
    };
    if config.mode == Mode::Client {
        return format!(
            "Route: {requested} -> client relay -> {}{path}\nFinal model routing is resolved by the host.\nNo upstream request was made.",
            endpoint(&config.upstream_url).trim_end_matches('/')
        );
    }
    let claude = path == "/v1/messages" && config.claude.as_ref().is_some_and(|c| c.routing);
    let incoming = requested_effort(path, input);
    // Native Gemini bypasses ordered forwarding. Bound accounts have their
    // routes cleared, so their diagnostics continue through the legacy path.
    if !native && let Some(index) = config.route_index(requested, path) {
        let route = &config.routes[index];
        let mut lines = vec![
            format!("Route: {requested}"),
            format!("API: {path}"),
            "Rule: ordered provider route".into(),
            "Configured provider order:".into(),
        ];
        for index in 0..route.legs.len() {
            let leg = match route.resolve(config, index, incoming) {
                Ok(leg) => leg,
                Err(_) => {
                    return "Routing error: invalid provider route. No upstream request was made."
                        .into();
                }
            };
            let billing = match leg.billing_mode {
                crate::config::routes::BillingMode::IncludedSubscription => "included subscription",
                crate::config::routes::BillingMode::PayPerToken => "pay per token",
            };
            lines.push(format!(
                "{}. {} -> {}/{} ({billing})",
                index + 1,
                leg.provider,
                leg.implementation,
                leg.upstream_model
            ));
            if incoming.is_some() || leg.reasoning.is_some() {
                lines.push(format!(
                    "   Reasoning: {} -> {}",
                    incoming.unwrap_or("default"),
                    leg.reasoning.as_deref().unwrap_or("default")
                ));
            }
        }
        lines.push(
            "Quota was not checked; actual selection depends on quota and request replayability."
                .into(),
        );
        lines.push("No upstream request was made.".into());
        return lines.join("\n");
    }
    let alias = if claude {
        None
    } else if native {
        config.aliases.iter().find(|a| {
            a.api_shape.is_none()
                && (a.from == requested || a.from == format!("models/{requested}"))
        })
    } else {
        config.alias_for(requested, path)
    };
    let effort_route = if native {
        None
    } else {
        alias.and_then(|a| incoming.and_then(|e| a.reasoning_routes.get(e)))
    };
    let selected = effort_route
        .map(|r| r.to.as_str())
        .or_else(|| alias.and_then(|a| a.to.as_deref()))
        .unwrap_or(requested);
    let key = effort_route
        .and_then(|r| r.api_key.as_deref())
        .or_else(|| alias.and_then(|a| a.api_key.as_deref()))
        .unwrap_or(&config.default.api_key);
    let rule = if effort_route.is_some() {
        "reasoning route"
    } else if alias.is_some() {
        "alias"
    } else {
        "passthrough"
    };
    let outgoing_effort = if native || claude {
        incoming
    } else {
        alias.and_then(|a| a.reasoning.as_deref()).or(incoming)
    };
    let api_path = if chat::is_path(path) || messages::is_path(path) {
        "/v1/responses"
    } else {
        path
    };
    let (destination, upstream, credential) = if claude {
        let provider = config.claude.as_ref().unwrap();
        (
            format!("claude/{requested}"),
            format!(
                "{}/v1/messages",
                endpoint(&provider.upstream_url).trim_end_matches('/')
            ),
            "Authentication: Claude subscription".into(),
        )
    } else if native || selected.starts_with("gemini/") {
        if native && alias.and_then(|a| a.to.as_ref()).is_some() && !selected.starts_with("gemini/")
        {
            return "Routing error: native Gemini aliases require a gemini/model target. No upstream request was made.".into();
        }
        let model = selected
            .strip_prefix("gemini/")
            .unwrap_or(selected)
            .trim_start_matches("models/");
        let upstream = config
            .gemini
            .as_ref()
            .map(|p| {
                p.endpoint(
                    model,
                    input["stream"] == true || path.ends_with(":streamGenerateContent"),
                )
                .map(|url| endpoint(&url))
                .unwrap_or_else(|_| "invalid Gemini model".into())
            })
            .unwrap_or_else(|| "Gemini provider is not configured".into());
        (
            format!("gemini/{model}"),
            upstream,
            "Authentication: Gemini provider".into(),
        )
    } else {
        // Use the same rewrite implementation as forwarding (including API shape
        // and explicit openai/ stripping), without copying conversation history.
        let mut value = json!({"model":requested});
        for field in ["reasoning", "reasoning_effort", "output_config", "thinking"] {
            if let Some(v) = input.get(field) {
                value[field] = v.clone();
            }
        }
        if let Err(message) = rewrite_value(config, path, &mut value) {
            return format!("Routing error: {message}. No upstream request was made.");
        }
        let model = value["model"].as_str().unwrap_or(requested);
        let codex = config.codex.is_some()
            && config
                .upstream_url
                .trim_end_matches('/')
                .ends_with("/backend-api/codex");
        (
            format!("{}/{model}", if codex { "codex" } else { "openai" }),
            format!(
                "{}{api_path}",
                endpoint(&config.upstream_url).trim_end_matches('/')
            ),
            if codex {
                "Authentication: Codex subscription".into()
            } else {
                format!("API key alias: {key}")
            },
        )
    };
    let mut lines = vec![
        format!("Route: {requested} -> {destination}"),
        format!("API: {path} -> {api_path}"),
        format!("Rule: {rule}"),
        format!("Upstream: {upstream}"),
        credential,
    ];
    if incoming.is_some() || outgoing_effort.is_some() {
        lines.push(format!(
            "Reasoning: {} -> {}",
            incoming.unwrap_or("default"),
            outgoing_effort.unwrap_or("default")
        ));
    }
    let fallbacks = hey_proxy::fallback::targets(&config.fallbacks, selected);
    if !fallbacks.is_empty() && !native && !claude {
        lines.push(format!(
            "Configured fallbacks for {selected} (not attempted): {}",
            fallbacks.join(", ")
        ));
    }
    lines.push("No upstream request was made.".into());
    lines.join("\n")
}
