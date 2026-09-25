//! Native document tree, adapted from poe-code/toolcraft-design's MdNode model.
//! The shared CommonMark parser owns syntax; AppKit owns layout and selection.
//! Upstream license: tests/fixtures/poe-markdown-LICENSE.txt.
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    #[serde(rename = "type")]
    kind: &'static str,
    line_start: usize,
    line_end: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<Node>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lang: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    align: Vec<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tokens: Vec<Token>,
}
#[derive(Serialize)]
struct Token {
    kind: &'static str,
    value: String,
}
impl Node {
    fn new(kind: &'static str, first: usize, last: usize) -> Self {
        Self {
            kind,
            line_start: first,
            line_end: last,
            children: vec![],
            value: None,
            url: None,
            depth: None,
            start: None,
            checked: None,
            lang: None,
            align: vec![],
            tokens: vec![],
        }
    }
}
pub fn render_native_document(source: &str) -> String {
    // CommonMark accepts all newline conventions. Strip the BOM before looking
    // for a leading metadata block, while retaining original line numbers.
    let normalized;
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let source = if source.contains('\r') {
        normalized = source.replace("\r\n", "\n").replace('\r', "\n");
        normalized.as_str()
    } else {
        source
    };
    let lines: Vec<usize> = std::iter::once(0)
        .chain(source.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let line = |offset| lines.partition_point(|start| *start <= offset).max(1);
    let mut stack = vec![Node::new("root", 1, lines.len())];
    // The parser's metadata option also recognizes fences later in a document.
    // Only a complete leading block is frontmatter; later fences are Markdown.
    let mut body_start = 0;
    if let Some(metadata) = source.strip_prefix("---\n") {
        let mut offset = 4;
        for part in metadata.split_inclusive('\n') {
            offset += part.len();
            if part.trim_end_matches('\n') == "---" {
                body_start = offset;
                stack[0]
                    .children
                    .push(Node::new("frontmatter", 1, line(offset.saturating_sub(1))));
                break;
            }
        }
    }
    let mut options = super::parser_options();
    options.remove(pulldown_cmark::Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    let mut links = linkify::LinkFinder::new();
    links.url_must_have_scheme(false);
    for (event, range) in Parser::new_ext(&source[body_start..], options).into_offset_iter() {
        let first = line(body_start + range.start);
        let last = line((body_start + range.end).saturating_sub(1));
        match event {
            Event::Start(tag) => {
                if stack.len() >= 48 {
                    let mut plain = Node::new("code", 1, lines.len());
                    plain.value = Some(source.to_owned());
                    return serde_json::to_string(&plain).unwrap();
                }
                let mut node = Node::new("container", first, last);
                let image = matches!(&tag, Tag::Image { .. });
                let header = matches!(&tag, Tag::TableHead);
                match tag {
                    Tag::Paragraph => node.kind = "paragraph",
                    Tag::Heading { level, .. } => {
                        node.kind = "heading";
                        node.depth = Some(level as u32);
                    }
                    Tag::BlockQuote(kind) => {
                        node.kind = if kind.is_some() {
                            "alert"
                        } else {
                            "blockquote"
                        };
                        node.value = kind.map(|kind| format!("{kind:?}"));
                    }
                    Tag::CodeBlock(kind) => {
                        node.kind = "code";
                        if let CodeBlockKind::Fenced(info) = kind {
                            node.lang = Some(info.split_whitespace().next().unwrap_or("").into());
                        }
                    }
                    Tag::List(start) => {
                        node.kind = "list";
                        node.start = start;
                    }
                    Tag::Item => node.kind = "listItem",
                    Tag::Emphasis => node.kind = "emphasis",
                    Tag::Strong => node.kind = "strong",
                    Tag::Strikethrough => node.kind = "strikethrough",
                    Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                        node.kind = if image { "image" } else { "link" };
                        if super::safe_destination(&dest_url) {
                            node.url = Some(dest_url.to_string());
                        }
                    }
                    Tag::Table(align) => {
                        node.kind = "table";
                        node.align = align
                            .iter()
                            .map(|a| match a {
                                pulldown_cmark::Alignment::Center => "center",
                                pulldown_cmark::Alignment::Right => "right",
                                _ => "left",
                            })
                            .collect();
                    }
                    Tag::TableHead | Tag::TableRow => {
                        node.kind = "tableRow";
                        node.checked = Some(header);
                    }
                    Tag::TableCell => node.kind = "tableCell",
                    Tag::FootnoteDefinition(label) => {
                        node.kind = "footnoteDefinition";
                        node.value = Some(label.to_string());
                    }
                    Tag::MetadataBlock(_) => node.kind = "frontmatter",
                    _ => {}
                }
                stack.push(node);
            }
            Event::End(_) => {
                let mut node = stack.pop().unwrap();
                if node.kind == "code" {
                    if let Some(first) = node.children.first() {
                        node.line_start = first.line_start;
                    }
                    let code: String = node
                        .children
                        .iter()
                        .filter_map(|node| node.value.as_deref())
                        .collect();
                    if let Some(tokens) =
                        crate::syntax::tokens(&code, node.lang.as_deref().unwrap_or(""))
                    {
                        node.tokens = tokens
                            .into_iter()
                            .map(|(kind, value)| Token {
                                kind,
                                value: value.to_owned(),
                            })
                            .collect();
                    }
                    node.value = Some(code);
                    node.children.clear();
                }
                stack.last_mut().unwrap().children.push(node);
            }
            event => {
                let mut node = Node::new("text", first, last);
                match event {
                    Event::Text(text) => node.value = Some(text.to_string()),
                    Event::Code(text) => {
                        node.kind = "inlineCode";
                        node.value = Some(text.to_string());
                    }
                    Event::Html(text) | Event::InlineHtml(text) => {
                        node.kind = "html";
                        node.value = Some(text.to_string());
                    }
                    Event::SoftBreak => {
                        node.kind = "softBreak";
                        node.value = Some("\n".into());
                    }
                    Event::HardBreak => {
                        node.kind = "break";
                        node.value = Some("\n".into());
                    }
                    Event::Rule => node.kind = "thematicBreak",
                    Event::TaskListMarker(checked) => {
                        node.kind = "task";
                        node.checked = Some(checked);
                    }
                    Event::FootnoteReference(label) => {
                        node.kind = "footnoteReference";
                        node.value = Some(label.to_string());
                    }
                    _ => continue,
                }
                if node.kind == "text"
                    && !stack
                        .iter()
                        .any(|node| matches!(node.kind, "code" | "link" | "image" | "frontmatter"))
                {
                    let text = node.value.as_deref().unwrap_or("");
                    let mut end = 0;
                    for link in links.links(text) {
                        if link.start() > end {
                            let mut part = Node::new("text", first, last);
                            part.value = Some(text[end..link.start()].to_owned());
                            node.children.push(part);
                        }
                        let mut part = Node::new("link", first, last);
                        part.url = Some(match link.kind() {
                            linkify::LinkKind::Email => format!("mailto:{}", link.as_str()),
                            _ if !link.as_str().contains("://") => {
                                format!("https://{}", link.as_str())
                            }
                            _ => link.as_str().to_owned(),
                        });
                        let mut label = Node::new("text", first, last);
                        label.value = Some(link.as_str().to_owned());
                        part.children.push(label);
                        node.children.push(part);
                        end = link.end();
                    }
                    if end > 0 {
                        if end < text.len() {
                            let mut part = Node::new("text", first, last);
                            part.value = Some(text[end..].to_owned());
                            node.children.push(part);
                        }
                        node.kind = "container";
                        node.value = None;
                    }
                }
                stack.last_mut().unwrap().children.push(node);
            }
        }
    }
    serde_json::to_string(&stack.pop().unwrap())
        .expect("native document contains only valid strings")
}
