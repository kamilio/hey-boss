// Adapted examples from poe-code's terminal-markdown HTML/parser tests.
// Original test examples: Copyright (c) 2026 Poe Platform, MIT.
// See tests/fixtures/poe-markdown-LICENSE.txt for the preserved license.
use hey_boss::markdown::render_document;

fn body(source: &str) -> String {
    render_document(source)
        .split_once("<article>")
        .unwrap()
        .1
        .strip_suffix("</article></body></html>")
        .unwrap()
        .to_owned()
}
fn visible_html_text(html: &str) -> String {
    let mut in_tag = false;
    html.chars()
        .filter(|ch| {
            if *ch == '<' {
                in_tag = true;
                return false;
            }
            if *ch == '>' {
                in_tag = false;
                return false;
            }
            !in_tag
        })
        .collect()
}
fn includes(source: &str, expected: &[&str]) {
    let html = body(source);
    for fragment in expected {
        assert!(html.contains(fragment), "missing {fragment:?} in {html:?}");
    }
}

#[test]
fn headings_inline_styles_and_code() {
    includes(
        "# Status\n\nReady with **strong**, *emphasis*, ~~old~~, and `poe-code configure`.",
        &[
            "<h1>Status</h1>",
            "<strong>strong</strong>",
            "<em>emphasis</em>",
            "<del>old</del>",
            "<code>poe-code configure</code>",
        ],
    );
}
#[test]
fn titled_links_images_and_hard_breaks() {
    includes(
        "[Docs](https://example.com/docs \"Read docs\") and ![Diagram](https://example.com/image.png \"System\")\nline one  \nline two",
        &[
            "href=\"https://example.com/docs\"",
            "title=\"Read docs\"",
            "alt=\"Diagram\"",
            "<br />",
        ],
    );
}
#[test]
fn ordered_unordered_nested_and_task_lists() {
    includes(
        "3. ordered\n4. next\n\n- plain\n- [x] complete\n- [ ] pending\n  - nested",
        &[
            "<ol start=\"3\">",
            "<li>ordered</li>",
            "<ul>",
            "disabled=\"\"",
            "checked=\"\"",
            "nested",
        ],
    );
}
#[test]
fn aligned_tables_have_head_and_body() {
    includes(
        "| Feature | Status | Count |\n| :------ | :----: | ----: |\n| HTML | ready | 2 |",
        &[
            "<table>",
            "<thead>",
            "<tbody>",
            "text-align: left",
            "text-align: center",
            "text-align: right",
            "HTML",
            "ready",
        ],
    );
}
#[test]
fn footnotes_preserve_order_and_emphasis() {
    includes(
        "Second first[^b], then first[^a].\n\n[^a]: Alpha note.\n[^b]: Beta **note**.",
        &[
            "href=\"#b\"",
            "href=\"#a\"",
            "Alpha note.",
            "Beta <strong>note</strong>",
        ],
    );
}
#[test]
fn frontmatter_hidden_by_default() {
    assert_eq!(
        body("---\ntitle: Demo\ndraft: false\n---\n\nBody"),
        "<p>Body</p>\n"
    );
}
#[test]
fn unsafe_urls_preserve_labels_without_executable_destinations() {
    let html = body(
        "[bad](javascript:alert(1)) ![bad image](javascript:alert(1)) [safe](https://example.com) [anchor](#top)",
    );
    assert!(!html.contains("href=\"javascript:"));
    assert!(!html.contains("src=\"javascript:"));
    assert!(html.contains("bad") && html.contains("bad image"));
    assert!(html.contains("href=\"#top\""));
}
#[test]
fn raw_html_never_becomes_active_markup() {
    let html =
        body("<section>\n<script>alert(1)</script>\n</section>\n\nInline <span>safe?</span>");
    assert!(!html.contains("<script>") && !html.contains("<span>"));
    assert!(html.contains("&lt;script&gt;") && html.contains("&lt;span&gt;"));
}
#[test]
fn unicode_and_malformed_markdown() {
    includes(
        "# 你好世界 🌍\n\nEmoji 🎉 and ñ",
        &["<h1>你好世界 🌍</h1>", "Emoji 🎉 and ñ"],
    );
    includes(
        "This has *unclosed emphasis and **unclosed strong",
        &["*unclosed emphasis", "**unclosed strong"],
    );
    includes(
        "[broken link(no close paren",
        &["[broken link(no close paren"],
    );
}
#[test]
fn empty_document_is_empty_article() {
    assert_eq!(body(""), "");
}
#[test]
fn unknown_language_is_literal_safe_code() {
    includes(
        "```unknown-language\n<x>\n```",
        &["language-unknown-language", "&lt;x&gt;"],
    );
}
#[test]
fn fenced_markdown_markers_remain_literal() {
    let rendered = body("```ts\nconst sample = \"**still literal**\";\n  return value + 1;\n```");
    let visible = visible_html_text(&rendered);
    assert!(visible.contains("**still literal**"));
    assert!(visible.contains("  return value + 1;"));
    assert!(rendered.contains("token-keyword"));
}

