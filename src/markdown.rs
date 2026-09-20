//! CommonMark/GFM parsing with a controlled, script-free document presentation.
use linkify::{LinkFinder, LinkKind};
use pulldown_cmark::{
    CodeBlockKind, Event, Options, Parser, Tag, TagEnd, TextMergeWithOffset, html,
};

fn safe_destination(value: &str) -> bool {
    let value = value.trim();
    if value.chars().any(char::is_control) {
        return false;
    }
    let Some((scheme, _)) = value.split_once(':') else {
        return true;
    };
    if scheme.contains(['/', '#', '?']) {
        return true;
    }
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "https" | "http" | "file" | "mailto"
    )
}

pub fn render_document(source: &str) -> String {
    render(source, false)
}
/// Render with parser-derived source line anchors for native document review.
pub fn render_review_document(source: &str) -> String {
    render(source, true)
}
/// Controlled Markdown HTML for embedding in an existing document.
pub fn render_fragment(source: &str) -> String {
    let mut body = render_body(source, false);
    // The embedded app forbids inline styles. Convert only parser-generated
    // table alignment attributes; source HTML has already been escaped.
    for alignment in ["left", "center", "right"] {
        body = body.replace(
            &format!(" style=\"text-align: {alignment}\""),
            &format!(" class=\"markdown-align-{alignment}\""),
        );
    }
    body = body.replace("<pre>", "<pre tabindex=\"0\">");
    body
}
/// Share the reader's dialect with document selection matching.
pub(crate) fn parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
}
fn render_body(source: &str, source_map: bool) -> String {
    let options = parser_options();
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(source.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let line_at = |offset: usize| line_starts.partition_point(|start| *start <= offset).max(1);
    let mut events: Vec<_> = TextMergeWithOffset::new(Parser::new_ext(source, options).into_offset_iter()).map(
        |(event, range)| {
            let event = match event {
                // Raw HTML is displayed as text, never executed as document markup.
                Event::Html(text) | Event::InlineHtml(text) => {
                    let escaped = crate::syntax::escape(&text);
                    Event::Html(if source_map {
                        let start = line_at(range.start);
                        let end = line_at(range.end.saturating_sub(1));
                        format!("<span data-source-start=\"{start}\" data-source-end=\"{end}\">{escaped}</span>").into()
                    } else {
                        escaped.into()
                    })
                }
                Event::Start(mut tag) => {
                    match &mut tag {
                        Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }
                            if !safe_destination(dest_url) =>
                        {
                            *dest_url = "".into();
                        }
                        _ => {}
                    }
                    Event::Start(tag)
                }
                other => other,
            };
            (event, range)
        },
    ).collect();
    for index in 0..events.len() {
        let Event::TaskListMarker(checked) = events[index].0 else {
            continue;
        };
        let mut label = String::new();
        for (event, _) in &events[index + 1..] {
            match event {
                Event::End(TagEnd::Paragraph | TagEnd::Item) | Event::Start(Tag::List(_)) => break,
                Event::Text(text) | Event::Code(text) => {
                    label.push_str(&crate::syntax::escape(text))
                }
                Event::Html(text) if !source_map => label.push_str(text),
                Event::SoftBreak | Event::HardBreak => label.push(' '),
                _ => {}
            }
        }
        let label = if label.trim().is_empty() {
            "Task"
        } else {
            label.trim()
        };
        events[index].0 = Event::Html(
            format!(
                "<input disabled=\"\" type=\"checkbox\"{} aria-label=\"{label}\" />",
                if checked { " checked=\"\"" } else { "" }
            )
            .into(),
        );
    }
    let mut styled = Vec::new();
    let mut events = events.into_iter().peekable();
    let mut image_depth = 0;
    let mut literal_depth = 0;
    let mut finder = LinkFinder::new();
    finder.url_must_have_scheme(false);
    while let Some((event, range)) = events.next() {
        if matches!(event, Event::Start(Tag::Image { .. })) {
            image_depth += 1;
        }
        if matches!(event, Event::End(TagEnd::Image)) {
            image_depth -= 1;
        }
        match &event {
            Event::Start(Tag::Link { .. } | Tag::CodeBlock(CodeBlockKind::Indented)) => {
                literal_depth += 1;
            }
            Event::End(TagEnd::Link | TagEnd::CodeBlock) => literal_depth -= 1,
            _ => {}
        }
        if let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) = &event {
            let language = info.split_whitespace().next().unwrap_or("");
            let mut code = String::new();
            let mut code_events = Vec::new();
            let mut code_start = range.start;
            let mut saw_code = false;
            for (next, code_range) in events.by_ref() {
                if matches!(next, Event::End(TagEnd::CodeBlock)) {
                    break;
                }
                if let Event::Text(text) = &next {
                    if !saw_code {
                        code_start = code_range.start;
                        saw_code = true;
                    }
                    code.push_str(text);
                }
                code_events.push(next);
            }
            if source_map {
                let mut highlighted = String::new();
                for (index, line) in code.split_inclusive('\n').enumerate() {
                    let number = line_at(code_start) + index;
                    let text = crate::syntax::highlight(line, language)
                        .unwrap_or_else(|| crate::syntax::escape(line));
                    highlighted.push_str(&format!("<span data-source-start=\"{number}\" data-source-end=\"{number}\">{text}</span>"));
                }
                styled.push(Event::Html(
                    format!(
                        "<pre><code class=\"language-{}\">{highlighted}</code></pre>\n",
                        crate::syntax::escape(language)
                    )
                    .into(),
                ));
            } else if let Some(highlighted) = crate::syntax::highlight(&code, language) {
                styled.push(Event::Html(
                    format!(
                        "<pre><code class=\"language-{}\">{highlighted}</code></pre>\n",
                        crate::syntax::escape(language)
                    )
                    .into(),
                ));
            } else {
                styled.push(event);
                styled.extend(code_events);
                styled.push(Event::End(TagEnd::CodeBlock));
            }
        } else if image_depth == 0
            && let Event::Text(text) = &event
        {
            let linked = (literal_depth == 0)
                .then(|| autolink_text(text, &finder))
                .flatten();
            if source_map {
                let start = line_at(range.start);
                let end = line_at(range.end.saturating_sub(1));
                styled.push(Event::Html(
                    format!(
                        "<span data-source-start=\"{start}\" data-source-end=\"{end}\">{}</span>",
                        linked.unwrap_or_else(|| crate::syntax::escape(text))
                    )
                    .into(),
                ));
            } else if let Some(linked) = linked {
                styled.push(Event::Html(linked.into()));
            } else {
                styled.push(event);
            }
        } else {
            styled.push(event);
        }
    }
    let mut body = String::new();
    html::push_html(&mut body, styled.into_iter());
    body
}

