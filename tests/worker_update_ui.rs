use hey_boss::worker_tui::{Dashboard, ui};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

#[test]
fn approval_infrastructure_hold_has_readable_history_at_terminal_sizes() {
    let mut app = Dashboard::default();
    app.history = true;
    app.apply(json!({
        "worker_id":"test", "workers":[{"id":"test","pid":123,"active":0,
            "config":{"name":"MacBook","enabled":true,"concurrency":2}}],
        "runs":[{"id":"held","worker_id":"test","project_name":"hey-boss",
            "title":"Preserve worker continuation", "number":131,
            "state":"infrastructure_blocked", "started_at":0,"finished_at":1,
            "summary":"Restore the approval service, then reopen the issue. Saved session retained.",
            "events":[]}]
    }));
    for (width, height) in [(48, 12), (80, 24), (120, 36)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| ui::render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(text.contains("131"), "{width} x {height}: {text}");
        assert!(
            !text.contains("infrastructure_blocked"),
            "Status must use human language: {text}"
        );
        if width >= 80 {
            assert!(text.contains("Approval service unavailable"), "{text}");
            assert!(text.contains("Restore the approval service"), "{text}");
        }
    }
}

#[test]
fn running_chief_is_selectable_and_visible_without_using_issue_slots() {
    let mut app = Dashboard::default();
    app.apply(json!({
        "worker_id": "test", "workers": [{
            "id": "test", "pid": 123, "active": 0,
            "config": {"name": "Poe", "enabled": true, "concurrency": 2,
                "projects": ["named:poe2"]}
        }], "runs": [], "chiefs": [{
            "id": "chief:poe2", "kind": "chief", "project_name": "poe2",
            "title": "Organizing project", "state": "running", "pid": 456,
            "session_id": "chief-thread", "started_at": 0, "finished_at": null,
            "last_event": "Checking attached PRs"
        }], "fleet": {"supervisor_connection": {"state": "local"}}
    }));
    assert_eq!(app.run_id.as_deref(), Some("chief:poe2"));
    for (width, height) in [(48, 12), (80, 24), (120, 36)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| ui::render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(text.contains("Chief"), "{width} × {height}: {text}");
        assert!(text.contains("0 busy · 2 available"), "{text}");
        assert!(!text.contains("poe2 #null"), "{text}");
        if width >= 80 {
            assert!(text.contains("Checking attached PRs"), "{text}");
            assert!(text.contains("chief-thread"), "{text}");
        }
    }
    app.history = true;
    assert!(
        app.runs().is_empty(),
        "An active Chief is absent from issue history"
    );
}

#[test]
fn update_and_pause_states_remain_distinct_at_supported_terminal_sizes() {
    for (width, height) in [(48, 12), (80, 24), (120, 36)] {
        for (draining, enabled, expected) in [
            (false, true, "AVAILABLE"),
            (true, true, "Emergency update drain"),
            (false, false, "Finishing work before pause"),
        ] {
            let mut app = Dashboard::default();
            app.apply(json!({
                "worker_id": "test", "workers": [{
                    "id": "test", "pid": 123, "active": 1, "upgrading": draining,
                    "config": {"name": "MacBook", "enabled": enabled, "concurrency": 2,
                        "directory": "/work/hey-boss", "projects": ["named:hey-boss"]}
                }], "runs": [], "fleet": {"supervisor_connection": {"state": "local"}}
            }));
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| ui::render(frame, &app)).unwrap();
            let buffer = terminal.backend().buffer();
            let text: String = (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        + "\n"
                })
                .collect();
            assert!(text.contains(expected), "{width} × {height}: {text}");
            assert!(!text.contains("Finishing work before update"));
            assert!(text.contains("q quit"));
        }
    }
}
