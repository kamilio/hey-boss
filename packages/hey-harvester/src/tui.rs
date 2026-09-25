use crate::health::{Snapshot, Store, readable_bytes, remote};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};
use std::{
    io::{self, IsTerminal},
    sync::mpsc,
    time::{Duration, Instant},
};

struct Machine {
    host: Option<String>,
    snapshot: Option<Snapshot>,
    busy: bool,
    error: Option<String>,
    updated: Option<Instant>,
}
struct Dashboard {
    machines: Vec<Machine>,
    selected: usize,
    page: usize,
    row: usize,
    detail_scroll: Option<u16>,
    confirmation: Option<(usize, Vec<String>, String)>,
}
impl Dashboard {
    fn new(hosts: Vec<String>, selected: Option<&str>) -> Self {
        let mut all = vec![None];
        all.extend(hosts.into_iter().map(Some));
        if let Some(host) = selected.filter(|h| !all.iter().any(|v| v.as_deref() == Some(h))) {
            all.push(Some(host.to_owned()));
        }
        let selected = all
            .iter()
            .position(|v| v.as_deref() == selected)
            .unwrap_or(0);
        Self {
            machines: all
                .into_iter()
                .map(|host| Machine {
                    host,
                    snapshot: None,
                    busy: false,
                    error: None,
                    updated: None,
                })
                .collect(),
            selected,
            page: 0,
            row: 0,
            detail_scroll: None,
            confirmation: None,
        }
    }
    fn switch(&mut self, backward: bool) {
        let len = self.machines.len();
        self.selected = (self.selected + if backward { len - 1 } else { 1 }) % len;
        self.row = 0;
        self.detail_scroll = None;
    }
    fn complete(&mut self, index: usize, result: Result<Snapshot, String>) {
        let machine = &mut self.machines[index];
        machine.busy = false;
        machine.updated = Some(Instant::now());
        match result {
            Ok(snapshot) => {
                machine.snapshot = Some(snapshot);
                machine.error = None;
            }
            Err(error) => machine.error = Some(error),
        }
    }
}

