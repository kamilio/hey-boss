use super::*;
use tokio::time::Instant;

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

pub(super) async fn cooling_down(proxy: &Proxy, model: &str) -> bool {
    let binding = proxy.binding.as_ref().unwrap();
    binding
        .quota
        .exhausted
        .lock()
        .await
        .get(model)
        .is_some_and(|until| *until > Instant::now())
}

pub(super) async fn exhausted(proxy: &Proxy, model: &str) {
    let binding = proxy.binding.as_ref().unwrap();
    let mut signals = binding.quota.exhausted.lock().await;
    if signals.len() >= 256 {
        signals.retain(|_, until| *until > Instant::now());
    }
    if signals.len() < 256 || signals.contains_key(model) {
        signals.insert(model.into(), Instant::now() + Duration::from_secs(30));
    }
}
