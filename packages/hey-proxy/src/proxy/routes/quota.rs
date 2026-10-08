use super::*;
use tokio::time::Instant;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Availability {
    Included,
    Exhausted,
    Unavailable,
}

// A reset invalidates an older reading; elapsed time never creates capacity.
pub(in crate::proxy) fn crossed_reset(data: Option<&Value>, updated: Option<u64>) -> bool {
    let Some(updated) = updated else { return false };
    data.and_then(|d| d["windows"].as_array())
        .is_some_and(|windows| {
            windows.iter().any(|w| {
                w["resets_at"]
                    .as_str()
                    .and_then(hey_proxy::usage::parse_timestamp)
                    .is_some_and(|t| t > updated && t <= crate::codex_auth::now())
            })
        })
}

#[cfg(test)]
pub(super) async fn availability(proxy: &Proxy, model: &str) -> Availability {
    check(proxy, model).await.0
}

pub(super) async fn check(proxy: &Proxy, model: &str) -> (Availability, Option<Value>) {
    let binding = proxy.binding.as_ref().expect("selected account");
    let signal = binding.quota.exhausted.lock().await.get(model).copied();
    if let Some((until, observed)) = signal {
        if until > Instant::now() {
            return (Availability::Exhausted, None);
        }
        match binding.implementation {
            "codex" => binding.quota.codex.lock().await.expire_before(observed),
            "claude" => binding.quota.claude.lock().await.expire_before(observed),
            _ => return (Availability::Unavailable, None),
        }
    }
    let reading = match tokio::time::timeout(Duration::from_secs(25), async {
        match binding.implementation {
            "codex" => codex::reading(proxy).await,
            "claude" => claude::reading(proxy).await,
            _ => Value::Null,
        }
    })
    .await
    {
        Ok(reading) => reading,
        Err(_) => return (Availability::Unavailable, None),
    };
    let availability = evaluate(
        &reading,
        binding.implementation,
        model,
        crate::codex_auth::now(),
    );
    (availability, Some(reading))
}

pub(super) fn applies(window: &Value, implementation: &str, model: &str) -> Option<bool> {
    match (implementation, window["id"].as_str().unwrap_or("")) {
        (_, "five_hour" | "seven_day")
        | ("claude", "seven_day_oauth_apps")
        | ("codex", "codex_included_limit") => Some(true),
        ("claude", "seven_day_sonnet") => Some(model.starts_with("claude-sonnet-")),
        ("claude", "seven_day_opus") => Some(model.starts_with("claude-opus-")),
        ("claude", "seven_day_routines") => Some(false),
        ("codex", "codex-spark" | "codex-spark-weekly") => Some(model.contains("spark")),
        _ => window["model"].as_str().map(|scope| scope == model),
    }
}

pub(super) async fn exhausted(proxy: &Proxy, model: &str) {
    let binding = proxy.binding.as_ref().unwrap();
    let mut signals = binding.quota.exhausted.lock().await;
    if signals.len() >= 256 {
        signals.retain(|_, (until, _)| *until > Instant::now());
    }
    if signals.len() < 256 || signals.contains_key(model) {
        signals.insert(
            model.into(),
            (
                Instant::now() + Duration::from_secs(30),
                crate::codex_auth::now(),
            ),
        );
    }
}

fn evaluate(reading: &Value, implementation: &str, model: &str, now: u64) -> Availability {
    if reading["state"] != "ok"
        || reading["data"]["availability_unknown"] == true
        || !reading["updated_at"]
            .as_u64()
            .is_some_and(|t| t <= now && now - t <= 65)
    {
        return Availability::Unavailable;
    }
    let Some(windows) = reading["data"]["windows"]
        .as_array()
        .filter(|w| !w.is_empty())
    else {
        return Availability::Unavailable;
    };
    let mut applicable = 0;
    let mut exhausted = false;
    for window in windows {
        match applies(window, implementation, model) {
            Some(true) => {}
            Some(false) => continue,
            None => return Availability::Unavailable,
        }
        applicable += 1;
        let Some(remaining) = window["remaining_percent"]
            .as_f64()
            .or_else(|| window["used_percent"].as_f64().map(|v| 100.0 - v))
            .filter(|v| v.is_finite())
        else {
            return Availability::Unavailable;
        };
        exhausted |= remaining <= 0.0;
    }
    if applicable == 0 {
        Availability::Unavailable
    } else if exhausted {
        Availability::Exhausted
    } else {
        Availability::Included
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_model_limits_exclude_overage_and_stale_reset_guesses() {
        let mut reading = json!({"state":"ok","updated_at":100,"data":{"windows":[
            {"id":"five_hour","used_percent":20}, {"id":"seven_day_sonnet","used_percent":100},
            {"id":"seven_day_opus","used_percent":0}
        ],"extra_usage":{"enabled":true,"remaining_percent":100}}});
        assert_eq!(
            evaluate(&reading, "claude", "claude-sonnet-5-5", 100),
            Availability::Exhausted
        );
        assert_eq!(
            evaluate(&reading, "claude", "claude-opus-4-6", 100),
            Availability::Included
        );
        reading["state"] = json!("stale");
        assert_eq!(
            evaluate(&reading, "claude", "claude-opus-4-6", 100),
            Availability::Unavailable
        );
        reading["state"] = json!("ok");
        assert_eq!(
            evaluate(&reading, "claude", "claude-opus-4-6", 166),
            Availability::Unavailable
        );
        reading["data"]["windows"][0]["used_percent"] = Value::Null;
        assert_eq!(
            evaluate(&reading, "claude", "claude-sonnet-5-5", 100),
            Availability::Unavailable
        );
    }
}
