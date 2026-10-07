//! Responses tool search carried by ordinary Gemini function calls. Client
//! lookups stay client-executed; hosted lookups select exact catalog paths.
use super::{ConvertedRequest, ProviderConfig, ReasoningCodec, Tool, convert_request};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn native_name() -> String {
    super::native_tool_name("$tool_search")
}

pub(crate) fn describe_catalog(tools: &BTreeMap<String, Tool>, declarations: &mut [Value]) {
    let Some(search) = tools
        .get(&native_name())
        .filter(|t| t.search.as_deref() == Some("server"))
    else {
        return;
    };
    let catalog: Vec<_> = tools
        .values()
        .filter(|t| t.deferred)
        .map(|t| {
            let path = t
                .namespace
                .as_ref()
                .map(|ns| format!("{ns}.{}", t.name))
                .unwrap_or_else(|| t.name.clone());
            json!({"path":path,"description":t.definition.get("description"),"namespaces":t.scopes})
        })
        .collect();
    if let Some(declaration) = declarations.iter_mut().find(|d| d["name"] == native_name()) {
        let description = search.definition["description"]
            .as_str()
            .unwrap_or("Load tools needed for the task.");
        declaration["description"] = json!(format!(
            "{description}\nChoose exact tool paths or namespace paths from this catalog. A namespace path loads its tools. Schemas are returned by this call; wait for the result before using newly loaded tools. Catalog: {}",
            json!(catalog)
        ));
    }
}

impl ConvertedRequest {
    pub fn has_hosted_search(&self) -> bool {
        self.tools
            .values()
            .any(|t| t.search.as_deref() == Some("server"))
    }

    fn search(&self, arguments: &Value) -> Result<Vec<Value>> {
        let paths = arguments["paths"]
            .as_array()
            .ok_or_else(|| anyhow!("Hosted tool_search requires paths"))?;
        let mut selected = BTreeMap::<String, &Tool>::new();
        for path in paths {
            let path = path
                .as_str()
                .ok_or_else(|| anyhow!("tool_search paths must be strings"))?;
            for (native, tool) in &self.tools {
                if tool.search.is_some() {
                    continue;
                }
                let full = tool
                    .namespace
                    .as_ref()
                    .map(|ns| format!("{ns}.{}", tool.name))
                    .unwrap_or_else(|| tool.name.clone());
                if full == path || full.starts_with(&format!("{path}.")) {
                    selected.insert(native.clone(), tool);
                }
            }
        }
        fn insert(output: &mut Vec<Value>, scopes: &[Value], definition: &Value) {
            let Some((scope, rest)) = scopes.split_first() else {
                output.push(definition.clone());
                return;
            };
            let index = output
                .iter()
                .position(|v| v["type"] == "namespace" && v["name"] == scope["name"])
                .unwrap_or_else(|| {
                    let mut namespace = scope.clone();
                    namespace["tools"] = json!([]);
                    output.push(namespace);
                    output.len() - 1
                });
            insert(
                output[index]["tools"].as_array_mut().unwrap(),
                rest,
                definition,
            );
        }
        let mut output = Vec::new();
        for tool in selected.into_values() {
            insert(&mut output, &tool.scopes, &tool.definition);
        }
        Ok(output)
    }
}

/// State for the proxy's hosted discovery loop. Every native turn retains its
/// own authenticated carrier, and every loaded definition is returned to the
/// caller so a subsequent stateless request can reconstruct the same tool set.
pub struct HostedSearch {
    request: Value,
    output: Vec<Value>,
    native_turns: Vec<Value>,
    usage: Option<Value>,
    fields: serde_json::Map<String, Value>,
}
impl HostedSearch {
    pub fn new(request: Value, converted: &ConvertedRequest) -> Self {
        Self {
            request,
            output: Vec::new(),
            native_turns: Vec::new(),
            usage: None,
            fields: converted.response_fields.clone(),
        }
    }