type Completion = (usize, Result<Snapshot, String>);
fn request(
    dashboard: &mut Dashboard,
    index: usize,
    args: Vec<String>,
    sender: &mpsc::Sender<Completion>,
) {
    let machine = &mut dashboard.machines[index];
    if machine.busy {
        machine.error = Some(
            "A request is already running on this machine; wait before starting another.".into(),
        );
        return;
    }
    machine.busy = true;
    machine.error = None;
    let host = machine.host.clone();
    let sender = sender.clone();
    std::thread::spawn(move || {
        let result = fetch(host.as_deref(), &args).map_err(|e| e.to_string());
        let _ = sender.send((index, result));
    });
}
fn fetch(host: Option<&str>, args: &[String]) -> io::Result<Snapshot> {
    if let Some(host) = host {
        let bytes = remote::execute(host, args, remote::control_path(host))?;
        if matches!(args[0].as_str(), "enable" | "disable" | "configure") {
            return fetch(Some(host), &["status".into(), "--json".into()]);
        }
        return Ok(serde_json::from_slice(&bytes)?);
    }
    if args[0] == "status" {
        return Store::standard()?.status();
    }
    // Keep mutations in the standalone process so switching tabs never cancels work.
    let output = std::process::Command::new(crate::cli::executable()?)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    if matches!(args[0].as_str(), "enable" | "disable" | "configure") {
        return Store::standard()?.status();
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub fn run(selected: Option<&str>) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "The dashboard needs a terminal; use hey-harvester status --json for scripts",
        ));
    }
    if selected.is_some_and(|host| !remote::valid_host(host)) {
        return Err(io::Error::other("Invalid SSH host"));
    }
    let mut dashboard = Dashboard::new(remote::hosts()?, selected);
    let (sender, receiver) = mpsc::channel();
    let mut terminal = ratatui::try_init()?;
    // ratatui installs panic cleanup; this guard also restores the terminal on IO errors.
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            ratatui::restore();
        }
    }
    let _restore = Restore;
    let mut dirty = true;
    loop {
        while let Ok((index, result)) = receiver.try_recv() {
            dashboard.complete(index, result);
            dirty = true;
        }
        let index = dashboard.selected;
        let machine = &dashboard.machines[index];
        if !machine.busy
            && machine
                .updated
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
        {
            request(
                &mut dashboard,
                index,
                vec!["status".into(), "--json".into()],
                &sender,
            );
            dirty = true;
        }

        if dirty {
            terminal.draw(|frame| render(frame, &dashboard))?;
            dirty = false;
        }
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let input = event::read()?;
        let Event::Key(key) = input else {
            if matches!(input, Event::Resize(_, _)) {
                dirty = true;
            }
            continue;
        };
        dirty = true;
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            break;
        }
        if let Some((index, args, _)) = dashboard.confirmation.take() {
            if key.code == KeyCode::Char('y') {
                request(&mut dashboard, index, args, &sender);
            }
            continue;
        }
        if let Some(offset) = dashboard.detail_scroll {
            let size = terminal.size()?;
            let area = sections(Rect::new(0, 0, size.width, size.height))[2];
            let limit = detail_limit(&dashboard, area);
            let offset = offset.min(limit);
            dashboard.detail_scroll = match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Esc | KeyCode::Enter => None,
                KeyCode::Down | KeyCode::Char('j') => Some(offset.saturating_add(1).min(limit)),
                KeyCode::Up | KeyCode::Char('k') => Some(offset.saturating_sub(1)),
                KeyCode::PageDown => Some(
                    offset
                        .saturating_add(area.height.saturating_sub(2))
                        .min(limit),
                ),
                KeyCode::PageUp => Some(offset.saturating_sub(area.height.saturating_sub(2))),
                _ => Some(offset),
            };
            continue;
        }
        let mut args = None;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            KeyCode::Enter => dashboard.detail_scroll = Some(0),
            KeyCode::Tab | KeyCode::Right => dashboard.switch(false),
            KeyCode::BackTab | KeyCode::Left => dashboard.switch(true),
            KeyCode::Char(c @ '1'..='5') => {
                dashboard.page = c as usize - '1' as usize;
                dashboard.row = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                dashboard.row = dashboard
                    .row
                    .saturating_add(1)
                    .min(rows(&dashboard).len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => dashboard.row = dashboard.row.saturating_sub(1),
            KeyCode::Char('r') => args = Some(vec!["status".into(), "--json".into()]),
            KeyCode::Char('s') => args = Some(vec!["scan".into(), "--json".into()]),
            KeyCode::Char('c') => {
                dashboard.confirmation = Some((
                    index,
                    vec!["clean".into(), "--json".into()],
                    "Clean eligible candidates on this machine? y confirms; any other key cancels"
                        .into(),
                ))
            }
            KeyCode::Char('a') => {
                if let Some(s) = &dashboard.machines[index].snapshot {
                    args = Some(vec![
                        if s.config.automatic {
                            "disable"
                        } else {
                            "enable"
                        }
                        .into(),
                    ]);
                }
            }
            KeyCode::Char(c @ ('p' | 'w' | 'b' | 'l')) => {
                if let Some(s) = &dashboard.machines[index].snapshot {
                    let (flag, enabled) = match c {
                        'p' => ("--processes", s.config.harvest_processes),
                        'w' => ("--worktrees", s.config.clean_worktrees),
                        'b' => ("--caches", s.config.clean_caches),
                        _ => ("--logs", s.config.trim_worker_logs),
                    };
                    args = Some(vec![
                        "configure".into(),
                        flag.into(),
                        (!enabled).to_string(),
                    ]);
                }
            }
            KeyCode::Char('x') if dashboard.page == 2 => {
                if let Some(item) = dashboard.machines[index]
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.worktrees.get(dashboard.row))
                    .and_then(|item| item.worktree.as_ref())
                {
                    let path = item.path.to_string_lossy().into_owned();
                    dashboard.confirmation = Some((
                        index,
                        vec!["remove-worktree".into(), path.clone(), "--json".into()],
                        format!(
                            "Remove {path}? Safety checks still apply. y confirms; any other key cancels"
                        ),
                    ));
                }
            }
            _ => {}
        }
        if let Some(args) = args {
            request(&mut dashboard, index, args, &sender);
        }
    }
    Ok(())
}

