use clap::Parser;
use crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use hey_boss_worker_tui::{
    Dashboard,
    backend::{Client, Request},
    ui,
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Parser)]
#[command(about = "Live hey-boss worker dashboard. Exiting leaves workers running.")]
struct Options {
    /// hey-boss executable supplying worker status JSON.
    #[arg(long, default_value = "hey-boss")]
    binary: PathBuf,
    /// Authoritative SSH queue host, forwarded to hey-boss.
    #[arg(long)]
    host: Option<String>,
    /// Checkout directory used to resolve the queue project.
    #[arg(long)]
    directory: Option<PathBuf>,
    /// Initially selected worker.
    #[arg(long)]
    id: Option<String>,
    /// Show recent attempts on startup.
    #[arg(long)]
    history: bool,
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

fn main() {
    let options = Options::parse();
    if let Err(error) = run(options) {
        eprintln!("hey-boss-worker-tui: {error}");
        std::process::exit(1);
    }
}

fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(
            "An interactive terminal is required. For scripts use: hey-boss worker --json status"
                .into(),
        );
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        hook(info);
    }));
    let guard = TerminalGuard;
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    let client = Client {
        binary: options.binary,
        host: options.host,
        directory: options.directory,
        timeout: Duration::from_secs(10),
    };
    let (requests, rx) = mpsc::channel::<Request>();
    let (tx, results) = mpsc::channel();
    let background_cancel = cancelled.clone();
    let background = thread::spawn(move || {
        while let Ok(request) = rx.recv() {
            if background_cancel.load(Ordering::Relaxed) {
                break;
            }
            let result = client.execute(&request, &background_cancel);
            if tx.send((request, result)).is_err() {
                break;
            }
        }
    });
    let mut app = Dashboard {
        worker_id: options.id,
        history: options.history,
        ..Dashboard::default()
    };
    let result = event_loop(&mut terminal, &mut app, &requests, &results, &cancelled);
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
) -> Result<(), Box<dyn std::error::Error>> {
    let mut dirty = true;
    let mut next_refresh = Instant::now();
    let mut next_tick = Instant::now();
    let mut recover_inventory = false;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            break;
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
                    recover_inventory = matches!(request, Request::Refresh(Some(_)));
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
            break;
        }
        if key.code == KeyCode::Char('q') {
            break;
        }
        let size = terminal.size()?;
        if size.width < ui::MIN_WIDTH || size.height < ui::MIN_HEIGHT {
            if key.code == KeyCode::Char('q') {
                break;
            }
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
                KeyCode::Char('q') => break,
                _ => {}
            }
            continue;
        }
        let old_worker = app.worker_id.clone();
        match key.code {
            KeyCode::Char('q') => break,
            KeyCode::Up | KeyCode::Char('k') => app.navigate(-1),
            KeyCode::Down | KeyCode::Char('j') => app.navigate(1),
            KeyCode::Tab | KeyCode::BackTab => app.sessions_focused = !app.sessions_focused,
            KeyCode::Char('h') => {
                app.history = !app.history;
                app.normalize_run();
            }
            KeyCode::Char('r') => next_refresh = Instant::now(),
            KeyCode::Char('?') => app.help = true,
            KeyCode::Char('p') => app.confirm(false),
            KeyCode::Char('s') => app.confirm(true),
            KeyCode::PageDown => app.detail_scroll = app.detail_scroll.saturating_add(5),
            KeyCode::PageUp => app.detail_scroll = app.detail_scroll.saturating_sub(5),
            _ => {}
        }
        if app.worker_id != old_worker {
            next_refresh = Instant::now();
        }
    }
    Ok(())
}
