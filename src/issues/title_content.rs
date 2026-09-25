use super::{Result, body, identifier};
use std::borrow::Cow;

/// Keep the original operation intact for request-ID retries. Only stored content
/// is split, so CLI, native, web, phone and subtask creation share the same rule.
pub(super) fn split<'a>(title: &'a str, description: &'a str) -> Result<(&'a str, Cow<'a, str>)> {
    let (title, description) = if title.len() <= 512 {
        (title, Cow::Borrowed(description))
    } else {
        let mut end = 512;
        while !title.is_char_boundary(end) {
            end -= 1;
        }
        let prefix = &title[..end];
        // Prefer a supplied heading, then a whole word, then a UTF-8 boundary
        // for long unbroken text (including languages without word spaces).
        let boundary = prefix.find(['\r', '\n']).or_else(|| {
            prefix
                .char_indices()
                .rev()
                .find_map(|(i, c)| c.is_whitespace().then_some(i))
        });
        if let Some(boundary) = boundary.filter(|&i| !prefix[..i].trim().is_empty()) {
            end = boundary;
        }
        let remainder = title[end..].trim_start();
        let combined = if description.is_empty() {
            Cow::Borrowed(remainder)
        } else if remainder.is_empty() {
            Cow::Borrowed(description)
        } else {
            Cow::Owned(format!("{remainder}\n\n{description}"))
        };
        (title[..end].trim_end(), combined)
    };
    identifier(title, "title", 512)?;
    body(&description, false)?;
    Ok((title, description))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_checks_the_combined_description_limit() {
        let description = "x".repeat(crate::issues::BODY_LIMIT);
        assert!(split("Short title", &description).is_ok());
        assert!(split(&"x".repeat(513), &description).is_err());
        assert!(split("", "").is_err());
        assert!(split("Bad\0title", "").is_err());
    }

    #[test]
    fn splitting_never_loses_unbroken_unicode_text() {
        for sample in ["a", "é", "界", "🦀", "e\u{301}", "👩‍💻"] {
            for count in 120..530 {
                let input = sample.repeat(count);
                let (title, body) = split(&input, "").unwrap();
                assert!(title.len() <= 512);
                assert_eq!(format!("{title}{body}"), input);
            }
        }
    }
}
