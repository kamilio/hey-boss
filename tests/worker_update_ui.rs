use hey_boss::worker_tui::{Dashboard, ui};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

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