fn rows(d: &Dashboard) -> Vec<String> {
    let Some(s) = &d.machines[d.selected].snapshot else {
        return vec!["Waiting for machine status…".into()];
    };
    let items = |items: &[crate::health::Item]| {
        items
            .iter()
            .map(|i| {
                format!(
                    "{} {} — {}",
                    if i.error.is_some() {
                        "failed"
                    } else if i.eligible {
                        "eligible"
                    } else {
                        "preserved"
                    },
                    i.name,
                    i.detail
                )
            })
            .collect::<Vec<_>>()
    };
    match d.page {
        1 => {
            let mut rows = s
                .process_inventory
                .as_ref()
                .map(|ps| {
                    ps.iter()
                        .map(|p| {
                            format!(
                                "PID {}  {:>6.1}% CPU  {:>10}  {}",
                                p.pid,
                                p.cpu_percent,
                                readable_bytes(p.resident_bytes),
                                p.executable
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            rows.extend(items(&s.processes));
            rows
        }
        2 => items(&s.worktrees),
        3 => items(&s.caches),
        4 => s
            .activity
            .iter()
            .rev()
            .map(|a| format!("{}  {}  {}", a.at, a.category, a.message))
            .collect(),
        _ => {
            let bytes = |v: Option<u64>| {
                v.map(readable_bytes)
                    .unwrap_or_else(|| "unavailable".into())
            };
            let mut rows = vec![
                format!(
                    "Disk: {} available / {}",
                    bytes(s.metrics.disk_available_bytes),
                    bytes(s.metrics.disk_total_bytes)
                ),
                format!(
                    "Memory: {} available / {} · {} · {} swap",
                    bytes(s.metrics.memory_available_bytes),
                    bytes(s.metrics.memory_total_bytes),
                    s.metrics.memory_pressure,
                    bytes(s.metrics.swap_used_bytes)
                ),
                format!(
                    "Automatic: {} · interval {}s · aggressive: {}",
                    s.config.automatic, s.config.interval_seconds, s.config.aggressive
                ),
                format!(
                    "Processes: {}   Worktrees: {}   Caches: {}   Log trimming: {}",
                    s.config.harvest_processes,
                    s.config.clean_worktrees,
                    s.config.clean_caches,
                    s.config.trim_worker_logs
                ),
                format!(
                    "Last cleanup: {:?} · stopped {} processes · removed {} worktrees · removed {} caches · trimmed {} logs",
                    s.last_cleanup_at,
                    s.harvested_processes,
                    s.removed_worktrees,
                    s.removed_caches,
                    s.trimmed_logs
                ),
                format!(
                    "Last scan: {} · {}",
                    s.observed_at,
                    if s.phase.is_empty() {
                        "idle"
                    } else {
                        s.phase.as_str()
                    }
                ),
                format!("Inspection errors: {}", s.errors.len()),
                format!(
                    "Cache sweep: {} entries / {} ms; {} roots pending; last complete {}",
                    s.cache_progress.visited_this_cycle,
                    s.cache_progress.slice_millis,
                    s.cache_progress.roots_pending,
                    s.cache_progress.last_completion()
                ),
            ];
            rows.extend(
                s.config
                    .workspace_roots
                    .iter()
                    .map(|p| format!("Workspace: {}", p.display())),
            );
            rows.extend(s.errors.iter().map(|e| format!("Error: {e}")));
            rows
        }
    }
}
fn sections(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(4),
    ])
    .split(area)
}
fn detail(d: &Dashboard) -> Paragraph<'static> {
    Paragraph::new(
        rows(d)
            .get(d.row)
            .cloned()
            .unwrap_or_else(|| "No selected entry.".into()),
    )
    .wrap(Wrap { trim: false })
}
fn detail_limit(d: &Dashboard, area: Rect) -> u16 {
    detail(d)
        .line_count(area.width.saturating_sub(2))
        .saturating_sub(usize::from(area.height.saturating_sub(2)))
        .min(usize::from(u16::MAX)) as u16
}
fn render(frame: &mut ratatui::Frame, d: &Dashboard) {
    let chunks = sections(frame.area());
    let titles = d
        .machines
        .iter()
        .map(|m| {
            format!(
                "{}{}",
                m.host.as_deref().unwrap_or("local"),
                if m.busy {
                    " …"
                } else if m.error.is_some() {
                    " !"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Tabs::new(titles)
            .select(d.selected)
            .highlight_style(Style::default().fg(Color::Cyan))
            .block(Block::bordered().title("hey-harvester · Tab / Shift-Tab switches machines")),
        chunks[0],
    );
    frame.render_widget(
        Tabs::new([
            "1 Overview",
            "2 Processes",
            "3 Worktrees",
            "4 Caches",
            "5 Activity",
        ])
        .select(d.page)
        .highlight_style(Style::default().fg(Color::Cyan))
        .block(Block::bordered()),
        chunks[1],
    );
    if let Some(offset) = d.detail_scroll {
        frame.render_widget(
            detail(d)
                .scroll((offset.min(detail_limit(d, chunks[2])), 0))
                .block(Block::bordered().title("Selected entry")),
            chunks[2],
        );
    } else {
        let values = rows(d);
        let mut state =
            ListState::default().with_selected(Some(d.row.min(values.len().saturating_sub(1))));
        frame.render_stateful_widget(
            List::new(values.into_iter().map(ListItem::new))
                .highlight_style(Style::default().bg(Color::DarkGray))
                .block(Block::bordered()),
            chunks[2],
            &mut state,
        );
    }
    let status = if d.detail_scroll.is_some() {
        "↑/↓ or PgUp/PgDn Scroll details · Esc Back · q Quit"
    } else {
        d.confirmation.as_ref().map(|(_, _, text)| text.as_str()).or(d.machines[d.selected].error.as_deref()).unwrap_or("Enter Details · r Refresh · s Scan · c Clean · a Automatic · x Remove worktree\np Processes · w Worktrees · b Caches · l Log trimming · ↑/↓ Scroll · q Quit")
    };
    frame.render_widget(
        Paragraph::new(status)
            .wrap(Wrap { trim: false })
            .block(Block::bordered()),
        chunks[3],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tabs_wrap_and_respect_selected_machine() {
        let mut d = Dashboard::new(vec!["devbox".into(), "mac".into()], Some("devbox"));
        assert_eq!(d.selected, 1);
        d.switch(true);
        assert_eq!(d.selected, 0);
        d.switch(true);
        assert_eq!(d.selected, 2);
        d.switch(false);
        assert_eq!(d.selected, 0);
    }
    #[test]
    fn late_response_updates_original_machine_after_tab_switch() {
        let mut d = Dashboard::new(vec!["devbox".into()], None);
        d.machines[0].busy = true;
        d.switch(false);
        d.complete(0, Ok(Snapshot::default()));
        assert_eq!(d.selected, 1);
        assert!(d.machines[0].snapshot.is_some());
        assert!(!d.machines[0].busy);
        assert!(d.machines[1].snapshot.is_none());
        d.complete(1, Err("offline".into()));
        assert_eq!(d.machines[1].error.as_deref(), Some("offline"));
        assert!(d.machines[0].error.is_none());
    }
    #[test]
    fn dashboard_renders_machine_tabs_and_cleanup_settings() {
        let mut d = Dashboard::new(vec!["devbox".into()], None);
        d.complete(0, Ok(Snapshot::default()));
        let backend = ratatui::backend::TestBackend::new(120, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &d)).unwrap();
        let rendered = format!("{:?}", terminal.backend().buffer());
        assert!(rendered.contains("devbox"));
        assert!(rendered.contains("Automatic: false"));
        assert!(rendered.contains("Tab / Shift-Tab"));
    }

    #[test]
    fn selected_worktree_details_wrap_and_scroll_without_losing_ownership() {
        let mut d = Dashboard::new(vec![], None);
        let mut snapshot = Snapshot::default();
        snapshot.worktrees.push(crate::health::Item {
            name: "/Users/example/Workspace/very-long-project-name/active-issue-worktree".into(),
            detail: "Locked worktree; preserved — issue 147; owner fixture-session; queued validation; retain staged changes and receipts".into(),
            eligible: false,
            worktree: None, error: None,
        });
        d.complete(0, Ok(snapshot));
        d.page = 2;
        d.detail_scroll = Some(0);
        for (width, height) in [(120, 24), (80, 24), (48, 20)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| render(f, &d)).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            assert!(
                text.contains("fixture-session"),
                "Ownership missing at {width}x{height}: {text}"
            );
            assert!(
                text.contains("receipts"),
                "Preservation detail missing at {width}x{height}"
            );
            assert!(text.contains("Esc Back"));
        }
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(48, 14)).unwrap();
        d.detail_scroll = Some(4);
        terminal.draw(|f| render(f, &d)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(
            text.contains("receipts"),
            "The last lines must remain reachable by scrolling"
        );
    }
}
