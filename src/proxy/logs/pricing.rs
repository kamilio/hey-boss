use super::store::Entry;
use serde::Serialize;
use serde_json::Value;
use std::sync::LazyLock;

pub const BOOK: &str = include_str!("prices.json");
static PRICES: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(BOOK).expect("embedded prices"));

#[derive(Default, Serialize)]
pub struct Price {
    pub price_model: Option<String>,
    pub price_version: String,
    pub cost_nano_usd: Option<i64>,
}

pub fn price(entry: &Entry) -> Price {
    let resolve = |name: &str| {
        let name = PRICES["aliases"][name].as_str().unwrap_or(name);
        PRICES["prices"]
            .get(name)
            .or_else(|| PRICES["claude_prices"].get(name))
            .map(|rates| (name.to_owned(), rates))
    };
    let selected = entry
        .response_model
        .as_deref()
        .and_then(resolve)
        .or_else(|| entry.routed_model.as_deref().and_then(resolve))
        .or_else(|| {
            if entry
                .routed_model
                .as_deref()
                .is_some_and(|m| m.starts_with("gemini/"))
            {
                None
            } else {
                entry.requested_model.as_deref().and_then(resolve)
            }
        });
    let mut result = Price {
        price_model: selected.as_ref().map(|(name, _)| name.clone()),
        price_version: PRICES["version"].as_str().unwrap().into(),
        cost_nano_usd: None,
    };
    let Some((_, rates)) = selected else {
        return result;
    };
    if !matches!(entry.method.as_str(), "POST" | "SEND")
        || !matches!(
            entry.path.trim_end_matches('/'),
            "/v1/responses"
                | "/v1/responses/compact"
                | "/v1/chat/completions"
                | "/v1/completions"
                | "/v1/messages"
                | "/v1/custom/messages"
                | "/custom/v1/messages"
                | "/v1/custom/chat/completions"
                | "/custom/v1/chat/completions"
        )
    {
        return result;
    }
    let (Some(input), Some(output)) = (entry.input_tokens, entry.output_tokens) else {
        return result;
    };
    let cached = entry.cached_input_tokens.unwrap_or(0);
    let writes = entry.cache_write_tokens.unwrap_or(0);
    let writes_1h = entry.cache_write_1h_tokens.unwrap_or(0);
    let Some(uncached) = input
        .checked_sub(cached)
        .and_then(|n| n.checked_sub(writes))
    else {
        return result;
    };
    if writes_1h > writes {
        return result;
    }
    let input_rate = rates[0].as_f64().unwrap();
    let long = rates[4].as_u64().is_some_and(|threshold| input > threshold);
    let cost = (uncached as f64 * input_rate
        + cached as f64 * rates[1].as_f64().unwrap_or(input_rate)
        + (writes - writes_1h) as f64 * rates[3].as_f64().unwrap_or(input_rate)
        + writes_1h as f64 * rates[5].as_f64().unwrap_or(input_rate))
        * if long { 2.0 } else { 1.0 }
        + output as f64 * rates[2].as_f64().unwrap() * if long { 1.5 } else { 1.0 };
    let modifier = if result
        .price_model
        .as_deref()
        .is_some_and(|m| m.starts_with("claude-"))
    {
        let speed = match entry.speed.as_deref() {
            None | Some("standard") => 1.0,
            Some("fast") => match rates[6].as_f64() {
                Some(rate) => rate,
                None => return result,
            },
            _ => return result,
        };
        let geography = match entry.inference_geo.as_deref() {
            None | Some("global") => 1.0,
            Some("us") => 1.1,
            _ => return result,
        };
        speed * geography
    } else {
        1.0
    };
    let nanos = cost * modifier * 1000.0;
    if nanos.is_finite() && nanos < i64::MAX as f64 {
        result.cost_nano_usd = Some(nanos.round() as i64);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude(model: &str, input: u64, output: u64) -> Entry {
        Entry {
            method: "POST".into(),
            path: "/v1/messages".into(),
            routed_model: Some(model.into()),
            input_tokens: Some(input),
            output_tokens: Some(output),
            ..Entry::default()
        }
    }
    #[test]
    fn claude_estimates_cache_tiers_and_uses_the_reported_model() {
        let mut entry = claude("unknown-alias", 1_000_000, 100_000);
        entry.response_model = Some("claude-sonnet-5-5".into());
        entry.cached_input_tokens = Some(200_000);
        entry.cache_write_tokens = Some(100_000);
        entry.cache_write_1h_tokens = Some(20_000);
        assert_eq!(price(&entry).cost_nano_usd, Some(2_720_000_000));
        entry.cache_write_1h_tokens = Some(100_001);
        assert_eq!(price(&entry).cost_nano_usd, None);
        entry = claude("claude-sonnet-4-6", u64::MAX, 1);
        entry.cached_input_tokens = Some(u64::MAX);
        entry.cache_write_tokens = Some(1);
        assert_eq!(price(&entry).cost_nano_usd, None);
        entry = claude("claude-fable-5-1", 1_000_000, 0);
        entry.cached_input_tokens = Some(1_000_000);
        assert_eq!(price(&entry).cost_nano_usd, Some(250_000_000));
    }
    #[test]
    fn claude_context_speed_and_geography_modifiers_are_explicit() {
        assert_eq!(
            price(&claude("claude-sonnet-4-5", 300_000, 100_000)).cost_nano_usd,
            Some(4_050_000_000)
        );
        assert_eq!(
            price(&claude("claude-sonnet-4-6", 300_000, 100_000)).cost_nano_usd,
            Some(2_400_000_000)
        );
        let mut entry = claude("claude-opus-5-5", 1_000_000, 100_000);
        entry.speed = Some("fast".into());
        entry.inference_geo = Some("us".into());
        assert_eq!(price(&entry).cost_nano_usd, Some(13_200_000_000));
        entry.routed_model = Some("claude-opus-4-7".into());
        assert_eq!(price(&entry).cost_nano_usd, None); // No supported fast rate.
        entry.speed = None;
        entry.path = "/v1/messages/count_tokens".into();
        assert_eq!(price(&entry).cost_nano_usd, None);
    }
}
