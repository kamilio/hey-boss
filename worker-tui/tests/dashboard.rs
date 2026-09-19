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
fn worker_header_keeps_project_and_checkout_visible_while_draining() {
    let mut app = Dashboard::default();
    let mut value = snapshot();
    value["workers"][0]["config"]["projects"] = json!(["named:demo"]);
    value["workers"][0]["config"]["directory"] = json!("/work/demo-checkout");
    value["projects"] = json!([{"id":"named:demo","name":"Demo project"}]);
    value["workers"][0]["upgrading"] = json!(true);
    app.apply(value);
    for width in [48, 120] {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal.draw(|frame| ui::render(frame, &app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            screen.contains("Demo project"),
            "Project missing at width {width}"
        );
        assert!(
            screen.contains("/work/demo-checkout"),
            "Checkout missing at width {width}"
        );
        assert!(screen.contains("Finishing work before update"));
    }
}

#[test]
fn worker_header_describes_all_projects_without_inventing_a_checkout() {
    let mut app = Dashboard::default();
    app.apply(snapshot());
    assert!(screen(&app).contains("All projects"));
    assert!(screen(&app).contains("Per-project checkouts"));
}

#[test]
fn tab_title_uses_the_selected_workers_scope_and_sanitizes_queue_text() {
    let mut value = snapshot();
    value["workers"][0]["config"]["projects"] =
        json!(["named:demo", "github.com/kamilio/hey-boss"]);
    value["workers"][0]["config"]["directory"] = json!("/work/demo\u{7}\u{1b}\u{9c}\u{202e}\n");
    value["projects"] = json!([{"id":"named:demo","name":"Demo\u{7}\n"}]);
    assert_eq!(
        ui::worker_title(&value, Some("a")).as_deref(),
        Some("hey-boss · Demo, hey-boss · /work/demo")
    );
    assert_eq!(
        ui::worker_title(&value, Some("b")).as_deref(),
        Some("hey-boss · All projects · Per-project checkouts")
    );
    assert_eq!(ui::worker_title(&value, Some("missing")), None);
}

#[test]
fn navigation_preserves_session_and_worker_identity_across_reordering() {
    let mut app = Dashboard::default();
    app.apply(snapshot());
    assert_eq!(app.run_id.as_deref(), Some("live"));
    assert_eq!(app.runs().len(), 1);
    app.history = true;
    assert_eq!(app.runs().len(), 1);
    assert_eq!(app.runs()[0]["id"], "done");
    app.normalize_run();
    app.navigate(1);
    assert_eq!(app.run_id.as_deref(), Some("done"));
    app.history = false;
    app.normalize_run();
    assert_eq!(app.run_id.as_deref(), Some("live"));
    let mut reordered = snapshot();
    reordered["workers"].as_array_mut().unwrap().reverse();
    app.apply(reordered);
    assert_eq!(app.worker_id.as_deref(), Some("a"));
    app.navigate(-1);
    assert_eq!(app.worker_id.as_deref(), Some("a"));
    assert_eq!(app.run_id.as_deref(), Some("live"));
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
        assert!(screen.contains("history · 1m00s"));
        app.history = false;
        app.normalize_run();
        let active = self::screen(&app);
        assert!(active.contains(elapsed));
        assert!(active.contains(&format!("Manual claim deadline: {claim}")));
        app.history = true;
        app.normalize_run();
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
    assert!(screen.contains("AVAILABLE"));
    assert!(screen.contains("1 busy · 1 available / 2 slots"));
    assert!(screen.contains("? help  q quit"));
}

fn screen(app: &Dashboard) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
    terminal.draw(|frame| ui::render(frame, app)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

#[test]
fn dashboard_shows_only_current_worker_and_separates_history() {
    let mut app = Dashboard::default();
    app.apply(snapshot());
    let active = screen(&app);
    assert!(active.contains("Builder 🚀"));
    assert!(!active.contains("Review"));
    assert!(active.contains("AVAILABLE"));
    assert!(active.contains("1 busy"));
    assert!(active.contains("1 available"));
    assert!(!active.contains("Previous attempt"));
    app.history = true;
    app.normalize_run();
    let history = screen(&app);
    assert!(history.contains("Completed attempts"));
    assert!(history.contains("Previous attempt"));
    assert!(!history.contains("Build dashboard"));
}

#[test]
fn busy_and_finishing_states_use_plain_language() {
    let mut app = Dashboard::default();
    let mut value = snapshot();
    value["workers"][0]["active"] = json!(2);
    app.apply(value.clone());
    assert!(screen(&app).contains("BUSY"));
    value["workers"][0]["config"]["enabled"] = json!(false);
    app.apply(value.clone());
    assert!(screen(&app).contains("Finishing work before pause"));
    value["workers"][0]["upgrading"] = json!(true);
    app.apply(value);
    assert!(screen(&app).contains("Finishing work before update"));
    assert!(!screen(&app).contains("draining"));
}

#[test]
fn supervisor_connectivity_remains_visible_with_stale_sessions() {
    let mut app = Dashboard::default();
    let mut value = snapshot();
    value["fleet"] = json!({"role":"companion", "pending_changes":3,
        "supervisor_connection":{"state":"connected", "last_sync":1}});
    app.apply(value.clone());
    assert!(screen(&app).contains("Supervisor: connected"));
    value["fleet"]["supervisor_connection"]["state"] = json!("disconnected");
    app.apply(value);
    assert!(screen(&app).contains("Supervisor: disconnected"));
    assert!(screen(&app).contains("3 changes waiting to sync"));
    app.error = Some("Queue unavailable".into());
    assert!(screen(&app).contains("Supervisor: unknown"));
    assert!(screen(&app).contains("Build dashboard"));
}

#[test]
fn supervisor_connectivity_accepts_status_from_older_workers() {
    let mut app = Dashboard::default();
    let mut value = snapshot();
    value["fleet"] = json!({"role":"controller", "controller_connection":{"state":"local"}});
    app.apply(value);
    assert!(screen(&app).contains("Supervisor: this machine"));
}

#[test]
fn initial_selection_uses_status_worker_and_owned_dashboard_stays_pinned() {
    let mut value = snapshot();
    value["worker_id"] = json!("b");
    let mut app = Dashboard::default();
    app.apply(value);
    assert_eq!(app.worker_id.as_deref(), Some("b"));
    app.owned_worker = true;
    app.apply(snapshot());
    assert_eq!(app.worker_id.as_deref(), Some("b"));
    assert!(app.runs().is_empty());
    app.apply(json!({"worker_id":"a", "workers":[{"id":"a"}], "runs":[]}));
    assert_eq!(app.worker_id.as_deref(), Some("b"));
}
