//! Lightweight lexical highlighting, inspired by poe-code's code-highlight.ts.
//! Original approach/examples Copyright (c) 2026 Poe Platform, MIT; preserved
//! license in tests/fixtures/poe-markdown-LICENSE.txt. This is presentation only.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn highlight(source: &str, language: &str) -> Option<String> {
    tokens(source, language).map(|tokens| {
        tokens
            .into_iter()
            .map(|(kind, text)| span(kind, text))
            .collect()
    })
}

pub fn tokens<'a>(source: &'a str, language: &str) -> Option<Vec<(&'static str, &'a str)>> {
    let language = language.to_ascii_lowercase();
    let family = match language.as_str() {
        "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "rust" | "rs" | "swift"
        | "c" | "cpp" | "c++" | "java" | "kotlin" | "kt" | "go" | "golang" | "cs" | "csharp" => "c",
        "py" | "python" | "ruby" | "rb" | "sh" | "bash" | "zsh" | "shell" | "yaml" | "yml"
        | "toml" | "ini" => "hash",
        "json" | "jsonc" | "jsonl" => "data",
        "sql" => "sql",
        "html" | "xml" | "svg" => "markup",
        "css" | "scss" | "less" => "c",
        "diff" | "patch" => "diff",
        _ => return None,
    };
    if family == "diff" {
        return Some(
            source
                .split_inclusive('\n')
                .map(|line| {
                    let kind = if line.starts_with('+') {
                        "insert"
                    } else if line.starts_with('-') {
                        "delete"
                    } else if line.starts_with('@') {
                        "comment"
                    } else {
                        "plain"
                    };
                    (kind, line)
                })
                .collect(),
        );
    }
    let mut output: Vec<(&'static str, &'a str)> = Vec::new();
    let mut i = 0;
    while i < source.len() {
        let rest = &source[i..];
        let ch = rest.chars().next()?;
        let mut end = i + ch.len_utf8();
        let mut kind = "plain";
        let line_comment = (family == "hash" && rest.starts_with('#'))
            || ((family == "c" || language == "jsonc") && rest.starts_with("//"))
            || (family == "sql" && rest.starts_with("--"));
        if line_comment {
            end = i + rest.find('\n').unwrap_or(rest.len());
            kind = "comment";
        } else if (family == "c" || language == "jsonc") && rest.starts_with("/*") {
            end = i + rest[2..].find("*/").map_or(rest.len(), |n| n + 4);
            kind = "comment";
        } else if family == "markup" && rest.starts_with("<!--") {
            end = i + rest[4..].find("-->").map_or(rest.len(), |n| n + 7);
            kind = "comment";
        } else if matches!(ch, '\'' | '"' | '`') {
            let triple = (family == "hash") && rest.starts_with(&ch.to_string().repeat(3));
            let opening = if triple { 3 } else { 1 };
            end = i + opening;
            while end < source.len() {
                let next = source[end..].chars().next()?;
                if next == '\\' {
                    end += 1;
                    if end < source.len() {
                        end += source[end..].chars().next()?.len_utf8();
                    }
                } else if triple && source[end..].starts_with(&ch.to_string().repeat(3)) {
                    end += 3;
                    break;
                } else {
                    end += next.len_utf8();
                    if !triple && next == ch {
                        break;
                    }
                }
            }
            kind = if family == "data" && source[end..].trim_start().starts_with(':') {
                "key"
            } else {
                "string"
            };
        } else if ch.is_ascii_digit() {
            while end < source.len() && source.as_bytes()[end].is_ascii_alphanumeric() {
                end += 1;
            }
            kind = "number";
        } else if ch.is_alphabetic() || ch == '_' {
            while end < source.len() {
                let next = source[end..].chars().next()?;
                if next.is_alphanumeric() || next == '_' {
                    end += next.len_utf8();
                } else {
                    break;
                }
            }
            let word = &source[i..end];
            let lower = word.to_ascii_lowercase();
            if ["true", "false", "null", "none", "nil", "undefined"].contains(&lower.as_str()) {
                kind = "constant";
            } else if [
                "let",
                "var",
                "const",
                "fn",
                "func",
                "function",
                "return",
                "if",
                "else",
                "for",
                "while",
                "in",
                "class",
                "struct",
                "enum",
                "impl",
                "pub",
                "private",
                "public",
                "import",
                "from",
                "export",
                "async",
                "await",
                "try",
                "catch",
                "throw",
                "throws",
                "def",
                "with",
                "as",
                "match",
                "case",
                "switch",
                "break",
                "continue",
                "new",
                "type",
                "interface",
                "extends",
                "use",
                "mod",
                "mut",
                "self",
                "select",
                "where",
                "insert",
                "update",
                "delete",
                "create",
                "table",
                "join",
                "and",
                "or",
                "not",
            ]
            .contains(&lower.as_str())
            {
                kind = "keyword";
            } else if family == "hash" && source[end..].trim_start().starts_with([':', '=']) {
                kind = "key";
            } else if ch.is_uppercase() {
                kind = "type";
            }
        }
        if let Some((previous_kind, previous_text)) = output.last_mut()
            && *previous_kind == kind
        {
            *previous_text = &source[i - previous_text.len()..end];
        } else {
            output.push((kind, &source[i..end]));
        }
        i = end;
    }
    Some(output)
}
fn span(kind: &str, text: &str) -> String {
    if kind == "plain" {
        escape(text)
    } else {
        format!("<span class=\"token-{kind}\">{}</span>", escape(text))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strings_comments_and_html_are_escaped_and_separated() {
        let html = highlight(
            "const sample = \"<script>**literal**</script>\"; // comment\n",
            "ts",
        )
        .unwrap();
        assert!(
            html.contains("token-keyword")
                && html.contains("token-string")
                && html.contains("token-comment")
        );
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;**literal**&lt;/script&gt;"));
    }
    #[test]
    fn data_keys_booleans_and_unknown_languages() {
        let html = highlight("{\"ok\": true, \"count\": 42}", "json").unwrap();
        for token in ["token-key", "token-constant", "token-number"] {
            assert!(html.contains(token));
        }
        assert!(highlight("<x>", "unknown-language").is_none());
        assert!(
            highlight("let Résumé = \"你好 🌍\"", "swift")
                .unwrap()
                .contains("你好 🌍")
        );
    }
}
