#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Title,
    Description,
    Comment,
}

impl Field {
    pub(super) fn weight(self) -> f32 {
        match self {
            Self::Title => 5.0,
            Self::Description => 2.0,
            Self::Comment => 0.5,
        }
    }
}

impl Field {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Description => "description",
            Self::Comment => "comment",
        }
    }
    pub(super) fn parse(s: &str) -> Self {
        match s {
            "title" => Self::Title,
            "description" => Self::Description,
            _ => Self::Comment,
        }
    }
    pub(super) fn index(self) -> usize {
        match self {
            Self::Title => 0,
            Self::Description => 1,
            Self::Comment => 2,
        }
    }
}

pub(super) fn terms(text: &str) -> Vec<String> {
    let mut terms: Vec<_> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect();
    terms.sort();
    terms.dedup();
    terms.truncate(32);
    terms
}

/// Scores are bounded independently of document length and term repetition.
pub(super) fn lexical(text: &str, query: &str) -> f32 {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return 0.0;
    }
    let text = text.to_lowercase();
    if text.contains(&query) {
        return 1.0;
    }
    if query.contains('_') || query.contains("::") || query.contains('/') {
        return 0.0;
    }
    let wanted = terms(&query);
    if wanted.is_empty() {
        return 0.0;
    }
    let found = terms_unbounded(&text);
    let hits = wanted
        .iter()
        .filter(|term| found.contains(term.as_str()))
        .count();
    let coverage = hits as f32 / wanted.len() as f32;
    0.8 * coverage * coverage
}

fn terms_unbounded(text: &str) -> std::collections::HashSet<&str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Byte bound is also a conservative embedding token bound for arbitrary UTF-8.
pub(super) fn chunks(text: &str) -> Vec<String> {
    let text = text.trim();
    let mut result = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + 1800).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end < text.len() {
            if let Some((offset, _)) = text[start..end]
                .char_indices()
                .rev()
                .find(|(i, c)| *i > 1200 && c.is_whitespace())
            {
                end = start + offset;
            }
        }
        let part = text[start..end].trim();
        if !part.is_empty() {
            result.push(part.to_owned());
        }
        if end == text.len() {
            break;
        }
        start = end.saturating_sub(160).max(start + 1);
        while !text.is_char_boundary(start) {
            start += 1;
        }
    }
    result
}

pub(super) fn excerpt(text: &str, query: &str) -> String {
    let lower = text.to_lowercase();
    let at = terms(query)
        .iter()
        .filter_map(|t| lower.find(t))
        .min()
        .unwrap_or(0);
    // Lowercasing can change byte length; find safe boundaries in the original.
    let mut start = at.saturating_sub(70).min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let tail: String = text[start..].chars().take(240).collect();
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        tail.split_whitespace().collect::<Vec<_>>().join(" "),
        if start + tail.len() < text.len() {
            "…"
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_phrase_and_complete_terms_beat_incidental_words() {
        let q = "reconnect after sleep";
        assert!(
            lexical("Reconnect after sleep fails", q)
                > lexical("Sleep breaks reconnect after a pause", q)
        );
        assert!(
            lexical("Sleep breaks reconnect after a pause", q) > lexical("Sleep preferences", q)
        );
        assert_eq!(lexical("Completely unrelated", q), 0.0);
        assert_eq!(lexical("sleep", ""), 0.0);
        assert!(lexical("SQLITE_BUSY_SNAPSHOT on save", "SQLITE_BUSY_SNAPSHOT") > 0.9);
    }

    #[test]
    fn fields_have_bounded_influence_and_repetition_does_not_help() {
        let q = "socket reconnect";
        let score = lexical(q, q);
        assert!(Field::Title.weight() * score > Field::Description.weight() * score);
        assert!(Field::Description.weight() * score > Field::Comment.weight() * score);
        assert_eq!(lexical(q, q), lexical(&q.repeat(100), q));
        assert!(
            Field::Description.weight() * score
                > Field::Title.weight() * lexical("socket settings", q)
        );
    }

    #[test]
    fn chunking_keeps_the_tail_and_unicode_with_bounded_overlap() {
        let text = format!("{} UNIQUE_TAIL", "🦀 background sync. ".repeat(1200));
        let parts = chunks(&text);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| !p.is_empty() && p.len() <= 1800));
        assert!(parts.last().unwrap().ends_with("UNIQUE_TAIL"));
        assert!(parts.iter().map(String::len).sum::<usize>() < text.len() * 2);
        assert!(chunks("  ").is_empty());
    }
}

#[cfg(test)]
#[test]
fn identifiers_do_not_match_a_different_identifier_with_shared_words() {
    assert_eq!(lexical("new_unique_marker", "old_unique_marker"), 0.0);
}
