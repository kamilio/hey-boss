use serde_json::{Value, json};

pub(super) fn children(n: &Value) -> &[Value] {
    n["children"].as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub(super) fn string<'a>(n: &'a Value, key: &str) -> &'a str {
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
        "list" => {
            result["start"] = json!(
                n["start"]
                    .as_u64()
                    .unwrap_or_else(|| u64::from(n["ordered"] == true))
            )
        }
        "listItem" => result["checked"] = json!(n["checked"].as_bool().or_else(|| checked(n))),
        "alert" => {
            result["kind"] = json!(
                n["kind"]
                    .as_str()
                    .unwrap_or_else(|| string(n, "value"))
                    .to_uppercase()
            )
        }
        "table" => {
            result["align"] = json!(
                n["align"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| a.as_str().unwrap_or("left"))
                    .collect::<Vec<_>>()
            )
        }
        "link" | "image" => {
            result["url"] = json!(string(n, "url"));
            if kind == "image" {
                result["alt"] = n
                    .get("alt")
                    .filter(|v| v.is_string())
                    .cloned()
                    .unwrap_or_else(|| {
                        json!(
                            nested
                                .iter()
                                .map(|v| string(v, "value"))
                                .collect::<String>()
                        )
                    });
            }
        }
        "footnoteDefinition" | "footnoteReference" => {
            result["label"] = n.get("label").unwrap_or(&n["value"]).clone()
        }
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
pub(super) fn canonical(ast: &Value) -> Value {
    let mut actual = semantic(ast).remove(0);
    normalize(&mut actual);
    actual
}
