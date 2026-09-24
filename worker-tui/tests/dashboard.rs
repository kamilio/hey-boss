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
fn background_diagnostics_render_without_disabling_worker_controls() {
    let mut app = Dashboard::default();
    let mut value = snapshot();
    value["fleet"]["controller_connection"]["state"] = json!("local");
    app.apply(value);
    app.diagnostic = Some("Worker recovery: fixture failure\u{1b}[2J".into());
    let rendered = screen(&app);
    assert!(rendered.contains("Worker recovery: fixture failure"));
    assert!(rendered.contains("Active agents"));
    assert!(!rendered.contains('\u{1b}'));
    app.confirm(true);
    assert!(app.confirmation.is_some());
    app.apply(snapshot());
    assert!(
        app.diagnostic.is_some(),
        "Refresh lost the worker diagnostic"
    );
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
        assert!(screen.contains("Emergency update drain"));
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
    value["runs"][0]["state"] = json!("awaiting_claim");
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
fn activity_prioritizes_latest_event_without_duplicate_or_raw_metadata() {
    let mut app = Dashboard {
        now_ms: 120_000,
        ..Dashboard::default()
    };
    let mut value = snapshot();
    value["runs"][0]["last_event"] = json!("Checking layout\nDesktop and compact views");
    value["runs"][0]["events"] = json!([
        {"at": 115_000, "text": "Checking layout\nDesktop and compact views"},
        {"at": 100_000, "text": "Earlier activity"}
    ]);
    value["runs"][0]["goal"] = json!({"status":"active", "objective":"Improve readability"});
    app.apply(value);
    let rendered = screen(&app);
    assert_eq!(rendered.matches("Checking layout").count(), 1);
    assert!(rendered.contains("5s ago"));
    assert!(rendered.contains("Desktop and compact views"));
    assert!(rendered.contains("Goal: active"));
    assert!(!rendered.contains("Codex: session"));
    assert!(!rendered.contains("\"objective\""));
    assert!(rendered.find("Checking layout") < rendered.find("Earlier activity"));
}

#[test]
fn activity_retains_line_breaks_and_clamps_overscroll() {
    let mut app = Dashboard::default();
    let mut value = snapshot();
    value["runs"][0]["events"] = json!([{"text":"First line\nSecond line\u{7}\u{202e}"}]);
    value["runs"][0]["last_event"] = json!("First line\nSecond line\u{7}\u{202e}");
    app.apply(value);
    app.detail_scroll = u16::MAX;
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| ui::render(frame, &app)).unwrap();
    let buffer = terminal.backend().buffer();
    let first_y = (0..24)
        .find(|&y| {
            (0..80)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains("First line")
        })
        .unwrap();
    let second_y = (0..24)
        .find(|&y| {
            (0..80)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains("Second line")
        })
        .unwrap();
    assert_eq!(second_y, first_y + 1);
    assert!(
        buffer
            .content()
            .iter()
            .all(|c| !c.symbol().chars().any(char::is_control))
    );
}

#[test]
fn wide_dashboard_gives_activity_room_without_hiding_sessions() {
    let mut app = Dashboard::default();
    app.apply(snapshot());
    let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
    terminal.draw(|frame| ui::render(frame, &app)).unwrap();
    let buffer = terminal.backend().buffer();
    let activity_y = (0..36)
        .find(|&y| {
            (0..120)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains("Activity")
        })
        .unwrap();
    assert_eq!(
        activity_y, 5,
        "Wide terminals should show activity beside sessions"
    );
    assert!(
        buffer
            .content()
            .iter()
            .any(|c| c.fg == ratatui::style::Color::Rgb(161, 175, 255))
    );
}

#[test]
fn activity_empty_and_failure_states_explain_what_happened() {
    let mut app = Dashboard::default();
    let mut value = snapshot();
    value["runs"][0]["events"] = json!([]);
    value["runs"][0]["last_event"] = json!("");
    app.apply(value.clone());
    assert!(screen(&app).contains("Waiting for agent activity"));
    value["runs"][1]["state"] = json!("failed");
    value["runs"][1]["summary"] = json!("Network unavailable\nRetry after reconnecting");
    app.history = true;
    app.apply(value);
    let rendered = screen(&app);
    assert!(rendered.contains("Result"));
    assert!(rendered.contains("Network unavailable"));
    assert!(!rendered.contains("Waiting for agent activity"));
}

