//! The project list stays put while the reader follows the selected commit.
use super::*;
use crossterm::event::{self, Event, KeyCode as K, KeyModifiers as M};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint as C, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

const SKY: Color = Color::Rgb(113, 210, 224);
const MUTED: Color = Color::Rgb(144, 159, 179);
const SELECT: Color = Color::Rgb(40, 65, 83);
fn style(c: Color) -> Style {
    Style::default().fg(c)
}
fn panel(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(style(if focused { SKY } else { Color::DarkGray }))
}
struct Row {
    text: String,
    color: Color,
    location: Option<Value>,
}
fn row(text: &str, color: Color) -> Row {
    Row {
        text: clean(text),
        color,
        location: None,
    }
}
fn lines(v: &Value) -> Result<Vec<Row>> {
    let r = &v["review"];
    let mut rows = vec![
        row(
            field(r, "message")?.lines().next().unwrap_or("Commit"),
            Color::White,
        ),
        row(
            &format!("{}   {}", field(r, "author")?, &field(v, "sha")?[..10]),
            MUTED,
        ),
    ];
    for s in field(r, "message")?.lines().skip(1) {
        rows.push(row(s, Color::White));
    }
    let comments = v["comments"].as_array().ok_or("Missing review comments")?;
    rows.push(row(
        &format!(
            "{} review comment{}",
            comments.len(),
            if comments.len() == 1 { "" } else { "s" }
        ),
        SKY,
    ));
    for c in comments {
        rows.push(row(
            &format!(
                "  {}:{}  ·  {}",
                field(&c["location"], "file")?,
                c["location"]["line"],
                field(c, "author")?
            ),
            MUTED,
        ));
        for s in field(c, "body")?.lines() {
            rows.push(row(&format!("  {s}"), Color::Yellow));
        }
    }
    let (mut old, mut new) = (0u32, 0u32);
    let (mut a, mut b) = (String::new(), String::new());
    for s in field(r, "patch")?.lines() {
        if s.starts_with("index ") {
            continue;
        }
        let mut loc = None;
        let color = if s.starts_with("diff --git ") {
            old = 0;
            new = 0;
            a.clear();
            b.clear();
            rows.push(row("", MUTED));
            SKY
        } else if old == 0 && new == 0 && (s.starts_with("--- ") || s.starts_with("+++ ")) {
            let path = s[4..].trim_end_matches('\t');
            if s.starts_with("--- ") {
                a = path.strip_prefix("a/").unwrap_or("").into()
            } else {
                b = path.strip_prefix("b/").unwrap_or("").into()
            }
            MUTED
        } else if s.starts_with("@@ ") {
            let mut words = s.split_whitespace().skip(1);
            let mut next = || {
                words
                    .next()
                    .and_then(|s| s.get(1..))
                    .and_then(|s| s.split(',').next())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0)
            };
            old = next();
            new = next();
            Color::Cyan
        } else if old > 0 || new > 0 {
            let (path, n, side, col) = match s.as_bytes().first() {
                Some(b'-') => {
                    let n = old;
                    old += 1;
                    (&a, n, "old", Color::LightRed)
                }
                Some(b'+') => {
                    let n = new;
                    new += 1;
                    (&b, n, "new", Color::LightGreen)
                }
                Some(b' ') => {
                    let n = new;
                    new += 1;
                    old += 1;
                    (&b, n, "new", Color::White)
                }
                _ => {
                    rows.push(row(s, MUTED));
                    continue;
                }
            };
            if !path.is_empty() {
                loc = Some(json!({"file":path,"side":side,"line":n}));
            }
            col
        } else {
            MUTED
        };
        if s.starts_with("--- ") && loc.is_none() || s.starts_with("+++ ") && loc.is_none() {
            continue;
        }
        let label = if s.starts_with("diff --git ") {
            format!(
                "File  {}",
                s.rsplit_once(" b/").map(|(_, path)| path).unwrap_or(s)
            )
        } else {
            s.into()
        };
        let mut line = row(&label, color);
        line.location = loc;
        rows.push(line);
    }
    Ok(rows)
}
#[derive(Clone, Copy)]
enum Prompt {
    Stash,
    Restore,
    Link,
    Comment,
}
struct View {
    entries: Vec<Value>,
    index: usize,
    project: String,
    sha: String,
    rows: Vec<Row>,
    cursor: usize,
    horizontal: u16,
    branch: String,
    head: String,
    detail: bool,
    message: String,
    prompt: Option<Prompt>,
    input: String,
}
impl View {
    fn load(&mut self) -> Result<()> {
        self.rows.clear();
        self.sha.clear();
        self.cursor = 0;
        self.horizontal = 0;
        if let Some(e) = self.entries.get(self.index) {
            let sha = field(e, "sha")?;
            let value = if e["active"] == true {
                checkout::preview(sha)?
            } else {
                open(sha)?
            };
            self.rows = lines(&value)?;
            self.sha = field(&value, "sha")?.into();
        }
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        self.head = checkout::head().unwrap_or_default();
        self.branch = checkout::branch()?;
        let v = rpc(json!({"command":"list"}))?;
        self.project = clean(field(&v["project"], "name")?);
        self.entries = checkout::active(&self.head)?;
        self.entries.extend(
            v["entries"]
                .as_array()
                .ok_or("Missing entries")?
                .iter()
                .cloned(),
        );
        self.index = self
            .entries
            .iter()
            .position(|e| e["sha"] == self.sha && e["active"] != true)
            .unwrap_or(0);
        self.load()
    }
    fn select(&mut self, delta: isize) -> Result<()> {
        if self.detail {
            self.cursor = self
                .cursor
                .saturating_add_signed(delta)
                .min(self.rows.len().saturating_sub(1));
        } else {
            let i = self
                .index
                .saturating_add_signed(delta)
                .min(self.entries.len().saturating_sub(1));
            if i != self.index {
                self.index = i;
                self.load()?;
            }
        }
        Ok(())
    }
    fn begin(&mut self, p: Prompt) {
        self.prompt = Some(p);
        self.input.clear();
        self.message.clear();
    }
    fn submit(&mut self, p: Prompt) -> Result<()> {
        let text = self.input.trim();
        let msg = match p {
            Prompt::Stash => {
                if checkout::branch()? != self.branch {
                    return Err(
                        "Branch changed; reopen the shelf and select the commit again".into(),
                    );
                }
                checkout::stash_selected(
                    &self.sha,
                    &self.head,
                    if text.is_empty() {
                        None
                    } else {
                        Some(text.parse()?)
                    },
                )?
            }
            Prompt::Restore => checkout::unstash(&self.sha)?,
            Prompt::Link => {
                let n: i64 = text.parse()?;
                rpc(json!({"command":"link","sha":self.sha,"task":n}))?;
                format!("Linked to #{n}")
            }
            Prompt::Comment => {
                let loc = self
                    .rows
                    .get(self.cursor)
                    .and_then(|r| r.location.as_ref())
                    .ok_or("Select a changed or context line in the diff")?;
                rpc(json!({"command":"comment","sha":self.sha,"location":loc,"body":text}))?;
                "Comment saved".into()
            }
        };
        self.refresh()?;
        self.message = msg;
        Ok(())
    }
    fn draw(&self, f: &mut Frame) {
        let area = f.area();
        let narrow = area.width < 77;
        let height = if self.prompt.is_some() {
            4
        } else if !self.message.is_empty() {
            3
        } else if area.width < 90 {
            2
        } else {
            1
        };
        let [head, body, foot] =
            Layout::vertical([C::Length(3), C::Min(1), C::Length(height)]).areas(area);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("  SHELF  ", style(SKY).add_modifier(Modifier::BOLD)),
                    Span::styled(self.project.clone(), style(Color::White)),
                    Span::styled(
                        format!(
                            "   /   {} active · {} saved",
                            self.entries.iter().filter(|e| e["active"] == true).count(),
                            self.entries.iter().filter(|e| e["active"] != true).count()
                        ),
                        style(MUTED),
                    ),
                ]),
                Line::styled(
                    format!(
                        "  {}",
                        if self.branch.is_empty() {
                            "detached HEAD"
                        } else {
                            &self.branch
                        }
                    ),
                    style(MUTED),
                ),
            ]),
            head,
        );
        let (left, right) = if narrow {
            (
                if self.detail { None } else { Some(body) },
                if self.detail { Some(body) } else { None },
            )
        } else {
            let [l, r] = Layout::horizontal([C::Percentage(32), C::Percentage(68)]).areas(body);
            (Some(l), Some(r))
        };
        if let Some(left) = left {
            let block = panel("Commits", !self.detail);
            let inner = block.inner(left);
            f.render_widget(block, left);
            if self.entries.is_empty() {
                f.render_widget(
                    Paragraph::new("No active or saved commits.")
                        .wrap(Wrap { trim: false })
                        .style(style(MUTED)),
                    inner,
                );
            } else {
                let cap = (inner.height as usize / 2).max(1);
                let start = self.index.saturating_sub(cap - 1);
                let items = self.entries.iter().skip(start).take(cap).map(|e| {
                    let task = e["task"]
                        .as_i64()
                        .filter(|n| *n > 0)
                        .map(|n| format!("   #{n}"))
                        .unwrap_or_default();
                    let sha = e["sha"].as_str().unwrap_or_default();
                    ListItem::new(vec![
                        Line::raw(clean(e["title"].as_str().unwrap_or(""))),
                        Line::styled(
                            format!(
                                "{}  {}{}",
                                if e["active"] == true {
                                    "Active"
                                } else {
                                    "Saved"
                                },
                                sha.get(..10).unwrap_or(sha),
                                task
                            ),
                            style(MUTED),
                        ),
                    ])
                });
                let mut state = ListState::default().with_selected(Some(self.index - start));
                f.render_stateful_widget(
                    List::new(items)
                        .highlight_symbol("▸ ")
                        .highlight_style(Style::default().bg(SELECT).fg(Color::White)),
                    inner,
                    &mut state,
                );
            }
        }
        if let Some(right) = right {
            let block = panel("Review", self.detail);
            let inner = block.inner(right);
            f.render_widget(block, right);
            if self.rows.is_empty() {
                f.render_widget(
                    Paragraph::new("Select a commit to review its changes.")
                        .wrap(Wrap { trim: false })
                        .style(style(MUTED)),
                    inner,
                );
            } else {
                let start = self
                    .cursor
                    .saturating_sub((inner.height as usize).saturating_sub(1));
                let rows = self
                    .rows
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(inner.height.into())
                    .map(|(i, r)| {
                        Line::styled(
                            r.text.as_str(),
                            if i == self.cursor && self.detail {
                                style(r.color).bg(SELECT)
                            } else {
                                style(r.color)
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                f.render_widget(Paragraph::new(rows).scroll((0, self.horizontal)), inner);
            }
        }
        let active = self
            .entries
            .get(self.index)
            .is_some_and(|e| e["active"] == true);
        let hint = if active && self.detail {
            "↑↓ lines  ←→ pan  Ctrl+S stash-commit  Esc back"
        } else if active {
            "↑↓ select  Enter review  Ctrl+S stash-commit  Esc quit"
        } else if self.detail {
            "↑↓ lines  ←→ pan  Tab shelf  Ctrl+C comment  Esc back"
        } else {
            "↑↓ select  Enter review  Ctrl+U apply-commit  Ctrl+L link  Esc quit"
        };
        if let Some(p) = self.prompt {
            let label = match p {
                Prompt::Stash => "Stash commit · task # (optional, recommended)",
                Prompt::Restore => "Apply selected commit here (cherry-pick)?",
                Prompt::Link => "Link this review to task #",
                Prompt::Comment => "Comment on the selected line",
            };
            f.render_widget(
                Paragraph::new(vec![
                    Line::styled(label, style(SKY)),
                    Line::raw(format!("› {}▏", self.input)),
                    Line::styled("Enter confirm   Esc cancel", style(MUTED)),
                ]),
                foot,
            );
        } else if !self.message.is_empty() {
            f.render_widget(
                Paragraph::new(self.message.as_str())
                    .wrap(Wrap { trim: false })
                    .style(style(Color::Yellow)),
                foot,
            );
        } else {
            f.render_widget(
                Paragraph::new(hint)
                    .wrap(Wrap { trim: false })
                    .style(style(MUTED)),
                foot,
            );
        }
    }
}
fn session(t: &mut DefaultTerminal, initial: Option<&str>) -> Result<()> {
    let mut v = View {
        entries: vec![],
        index: 0,
        project: String::new(),
        branch: String::new(),
        head: String::new(),
        sha: String::new(),
        rows: vec![],
        cursor: 0,
        horizontal: 0,
        detail: initial.is_some(),
        message: String::new(),
        prompt: None,
        input: String::new(),
    };
    t.draw(|f| {
        f.render_widget(
            Paragraph::new("  Loading project shelf…").style(style(SKY)),
            f.area(),
        )
    })?;
    if let Some(sha) = initial {
        v.sha = field(&open(sha)?, "sha")?.into();
    }
    v.refresh()?;
    loop {
        t.draw(|f| v.draw(f))?;
        let Event::Key(k) = event::read()? else {
            continue;
        };
        if k.kind == event::KeyEventKind::Release {
            continue;
        }
        v.message.clear();
        if let Some(p) = v.prompt {
            match k.code {
                K::Esc => v.prompt = None,
                K::Backspace => {
                    v.input.pop();
                }
                K::Enter => {
                    v.prompt = None;
                    v.message = "Working…".into();
                    t.draw(|f| v.draw(f))?;
                    if let Err(e) = v.submit(p) {
                        v.message = e.to_string();
                    }
                }
                K::Char(c) if !c.is_control() => v.input.push(c),
                _ => {}
            }
            continue;
        }
        let active = v.entries.get(v.index).is_some_and(|e| e["active"] == true);
        let result = match (k.code, k.modifiers.contains(M::CONTROL)) {
            (K::Esc, _) if v.detail && initial.is_none() => {
                v.detail = false;
                Ok(())
            }
            (K::Esc, _) | (K::Char('q'), _) => break,
            (K::Tab, _) | (K::Enter, _) => {
                v.detail = !v.detail && !v.entries.is_empty();
                Ok(())
            }
            (K::Down, _) => v.select(1),
            (K::Up, _) => v.select(-1),
            (K::PageDown, _) => v.select(12),
            (K::PageUp, _) => v.select(-12),
            (K::Right, _) => {
                v.horizontal = v.horizontal.saturating_add(8);
                Ok(())
            }
            (K::Left, _) => {
                v.horizontal = v.horizontal.saturating_sub(8);
                Ok(())
            }
            (K::Char('s'), true) if active && !v.sha.is_empty() => {
                v.begin(Prompt::Stash);
                Ok(())
            }
            (K::Char('u'), true) if !active && !v.sha.is_empty() => {
                v.begin(Prompt::Restore);
                Ok(())
            }
            (K::Char('l'), true) if !active && !v.sha.is_empty() => {
                v.begin(Prompt::Link);
                Ok(())
            }
            (K::Char('c'), true) if v.detail => {
                if active {
                    v.message = "Shelve this commit before adding review comments.".into();
                } else if v.rows.get(v.cursor).is_some_and(|r| r.location.is_some()) {
                    v.begin(Prompt::Comment);
                } else {
                    v.message = "Select a changed or context line to comment.".into();
                }
                Ok(())
            }
            (K::Char('c'), true) => break,
            _ => Ok(()),
        };
        if let Err(e) = result {
            v.message = e.to_string();
        }
    }
    Ok(())
}
fn run(initial: Option<&str>) -> Result<()> {
    let mut term = ratatui::init();
    let result = session(&mut term, initial);
    ratatui::restore();
    result
}
pub fn browse() -> Result<()> {
    run(None)
}
pub fn inspect(sha: &str) -> Result<()> {
    run(Some(sha))
}
