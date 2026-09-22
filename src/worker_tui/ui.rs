//! Pure responsive rendering; usable with any Ratatui backend.
use super::{Dashboard, project_name, text};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use serde_json::Value;

// Match the shared web app's dark palette, with explicit contrast on any terminal.
const BACKGROUND: Color = Color::Rgb(17, 21, 29);
const SURFACE: Color = Color::Rgb(32, 39, 51);
const TEXT: Color = Color::Rgb(227, 233, 242);
const MUTED: Color = Color::Rgb(162, 175, 193);
const BORDER: Color = Color::Rgb(79, 94, 120);
const ACCENT: Color = Color::Rgb(161, 175, 255);
const SELECTED: Color = Color::Rgb(47, 59, 91);
pub const MIN_WIDTH: u16 = 48;
pub const MIN_HEIGHT: u16 = 12;

fn block(title: impl Into<String>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", title.into()))
        .style(Style::default().bg(SURFACE).fg(TEXT))
        .border_style(Style::default().fg(if focused { ACCENT } else { BORDER }))
        .title_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
}

fn state(w: &Value) -> &'static str {
    if w["upgrading"] == true {
        "Emergency update drain"
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
        "AVAILABLE" | "completed" | "succeeded" => Color::Rgb(128, 207, 176),
        "running" | "BUSY" => ACCENT,
        "failed" | "error" | "offline" => Color::Rgb(242, 152, 169),
        _ => Color::Rgb(232, 197, 139),
    }
}

fn scope(worker: &Value, snapshot: &Value) -> String {
    let projects: Vec<String> = worker["config"]["projects"]
        .as_array()
        .map(|projects| {
            projects
                .iter()
                .map(|p| project_name(p.as_str().unwrap_or_default(), snapshot))
                .collect()
        })
        .unwrap_or_default();
    if projects.is_empty() {
        "All projects".into()
    } else {
        projects.join(", ")
    }
}

fn checkout(worker: &Value, snapshot: &Value) -> String {
    if let Some(directories) = worker["config"]["directories"].as_object()
        && !directories.is_empty()
    {
        return directories
            .iter()
            .map(|(project, path)| format!("{}: {}", project_name(project, snapshot), text(path)))
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
        checkout(worker, snapshot)
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

// Preserve paragraph boundaries while keeping the same bounded, safe queue text.
fn multiline(value: &Value) -> Vec<Line<'static>> {
    let bounded: String = value
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(8192)
        .collect();
    bounded
        .split('\n')
        .map(|line| Line::from(text(&Value::String(line.into()))))
        .collect()
}

fn activity(run: &Value, now_ms: i64) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "{} #{} · {}",
            text(&run["project_name"]),
            run["number"],
            text(&run["title"])
        ),
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    ))];
    let status = text(&run["state"]);
    lines.push(Line::from(Span::styled(
        format!("{status}{}", elapsed(run, now_ms)),
        Style::default().fg(color(&status)),
    )));
    if run["finished_at"].is_null()
        && run["state"] == "awaiting_claim"
        && run["claimed_at"].is_null()
        && let Some(expires) = run["reservation_expires"].as_i64()
    {
        let seconds = expires.saturating_sub(now_ms).max(0) / 1000;
        lines.push(Line::from(Span::styled(
            format!("Manual claim deadline: {seconds}s"),
            Style::default().fg(color("waiting")),
        )));
    }
    let goal = text(&run["goal"]["status"]);
    if !goal.is_empty() {
        lines[1].spans.push(Span::styled(
            format!(" · Goal: {goal}"),
            Style::default().fg(MUTED),
        ));
    }
    let summary = text(&run["summary"]);
    if !summary.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Result",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        lines.extend(multiline(&run["summary"]));
    }
    let events = run["events"].as_array();
    let latest = text(&run["last_event"]);
    // Status events arrive newest first. Render that order, without repeating
    // last_event above the same event or substituting raw goal/session JSON.
    if !latest.is_empty()
        && !events.is_some_and(|events| events.iter().any(|event| text(&event["text"]) == latest))
    {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Latest update",
            Style::default().fg(MUTED),
        )));
        lines.extend(multiline(&run["last_event"]));
    }
    let mut has_events = false;
    if let Some(events) = events {
        for event in events
            .iter()
            .take(12)
            .filter(|event| !text(&event["text"]).is_empty())
        {
            has_events = true;
            lines.push(Line::default());
            let age = event["at"]
                .as_i64()
                .map(|at| {
                    let seconds = now_ms.saturating_sub(at).max(0) / 1000;
                    if seconds < 60 {
                        format!("{seconds}s ago")
                    } else if seconds < 3600 {
                        format!("{}m ago", seconds / 60)
                    } else {
                        format!("{}h ago", seconds / 3600)
                    }
                })
                .unwrap_or_else(|| "Update".into());
            lines.push(Line::from(Span::styled(age, Style::default().fg(MUTED))));
            lines.extend(multiline(&event["text"]));
        }
    }
    if !has_events && latest.is_empty() && summary.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(if run["finished_at"].is_null() {
            "Waiting for agent activity…"
        } else {
            "No activity recorded for this attempt."
        }));
    }
    lines
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

