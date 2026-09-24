//! A contiguous viewport over a fixed tab order. Selection never reorders tabs.
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

fn fit_label(label: String, width: usize) -> String {
    let span = Span::raw(label);
    if span.width() <= width {
        return span.content.into_owned();
    }
    let mut result = String::new();
    let mut used = 0;
    // Preserve graphemes (including combining marks and emoji sequences), and
    // measure terminal columns rather than bytes or Unicode scalar values.
    for grapheme in span.styled_graphemes(Style::default()) {
        let columns = Span::raw(grapheme.symbol).width();
        if used + columns > width.saturating_sub(1) {
            break;
        }
        result.push_str(grapheme.symbol);
        used += columns;
    }
    result.push('…');
    result
}

pub(super) fn render(tabs: Vec<(String, bool)>, width: u16) -> Line<'static> {
    if tabs.is_empty() {
        return Line::from(" No configured projects");
    }
    // Reserve two columns on either side for overflow cues. Every tab keeps
    // its bracket columns even when inactive, so changing focus cannot shift it.
    let budget = usize::from(width).saturating_sub(4);
    let tabs: Vec<_> = tabs
        .into_iter()
        .map(|(label, selected)| {
            let label = fit_label(label, budget.saturating_sub(2));
            let width = Span::raw(&label).width() + 2;
            (label, selected, width)
        })
        .collect();
    let selected = tabs.iter().position(|tab| tab.1).unwrap_or(0);
    let mut start = 0;
    let mut used = tabs[..=selected].iter().map(|tab| tab.2).sum::<usize>() + selected * 2;
    while used > budget && start < selected {
        used -= tabs[start].2 + 2;
        start += 1;
    }
    let mut end = selected + 1;
    while end < tabs.len() && used + 2 + tabs[end].2 <= budget {
        used += 2 + tabs[end].2;
        end += 1;
    }
    let mut spans = vec![Span::styled(
        if start > 0 { "‹ " } else { "  " },
        Style::default().fg(super::MUTED),
    )];
    for (index, (label, selected, _)) in tabs[start..end].iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(if *selected {
            Span::styled(
                format!("[{label}]"),
                Style::default()
                    .fg(super::ACCENT)
                    .bg(super::SELECTED)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw(format!(" {label} "))
        });
    }
    spans.push(Span::raw(" ".repeat(budget.saturating_sub(used))));
    spans.push(Span::styled(
        if end < tabs.len() { " ›" } else { "  " },
        Style::default().fg(super::MUTED),
    ));
    Line::from(spans)
}
