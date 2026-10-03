//! Human-readable history for new watcher signals, committed with the wake-up.
use super::*;

// GitHub names are untrusted Markdown. Keep each item on one bounded line.
fn text(value: &str, limit: usize) -> String {
    let mut result = String::new();
    for ch in value.chars().take(limit) {
        if ch.is_whitespace() || ch.is_control() {
            result.push(' ');
        } else {
            if ch.is_ascii_punctuation() && !matches!(ch, '/' | '-' | '.') {
                result.push('\\');
            }
            result.push(ch);
        }
    }
    if value.chars().nth(limit).is_some() {
        result.push('…');
    }
    result
}

fn body(url: &str, observation: &hey_gh::watcher::Observation, new: &[String]) -> String {
    let new: std::collections::HashSet<_> = new.iter().collect();
    let url = url.trim_end_matches('/');
    let pr = hey_gh::watcher::pull_request_selector(url)
        .map(|(repository, number)| format!("[{}#{number}]({url})", text(&repository, 160)))
        .unwrap_or_else(|| text(url, 160));
    let mut body = format!(
        "GitHub update on {pr} · commit {}\n\n",
        text(&observation.head, 12)
    );
    if observation.blocking.iter().any(|key| new.contains(key)) {
        let names: BTreeSet<_> = observation.evidence["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|check| check["state"] == "failure")
            .filter_map(|check| check["context"].as_str())
            .collect();
        body.push_str("- **Required checks failed.** ");
        if names.is_empty() {
            body.push_str("A new failure or failed rerun needs attention.");
        } else {
            body.push_str("Currently failing: ");
            body.push_str(
                &names
                    .iter()
                    .take(4)
                    .map(|name| text(name, 80))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            let total = observation.evidence["required_counts"]["failure"]
                .as_u64()
                .unwrap_or(names.len() as u64);
            if total > names.len().min(4) as u64 {
                body.push_str(&format!(
                    " (and {} more)",
                    total - names.len().min(4) as u64
                ));
            }
            body.push('.');
        }
        body.push('\n');
    }
    if observation
        .completed
        .as_ref()
        .is_some_and(|key| new.contains(key))
    {
        body.push_str(
            "- **CI finished.** Check results and review status are ready for another look.\n",
        );
    }
    let feedback = observation
        .feedback
        .iter()
        .filter(|key| new.contains(key))
        .count();
    if feedback > 0 {
        body.push_str(&format!(
            "- **New feedback.** {feedback} new or changed review/discussion {}.\n",
            if feedback == 1 { "item" } else { "items" }
        ));
    }
    if new.iter().any(|key| key.starts_with("policy:")) {
        body.push_str(
            "- **Head or required-check policy changed.** Review the current PR evidence.\n",
        );
    }
    body
}

pub(super) fn record(
    db: &Connection,
    project: &str,
    number: i64,
    url: &str,
    observation: &hey_gh::watcher::Observation,
    new: &[String],
) -> Result<()> {
    let body = body(url, observation, new);
    let now = crate::issues::worker::now();
    db.execute("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES(?1,?2,?3,?4,?5)", params![project,number,WATCHER,body,now])?;
    event(
        db,
        project,
        number,
        WATCHER,
        "commented",
        now,
        &json!({"comment_id":db.last_insert_rowid(),"body":body}),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_reasons_are_separate_from_already_seen_failures() {
        let observation = hey_gh::watcher::Observation {
            head: "0123456789abcdef".into(),
            blocking: vec!["failed".into()],
            completed: Some("done".into()),
            feedback: vec!["review".into()],
            evidence: json!({"required":[{"context":"Build","state":"failure"}]}),
        };
        let comment = body(
            "https://github.com/o/r/pull/1",
            &observation,
            &["done".into(), "review".into()],
        );
        assert!(comment.contains("CI finished"));
        assert!(comment.contains("1 new or changed review/discussion item."));
        assert!(!comment.contains("Required checks failed"));
        assert!(comment.contains("commit 0123456789ab"));
    }

    #[test]
    fn long_and_hostile_check_names_remain_bounded_plain_text() {
        let observation = hey_gh::watcher::Observation {
            head: "head".into(),
            blocking: vec!["failed".into()],
            completed: None,
            feedback: vec![],
            evidence: json!({"required_counts":{"failure":20},"required":[
                {"context":"<img>\n**oops** [link](javascript:x)","state":"failure"},
                {"context":"界".repeat(2000),"state":"failure"}
            ]}),
        };
        let comment = body(
            "https://github.com/o/r/pull/1/",
            &observation,
            &["failed".into()],
        );
        assert!(comment.contains("(and 18 more)"));
        assert!(comment.contains("\\<img\\> \\*\\*oops\\*\\*"));
        assert!(comment.len() < 1000);
        assert_eq!(comment.lines().count(), 3);
    }
}