fn page_layout(size: Rect) -> [Rect; 3] {
    Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(5),
        Constraint::Length(2),
    ])
    .areas(size)
}

fn body_layout(area: Rect) -> [Rect; 2] {
    if area.height < 10 {
        Layout::vertical([Constraint::Min(3), Constraint::Length(0)]).areas(area)
    } else if area.width >= 100 {
        Layout::horizontal([Constraint::Percentage(36), Constraint::Min(40)]).areas(area)
    } else {
        Layout::vertical([Constraint::Length(4), Constraint::Min(4)]).areas(area)
    }
}

fn scroll_limit(paragraph: &Paragraph<'_>, inner: Rect) -> u16 {
    paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16
}

/// Clamp before moving too, so one Page Up always leaves the bottom of the log.
pub fn scroll_activity(app: &mut Dashboard, size: Rect, delta: i16) {
    let area = body_layout(page_layout(size)[1])[1];
    if area.height == 0 {
        return;
    }
    let Some(run) = app
        .runs()
        .into_iter()
        .find(|run| run["id"].as_str() == app.run_id.as_deref())
    else {
        return;
    };
    let paragraph = Paragraph::new(activity(run, app.now_ms)).wrap(Wrap { trim: false });
    let limit = scroll_limit(&paragraph, block("", false).inner(area));
    app.detail_scroll = app
        .detail_scroll
        .min(limit)
        .saturating_add_signed(delta)
        .min(limit);
}

