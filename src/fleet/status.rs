use serde_json::Value;
use std::fmt::Write;

// Escape untrusted control characters so device errors cannot rewrite the terminal.
fn text(v: &Value) -> String {
    match v {
        Value::Null => "unknown".into(),
        Value::String(s) => s.chars().flat_map(|c| c.escape_debug()).collect(),
        _ => v.to_string(),
    }
}

pub(super) fn status_text(v: &Value) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Fleet · authoritative supervisor {}",
        text(&v["supervisor"])
    );
    let _ = writeln!(out, "Desired build: {}", text(&v["desired_build"]));
    let c = &v["counts"];
    let _ = writeln!(
        out,
        "{} machines · {} unresolved conflicts\nSignals: {} pending / {} total · Events: {} retained\n",
        text(&c["machines"]),
        text(&c["unresolved_conflicts"]),
        text(&c["pending_signals"]),
        text(&c["signals"]),
        text(&c["retained_events"])
    );
    for (label, key) in [
        ("Pending changes", "reported_pending_changes"),
        ("Companion conflicts", "reported_companion_conflicts"),
    ] {
        let _ = writeln!(
            out,
            "{label}: {} reported · {} machines unknown",
            text(&c[key]["known"]),
            text(&c[key]["unknown_machines"])
        );
    }
    out.push('\n');
    for m in v["machines"].as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "{} ({}) · {} · {}",
            text(&m["host"]),
            text(&m["hostname"]),
            text(&m["role"]),
            text(&m["state"])
        );
        let _ = writeln!(
            out,
            "  Build: {} · deployment: {}",
            text(&m["build"]),
            text(&m["deployment"])
        );
        let _ = writeln!(
            out,
            "  Pending: {} · conflicts: {}\n  Last sync: {} · heartbeat: {}",
            text(&m["pending"]),
            text(&m["conflicts"]),
            text(&m["last_sync"]),
            text(&m["heartbeat"])
        );
        let _ = writeln!(
            out,
            "  Applied revision: {}\n  Desired revision: {}",
            text(&m["applied_revision"]),
            text(&m["desired_revision"])
        );
        for key in ["error", "deployment_error", "configuration_error"] {
            if !m[key].is_null() {
                let _ = writeln!(out, "  {key}: {}", text(&m[key]));
            }
        }
        let _ = writeln!(
            out,
            "  Workers: {} ({} omitted)",
            text(&m["worker_count"]),
            text(&m["workers_omitted"])
        );
        for w in m["workers"].as_array().into_iter().flatten() {
            let _ = writeln!(
                out,
                "    {} · {} · enabled {}\n      Active {} / slots {} · free {} · build {}",
                text(&w["id"]),
                text(&w["intent"]),
                text(&w["enabled"]),
                text(&w["active"]),
                text(&w["concurrency"]),
                text(&w["free"]),
                text(&w["build"])
            );
            if !w["error"].is_null() {
                let _ = writeln!(out, "      error: {}", text(&w["error"]));
            }
        }
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "Machines shown: {} / {} (offset {}).",
        text(&v["page"]["returned"]),
        text(&v["page"]["total"]),
        text(&v["page"]["offset"])
    );
    if !v["page"]["next_offset"].is_null() {
        let _ = writeln!(
            out,
            "Next: hey-boss fleet status --offset {}",
            text(&v["page"]["next_offset"])
        );
    }
    out.push_str("Last reported machine state; disconnected data may be stale. Unknown is not healthy.\nConflicts count all unresolved supervisor records; companion counts are separate.\nEvents cover retained supervisor memory only, not complete history.\nSummary text is limited to 240 characters; at most 20 workers per machine.\nDetails: hey-boss fleet status --records machines|conflicts|events|signals --limit 1 --offset 0\nJSON: hey-boss fleet status --json · Live pages may shift between reads.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknowns_and_terminal_controls_remain_visible() {
        let v = serde_json::json!({"machines":[{"host":"peer\n\u{1b}[2J","error":"failed","workers":null}],"counts":{"unresolved_conflicts":125}});
        let output = status_text(&v);
        assert!(output.contains("125 unresolved conflicts"));
        assert!(output.contains("peer\\n\\u{1b}[2J"));
        assert!(output.contains("error: failed"));
        assert!(output.contains("Workers: unknown"));
        assert!(!output.contains('\u{1b}'));
    }
}
