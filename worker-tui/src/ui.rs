//! Pure responsive rendering; usable with any Ratatui backend.
use crate::{Dashboard, text};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use serde_json::Value;

const ACCENT: Color = Color::Cyan;
pub const MIN_WIDTH: u16 = 48;
pub const MIN_HEIGHT: u16 = 12;

fn block(title: impl Into<String>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", title.into()))
        .border_style(Style::default().fg(if focused { ACCENT } else { Color::DarkGray }))
}

fn state(w: &Value) -> &'static str {
    if w["upgrading"] == true {
        "draining"
    } else if w["pid"].is_null() {
        if w["config"]["enabled"] == true {
            "offline"
        } else {
            "stopped"
        }
    } else if w["config"]["enabled"] != true {
        if w["active"].as_u64().unwrap_or(0) > 0 {
            "draining"
        } else {
            "paused"
        }
    } else {
        "running"
    }
}

fn color(state: &str) -> Color {
    match state {
        "running" | "completed" | "succeeded" => Color::Green,
        "failed" | "error" | "offline" => Color::Red,
        _ => Color::Yellow,
    }
}

fn scope(worker: &Value) -> String {
    let projects: Vec<String> = worker["config"]["projects"]
        .as_array()
        .map(|projects| {
            projects
                .iter()
                .map(|p| {
                    let id = text(p);
                    id.strip_prefix("named:")
                        .unwrap_or_else(|| id.rsplit('/').next().unwrap_or(&id))
                        .to_owned()
                })
                .collect()
        })
        .unwrap_or_default();
    if projects.is_empty() {
        "All projects".into()
    } else {
        projects.join(", ")
    }
}

fn elapsed(run: &Value, now_ms: i64) -> String {
    let Some(start) = run["started_at"].as_i64() else {
        return String::new();
    };
    let end = run["finished_at"].as_i64().unwrap_or(now_ms);
    let seconds = end.saturating_sub(start).max(0) / 1000;
    format!(" · {}m{:02}s", seconds / 60, seconds % 60)
}

fn overlay(frame: &mut Frame, title: &str, message: &str) {
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(72);
    let height = area.height.saturating_sub(2).min(12);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(message)
            .wrap(Wrap { trim: false })
            .block(block(title, true)),
        rect,
    );
}