#[test]
fn nested_blockquotes_remain_nested() {
    let html = body("> Outer quote\n> > Nested quote\n> > > Deep quote");
    assert_eq!(html.matches("<blockquote>").count(), 3);
}
#[test]
fn alerts_all_have_distinct_semantics() {
    for (name, class) in [
        ("NOTE", "note"),
        ("TIP", "tip"),
        ("IMPORTANT", "important"),
        ("WARNING", "warning"),
        ("CAUTION", "caution"),
    ] {
        includes(
            &format!("> [!{name}]\n> Wrapped content stays aligned beneath the bar."),
            &[&format!("markdown-alert-{class}"), "Wrapped content"],
        );
    }
}
#[test]
fn table_stops_before_blockquote_heading_or_list_with_pipe() {
    for (ending, expected) in [
        ("> quoted | row", "<blockquote>"),
        ("# heading | pipe", "<h1>heading | pipe</h1>"),
        ("- item | value", "<li>item | value</li>"),
    ] {
        includes(
            &format!("| A | B |\n| --- | --- |\n{ending}"),
            &["</table>", expected],
        );
    }
}
#[test]
fn invalid_table_separator_is_plain_text() {
    assert!(!body("| A | B |\n| --- | nope |\n| C | D |").contains("<table>"));
}
#[test]
fn setext_headings_and_thematic_break() {
    includes(
        "Title\n=====\n\nSubtitle\n-----\n\n***",
        &["<h1>Title</h1>", "<h2>Subtitle</h2>", "<hr />"],
    );
}
#[test]
fn code_spans_escape_tags_and_normalize_internal_line_breaks() {
    includes(
        "`<tag>` and `` code ` ticks ``\n\n`line\nbreak`",
        &[
            "<code>&lt;tag&gt;</code>",
            "<code>code ` ticks</code>",
            "<code>line break</code>",
        ],
    );
}
#[test]
fn escapes_entities_and_reference_links() {
    includes(
        "\\*literal\\* &amp; [Docs][doc]\n\n[doc]: https://example.com \"Reference\"",
        &[
            "*literal* &amp;",
            "href=\"https://example.com\"",
            "title=\"Reference\"",
        ],
    );
}
#[test]
fn empty_list_items_and_loose_nested_lists() {
    includes(
        "-\n- second\n\n  second paragraph\n\n  - nested",
        &["<ul>", "<li>", "second paragraph", "nested"],
    );
}
#[test]
fn large_document_and_long_code_line_are_not_truncated() {
    let source = format!(
        "# Large\n\n{}\n\n```text\n{}\n```",
        "Paragraph **bold**.\n\n".repeat(5000),
        "x".repeat(20000)
    );
    let html = body(&source);
    assert_eq!(html.matches("<strong>bold</strong>").count(), 5000);
    assert!(html.contains(&"x".repeat(20000)));
}
