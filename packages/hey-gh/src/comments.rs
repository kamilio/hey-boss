//! Shared comment readability policy. Call before any mutation.

pub const GUIDANCE: &str = include_str!("comment-rejection.md");

pub fn too_long(body: &str) -> bool {
    let body = body.trim();
    body.chars().take(301).count() > 300
        || body
            .lines()
            .flat_map(|line| line.split(['\r', '\u{85}', '\u{2028}', '\u{2029}']))
            .take(3)
            .count()
            > 2
}