pub fn render(frame: &mut Frame, app: &Dashboard) {
    let size = frame.area();
    if size.width < MIN_WIDTH || size.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Resize to at least 48 × 12.\nCtrl+C / q: quit")
                .wrap(Wrap { trim: true }),
            size,
        );
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(5),
        Constraint::Length(2),
    ])
    .split(size);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    " HEY BOSS ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  Workers"),
            ]),
            Line::from(format!(
                " {} · {}",
                text(&app.snapshot["store"]["host"]),
                text(&app.snapshot["store"]["database"])
            )),
        ]),
        rows[0],
    );
    let wide = size.width >= 90;
    let panels = if wide {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
            .split(rows[1])
    } else {
        Layout::vertical([Constraint::Length(4), Constraint::Min(2)]).split(rows[1])
    };
    let workers = app.workers();
    let items: Vec<ListItem> = workers
        .iter()
        .map(|w| {
            let status = state(w);
            let label = Line::from(vec![
                Span::styled(
                    format!("{} ", text(&w["config"]["name"])),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{status} {}/{}", w["active"], w["config"]["concurrency"]),
                    Style::default().fg(color(status)),
                ),
            ]);
            if wide {
                ListItem::new(vec![
                    label,
                    Line::from(Span::styled(
                        format!(
                            "  {} · {}",
                            scope(w),
                            text(&w["id"]).chars().take(8).collect::<String>()
                        ),
                        Style::default().fg(Color::DarkGray),
                    )),
                ])
            } else {
                ListItem::new(label)
            }
        })
        .collect();
    let mut selection = ListState::default().with_selected(
        workers
            .iter()
            .position(|w| w["id"].as_str() == app.worker_id.as_deref()),
    );
    let workers_block = block(
        format!("Workers · {}", workers.len()),
        !app.sessions_focused,
    );
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(if app.pending {
                "Connecting to queue…"
            } else {
                "No workers. Start: hey-boss worker"
            })
            .wrap(Wrap { trim: true })
            .block(workers_block),
            panels[0],
        );
    } else {
        frame.render_stateful_widget(
            List::new(items)
                .block(workers_block)
                .highlight_symbol("› ")
                .highlight_style(Style::default().bg(Color::DarkGray)),
            panels[0],
            &mut selection,
        );
    }
    let right = if panels[1].height < 10 {
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(0),
        ])
        .split(panels[1])
    } else {
        Layout::vertical([
            Constraint::Length(2),
            Constraint::Percentage(45),
            Constraint::Min(2),
        ])
        .split(panels[1])
    };
    let summary = if app.snapshot["worker_id"].as_str() == app.worker_id.as_deref() {
        format!(
            " {} busy · {} free / {} slots · {} eligible\n tags {} · {}",
            app.snapshot["active"],
            app.snapshot["free"],
            app.snapshot["config"]["concurrency"],
            app.snapshot["eligible"],
            text(&Value::String(app.snapshot["config"]["tags"].to_string())),
            if app.snapshot["fleet"]["role"] == "agent" {
                "fleet replica"
            } else {
                "local queue"
            }
        )
    } else {
        " Loading worker sessions…".into()
    };
    frame.render_widget(Paragraph::new(summary), right[0]);
    let runs = app.runs();
    let compact_sessions = right[1].height < 4;
    let items: Vec<ListItem> = runs
        .iter()
        .map(|r| {
            let status = text(&r["state"]);
            if compact_sessions {
                return ListItem::new(Line::from(vec![
                    Span::raw(format!("#{} ", r["number"])),
                    Span::styled(status.clone(), Style::default().fg(color(&status))),
                    Span::raw(format!(" · {}", text(&r["title"]))),
                ]));
            }
            ListItem::new(vec![
                Line::from(vec![
                    Span::raw(format!("{} #{} ", text(&r["project_name"]), r["number"])),
                    Span::styled(status.clone(), Style::default().fg(color(&status))),
                    Span::raw(if r["finished_at"].is_null() {
                        " · active"
                    } else {
                        " · history"
                    }),
                    Span::raw(elapsed(r, app.now_ms)),
                ]),
                Line::from(format!("  {}", text(&r["title"]))),
            ])
        })
        .collect();
    let selected = runs
        .iter()
        .position(|r| r["id"].as_str() == app.run_id.as_deref());
    let mut selection = ListState::default().with_selected(selected);
    let sessions_block = block(
        if app.history {
            "Sessions + recent attempts"
        } else {
            "Active sessions"
        },
        app.sessions_focused,
    );
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new("No sessions to show. h: toggle history")
                .wrap(Wrap { trim: true })
                .block(sessions_block),
            right[1],
        );
    } else {
        frame.render_stateful_widget(
            List::new(items)
                .block(sessions_block)
                .highlight_symbol("› ")
                .highlight_style(Style::default().bg(Color::DarkGray)),
            right[1],
            &mut selection,
        );
    }
    let detail = selected
        .map(|i| {
            let run = runs[i];
            let mut lines = vec![
                Line::from(format!("Codex: {}", text(&run["session_id"]))),
                Line::from(text(&run["last_event"])),
                Line::from(text(&run["summary"])),
            ];
            if run["finished_at"].is_null()
                && let Some(expires) = run["reservation_expires"].as_i64()
            {
                let seconds = expires.saturating_sub(app.now_ms).max(0) / 1000;
                lines.insert(1, Line::from(format!("Manual claim deadline: {seconds}s")));
            }
            if let Some(events) = run["events"].as_array() {
                lines.extend(events.iter().map(|event| Line::from(text(&event["text"]))));
            }
            if !run["goal"].is_null() {
                lines.push(Line::from(text(&Value::String(run["goal"].to_string()))));
            }
            lines
        })
        .unwrap_or_else(|| {
            vec![Line::from(
                "Select a session to inspect its latest activity.",
            )]
        });
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0))
            .block(block("Activity · PgUp/PgDn", false)),
        right[2],
    );
    let status = if let Some(error) = &app.error {
        format!(" {}", text(&Value::String(error.clone())))
    } else if app.pending {
        " Refreshing… navigation remains available".into()
    } else {
        " Live · refresh every 2s · q exits dashboard; workers keep running".into()
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                status,
                Style::default().fg(if app.error.is_some() {
                    Color::Red
                } else {
                    Color::DarkGray
                }),
            )),
            Line::from(if wide {
                " ↑↓/jk move  Tab focus  h history  r refresh  p pause  s stop  ? help  q quit"
            } else {
                " ↑↓ move  Tab focus  ? help  q quit"
            }),
        ]),
        rows[2],
    );
    if app.help {
        overlay(
            frame,
            "Keyboard",
            "↑/↓ or j/k  Select worker or session\nTab        Switch between workers and sessions\nh          Show/hide completed attempts\nPgUp/PgDn  Scroll session activity\nr          Refresh now / retry after an error\np          Pause pickup; running sessions drain\ns          Stop worker and its sessions (confirmation)\nq / Ctrl+C Quit dashboard; workers keep running\nEsc / ?    Close help",
        );
    }
    if let Some(c) = &app.confirmation {
        overlay(
            frame,
            if c.stop {
                "Stop worker?"
            } else {
                "Pause worker?"
            },
            &format!(
                "{}\n{}\n\n{}\n\nEnter: confirm    Esc: cancel",
                c.name,
                c.worker_id,
                if c.stop {
                    "Stops this worker and its active Codex sessions."
                } else {
                    "Stops pickup. Existing Codex sessions finish normally."
                }
            ),
        );
    }
}