    pub fn failed_response(&self, id: &str, error: Value) -> Value {
        json!({"id":id,"object":"response","model":self.request["model"],"status":"failed",
            "output":self.output,"usage":self.usage,"error":error,"gemini":{"turns":self.native_turns}})
    }

    /// Append this completed turn, resolve hosted searches, and prepare the next
    /// model request. Stop before executing any client-owned tool calls.
    pub fn advance(
        &mut self,
        response: &mut Value,
        converted: &ConvertedRequest,
        config: &ProviderConfig,
        codec: &ReasoningCodec,
    ) -> Result<Option<ConvertedRequest>> {
        let mut turn = response["output"]
            .as_array()
            .ok_or_else(|| anyhow!("Missing response output"))?
            .clone();
        let mut loaded = Vec::new();
        if response["status"] == "completed" {
            for item in &turn {
                if item["type"] == "tool_search_call" && item["execution"] == "server" {
                    loaded.push(json!({"id":format!("{}_output",item["id"].as_str().unwrap_or("search")),"type":"tool_search_output","execution":"server","call_id":null,"status":"completed","tools":converted.search(&item["arguments"])?}));
                }
            }
        }
        let client_call = turn.iter().any(|i| {
            matches!(
                i["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            ) || (i["type"] == "tool_search_call" && i["execution"] == "client")
        });
        turn.extend(loaded.clone());
        self.output.extend(turn.clone());
        self.native_turns.push(response["gemini"].clone());
        if let Some(usage) = response.get("usage").filter(|v| v.is_object()) {
            let total = self.usage.get_or_insert_with(|| json!({"input_tokens":0,"output_tokens":0,"total_tokens":0,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}));
            for field in ["input_tokens", "output_tokens", "total_tokens"] {
                total[field] = json!(
                    total[field]
                        .as_u64()
                        .unwrap_or(0)
                        .saturating_add(usage[field].as_u64().unwrap_or(0))
                );
            }
            for (field, detail) in [
                ("input_tokens_details", "cached_tokens"),
                ("output_tokens_details", "reasoning_tokens"),
            ] {
                total[field][detail] = json!(
                    total[field][detail]
                        .as_u64()
                        .unwrap_or(0)
                        .saturating_add(usage[field][detail].as_u64().unwrap_or(0))
                );
            }
        }
        response["output"] = json!(self.output);
        response["usage"] = self.usage.clone().unwrap_or(Value::Null);
        response["gemini"] = json!({"turns":self.native_turns});
        response
            .as_object_mut()
            .unwrap()
            .extend(self.fields.clone());
        if loaded.is_empty() || client_call {
            return Ok(None);
        }
        if let Some(limit) = self.fields.get("max_output_tokens").and_then(Value::as_u64) {
            let spent = self
                .usage
                .as_ref()
                .and_then(|u| u["output_tokens"].as_u64())
                .ok_or_else(|| {
                    anyhow!("Hosted tool_search requires usage to enforce max_output_tokens")
                })?;
            if spent >= limit {
                response["status"] = json!("incomplete");
                response["incomplete_details"] = json!({"reason":"max_output_tokens"});
                return Ok(None);
            }
            self.request["max_output_tokens"] = json!(limit - spent);
        }
        if self.native_turns.len() >= 16 {
            bail!("Gemini hosted tool_search exceeded 16 discovery turns");
        }
        let input = self.request["input"].clone();
        let mut items = if let Some(text) = input.as_str() {
            vec![json!({"role":"user","content":text})]
        } else {
            input
                .as_array()
                .ok_or_else(|| anyhow!("input must be text or an array"))?
                .clone()
        };
        items.extend(turn);
        self.request["input"] = json!(items);
        // A forced search is satisfied by the discovery step; subsequent turns
        // may choose the newly loaded function or complete with an answer.
        if self.request.pointer("/tool_choice/type") == Some(&json!("tool_search")) {
            self.request["tool_choice"] = json!("auto");
        }
        Ok(Some(convert_request(&self.request, config, codec)?))
    }
}
