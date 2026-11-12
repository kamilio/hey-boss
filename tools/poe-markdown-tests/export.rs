//! Rebuild the independent reference corpus from a complete upstream Vitest run.
#[path = "../../tests/markdown_poe_suite/semantics.rs"]
mod semantics;
use semantics::canonical;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

fn read(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}
fn id(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..16].to_owned()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Corpus {
    revision: Value,
    test_count: usize,
    tests: Vec<Value>,
    documents: Vec<Value>,
    code: Vec<Value>,
    render_nodes: Vec<Value>,
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let base = root.join("tools/poe-markdown-tests");
    let report = read(&base.join("report.json"));
    let total = report["numTotalTests"].as_u64().unwrap() as usize;
    assert!(
        total > 0
            && report["success"] == true
            && report["numPassedTests"] == total
            && report["numFailedTests"] == 0
            && report["numPendingTests"] == 0,
        "upstream suite did not pass"
    );
    let mut cases = BTreeMap::new();
    for suite in report["testResults"].as_array().unwrap() {
        for test in suite["assertionResults"].as_array().unwrap() {
            assert_eq!(test["status"], "passed");
            let name = test["ancestorTitles"]
                .as_array()
                .unwrap()
                .iter()
                .chain([&test["title"]])
                .map(|s| s.as_str().unwrap())
                .collect::<Vec<_>>()
                .join(" > ");
            let calls = test["meta"]["calls"]
                .as_array()
                .expect("missing reference observations");
            assert!(
                cases.insert(name, calls).is_none(),
                "duplicate upstream test name"
            );
        }
    }
    assert_eq!(cases.len(), total);
    let mut tests = vec![];
    let mut sources: BTreeMap<String, Value> = BTreeMap::new();
    let mut codes = BTreeMap::new();
    let mut render_nodes = vec![];
    for (name, calls) in cases {
        let mut operations = vec![];
        for call in calls {
            let op = call["operation"].as_str().unwrap();
            if !operations.contains(&op) {
                operations.push(op);
            }
            if let Some(source) = call["source"].as_str() {
                let id = id(source);
                let fixture = sources.entry(id.clone()).or_insert_with(|| {
                    json!({
                        "id": id,
                        "source": source,
                        "expected": canonical(&call["ast"]),
                        "tests": []
                    })
                });
                assert_eq!(fixture["source"], source, "Markdown fixture ID collision");
                fixture["tests"].as_array_mut().unwrap().push(json!(name));
            } else if op == "highlightCodeBlock" {
                let arg = &call["code"];
                let source = arg["value"].as_str().unwrap();
                let lang = arg["lang"].as_str().unwrap_or("");
                let id = id(&format!("{lang}\0{source}"));
                let mut sample = json!({"id":id,"source":source,"lang":lang});
                if let Some(tokens) = call.get("result") {
                    sample["tokens"] = tokens.clone();
                }
                sample["tests"] = json!([name]);
                codes.insert(id, sample);
            } else if !call["ast"].is_null() {
                render_nodes.push(json!({"test":name,"ast":call["ast"]}));
            }
        }
        tests.push(json!({"name":name,"operations":operations}));
    }
    let corpus = Corpus {
        revision: read(&base.join("upstream-manifest.json"))["revision"].clone(),
        test_count: total,
        tests,
        documents: sources.into_values().collect(),
        code: codes.into_values().collect(),
        render_nodes,
    };
    let mut output = serde_json::to_vec_pretty(&corpus).unwrap();
    output.push(b'\n');
    fs::write(root.join("tests/fixtures/poe-markdown-suite.json"), output).unwrap();
    println!(
        "{total} tests; {} Markdown inputs; {} code inputs; {} direct renderer inputs",
        corpus.documents.len(),
        corpus.code.len(),
        corpus.render_nodes.len()
    );
}
