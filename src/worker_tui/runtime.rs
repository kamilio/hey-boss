use super::{
    Dashboard,
    backend::{Client, Request},
    ui,
};
use crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::{self, IsTerminal},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Runtime options for standalone dashboards and embedded workers.
pub struct Options {
    pub client: Client,
    pub id: Option<String>,
    pub history: bool,
    /// Quitting an embedded dashboard stops its owning worker's sessions.
    pub owned_worker: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Quit,
    Stopped,
}

struct TerminalGuard;
fn restore() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    );
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

pub fn run(
    options: Options,
    cancelled: Arc<AtomicBool>,
) -> Result<Exit, Box<dyn std::error::Error>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(
            "An interactive terminal is required. For scripts use: hey-boss worker --json status"
                .into(),
        );
    }
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        hook(info);
    }));
    let diagnostics = super::diagnostics::Capture::start();
    let guard = TerminalGuard;
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    let client = options.client;
    let (requests, rx) = mpsc::channel::<Request>();
    let (tx, results) = mpsc::channel();
    let background_cancel = cancelled.clone();
    let background = thread::spawn(move || {
        let mut last_title = None;
        while let Ok(request) = rx.recv() {
            if background_cancel.load(Ordering::Relaxed) {
                break;
            }
            let result = client.execute(&request, &background_cancel);
            if let (Request::Refresh(_), Ok(snapshot)) = (&request, &result) {
                let title = ui::worker_title(snapshot, snapshot["worker_id"].as_str());
                if title != last_title {
                    if let Some(title) = &title {
                        super::terminal_name::set(title);
                    }
                    last_title = title;
                }
            }
            if tx.send((request, result)).is_err() {
                break;
            }
        }
    });
    let mut app = Dashboard {
        worker_id: options.id,
        history: options.history,
        owned_worker: options.owned_worker,
        ..Dashboard::default()
    };
    let result = event_loop(
        &mut terminal,
        &mut app,
        &requests,
        &results,
        &cancelled,
        &diagnostics,
    );
    cancelled.store(true, Ordering::Relaxed);
    drop(requests);
    let _ = background.join();
    drop(guard);
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut Dashboard,
    requests: &mpsc::Sender<Request>,
    results: &mpsc::Receiver<(Request, Result<serde_json::Value, String>)>,
    cancelled: &AtomicBool,
    diagnostics: &super::diagnostics::Capture,
) -> Result<Exit, Box<dyn std::error::Error>> {
    let mut dirty = true;
    let mut next_refresh = Instant::now();
    let mut next_tick = Instant::now();
    let mut recover_inventory = false;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(Exit::Stopped);
        }
        let diagnostic = diagnostics.latest();
        if diagnostic != app.diagnostic {
            app.diagnostic = diagnostic;
            dirty = true;
        }
        while let Ok((request, result)) = results.try_recv() {
            app.pending = false;
            match result {
                Ok(value) => match request {
                    Request::Refresh(id) => {
                        if id == app.worker_id || id.is_none() {
                            app.apply(value);
                            next_refresh = Instant::now() + Duration::from_secs(2);
                        } else {
                            next_refresh = Instant::now();
                        }
                    }
                    Request::Control { .. } => {
                        next_refresh = Instant::now();
                    }
                },
                Err(error) => {
                    app.error = Some(error);
                    // Recover the inventory if the selected worker disappeared.
                    // Keep the last good snapshot visible during ordinary outages.
                    recover_inventory =
                        !app.owned_worker && matches!(request, Request::Refresh(Some(_)));
                    next_refresh = Instant::now() + Duration::from_secs(2);
                }
            }
            dirty = true;
        }
        if !app.pending && Instant::now() >= next_refresh {
            let id = if recover_inventory {
                recover_inventory = false;
                None
            } else {
                app.worker_id.clone()
            };
            requests.send(Request::Refresh(id))?;
            app.pending = true;
            dirty = true;
        }
        if Instant::now() >= next_tick {
            app.now_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
            next_tick = Instant::now() + Duration::from_secs(1);
            dirty |= app.runs().iter().any(|run| run["finished_at"].is_null());
        }
        if dirty {
            terminal.draw(|frame| ui::render(frame, app))?;
            dirty = false;
        }
        if !event::poll(Duration::from_millis(30))? {
            continue;
        }
        let event = event::read()?;
        if matches!(event, Event::Resize(_, _)) {
            dirty = true;
            continue;
        }
        let Event::Key(key) = event else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(Exit::Quit);
        }
        if key.code == KeyCode::Char('q') {
            return Ok(Exit::Quit);
        }
        let size = terminal.size()?;
        if size.width < ui::MIN_WIDTH || size.height < ui::MIN_HEIGHT {
            continue;
        }
        dirty = true;
        if app.confirmation.is_some() {
            match key.code {
                KeyCode::Esc => app.confirmation = None,
                KeyCode::Enter if !app.pending => {
                    let c = app.confirmation.take().unwrap();
                    requests.send(Request::Control {
                        worker_id: c.worker_id,
                        stop: c.stop,
                    })?;
                    app.pending = true;
                }
                _ => {}
            }
            continue;
        }
        if app.help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') => app.help = false,
                KeyCode::Char('q') => return Ok(Exit::Quit),
                _ => {}
            }
            continue;
        }
        match key.code {
            KeyCode::Char('q') => return Ok(Exit::Quit),
            KeyCode::Up | KeyCode::Char('k') => app.navigate(-1),
            KeyCode::Down | KeyCode::Char('j') => app.navigate(1),
            KeyCode::Char('h') => {
                app.history = !app.history;
                app.normalize_run();
            }
            KeyCode::Char('r') => next_refresh = Instant::now(),
            KeyCode::Char('?') => app.help = true,
            KeyCode::Char('p') => app.confirm(false),
            KeyCode::Char('s') => app.confirm(true),
            KeyCode::PageDown => ui::scroll_activity(
                app,
                ratatui::layout::Rect::new(0, 0, size.width, size.height),
                5,
            ),
            KeyCode::PageUp => ui::scroll_activity(
                app,
                ratatui::layout::Rect::new(0, 0, size.width, size.height),
                -5,
            ),
            _ => {}
        }
    }
}
