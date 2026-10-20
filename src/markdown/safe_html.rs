use std::borrow::Cow;

/// Author HTML may format a document, never name or control application UI.
pub(super) fn clean(html: &str) -> String {
    let mut builder = ammonia::Builder::default();
    builder
        .add_tags(["details", "summary", "input"])
        .add_tag_attributes("details", ["open"])
        .add_tag_attributes(
            "input",
            [
                "type",
                "disabled",
                "checked",
                "aria-label",
                "data-task-line",
            ],
        )
        .set_tag_attribute_value("input", "type", "checkbox")
        .set_tag_attribute_value("input", "disabled", "")
        .add_tag_attributes("pre", ["tabindex"])
        .add_generic_attributes(["class", "id"])
        .rm_generic_attributes(["name"])
        .add_url_schemes(["file"])
        .link_rel(None)
        .attribute_filter(|tag, attribute, value| match attribute {
            "class" => {
                let safe: Vec<_> = value
                    .split_whitespace()
                    .filter(|class| {
                        matches!(
                            *class,
                            "markdown-short-list"
                                | "markdown-task"
                                | "markdown-align-left"
                                | "markdown-align-center"
                                | "markdown-align-right"
                                | "footnote-reference"
                                | "footnote-definition"
                                | "footnote-definition-label"
                        ) || (tag == "code"
                            && class.starts_with("language-")
                            && class
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
                            || (tag == "span"
                                && [
                                    "token-comment",
                                    "token-keyword",
                                    "token-string",
                                    "token-number",
                                    "token-constant",
                                    "token-key",
                                    "token-type",
                                    "token-insert",
                                    "token-delete",
                                ]
                                .contains(class))
                            || (tag == "blockquote"
                                && [
                                    "markdown-alert-note",
                                    "markdown-alert-tip",
                                    "markdown-alert-important",
                                    "markdown-alert-warning",
                                    "markdown-alert-caution",
                                ]
                                .contains(class))
                    })
                    .collect();
                (!safe.is_empty()).then(|| Cow::Owned(safe.join(" ")))
            }
            "id" => Some(Cow::Owned(format!("hb-content-{value}"))),
            "href" if value.starts_with('#') => {
                Some(Cow::Owned(format!("#hb-content-{}", &value[1..])))
            }
            "type" if tag == "input" => Some(Cow::Borrowed("checkbox")),
            "tabindex" => Some(Cow::Borrowed("0")),
            _ => Some(Cow::Borrowed(value)),
        });
    builder.clean(html).to_string()
}
