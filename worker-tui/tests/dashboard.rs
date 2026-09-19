use hey_boss_worker_tui::{Dashboard, ui};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

fn snapshot() -> serde_json::Value {
    json!({"ok":true,"worker_id":"a","workers":[
        {"id":"a","pid":42,"active":1,"config":{"name":"Builder 🚀","enabled":true,"concurrency":2}},
        {"id":"b","pid":43,"active":0,"config":{"name":"Review","enabled":false,"concurrency":1}}
    ],"config":{"concurrency":2,"tags":["ready"]},"active":1,"free":1,"eligible":4,
    "store":{"host":"test","database":"/tmp/synthetic.db"},"runs":[
        {"id":"live","number":6,"project_name":"demo","title":"Build dashboard","state":"running","finished_at":null,"last_event":"Testing","session_id":"session","events":[{"text":"hello"}]},
        {"id":"done","number":5,"title":"Previous attempt","state":"completed","finished_at":123}
    ]})
}

#[test]
fn navigation_preserves_identity_across_reordering_and_hides_stale_runs() {
    let mut app = Dashboard::default();
    app.apply(snapshot());
    assert_eq!(app.run_id.as_deref(), Some("live"));
    assert_eq!(app.runs().len(), 1);
    app.history = true;
    assert_eq!(app.runs().len(), 2);
    app.sessions_focused = true;
    app.navigate(1);
    assert_eq!(app.run_id.as_deref(), Some("done"));
    app.history = false;
    app.normalize_run();
    assert_eq!(app.run_id.as_deref(), Some("live"));
    let mut reordered = snapshot();
    reordered["workers"].as_array_mut().unwrap().reverse();
    app.apply(reordered);
    assert_eq!(app.worker_id.as_deref(), Some("a"));
    app.sessions_focused = false;
    app.navigate(-1);
    assert_eq!(app.worker_id.as_deref(), Some("b"));
    assert!(app.runs().is_empty());
}

#[test]
fn confirmation_captures_exact_worker_and_is_disabled_during_requests_or_errors() {
    let mut app = Dashboard::default();
    app.apply(snapshot());
    app.pending = true;
    app.confirm(true);
    assert!(app.confirmation.is_none());
    app.pending = false;
    app.error = Some("Disconnected".into());
    app.confirm(true);
    assert!(app.confirmation.is_none());
    app.error = None;
    app.confirm(true);
    app.navigate(1);
    assert_eq!(app.confirmation.unwrap().worker_id, "a");
}

#[test]
fn owned_worker_footer_and_help_describe_shutdown_instead_of_detachment() {
    let mut app = Dashboard::default();
    app.apply(snapshot());
    let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
    for owned in [false, true] {
        app.owned_worker = owned;
        for help in [false, true] {
            app.help = help;
            terminal.draw(|frame| ui::render(frame, &app)).unwrap();
            let screen: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            if owned {
                assert!(
                    screen.contains("stops this worker") || screen.contains("Stop this worker")
                );
                assert!(!screen.contains("workers keep running"));
            } else {
                assert!(screen.contains("workers keep running"));
            }
        }
    }
}

#[test]
fn renders_resizes_empty_states_modals_and_untrusted_text_without_panicking() {
    let mut app = Dashboard::default();
    app.apply(snapshot());
    app.snapshot["runs"][0]["title"] = json!("中\u{001b}[2J\u{0007}\u{202e}text");
    for (width, height) in [(1, 1), (30, 8), (48, 12), (64, 18), (90, 24), (120, 36)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        for mode in 0..4 {
            app.help = mode == 1;
            app.confirmation = None;
            if mode == 2 {
                app.confirm(false);
            }
            if mode == 3 {
                app.error = Some("Offline".into());
            }
            terminal.draw(|frame| ui::render(frame, &app)).unwrap();
            for cell in terminal.backend().buffer().content() {
                assert!(!cell.symbol().chars().any(char::is_control));
            }
        }
    }
    let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
    terminal
        .draw(|frame| ui::render(frame, &Dashboard::default()))
        .unwrap();
}

#[test]
fn refresh_removes_vanished_selection() {
    let mut app = Dashboard::default();
    app.apply(snapshot());
    app.apply(json!({"workers":[], "runs":[]}));
    assert!(app.worker_id.is_none());
    assert!(app.run_id.is_none());
}

#[test]
fn live_timers_advance_but_finished_attempts_keep_their_duration() {
    let mut app = Dashboard {
        history: true,
        now_ms: 65_000,
        ..Dashboard::default()
    };
    let mut value = snapshot();
    value["runs"][0]["started_at"] = json!(1000);
    value["runs"][0]["reservation_expires"] = json!(75_000);
    value["runs"][1]["started_at"] = json!(1000);
    value["runs"][1]["finished_at"] = json!(61_000);
    app.apply(value);
    let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
    for (now, elapsed, claim) in [(65_000, "1m04s", "10s"), (80_000, "1m19s", "0s")] {
        app.now_ms = now;
        terminal.draw(|frame| ui::render(frame, &app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains(elapsed));
        assert!(screen.contains("history · 1m00s"));
        assert!(screen.contains(&format!("Manual claim deadline: {claim}")));
    }
}

#[test]
fn smallest_supported_layout_keeps_the_selected_session_visible() {
    let mut app = Dashboard::default();
    app.apply(snapshot());
    let mut terminal = Terminal::new(TestBackend::new(48, 12)).unwrap();
    terminal.draw(|frame| ui::render(frame, &app)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(screen.contains("#6 running · Build dashboard"));
    assert!(screen.contains("? help  q quit"));
}