/// Link prose only, after Markdown parsing, so code and explicit link labels stay
/// literal. Linkify handles punctuation, balanced parentheses and Unicode.
fn autolink_text(text: &str, finder: &LinkFinder) -> Option<String> {
    let mut html = String::new();
    let mut end = 0;
    for link in finder.links(text) {
        let label = link.as_str();
        let lower = label.to_ascii_lowercase();
        let destination = match link.kind() {
            LinkKind::Email => format!("mailto:{label}"),
            LinkKind::Url if lower.starts_with("www.") => format!("https://{label}"),
            LinkKind::Url if lower.starts_with("https://") || lower.starts_with("http://") => {
                label.to_owned()
            }
            _ => continue,
        };
        if !safe_destination(&destination) {
            continue;
        }
        html.push_str(&crate::syntax::escape(&text[end..link.start()]));
        html.push_str(&format!(
            "<a href=\"{}\">{}</a>",
            crate::syntax::escape(&destination),
            crate::syntax::escape(label)
        ));
        end = link.end();
    }
    if end == 0 {
        return None;
    }
    html.push_str(&crate::syntax::escape(&text[end..]));
    Some(html)
}
fn render(source: &str, source_map: bool) -> String {
    let body = render_body(source, source_map);
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src https: http: data:; base-uri 'none'; form-action 'none'">
<style>
:root {{ color-scheme: light dark; --bg:#fff; --fg:#202124; --muted:#63666b; --line:#e3e5e8; --code:#f4f5f7; --link:#0068d9; }}
@media(prefers-color-scheme:dark) {{ :root {{ --bg:#1e1f22; --fg:#eceef1; --muted:#a6aab2; --line:#393b40; --code:#282a2f; --link:#78b5ff; }} }}
* {{ box-sizing:border-box; }} body {{ margin:0; background:var(--bg); color:var(--fg); font:15px/1.65 -apple-system,BlinkMacSystemFont,sans-serif; overflow-wrap:anywhere; }}
article {{ max-width:820px; margin:auto; padding:36px 40px 64px; }}
h1,h2,h3,h4,h5,h6 {{ line-height:1.25; letter-spacing:-.025em; margin:1.6em 0 .65em; font-weight:650; }}
h1 {{ font-size:30px; }} h2 {{ font-size:23px; padding-bottom:.4em; border-bottom:1px solid var(--line); }} h3 {{ font-size:19px; }} h4,h5,h6 {{ font-size:16px; }} article>:first-child {{ margin-top:0; }}
p,ul,ol,blockquote,pre,table {{ margin:0 0 1.15em; }} li>p {{ margin:.35em 0; }} ul,ol {{ padding-left:1.65em; }} li>ul,li>ol {{ margin:.3em 0; }} li+li {{ margin-top:.2em; }}
a {{ color:var(--link); text-decoration:none; }} a:hover {{ text-decoration:underline; }} a:focus-visible {{ outline:2px solid var(--link); outline-offset:3px; border-radius:3px; }}
code,pre {{ font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:13px; }} code {{ background:var(--code); padding:.15em .35em; border-radius:4px; }}
pre {{ background:var(--code); padding:16px; border:1px solid var(--line); border-radius:10px; overflow-x:auto; overflow-wrap:normal; line-height:1.55; }} .token-comment {{ color:var(--muted); font-style:italic; }} .token-keyword {{ color:#9950a6; }} .token-string {{ color:#9a402c; }} .token-number,.token-constant {{ color:#256b9c; }} .token-key,.token-type {{ color:#6e558f; }} .token-insert {{ color:#287a45; }} .token-delete {{ color:#b43d43; }}
@media(prefers-color-scheme:dark) {{ .token-keyword {{ color:#dba4e1; }} .token-string {{ color:#e7ad91; }} .token-number,.token-constant {{ color:#89c0ef; }} .token-key,.token-type {{ color:#c4b2e4; }} .token-insert {{ color:#9fd4ac; }} .token-delete {{ color:#efa4a9; }} }}
pre code {{ background:none; padding:0; white-space:pre; }}
blockquote {{ border-left:3px solid var(--line); padding:.2em 0 .2em 16px; color:var(--muted); }} blockquote>:last-child {{ margin-bottom:0; }}
blockquote[class^=markdown-alert] {{ border-radius:8px; padding:12px 16px; color:var(--fg); background:var(--code); }}
blockquote[class^=markdown-alert]::before {{ display:block; font-weight:650; margin-bottom:6px; }}
.markdown-alert-note {{ border-color:#448aff; }} .markdown-alert-note::before {{ content:"Note"; color:#448aff; }}
.markdown-alert-tip {{ border-color:#36a565; }} .markdown-alert-tip::before {{ content:"Tip"; color:#36a565; }}
.markdown-alert-important {{ border-color:#9970dd; }} .markdown-alert-important::before {{ content:"Important"; color:#9970dd; }}
.markdown-alert-warning {{ border-color:#c18313; }} .markdown-alert-warning::before {{ content:"Warning"; color:#c18313; }}
.markdown-alert-caution {{ border-color:#e05555; }} .markdown-alert-caution::before {{ content:"Caution"; color:#e05555; }}
table {{ border-collapse:collapse; display:block; max-width:100%; overflow-x:auto; font-size:14px; }} th,td {{ padding:9px 13px; border:1px solid var(--line); }} th {{ background:var(--code); text-align:left; font-weight:600; }}
hr {{ border:0; border-top:1px solid var(--line); margin:28px 0; }} img {{ max-width:100%; height:auto; border-radius:8px; }} input[type=checkbox] {{ accent-color:var(--link); margin-right:.45em; }}
.footnote-definition {{ font-size:13px; color:var(--muted); }} .footnote-definition-label {{ float:left; margin-right:.6em; }}
@media(max-width:500px) {{ article {{ padding:24px 22px 48px; }} h1 {{ font-size:26px; }} }}
</style></head><body><article>{body}</article></body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bare_links_handle_pr_urls_punctuation_entities_and_unicode() {
        let html = render_fragment(
            "PRs:\n- https://github.com/poe-internal/poe2/pull/14920\n- https://github.com/poe-internal/poe2/pull/14921\n\nSee (https://example.com/a_(b)). https://example.com/?a=1&amp;b=2, **www.example.com** or agent@example.com. 日本語 https://example.com/日本語.\n\nA filename: report.md",
        );
        for destination in [
            "https://github.com/poe-internal/poe2/pull/14920",
            "https://github.com/poe-internal/poe2/pull/14921",
            "https://example.com/a_(b)",
            "https://example.com/?a=1&amp;b=2",
            "https://www.example.com",
            "mailto:agent@example.com",
            "https://example.com/日本語",
        ] {
            assert!(html.contains(&format!("href=\"{destination}\"")), "{html}");
        }
        assert!(html.contains("</a>)."), "{html}");
        assert!(!html.contains("href=\"https://report.md"));
    }

    #[test]
    fn autolinks_preserve_code_existing_links_images_and_script_safety() {
        let html = render_fragment(
            "`https://inline.example.com`\n\n```text\nhttps://fenced.example.com\n```\n\n    https://indented.example.com\n\n[https://label.example.com](https://destination.example.com)\n\n<https://automatic.example.com>\n\n![https://alt.example.com](https://image.example.com/a.png)\n\n<script>https://script.example.com</script>\n\njavascript://evil.example.com data:text/html,test [bad](javascript:alert(1))\n\nhttps://example.com/?q=\" onmouseover=\"alert(1)\"",
        );
        assert_eq!(html.matches("<a ").count(), 4, "{html}");
        for literal in ["inline", "fenced", "indented", "label", "alt", "script"] {
            assert!(
                !html.contains(&format!("href=\"https://{literal}.example.com")),
                "{html}"
            );
        }
        assert!(html.contains("alt=\"https://alt.example.com\""), "{html}");
        assert!(!html.contains("href=\"javascript:"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("\" onmouseover=\""));
    }

    #[test]
    fn fragment_table_alignment_works_without_inline_styles() {
        let html = render_fragment("| Left | Center | Right |\n|:---|:---:|---:|\n| a | b | c |");
        for alignment in ["left", "center", "right"] {
            assert_eq!(
                html.matches(&format!("class=\"markdown-align-{alignment}\""))
                    .count(),
                2,
                "{html}"
            );
        }
        assert!(!html.contains("style="));
    }

    #[test]
    fn fragment_tasks_have_names_and_code_can_scroll_with_the_keyboard() {
        let html = render_fragment(
            "- [x] Keep **context** and `code`\n- [ ] Review <script>literally</script>\n\n```unknown\nwide code\n```\n\n    indented code\n",
        );
        assert!(
            html.contains("aria-label=\"Keep context and code\""),
            "{html}"
        );
        assert!(
            html.contains("aria-label=\"Review &lt;script&gt;literally&lt;/script&gt;\""),
            "{html}"
        );
        assert_eq!(html.matches("<pre tabindex=\"0\">").count(), 2, "{html}");
        assert!(
            html.contains("disabled=\"\" type=\"checkbox\" checked=\"\""),
            "{html}"
        );
        let nested = render_fragment("- [ ] Parent \"quoted\"\n  - [x] Child\n");
        assert!(
            nested.contains("aria-label=\"Parent &quot;quoted&quot;\""),
            "{nested}"
        );
        assert!(nested.contains("aria-label=\"Child\""), "{nested}");
        let review = render_review_document("- [x] <b>literal</b> and context\n");
        assert!(!review.contains("aria-label=\"<span"), "{review}");
    }

    #[test]
    fn review_source_map_tracks_multiline_markdown_and_code_without_enabling_html() {
        let source = "# Heading\n\nSelected **paragraph**.\nNext line 🌍.\n\n```rust\nlet first = true;\nlet second = false;\n```\n\n![diagram](https://example.com/image.png)";
        let html = render_review_document(source);
        for line in [3, 4, 7, 8] {
            assert!(html.contains(&format!("data-source-start=\"{line}\"")));
        }
        assert!(html.contains("alt=\"diagram\""));
        assert!(html.contains("token-keyword"));
        assert!(!html.contains("<script>"));
        let raw = render_review_document("# Review\n\n<script>literal</script>");
        assert!(raw.contains("data-source-start=\"3\""));
        assert!(raw.contains("&lt;script&gt;literal&lt;/script&gt;"));
    }
    #[test]
    fn renders_nested_lists_tables_tasks_and_fenced_code() {
        let html = render_document(
            "# Report\n\n- parent\n  - **child**\n\n| A | B |\n|---|---|\n| one | two |\n\n- [x] done\n\n```rust\nlet x = \"<tag>\";\n```\n\n~~old~~",
        );
        for expected in [
            "<h1>Report</h1>",
            "<strong>child</strong>",
            "<table>",
            "type=\"checkbox\"",
            "language-rust",
            "&lt;tag&gt;",
            "<del>old</del>",
        ] {
            assert!(html.contains(expected), "{expected}");
        }
    }
    #[test]
    fn raw_html_is_literal_and_document_blocks_scripts() {
        let html = render_document(
            "<script>alert('x')</script>\n\n<img src=x onerror=alert(1)>\n\n[link](https://example.com)",
        );
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<img src=x"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("default-src 'none'"));
        assert!(html.contains("href=\"https://example.com\""));
    }
}
