//! Complete poe-code Markdown corpus. Original tests and their exact assertions
//! live under tools/poe-markdown-tests/upstream; this exercises the native parser.
use hey_boss::markdown::render_native_document;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

fn children(n: &Value) -> &[Value] {
    n["children"].as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn string<'a>(n: &'a Value, key: &str) -> &'a str {
    n[key].as_str().unwrap_or("")
}
fn checked(n: &Value) -> Option<bool> {
    match string(n, "type") {
        "task" => n["checked"].as_bool(),
        "list" => None,
        _ => children(n).iter().find_map(checked),
    }
}
fn semantic(n: &Value) -> Vec<Value> {
    let kind = string(n, "type");
    if matches!(kind, "frontmatter" | "task") {
        return vec![];
    }
    let mut nested: Vec<Value> = vec![];
    for child in children(n).iter().flat_map(semantic) {
        if child["type"] == "text" && nested.last().is_some_and(|v| v["type"] == "text") {
            let last = nested.last_mut().unwrap();
            last["value"] = json!(format!(
                "{}{}",
                string(last, "value"),
                string(&child, "value")
            ));
        } else {
            nested.push(child);
        }
    }
    if kind == "container" {
        return nested;
    }
    if matches!(kind, "text" | "html" | "softBreak" | "break") {
        return vec![json!({"type":"text","value":n["value"].as_str().unwrap_or("\n")})];
    }
    if kind == "listItem" {
        let mut blocks: Vec<Value> = vec![];
        for child in nested {
            if matches!(
                string(&child, "type"),
                "text"
                    | "emphasis"
                    | "strong"
                    | "strikethrough"
                    | "inlineCode"
                    | "link"
                    | "image"
                    | "footnoteReference"
            ) {
                if !blocks.last().is_some_and(|v| v["type"] == "paragraph") {
                    blocks.push(json!({"type":"paragraph","children":[]}));
                }
                blocks.last_mut().unwrap()["children"]
                    .as_array_mut()
                    .unwrap()
                    .push(child);
            } else {
                blocks.push(child);
            }
        }
        nested = blocks;
    }
    let mut result = json!({"type":kind});
    match kind {
        "code" => {
            let value = string(n, "value");
            result["value"] = json!(value.strip_suffix('\n').unwrap_or(value));
            result["lang"] = json!(string(n, "lang").split_whitespace().next().unwrap_or(""));
        }
        "inlineCode" => result["value"] = n["value"].clone(),
        "heading" => result["depth"] = n["depth"].clone(),
        "list" => result["start"] = json!(n["start"].as_u64().unwrap_or(0)),
        "listItem" => result["checked"] = json!(n["checked"].as_bool().or_else(|| checked(n))),
        "alert" => result["kind"] = json!(string(n, "value").to_uppercase()),
        "table" => result["align"] = n["align"].clone(),
        "link" | "image" => {
            result["url"] = json!(string(n, "url"));
            if kind == "image" {
                result["alt"] = json!(
                    nested
                        .iter()
                        .map(|v| string(v, "value"))
                        .collect::<String>()
                );
            }
        }
        "footnoteDefinition" | "footnoteReference" => result["label"] = n["value"].clone(),
        _ => {}
    }
    if !matches!(
        kind,
        "text" | "code" | "inlineCode" | "image" | "thematicBreak" | "footnoteReference"
    ) {
        result["children"] = json!(nested);
    }
    vec![result]
}
fn collapse_whitespace(text: &str) -> String {
    let mut result = String::new();
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !space {
                result.push(' ')
            };
            space = true;
        } else {
            result.push(c);
            space = false;
        }
    }
    result
}
fn normalize(n: &mut Value) {
    if n["type"] == "text" {
        n["value"] = json!(collapse_whitespace(string(n, "value")));
    }
    if let Some(children) = n.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            normalize(child);
        }
    }
    if n["type"] == "root"
        && let Some(children) = n.get_mut("children").and_then(Value::as_array_mut)
    {
        for child in children {
            if child["type"] == "text" {
                child["value"] = json!(string(child, "value").trim_end());
            }
        }
    }
}
fn validate(n: &Value, lines: usize) {
    let start = n["lineStart"].as_u64().unwrap() as usize;
    let end = n["lineEnd"].as_u64().unwrap() as usize;
    assert!(
        (1..=lines).contains(&start) && end >= start && end <= lines,
        "Invalid source lines: {n}"
    );
    if let Some(tokens) = n["tokens"].as_array() {
        assert_eq!(
            tokens
                .iter()
                .map(|t| string(t, "value"))
                .collect::<String>(),
            string(n, "value"),
            "Code tokens changed source"
        );
    }
    if let Some(url) = n["url"].as_str() {
        assert!(!url.to_ascii_lowercase().starts_with("javascript:"));
    }
    for child in children(n) {
        validate(child, lines);
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/poe-markdown-suite.json")).unwrap()
}

#[test]
fn complete_upstream_suite_is_preserved_byte_for_byte() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/poe-markdown-tests");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(base.join("upstream-manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["testFiles"].as_array().unwrap().len(), 4);
    for (file, digest) in manifest["files"].as_object().unwrap() {
        let bytes = std::fs::read(base.join("upstream").join(file)).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            digest.as_str().unwrap(),
            "Changed upstream file: {file}"
        );
    }
    let corpus = corpus();
    assert_eq!(corpus["testCount"], 346);
    assert_eq!(corpus["tests"].as_array().unwrap().len(), 346);
    assert_eq!(corpus["revision"], manifest["revision"]);
}