pub fn render(frame: &mut Frame, app: &Dashboard) {
    let size = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BACKGROUND).fg(TEXT)),
        size,
    );
    if size.width < MIN_WIDTH || size.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Resize to at least 48 × 12.\nCtrl+C / q: quit")
                .wrap(Wrap { trim: true }),
            size,
        );
        return;
    }
    let rows = page_layout(size);
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
            Span::styled(
                " HEY BOSS ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
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
                    .fg(BACKGROUND)
                    .bg(color(status))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  {active} busy · {} available / {slots} slots",
                slots.saturating_sub(active)
            )),
        ]));
        header.push(Line::from(Span::styled(
            format!(" {}", checkout(w, &app.snapshot)),
            Style::default().fg(MUTED),
        )));
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
                "connected" | "local" => color("AVAILABLE"),
                "disconnected" => color("offline"),
                "standalone" => MUTED,
                _ => color("unknown"),
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
            Style::default().fg(if app.history { MUTED } else { ACCENT }),
        ),
        Span::styled(
            if app.history {
                " [History] "
            } else {
                " History "
            },
            Style::default().fg(if app.history { ACCENT } else { MUTED }),
        ),
        Span::raw(" · h switch"),
    ]));
    frame.render_widget(Paragraph::new(header), rows[0]);
    let right = body_layout(rows[1]);
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
                .highlight_style(Style::default().bg(SELECTED).fg(TEXT)),
            right[0],
            &mut selection,
        );
    }
    let detail = selected
        .map(|i| activity(runs[i], app.now_ms))
        .unwrap_or_else(|| {
            vec![Line::from(
                "Select a session to inspect its latest activity.",
            )]
        });
    let activity_block = block("Activity · PgUp/PgDn", false);
    let inner = activity_block.inner(right[1]);
    let paragraph = Paragraph::new(detail).wrap(Wrap { trim: false });
    let max_scroll = scroll_limit(&paragraph, inner);
    let scroll = app.detail_scroll.min(max_scroll);
    let position = if max_scroll == 0 {
        " Latest ".to_owned()
    } else {
        format!(
            " {} · {}/{} ",
            if scroll == 0 { "Latest" } else { "Older" },
            scroll + 1,
            max_scroll + 1
        )
    };
    frame.render_widget(
        paragraph
            .scroll((scroll, 0))
            .block(activity_block.title_bottom(Line::from(position).right_aligned())),
        right[1],
    );
    let status = if let Some(error) = app.error.as_ref().or(app.diagnostic.as_ref()) {
        format!(" {}", text(&Value::String(error.clone())))
    } else if size.width < 90 && app.owned_worker {
        " Live · q / Ctrl+C stops worker + agents".into()
    } else if size.width < 90 && !app.pending {
        " Live · q exits; workers keep running".into()
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
                Style::default().fg(if app.error.is_some() || app.diagnostic.is_some() {
                    color("error")
                } else {
                    MUTED
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
        let message = if size.width < 90 {
            format!(
                "↑↓ / j k  Select session\nh         Active / history\nPgUp/PgDn Scroll activity\nr         Refresh / retry\np         Pause pickup (confirm)\ns         Stop worker + agents (confirm)\nq / Ctrl+C {}\nEsc / ?   Close help",
                if app.owned_worker {
                    "Stop worker + agents"
                } else {
                    "Exit; workers keep running"
                }
            )
        } else {
            format!(
                "↑/↓ or j/k  Select session\nh          Switch active work / completed history\nPgUp/PgDn  Scroll session activity\nr          Refresh now / retry after an error\np          Pause pickup; existing sessions finish normally\ns          Stop worker and its sessions (confirmation)\nq / Ctrl+C {}\nEsc / ?    Close help",
                if app.owned_worker {
                    "Stop this worker and its sessions"
                } else {
                    "Quit dashboard; workers keep running"
                }
            )
        };
        overlay(frame, "Keyboard", &message);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multi_project_checkout_labels_use_registered_names() {
        let local = "local:machine:/workspace/hey-gh";
        let snapshot = serde_json::json!({
            "projects": [
                {"id": "github.com/kamilio/ashby-mcp", "name": "ashby-mcp"},
                {"id": local, "name": "hey-gh"}
            ],
            "workers": [{"id": "worker", "config": {
                "projects": ["github.com/kamilio/ashby-mcp", local],
                "directories": {
                    "github.com/kamilio/ashby-mcp": "/workspace/ashby-mcp",
                    local: "/workspace/hey-gh"
                }
            }}]
        });
        let title = worker_title(&snapshot, Some("worker")).unwrap();
        assert_eq!(
            title,
            "hey-boss · ashby-mcp, hey-gh · ashby-mcp: /workspace/ashby-mcp · hey-gh: /workspace/hey-gh"
        );
        let mut loading = snapshot.clone();
        loading["projects"] = serde_json::json!([]);
        assert_eq!(worker_title(&loading, Some("worker")), Some(title));
    }
    #[test]
    fn claim_deadline_is_only_shown_when_the_agent_is_awaiting_claim() {
        for state in ["reserved", "awaiting_model", "awaiting_claim", "running"] {
            let run = serde_json::json!({
                "state": state, "finished_at": null, "claimed_at": null,
                "reservation_expires": 600_000
            });
            let content: String = activity(&run, 0)
                .iter()
                .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
                .collect();
            assert_eq!(
                content.contains("Manual claim deadline"),
                state == "awaiting_claim",
                "{state}"
            );
        }
    }
}