#[test]
fn activity_paging_stops_at_the_end_and_returns_with_one_page_up() {
    let mut app = Dashboard::default();
    let mut value = snapshot();
    value["runs"][0]["events"] = json!([{"text": "Wrapped activity message. ".repeat(100)}]);
    app.apply(value);
    let size = ratatui::layout::Rect::new(0, 0, 80, 24);
    for _ in 0..100 {
        ui::scroll_activity(&mut app, size, 5);
    }
    let bottom = app.detail_scroll;
    assert!(bottom > 5 && bottom < 500);
    ui::scroll_activity(&mut app, size, 5);
    assert_eq!(app.detail_scroll, bottom);
    ui::scroll_activity(&mut app, size, -5);
    assert_eq!(app.detail_scroll, bottom - 5);
    app.detail_scroll = u16::MAX;
    ui::scroll_activity(&mut app, size, -5);
    assert_eq!(app.detail_scroll, bottom - 5);
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
    assert!(screen(&app).contains("Emergency update drain"));
    assert!(!screen(&app).contains("Finishing work before update"));
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

fn project_snapshot() -> serde_json::Value {
    json!({"ok":true,"project_tabs":true,"workers":[
        {"id":"one","pid":1,"config":{"name":"One","enabled":true,"projects":["named:Alpha","named:Beta"],"concurrency":2},"runs":[
            {"id":"a","worker_id":"one","project_id":"named:Alpha","title":"Alpha work","finished_at":null},
            {"id":"b","worker_id":"one","project_id":"named:Beta","title":"Beta work","finished_at":null}]},
        {"id":"two","pid":2,"config":{"name":"Two","enabled":true,"projects":["named:Alpha"],"concurrency":1},"runs":[
            {"id":"a2","worker_id":"two","project_id":"named:Alpha","title":"Second Alpha","finished_at":null}]}]})
}

#[test]
fn project_tabs_collect_every_worker_and_wrap_in_both_directions() {
    let mut app = Dashboard::default();
    app.apply(project_snapshot());
    assert_eq!(app.project_ids(), ["named:Alpha", "named:Beta"]);
    assert_eq!(app.project_id.as_deref(), Some("named:Alpha"));
    assert_eq!(app.runs().len(), 2);
    app.switch_project(-1);
    assert_eq!(app.project_id.as_deref(), Some("named:Beta"));
    assert_eq!(app.runs()[0]["id"], "b");
    app.switch_project(1);
    assert_eq!(app.project_id.as_deref(), Some("named:Alpha"));
    app.navigate(1);
    app.confirm(false);
    assert_eq!(app.confirmation.as_ref().unwrap().worker_id, "two");
}

#[test]
fn project_selection_survives_refresh_and_removed_projects_fall_back() {
    let mut app = Dashboard::default();
    let mut value = project_snapshot();
    app.apply(value.clone());
    app.switch_project(1);
    value["workers"].as_array_mut().unwrap().reverse();
    app.apply(value.clone());
    assert_eq!(app.project_id.as_deref(), Some("named:Beta"));
    value["workers"].as_array_mut().unwrap().pop();
    app.apply(value);
    assert_eq!(app.project_id.as_deref(), Some("named:Alpha"));
    assert_eq!(app.runs().len(), 1);
}

#[test]
fn project_tab_header_is_visible_at_small_terminal_sizes() {
    let mut app = Dashboard::default();
    app.apply(project_snapshot());
    app.switch_project(1);
    let mut terminal = Terminal::new(TestBackend::new(48, 16)).unwrap();
    terminal.draw(|frame| ui::render(frame, &app)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(screen.contains("[Beta]"), "{screen}");
    assert!(screen.contains("Shift+Tab"), "{screen}");
    assert!(!screen.contains("Alpha work"), "{screen}");
}

#[test]
fn worker_manager_can_remove_an_idle_worker_without_selecting_an_agent() {
    let mut value = project_snapshot();
    value["workers"][1]["runs"] = json!([]);
    let mut app = Dashboard::default();
    app.apply(value);
    app.navigate_worker(1);
    app.confirm_remove();
    let removal = app.confirmation.unwrap();
    assert_eq!(removal.worker_id, "two");
    assert!(removal.graceful);
    assert!(!removal.stop);
}

#[test]
fn add_form_preserves_spaces_in_each_checkout_and_shares_slots() {
    use hey_boss_worker_tui::{AddWorker, backend::Request};
    let form = AddWorker {
        id: "retry-id".into(),
        fields: vec![
            "Shared tools".into(),
            "2".into(),
            "/work/Tool One".into(),
            "/work/Tool Two".into(),
        ],
        selected: 0,
        error: None,
    };
    match form.request().unwrap() {
        Request::AddWorker {
            id,
            name,
            concurrency,
            directories,
        } => {
            assert_eq!(name, "Shared tools");
            assert_eq!(id, "retry-id");
            assert_eq!(concurrency, 2);
            assert_eq!(directories, ["/work/Tool One", "/work/Tool Two"]);
        }
        _ => panic!("Expected worker creation"),
    }
    assert!(AddWorker::default().request().is_err());
}

#[test]
fn many_checkouts_keep_selected_worker_and_add_fields_visible_on_small_terminals() {
    let mut app = Dashboard::default();
    let mut value = project_snapshot();
    let workers = value["workers"].as_array_mut().unwrap();
    let base = workers[1].clone();
    workers.clear();
    for i in 0..9 {
        let mut worker = base.clone();
        worker["id"] = json!(format!("worker-{i}"));
        worker["config"]["name"] = json!(format!("Checkout {i}"));
        workers.push(worker);
    }
    app.apply(value);
    app.manage_workers = true;
    app.navigate_worker(8);
    let mut terminal = Terminal::new(TestBackend::new(48, 16)).unwrap();
    terminal.draw(|frame| ui::render(frame, &app)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(screen.contains("Checkout 8"), "{screen}");
    assert!(screen.contains("remove gracefully"), "{screen}");
    let mut form = hey_boss_worker_tui::AddWorker::default();
    for i in 0..8 {
        form.fields
            .push(format!("/Users/person/Workspace/long-checkout-{i}"));
    }
    form.selected = form.fields.len() - 1;
    app.add_worker = Some(form);
    terminal.draw(|frame| ui::render(frame, &app)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(screen.contains("long-checkout-7"), "{screen}");
    assert!(screen.contains("Enter add"), "{screen}");
}
