//! Complete poe-code Markdown corpus. Original tests and their exact assertions
//! live under tools/poe-markdown-tests/upstream; this exercises the native parser.
use hey_boss::markdown::render_native_document;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

#[path = "markdown_poe_suite/semantics.rs"]
mod semantics;
use semantics::{canonical, children, string};

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
        let actual = canonical(&ast);
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

/// Diagnostic report for a specific installed CLI. Include known dialect differences.
#[test]
#[ignore = "runs the installed CLI against the full upstream corpus"]
fn compare_installed_markdown() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("out");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir_in(&out).unwrap();
    let input = dir.path().join("in.md");
    let output = dir.path().join("out.json");
    let cli = std::env::var_os("HEY_BOSS_CLI_PATH").unwrap_or_else(|| "hey-boss".into());
    let corpus = corpus();
    let documents = corpus["documents"].as_array().unwrap();
    let mut mismatches = vec![];
    for item in documents {
        std::fs::write(&input, string(item, "source")).unwrap();
        let run = std::process::Command::new(&cli)
            .arg("render-markdown")
            .arg(&input)
            .arg(&output)
            .arg("--native")
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "fixture {}: {}",
            item["id"],
            String::from_utf8_lossy(&run.stderr)
        );
        let ast = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let actual = canonical(&ast);
        if actual != item["expected"] {
            mismatches.push(json!({"id":item["id"],"tests":item["tests"],"source":item["source"],"expected":item["expected"],"actual":actual}));
        }
    }
    std::fs::write(
        out.join("poe-mismatches.json"),
        serde_json::to_vec_pretty(&mismatches).unwrap(),
    )
    .unwrap();
    println!(
        "{}/{} match; mismatches in out/poe-mismatches.json",
        documents.len() - mismatches.len(),
        documents.len()
    );
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
