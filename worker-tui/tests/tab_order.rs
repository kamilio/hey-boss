use hey_boss_worker_tui::{Dashboard, ui};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

fn project_snapshot() -> serde_json::Value {
    json!({"project_tabs":true,"workers":[
        {"id":"a","config":{"projects":["named:Alpha"]}},
        {"id":"b","config":{"projects":["named:Beta"]}}
    ]})
}

fn project_header(app: &Dashboard, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 16)).unwrap();
    terminal.draw(|frame| ui::render(frame, app)).unwrap();
    (0..width)
        .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
        .collect()
}

#[test]
fn project_tabs_keep_their_order_and_columns_when_selection_changes() {
    let mut app = Dashboard::default();
    app.apply(project_snapshot());
    for width in [48, 80, 120] {
        let first = project_header(&app, width);
        app.switch_project(1);
        let second = project_header(&app, width);
        assert!(second.contains("[Beta]"), "{second}");
        for name in ["Alpha", "Beta"] {
            assert_eq!(first.find(name), second.find(name), "{second}");
        }
        let mut refreshed = project_snapshot();
        refreshed["workers"].as_array_mut().unwrap().reverse();
        app.apply(refreshed);
        assert_eq!(second, project_header(&app, width));
        app.switch_project(1);
    }
}

#[test]
fn overflowing_project_tabs_show_a_contiguous_window_in_navigation_order() {
    let mut value = project_snapshot();
    let ids: Vec<_> = (0..10).map(|i| format!("named:Project {i:02}")).collect();
    value["workers"] = json!(
        ids.iter()
            .map(|id| json!({"id":id,"config":{"projects":[id]}}))
            .collect::<Vec<_>>()
    );
    let mut app = Dashboard::default();
    app.apply(value);
    for direction in [1, -1] {
        for step in 0..10 {
            let selected = format!(
                "Project {:02}",
                if direction == 1 {
                    step
                } else {
                    (10 - step) % 10
                }
            );
            let header = project_header(&app, 48);
            assert!(header.contains(&format!("[{selected}]")), "{header}");
            let visible: Vec<_> = (0..10)
                .filter_map(|i| header.find(&format!("Project {i:02}")).map(|at| (i, at)))
                .collect();
            assert!(
                visible
                    .windows(2)
                    .all(|pair| pair[0].0 + 1 == pair[1].0 && pair[0].1 < pair[1].1),
                "{header}"
            );
            assert_eq!(header.starts_with("‹ "), visible[0].0 > 0, "{header}");
            assert_eq!(
                header.ends_with(" ›"),
                visible.last().unwrap().0 < 9,
                "{header}"
            );
            app.switch_project(direction);
        }
    }
}

#[test]
fn oversized_unicode_project_tabs_keep_the_selection_marker_and_overflow_cues() {
    let mut app = Dashboard::default();
    app.apply(json!({"project_tabs":true,"workers":[{"id":"a","config":{"projects":["named:a"]}},{"id":"b","config":{"projects":["named:b"]}}],
        "projects":[{"id":"named:a","name":"界e\u{301}👩‍💻".repeat(30)},{"id":"named:b","name":"Next"}]}));
    let header = project_header(&app, 48);
    assert!(
        header.contains("[界") && header.contains("e\u{301}👩‍💻"),
        "{header}"
    );
    assert!(header.contains("…]"), "{header}");
    assert!(header.ends_with(" ›"), "{header}");
    app.switch_project(1);
    let header = project_header(&app, 48);
    assert!(header.starts_with("‹ "), "{header}");
    assert!(header.contains("[Next]"), "{header}");
}
