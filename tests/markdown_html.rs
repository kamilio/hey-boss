use hey_boss::markdown::render_fragment;

#[test]
fn native_html_uses_the_shared_sandbox_without_loading_it_for_plain_markdown() {
    let plain: serde_json::Value = serde_json::from_str(
        &hey_boss::markdown::render_native_document("# Plain\n\nA normal document."),
    )
    .unwrap();
    assert!(plain.get("web").is_none());
    let native: serde_json::Value = serde_json::from_str(
        &hey_boss::markdown::render_native_document(include_str!("fixtures/html-page.html")),
    )
    .unwrap();
    let web = &native["web"];
    let document = web["document"].as_str().unwrap();
    assert!(document.contains("script-src 'sha256-"));
    assert!(document.contains("language-html-render"));
    assert!(document.contains("frame-src hey-boss-preview:"));
    assert!(
        web["policy"]
            .as_str()
            .unwrap()
            .contains("sandbox allow-scripts")
    );
    assert!(
        !web["policy"]
            .as_str()
            .unwrap()
            .contains("allow-same-origin")
    );
    assert_eq!(
        web["frame"],
        include_str!("../src/issues/web/html-preview.html")
    );
}

#[test]
fn complete_html_documents_render_directly_without_a_markdown_fence() {
    let source = "\u{feff}\n<!DOCTYPE html>\n<html><head><style>body{color:red}</style></head><body><h1>A whole page</h1><script>document.body.dataset.ready='yes'</script></body></html>\n";
    let html = render_fragment(source);
    assert_eq!(
        html.matches("class=\"language-html-render\"").count(),
        1,
        "{html}"
    );
    assert!(html.contains("&lt;!DOCTYPE html&gt;"), "{html}");
    assert!(
        html.contains("&lt;script&gt;document.body.dataset.ready='yes'&lt;/script&gt;"),
        "{html}"
    );
    assert!(!html.contains("<script>"), "{html}");
    assert!(!render_fragment("`<!doctype html>` is an example").contains("language-html-render"));
    assert!(!render_fragment("<div>Safe inline HTML</div>").contains("language-html-render"));
}

#[test]
fn safe_html_composes_with_markdown_and_does_not_change_code_examples() {
    let html = render_fragment(
        "# Notes\n\n<details open><summary>Evidence</summary>\n\n**Passed** <sup>2</sup>\n\n</details>\n\n```html\n<button onclick=\"run()\">Example</button>\n```\n",
    );
    assert!(
        html.contains("<details open=\"\"><summary>Evidence</summary>"),
        "{html}"
    );
    assert!(
        html.contains("<strong>Passed</strong> <sup>2</sup>"),
        "{html}"
    );
    assert!(html.contains("class=\"language-html\""), "{html}");
    assert!(!html.contains("<button"), "{html}");
}

#[test]
fn author_html_cannot_control_the_host_or_execute_code() {
    let html = render_fragment(
        "<div id=\"issue-list\" class=\"toast\" data-action=\"delete\" style=\"position:fixed;inset:0\" onclick=\"run()\"><script>run()</script><iframe src=\"/api/bootstrap\"></iframe><form><input name=\"location\"></form><a href=\"javascript:run()\">Link</a><img src=\"x\" onerror=\"run()\">Safe</div>",
    );
    assert!(html.contains("Safe"), "{html}");
    for unsafe_text in [
        "<script",
        "<iframe",
        "<form",
        "onclick=",
        "onerror=",
        "data-action=",
        "style=",
        "class=\"toast\"",
        "id=\"issue-list\"",
        "javascript:",
    ] {
        assert!(!html.contains(unsafe_text), "{unsafe_text}: {html}");
    }
}

#[test]
fn rich_html_requires_the_explicit_render_fence_and_preserves_its_source() {
    let code = "<style>button{color:red}</style>\n<button onclick=\"this.textContent++\">0</button>\n<script>console.log('ready')</script>\n";
    let html = render_fragment(&format!(
        "```html render\n{code}```\n\n```html\n{code}```\n\n`<b>example</b>`"
    ));
    assert_eq!(
        html.matches("class=\"language-html-render\"").count(),
        1,
        "{html}"
    );
    assert!(!html.contains("<script>"), "{html}");
    assert!(
        html.contains("&lt;script&gt;console.log('ready')&lt;/script&gt;"),
        "{html}"
    );
    assert!(
        html.contains("<code>&lt;b&gt;example&lt;/b&gt;</code>"),
        "{html}"
    );
}

#[test]
fn raw_lists_cannot_shift_markdown_list_containment_or_break_footnotes() {
    let mut source = "<ul><li>HTML list</li></ul>\n\n".to_owned();
    for i in 0..130 {
        source.push_str(&format!("- Item {i}\n"));
    }
    source.push_str("\nFootnote[^one].\n\n[^one]: Detail\n");
    let html = render_fragment(&source);
    assert!(!html.contains("markdown-short-list"), "{html}");
    assert!(html.contains("id=\"hb-content-one\""), "{html}");
    assert!(html.contains("href=\"#hb-content-one\""), "{html}");
}
