use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};
use std::ffi::{CStr, CString, c_char};

fn destination_allowed(value: &str) -> bool {
    let value = value.trim();
    if value.chars().any(char::is_control) {
        return false;
    }
    let Some((scheme, _)) = value.split_once(':') else {
        return true;
    };
    scheme.contains(['/', '#', '?'])
        || matches!(
            scheme.to_ascii_lowercase().as_str(),
            "https" | "http" | "mailto" | "file" | "t3" | "slack" | "obsidian"
        )
}

fn render(source: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut title = String::new();
    let mut in_title = false;
    let mut found_title = false;
    let mut metadata = false;
    let events =
        Parser::new_ext(source.trim_start_matches('\u{feff}'), options).filter_map(|event| {
            match event {
                Event::Start(Tag::MetadataBlock(_)) => {
                    metadata = true;
                    return None;
                }
                Event::End(TagEnd::MetadataBlock(_)) => {
                    metadata = false;
                    return None;
                }
                _ if metadata => return None,
                Event::Start(Tag::Heading {
                    level: HeadingLevel::H1,
                    ..
                }) if !found_title => {
                    in_title = true;
                    found_title = true;
                    return None;
                }
                Event::End(TagEnd::Heading(HeadingLevel::H1)) if in_title => {
                    in_title = false;
                    return None;
                }
                Event::Text(ref text) | Event::Code(ref text) if in_title => {
                    title.push_str(text);
                    return None;
                }
                Event::SoftBreak | Event::HardBreak if in_title => {
                    title.push(' ');
                    return None;
                }
                _ if in_title => return None,
                _ => {}
            }
            Some(match event {
                // Markdown is content, never executable HTML.
                Event::Html(text) | Event::InlineHtml(text) => Event::Text(text),
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => Event::Start(Tag::Link {
                    link_type,
                    dest_url: if destination_allowed(&dest_url) {
                        dest_url
                    } else {
                        "".into()
                    },
                    title,
                    id,
                }),
                Event::Start(Tag::Image {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => Event::Start(Tag::Image {
                    link_type,
                    dest_url: if destination_allowed(&dest_url) {
                        dest_url
                    } else {
                        "".into()
                    },
                    title,
                    id,
                }),
                other => other,
            })
        });
    let mut body = String::new();
    html::push_html(&mut body, events);
    serde_json::json!({ "title": if title.trim().is_empty() { "Daily Focus" } else { title.trim() }, "html": body }).to_string()
}

/// # Safety
/// `source` must be a valid, NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sticky_render(source: *const c_char) -> *mut c_char {
    if source.is_null() {
        return std::ptr::null_mut();
    }
    let Ok(source) = (unsafe { CStr::from_ptr(source) }).to_str() else {
        return std::ptr::null_mut();
    };
    CString::new(render(source)).unwrap().into_raw()
}

/// # Safety
/// `value` must be a pointer returned by sticky_render, freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sticky_render_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document(source: &str) -> serde_json::Value {
        serde_json::from_str(&render(source)).unwrap()
    }
    #[test]
    fn title_and_rich_body() {
        let doc = document(
            "---\nprivate: hidden\n---\n# My **focus**\n\n1. [Plan](https://example.com)\n2. Ship\n\n- [x] Done\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```mermaid\npie\n \"Work\" : 3\n```\n",
        );
        assert_eq!(doc["title"], "My focus");
        let body = doc["html"].as_str().unwrap();
        for expected in [
            "<ol>",
            "https://example.com",
            "checkbox",
            "<table>",
            "language-mermaid",
        ] {
            assert!(body.contains(expected), "{expected}");
        }
        assert!(!body.contains("<h1>"));
        assert!(!body.contains("hidden"));
    }
    #[test]
    fn content_cannot_execute_script() {
        let doc = document(
            "# Safe\n<script>alert(1)</script>\n\n[x](javascript:alert%281%29)\n\n![x](data:text/html,bad)",
        );
        let body = doc["html"].as_str().unwrap();
        assert!(!body.contains("<script>"));
        assert!(!body.contains("javascript:"));
        assert!(!body.contains("data:text"));
    }
    #[test]
    fn heading_inside_code_is_not_title() {
        let doc = document("```\n# Not a title\n```\n\n# Actual `title`\n\n## Section\nbody");
        assert_eq!(doc["title"], "Actual title");
        assert!(doc["html"].as_str().unwrap().contains("<h2>"));
    }
}
