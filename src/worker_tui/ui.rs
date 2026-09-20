//! Pure responsive rendering; usable with any Ratatui backend.
use super::{Dashboard, text};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
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
        "Finishing work before update"
    } else if w["pid"].is_null() {
        if w["config"]["enabled"] == true {
            "offline"
        } else {
            "stopped"
        }
    } else if w["config"]["enabled"] != true {
        if w["active"].as_u64().unwrap_or(0) > 0 {
            "Finishing work before pause"
        } else {
            "paused"
        }
    } else if w["active"].as_u64().unwrap_or(0) >= w["config"]["concurrency"].as_u64().unwrap_or(1)
    {
        "BUSY"
    } else {
        "AVAILABLE"
    }
}

fn color(state: &str) -> Color {
    match state {
        "AVAILABLE" | "running" | "completed" | "succeeded" => Color::Green,
        "failed" | "error" | "offline" => Color::Red,
        _ => Color::Yellow,
    }
}

fn scope(worker: &Value, snapshot: &Value) -> String {
    let projects: Vec<String> = worker["config"]["projects"]
        .as_array()
        .map(|projects| {
            projects
                .iter()
                .map(|p| {
                    let id = text(p);
                    if let Some(project) = snapshot["projects"]
                        .as_array()
                        .and_then(|projects| projects.iter().find(|project| project["id"] == *p))
                    {
                        return text(&project["name"]);
                    }
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

fn checkout(worker: &Value) -> String {
    if let Some(directories) = worker["config"]["directories"].as_object()
        && !directories.is_empty()
    {
        return directories
            .iter()
            .map(|(project, path)| {
                format!(
                    "{}: {}",
                    project.strip_prefix("named:").unwrap_or(project),
                    text(path)
                )
            })
            .collect::<Vec<_>>()
            .join(" · ");
    }
    let directory = text(&worker["config"]["directory"]);
    if directory.is_empty() {
        "Per-project checkouts".into()
    } else {
        directory
    }
}

/// The tab follows the selected worker, not the dashboard's own directory.
pub fn worker_title(snapshot: &Value, worker_id: Option<&str>) -> Option<String> {
    let worker = snapshot["workers"]
        .as_array()?
        .iter()
        .find(|w| w["id"].as_str() == worker_id)?;
    Some(format!(
        "hey-boss · {} · {}",
        scope(worker, snapshot),
        checkout(worker)
    ))
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
        Constraint::Length(5),
        Constraint::Min(5),
        Constraint::Length(2),
    ])
    .split(size);
    let worker = app
        .workers()
        .into_iter()
        .find(|w| w["id"].as_str() == app.worker_id.as_deref());
    let mut header = vec![];
    if let Some(w) = worker {
        let status = state(w);
        let active = w["active"].as_u64().unwrap_or(0);
        let slots = w["config"]["concurrency"].as_u64().unwrap_or(0);
        header.push(Line::from(vec![
            Span::raw(" HEY BOSS "),
            Span::styled(
                format!(
                    " {} · {}",
                    scope(w, &app.snapshot),
                    text(&w["config"]["name"])
                ),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]));
        header.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!(" {status} "),
                Style::default()
                    .fg(Color::Black)
                    .bg(color(status))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  {active} busy · {} available / {slots} slots",
                slots.saturating_sub(active)
            )),
        ]));
        header.push(Line::from(format!(" {}", checkout(w))));
    } else {
        header.push(Line::from(if app.pending {
            " Connecting to queue…"
        } else {
            " No current worker. Start: hey-boss worker"
        }));
        header.push(Line::default());
        header.push(Line::default());
    }
    let connection = if app.error.is_some() {
        "unknown"
    } else {
        let fleet = &app.snapshot["fleet"];
        fleet
            .get("supervisor_connection")
            .unwrap_or(&fleet["controller_connection"])["state"]
            .as_str()
            .unwrap_or("unknown")
    };
    let connection_label = match connection {
        "local" => "Supervisor: this machine".to_owned(),
        "standalone" => "Supervisor: not configured (local queue)".to_owned(),
        state => format!("Supervisor: {state}"),
    };
    header.push(Line::from(vec![
        Span::styled(
            format!(" {connection_label}"),
            Style::default().fg(match connection {
                "connected" | "local" => Color::Green,
                "disconnected" => Color::Red,
                _ => Color::Yellow,
            }),
        ),
        Span::raw(
            if matches!(
                app.snapshot["fleet"]["role"].as_str(),
                Some("companion" | "agent")
            ) {
                format!(
                    " · {} changes waiting to sync",
                    app.snapshot["fleet"]["pending_changes"]
                        .as_u64()
                        .unwrap_or(0)
                )
            } else {
                String::new()
            },
        ),
    ]));
    header.push(Line::from(vec![
        Span::styled(
            if app.history {
                " Active "
            } else {
                " [Active] "
            },
            Style::default().fg(if app.history { Color::DarkGray } else { ACCENT }),
        ),
        Span::styled(
            if app.history {
                " [History] "
            } else {
                " History "
            },
            Style::default().fg(if app.history { ACCENT } else { Color::DarkGray }),
        ),
        Span::raw(" · h switch"),
    ]));
    frame.render_widget(Paragraph::new(header), rows[0]);
    let right = if rows[1].height < 10 {
        Layout::vertical([Constraint::Min(3), Constraint::Length(0)]).split(rows[1])
    } else {
        Layout::vertical([Constraint::Percentage(45), Constraint::Min(3)]).split(rows[1])
    };
    let runs = app.runs();
    let compact_sessions = right[0].height < 8;
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
            "Completed attempts · no slots used"
        } else {
            "Active agents"
        },
        true,
    );
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(if app.history {
                "No completed attempts. h: active work"
            } else {
                "No active agents. Waiting for eligible issues. h: history"
            })
            .wrap(Wrap { trim: true })
            .block(sessions_block),
            right[0],
        );
    } else {
        frame.render_stateful_widget(
            List::new(items)
                .block(sessions_block)
                .highlight_symbol("› ")
                .highlight_style(Style::default().bg(Color::DarkGray)),
            right[0],
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
        right[1],
    );
    let status = if let Some(error) = &app.error {
        format!(" {}", text(&Value::String(error.clone())))
    } else if app.owned_worker {
        " Live · refresh every 2s · q / Ctrl+C stops this worker and its sessions".into()
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
            Line::from(if size.width >= 90 {
                " ↑↓/jk session  h active/history  r refresh  p pause  s stop  ? help  q quit"
            } else {
                " ↑↓ session  h history  ? help  q quit"
            }),
        ]),
        rows[2],
    );
    if app.help {
        overlay(
            frame,
            "Keyboard",
            &format!(
                "↑/↓ or j/k  Select session\nh          Switch active work / completed history\nPgUp/PgDn  Scroll session activity\nr          Refresh now / retry after an error\np          Pause pickup; existing sessions finish normally\ns          Stop worker and its sessions (confirmation)\nq / Ctrl+C {}\nEsc / ?    Close help",
                if app.owned_worker {
                    "Stop this worker and its sessions"
                } else {
                    "Quit dashboard; workers keep running"
                }
            ),
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
