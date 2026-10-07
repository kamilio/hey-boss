use hey_proxy::usage::{AccountUsage, SkipReason, State, included_quota};
use serde_json::json;

fn usage(provider: &str, id: &str) -> AccountUsage {
    serde_json::from_value(
        json!({"schema_version":1,"account":{"provider":provider,"id":id},
        "state":"ok","updated_at":1000,"retry_after_seconds":60,
        "data":{"windows":[
            {"id":"five_hour","label":"Session","remaining_percent":70,"resets_at":"2000"},
            {"id":"seven_day","label":"Week","remaining_percent":40,"resets_at":"3000"}],
            "extra_usage":{"enabled":true,"remaining_percent":100}}}),
    )
    .unwrap()
}

#[test]
fn included_capacity_requires_fresh_complete_verified_readings() {
    let original = usage("codex", "personal");
    assert_eq!(
        included_quota(&original, "gpt-6.1-sol", 1000)
            .unwrap()
            .remaining_percent,
        40.0
    );
    for state in [State::Stale, State::Error, State::Disabled, State::Unknown] {
        let mut u = original.clone();
        u.reading.state = state;
        assert_eq!(
            included_quota(&u, "gpt-6.1-sol", 1000).unwrap_err(),
            SkipReason::ReadingUnavailable
        );
    }
    for updated in [None, Some(0), Some(1001)] {
        let mut u = original.clone();
        u.reading.updated_at = updated;
        assert!(included_quota(&u, "gpt-6.1-sol", 1000).is_err());
    }
    assert_eq!(
        included_quota(&original, "gpt-6.1-sol", 1300).unwrap_err(),
        SkipReason::ReadingExpired
    );
    let mut u = original.clone();
    u.reading.data = None;
    assert!(included_quota(&u, "gpt-6.1-sol", 1000).is_err());
    for value in [None, Some(f64::NAN), Some(-1.0), Some(101.0)] {
        let mut u = original.clone();
        u.reading.data.as_mut().unwrap().windows[1].remaining_percent = value;
        assert_eq!(
            included_quota(&u, "gpt-6.1-sol", 1000).unwrap_err(),
            SkipReason::UnknownLimit
        );
    }
    let mut u = original;
    u.reading.data.as_mut().unwrap().windows.clear();
    assert_eq!(
        included_quota(&u, "gpt-6.1-sol", 1000).unwrap_err(),
        SkipReason::UnknownLimit
    );
}

#[test]
fn both_subscriptions_exhausted_with_paid_capacity_remain_ineligible() {
    for provider in ["codex", "claude"] {
        for index in [0, 1] {
            let mut u = usage(provider, "work");
            u.reading.data.as_mut().unwrap().windows[index].remaining_percent = Some(0.0);
            assert_eq!(
                included_quota(&u, "gpt-6.1-sol", 1000).unwrap_err(),
                SkipReason::IncludedExhausted
            );
        }
    }
}

#[test]
fn scoped_limits_follow_upstream_model_and_unknown_scope_fails_closed() {
    let mut u = usage("claude", "personal");
    let mut scope = u.reading.data.as_ref().unwrap().windows[0].clone();
    scope.id = "seven_day_opus".into();
    scope.remaining_percent = Some(0.0);
    u.reading.data.as_mut().unwrap().windows.push(scope);
    assert!(included_quota(&u, "claude-sonnet-5-5", 1000).is_ok());
    assert_eq!(
        included_quota(&u, "claude-opus-4-6", 1000).unwrap_err(),
        SkipReason::IncludedExhausted
    );
    u.reading.data.as_mut().unwrap().windows[2].id = "seven_day_sonnet".into();
    assert_eq!(
        included_quota(&u, "claude-sonnet-5-5", 1000).unwrap_err(),
        SkipReason::IncludedExhausted
    );
    let mut u = usage("codex", "work");
    let mut scope = u.reading.data.as_ref().unwrap().windows[0].clone();
    scope.id = "codex-spark".into();
    scope.group = Some("model".into());
    scope.remaining_percent = None;
    u.reading.data.as_mut().unwrap().windows.push(scope);
    assert!(included_quota(&u, "gpt-6.1-sol", 1000).is_ok());
    assert_eq!(
        included_quota(&u, "gpt-5.3-codex-spark", 1000).unwrap_err(),
        SkipReason::UnknownLimit
    );
    u.reading.data.as_mut().unwrap().windows[2].id = "unrecognized-model-limit".into();
    assert_eq!(
        included_quota(&u, "gpt-6.1-sol", 1000).unwrap_err(),
        SkipReason::UnknownScope
    );
}

#[test]
fn reset_invalidates_evidence_until_provider_refresh() {
    let mut u = usage("codex", "work");
    u.reading.data.as_mut().unwrap().windows[0].resets_at = Some("1050".into());
    assert_eq!(
        included_quota(&u, "gpt-6.1-sol", 1000).unwrap().expires_at,
        1050
    );
    assert_eq!(
        included_quota(&u, "gpt-6.1-sol", 1050).unwrap_err(),
        SkipReason::ResetNeedsRefresh
    );
    u.reading.updated_at = Some(1050);
    u.reading.data.as_mut().unwrap().windows[0].resets_at = Some("2050".into());
    assert!(included_quota(&u, "gpt-6.1-sol", 1050).is_ok());
}

#[test]
fn exact_model_scopes_and_conflicting_percentages_fail_safely() {
    let mut u = usage("codex", "work");
    let mut scoped = u.reading.data.as_ref().unwrap().windows[0].clone();
    scoped.group = Some("model".into());
    scoped.model = Some("gpt-6-astra".into());
    scoped.remaining_percent = None;
    u.reading.data.as_mut().unwrap().windows.push(scoped);
    assert!(included_quota(&u, "gpt-6.1-sol", 1000).is_ok());
    assert_eq!(
        included_quota(&u, "gpt-6-astra", 1000).unwrap_err(),
        SkipReason::UnknownLimit
    );
    u.reading.data.as_mut().unwrap().windows[0].used_percent = Some(100.0);
    assert_eq!(
        included_quota(&u, "gpt-6.1-sol", 1000).unwrap_err(),
        SkipReason::IncludedExhausted
    );
}

#[test]
fn denied_provider_availability_is_not_overridden_by_positive_windows() {
    let mut u = usage("codex", "work");
    u.reading.data.as_mut().unwrap().availability_unknown = true;
    assert_eq!(
        included_quota(&u, "gpt-6.1-sol", 1000).unwrap_err(),
        SkipReason::UnknownLimit
    );
}
