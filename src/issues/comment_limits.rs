//! Readability limits for comments submitted by agents, before any mutation.
use super::{Error, Operation, Request, Result};

pub(super) fn validate(request: &Request) -> Result<()> {
    if request
        .actor
        .as_ref()
        .is_none_or(|actor| actor.id.starts_with("human:"))
    {
        return Ok(());
    }
    let (body, allow_long_comment) = match &request.operation {
        Operation::Comment {
            body,
            allow_long_comment,
            ..
        }
        | Operation::Close {
            comment: Some(body),
            allow_long_comment,
            ..
        }
        | Operation::Block {
            comment: Some(body),
            allow_long_comment,
            ..
        } => (body, allow_long_comment),
        Operation::Artifact {
            operation:
                crate::artifacts::Operation::Comment {
                    body,
                    allow_long_comment,
                    ..
                },
        } => (body, allow_long_comment),
        _ => return Ok(()),
    };
    if *allow_long_comment {
        return Ok(());
    }
    let body = body.trim();
    if body.chars().take(301).count() > 300
        || body
            .lines()
            .flat_map(|line| line.split(['\r', '\u{85}', '\u{2028}', '\u{2029}']))
            .take(3)
            .count()
            > 2
    {
        return Err(Error::new(
            "comment_too_long",
            format!(
                "{} Agent comments must be at most 2 lines and 300 characters. To override, retry with --allow-long-comment (API: allow_long_comment: true).",
                include_str!("comment-rejection.md").trim()
            ),
        ));
    }
    Ok(())
}
