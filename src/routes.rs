//! Resource routing shared with every embedded web view through routes.json.
use crate::issues::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

pub const DEFINITIONS: &str = include_str!("issues/web/routes.json");
pub fn javascript() -> String {
    include_str!("issues/web/routes.js").replace("/* ROUTE_DEFINITIONS */ []", DEFINITIONS)
}
#[derive(Deserialize)]
struct Rule {
    paths: Vec<String>,
    #[serde(default)]
    when: HashMap<String, String>,
    present: Option<String>,
    selector: Option<String>,
    query_selector: Option<String>,
    #[serde(default)]
    number: bool,
    entity: String,
    collection: String,
}
#[derive(Debug, Serialize)]
pub struct Route {
    pub entity: String,
    pub id: String,
    pub project: Option<String>,
    pub host: Option<String>,
    pub params: HashMap<String, String>,
}
fn parameters(value: &str) -> Result<HashMap<String, String>> {
    // Decode form parameters directly: reparsing a URL would interpret a literal
    // '#' inside the fragment's parameters as another fragment boundary.
    let bytes = value.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (i + 2 >= bytes.len()
                || !bytes[i + 1].is_ascii_hexdigit()
                || !bytes[i + 2].is_ascii_hexdigit())
        {
            return Err(Error::invalid("Invalid URL parameter encoding"));
        }
    }
    fn decode(value: &str) -> Result<String> {
        let bytes = value.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            decoded.push(match bytes[index] {
                b'+' => b' ',
                b'%' => {
                    let high = (bytes[index + 1] as char).to_digit(16).unwrap();
                    let low = (bytes[index + 2] as char).to_digit(16).unwrap();
                    index += 2;
                    (high * 16 + low) as u8
                }
                byte => byte,
            });
            index += 1;
        }
        String::from_utf8(decoded).map_err(|_| Error::invalid("URL parameters must be UTF-8"))
    }
    let mut params = HashMap::new();
    for part in value.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        // URLSearchParams.get uses the first duplicate, not the last.
        params.entry(decode(key)?).or_insert(decode(value)?);
    }
    Ok(params)
}
pub fn resolve(input: &str) -> Result<Route> {
    if input.len() > 8192 || input.chars().any(char::is_control) {
        return Err(Error::invalid(
            "URL must be at most 8192 bytes without control characters",
        ));
    }
    let url = reqwest::Url::parse(input.trim())
        .map_err(|_| Error::invalid("Paste a complete HTTP(S) hey-boss URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::invalid("Use an HTTP(S) URL without credentials"));
    }
    let params = parameters(url.fragment().unwrap_or(""))?;
    let query = parameters(url.query().unwrap_or(""))?;
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    let rule = RULES
        .get_or_init(|| serde_json::from_str(DEFINITIONS).expect("valid shared route definitions"))
        .iter()
        .find(|rule| {
            rule.paths.iter().any(|path| path == url.path())
                && rule
                    .when
                    .iter()
                    .all(|(key, value)| params.get(key) == Some(value))
                && rule
                    .present
                    .as_ref()
                    .is_none_or(|key| params.get(key).is_some_and(|v| !v.is_empty()))
                && rule
                    .query_selector
                    .as_ref()
                    .is_none_or(|key| query.get(key).is_some_and(|v| !v.is_empty()))
        })
        .ok_or_else(|| Error::invalid("This URL is not a hey-boss resource route"))?;
    let id = rule
        .query_selector
        .as_ref()
        .and_then(|key| query.get(key))
        .or_else(|| rule.selector.as_ref().and_then(|key| params.get(key)))
        .cloned()
        .unwrap_or_default();
    if !id.is_empty() {
        crate::issues::identifier(&id, "resource ID", 8192)?;
        if rule.number
            && (!id.bytes().all(|c| c.is_ascii_digit())
                || id.starts_with('0')
                || id
                    .parse::<u64>()
                    .ok()
                    .is_none_or(|number| number > 9_007_199_254_740_991))
        {
            return Err(Error::invalid(
                "Issue number must be a positive safe integer without leading zeros",
            ));
        }
    }
    if url.path() == "/agents/session"
        && (id.is_empty() || params.get("host").is_none_or(|host| host.is_empty()))
    {
        return Err(Error::invalid(
            "An agent conversation URL needs host and run",
        ));
    }
    if url.path() == "/project-resource" && id.is_empty() {
        return Err(Error::invalid("A project resource URL needs issue or node"));
    }
    if id.is_empty()
        && (params.get("new").is_some_and(|value| value == "1")
            || params.contains_key("quick-issue"))
    {
        return Err(Error::invalid(
            "This URL opens an editor, not an existing item",
        ));
    }
    Ok(Route {
        entity: if id.is_empty() {
            rule.collection.clone()
        } else {
            rule.entity.clone()
        },
        id,
        project: params.get("project").filter(|v| !v.is_empty()).cloned(),
        host: params.get("host").filter(|v| !v.is_empty()).cloned(),
        params,
    })
}