#[test]
fn every_upstream_markdown_input_matches_native_document_semantics() {
    let corpus = corpus();
    let dialects: Value =
        serde_json::from_str(include_str!("fixtures/poe-markdown-dialects.json")).unwrap();
    let mut native = vec![];
    let mut failures = vec![];
    let mut used = 0;
    for fixture in corpus["documents"].as_array().unwrap() {
        let source = string(fixture, "source");
        let ast: Value = serde_json::from_str(&render_native_document(source)).unwrap();
        validate(
            &ast,
            source
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .split('\n')
                .count(),
        );
        let mut actual = semantic(&ast).remove(0);
        normalize(&mut actual);
        let difference = &dialects[string(fixture, "id")];
        let expected = if difference.is_null() {
            &fixture["expected"]
        } else {
            used += 1;
            assert!(!string(difference, "reason").is_empty());
            assert_ne!(
                difference["expected"], fixture["expected"],
                "Remove obsolete dialect override"
            );
            &difference["expected"]
        };
        if &actual != expected {
            failures.push(format!(
                "{} {:?}\nsource={source:?}\nexpected={expected}\nactual={actual}",
                fixture["id"], fixture["tests"]
            ));
        }
        native.push(json!({"name":fixture["tests"][0],"ast":ast}));
    }
    assert_eq!(
        used,
        dialects.as_object().unwrap().len(),
        "Orphaned dialect expectations"
    );
    assert!(
        failures.is_empty(),
        "{} native regressions:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    if let Ok(path) = std::env::var("HEY_BOSS_NATIVE_CORPUS_OUTPUT") {
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"documents":native,"renderNodes":corpus["renderNodes"]}))
                .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn every_upstream_code_sample_preserves_exact_source_and_fallback() {
    for fixture in corpus()["code"].as_array().unwrap() {
        let source = string(fixture, "source");
        if let Some(tokens) = hey_boss::syntax::tokens(source, string(fixture, "lang")) {
            assert_eq!(
                tokens.iter().map(|(_, value)| *value).collect::<String>(),
                source,
                "{}",
                fixture["tests"]
            );
            assert!(tokens.iter().any(|(_, value)| !value.is_empty()));
        }
        // Unknown language families are still native monospaced code, lossless.
        let markdown = format!("```{}\n{source}\n```", string(fixture, "lang"));
        let ast: Value = serde_json::from_str(&render_native_document(&markdown)).unwrap();
        assert_eq!(ast["children"][0]["type"], "code");
        assert_eq!(ast["children"][0]["value"], format!("{source}\n"));
    }
}
