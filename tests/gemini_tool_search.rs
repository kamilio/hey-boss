use hey_proxy::gemini::*;
use serde_json::{Value, json};

fn config() -> ProviderConfig {
    serde_json::from_value(json!({"api_key":"synthetic"})).unwrap()
}
fn codec() -> ReasoningCodec {
    ReasoningCodec::new(&[19; 32])
}
fn convert(request: &Value) -> ConvertedRequest {
    convert_request(request, &config(), &codec()).unwrap()
}
fn tool() -> Value {
    json!({"type":"namespace","name":"crm","description":"Customer records and orders","tools":[{
        "type":"function","name":"lookup","description":"Find customer orders","defer_loading":true,"strict":true,
        "parameters":{"type":"object","properties":{"customer":{"type":"string"}},"required":["customer"],"additionalProperties":false}
    }]})
}
fn request(execution: &str) -> Value {
    let mut search =
        json!({"type":"tool_search","execution":execution,"description":"Discover project tools"});
    if execution == "client" {
        search["parameters"] =
            json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]});
    }
    json!({"model":"gemini/test","input":"Find customer C42","tools":[search,tool()]})
}
fn native(name: &str, args: Value) -> Value {
    json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":name,"args":args,"id":"native-call"},"thoughtSignature":"opaque-signature"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2,"totalTokenCount":12}})
}
fn search_name(converted: &ConvertedRequest) -> String {
    converted.body["tools"][0]["functionDeclarations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] != native_tool_name("crm.lookup"))
        .unwrap()["name"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn client_search_stream_and_signed_replay_load_exact_namespaced_schema() {
    let mut request = request("client");
    request["stream"] = json!(true);
    let converted = convert(&request);
    assert_eq!(
        converted.body["tools"][0]["functionDeclarations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let name = search_name(&converted);
    let original = native(&name, json!({"query":"customer orders 日本語"}));
    let mut stream = ResponseStream::new(converted, "search");
    let mut events = stream.feed(&original).unwrap();
    events.extend(stream.finish(&codec()).unwrap());
    let response = &events.last().unwrap()["response"];
    let call = &response["output"][1];
    assert_eq!(call["type"], "tool_search_call");
    assert_eq!(call["execution"], "client");
    assert_eq!(call["call_id"], "native-call");
    assert_eq!(call["arguments"], json!({"query":"customer orders 日本語"}));
    assert!(call.get("name").is_none());
    let mut input: Vec<Value> = events
        .iter()
        .filter(|e| e["type"] == "response.output_item.done")
        .map(|e| e["item"].clone())
        .collect();
    assert_eq!(input[0]["type"], "reasoning");
    input.push(json!({"type":"tool_search_output","execution":"client","call_id":"native-call","status":"completed","tools":[tool()]}));
    request["input"] = json!(input);
    let next = convert(&request);
    assert_eq!(
        next.body["contents"][0],
        original["candidates"][0]["content"]
    );
    assert_eq!(
        next.body["contents"][1]["parts"][0]["functionResponse"]["id"],
        "native-call"
    );
    let declarations = next.body["tools"][0]["functionDeclarations"]
        .as_array()
        .unwrap();
    assert_eq!(declarations.len(), 2);
    let found = declarations
        .iter()
        .find(|d| d["name"] == native_tool_name("crm.lookup"))
        .unwrap();
    assert_eq!(
        found["parametersJsonSchema"],
        tool()["tools"][0]["parameters"]
    );
    let result = convert_response(
        &native(&native_tool_name("crm.lookup"), json!({"customer":"C42"})),
        &next,
        &codec(),
        "lookup",
    )
    .unwrap();
    assert_eq!(result["output"][1]["namespace"], "crm");
    assert_eq!(result["output"][1]["name"], "lookup");
    assert!(
        convert_response(
            &native(&native_tool_name("crm.lookup"), json!({"customer":42})),
            &next,
            &codec(),
            "invalid"
        )
        .is_err()
    );
    request["input"][1]["execution"] = json!("server");
    assert!(convert_request(&request, &config(), &codec()).is_err());
}

#[test]
fn hosted_search_continues_with_loaded_tool_and_replays_combined_output() {
    let request = request("server");
    let first = convert(&request);
    assert!(first.has_hosted_search());
    let name = search_name(&first);
    let declaration = &first.body["tools"][0]["functionDeclarations"][0];
    assert!(
        declaration["description"]
            .as_str()
            .unwrap()
            .contains("Customer records and orders")
    );
    assert!(
        !declaration["description"]
            .as_str()
            .unwrap()
            .contains("additionalProperties")
    );
    let native_search = native(&name, json!({"paths":["crm"]}));
    let mut response = convert_response(&native_search, &first, &codec(), "search").unwrap();
    let mut session = HostedSearch::new(request.clone(), &first);
    let next = session
        .advance(&mut response, &first, &config(), &codec())
        .unwrap()
        .unwrap();
    assert_eq!(response["output"][1]["execution"], "server");
    assert!(response["output"][1]["call_id"].is_null());
    assert_eq!(response["output"][2]["tools"], json!([tool()]));
    assert_eq!(
        next.body["contents"][1],
        native_search["candidates"][0]["content"]
    );
    let native_lookup = native(&native_tool_name("crm.lookup"), json!({"customer":"C42"}));
    let mut response = convert_response(&native_lookup, &next, &codec(), "lookup").unwrap();
    assert!(
        session
            .advance(&mut response, &next, &config(), &codec())
            .unwrap()
            .is_none()
    );
    assert_eq!(response["usage"]["total_tokens"], 24);
    assert_eq!(response["gemini"]["turns"].as_array().unwrap().len(), 2);
    assert_eq!(response["output"].as_array().unwrap().len(), 5);
    let mut replay = request.clone();
    let mut input = vec![json!({"role":"user","content":"Find customer C42"})];
    input.extend(response["output"].as_array().unwrap().clone());
    input.push(json!({"type":"function_call_output","call_id":"native-call","output":"Found"}));
    replay["input"] = json!(input);
    let replay = convert(&replay);
    assert_eq!(
        replay.body["contents"][1],
        native_search["candidates"][0]["content"]
    );
    assert_eq!(
        replay.body["contents"][3],
        native_lookup["candidates"][0]["content"]
    );
}

#[test]
fn imported_search_history_loads_new_tools_without_top_level_declarations() {
    for execution in ["client", "server"] {
        let id = if execution == "client" {
            json!("search-1")
        } else {
            Value::Null
        };
        let request = json!({"model":"gemini/test","input":[
            {"type":"tool_search_call","execution":execution,"call_id":id,"arguments":{"paths":["crm"]}},
            {"type":"tool_search_output","execution":execution,"call_id":id,"status":"completed","tools":[tool()]}
        ]});
        let converted = convert(&request);
        assert_eq!(
            converted.body["tools"][0]["functionDeclarations"][0]["name"],
            native_tool_name("crm.lookup")
        );
        assert_eq!(
            converted.body["contents"][1]["parts"][0]["functionResponse"]["response"]["result"]["tools"],
            json!([tool()])
        );
    }
}

#[test]
fn discovery_rejects_unloaded_calls_conflicting_schemas_and_unknown_results() {
    let request = request("server");
    let first = convert(&request);
    assert!(
        convert_response(
            &native(&native_tool_name("crm.lookup"), json!({"customer":"C42"})),
            &first,
            &codec(),
            "early"
        )
        .is_err()
    );
    let mut bad = request.clone();
    bad["input"] = json!([{"type":"tool_search_output","execution":"client","call_id":"missing","tools":[tool()]}]);
    assert!(convert_request(&bad, &config(), &codec()).is_err());
    bad["input"] = json!([
        {"type":"tool_search_call","execution":"client","call_id":"search","arguments":{}},
        {"type":"tool_search_output","execution":"client","call_id":"search","tools":[tool()]}
    ]);
    bad["input"][1]["tools"][0]["tools"][0]["parameters"]["required"] = json!([]);
    assert!(convert_request(&bad, &config(), &codec()).is_err());
}

#[test]
fn forced_search_is_released_and_repeat_discovery_is_idempotent() {
    let mut request = request("server");
    request["tool_choice"] = json!({"type":"tool_search"});
    let mut converted = convert(&request);
    assert_eq!(
        converted.body["toolConfig"]["functionCallingConfig"]["mode"],
        "ANY"
    );
    let mut session = HostedSearch::new(request, &converted);
    for round in 0..2 {
        let mut response = convert_response(
            &native(
                &search_name(&converted),
                json!({"paths":["crm","crm.lookup"]}),
            ),
            &converted,
            &codec(),
            &format!("search{round}"),
        )
        .unwrap();
        converted = session
            .advance(&mut response, &converted, &config(), &codec())
            .unwrap()
            .unwrap();
        assert_eq!(
            converted.body["tools"][0]["functionDeclarations"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            converted.body["toolConfig"]["functionCallingConfig"]["mode"],
            "AUTO"
        );
    }
}

#[test]
fn additional_tools_activate_deferred_definitions_and_can_declare_search() {
    let request = json!({"model":"gemini/test","input":[
        {"role":"user","content":"Find C42"},
        {"type":"additional_tools","role":"developer","tools":[
            {"type":"tool_search","execution":"client","parameters":{"type":"object","properties":{}}},
            tool()
        ]}
    ]});
    let converted = convert(&request);
    assert_eq!(
        converted.body["tools"][0]["functionDeclarations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        convert_response(
            &native(&native_tool_name("crm.lookup"), json!({"customer":"C42"})),
            &converted,
            &codec(),
            "added"
        )
        .is_ok()
    );
}

#[test]
fn hosted_discovery_shares_output_token_budget() {
    for limit in [2, 10] {
        let mut request = request("server");
        request["max_output_tokens"] = json!(limit);
        let first = convert(&request);
        let mut session = HostedSearch::new(request, &first);
        let mut response = convert_response(
            &native(&search_name(&first), json!({"paths":["crm"]})),
            &first,
            &codec(),
            "budget",
        )
        .unwrap();
        let next = session
            .advance(&mut response, &first, &config(), &codec())
            .unwrap();
        if limit == 2 {
            assert!(next.is_none());
            assert_eq!(response["status"], "incomplete");
            assert_eq!(
                response["incomplete_details"]["reason"],
                "max_output_tokens"
            );
        } else {
            assert_eq!(next.unwrap().body["generationConfig"]["maxOutputTokens"], 8);
        }
    }
}

#[test]
fn incomplete_search_is_not_completed_for_execution() {
    let converted = convert(&request("client"));
    let mut original = native(&search_name(&converted), json!({"query":"orders"}));
    original["candidates"][0]["finishReason"] = json!("MAX_TOKENS");
    let mut stream = ResponseStream::new(converted, "incomplete");
    let mut events = stream.feed(&original).unwrap();
    events.extend(stream.finish(&codec()).unwrap());
    assert!(!events.iter().any(
        |e| e["type"] == "response.output_item.done" && e["item"]["type"] == "tool_search_call"
    ));
    assert_eq!(events.last().unwrap()["type"], "response.incomplete");
}

#[test]
fn hosted_search_groups_namespace_results_and_deduplicates_paths() {
    let mut request = request("server");
    let mut second = tool()["tools"][0].clone();
    second["name"] = json!("orders");
    request["tools"][1]["tools"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let first = convert(&request);
    let mut response = convert_response(
        &native(&search_name(&first), json!({"paths":["crm","crm.lookup"]})),
        &first,
        &codec(),
        "grouped",
    )
    .unwrap();
    let mut session = HostedSearch::new(request.clone(), &first);
    let next = session
        .advance(&mut response, &first, &config(), &codec())
        .unwrap()
        .unwrap();
    assert_eq!(response["output"][2]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(
        response["output"][2]["tools"][0]["tools"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        response["output"][2]["tools"][0]["description"],
        "Customer records and orders"
    );
    assert_eq!(
        next.body["tools"][0]["functionDeclarations"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}
