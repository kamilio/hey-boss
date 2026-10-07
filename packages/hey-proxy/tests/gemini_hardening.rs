use hey_proxy::gemini::*;
use serde_json::{Value, json};

fn config() -> ProviderConfig {
    serde_json::from_value(json!({"api_key":"synthetic"})).unwrap()
}
fn codec() -> ReasoningCodec {
    ReasoningCodec::new(&[77; 32])
}
fn request(custom: bool) -> ConvertedRequest {
    let tool = if custom {
        json!({"type":"custom","name":"run"})
    } else {
        json!({"type":"function","name":"run","parameters":{"type":"object"}})
    };
    convert_request(
        &json!({"model":"gemini/test","input":"hi","tools":[tool]}),
        &config(),
        &codec(),
    )
    .unwrap()
}
fn chunk(parts: Value) -> Value {
    json!({"candidates":[{"content":{"role":"model","parts":parts}}]})
}
fn executable_done(events: &[Value]) -> bool {
    events.iter().any(|e| {
        matches!(
            e["type"].as_str(),
            Some("response.function_call_arguments.done" | "response.custom_tool_call_input.done")
        ) || (e["type"] == "response.output_item.done"
            && matches!(
                e["item"]["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            ))
    })
}

#[test]
fn malformed_native_structures_fail_in_unary_and_streaming() {
    let mut cases = vec![
        Value::Null,
        json!([]),
        json!({"candidates":null}),
        json!({"candidates":{}}),
        json!({"candidates":[null]}),
        json!({"candidates":[{},{}]}),
        json!({"usageMetadata":[]}),
        json!({"usageMetadata":{"thoughtsTokenCount":-1}}),
        json!({"promptFeedback":{"blockReason":true}}),
    ];
    for bad in [
        json!(null),
        json!([]),
        json!({}),
        json!({"parts":null}),
        json!({"role":"user","parts":[]}),
    ] {
        cases.push(json!({"candidates":[{"content":bad,"finishReason":"STOP"}]}));
    }
    for bad in [json!(-1), json!(1), json!("0"), json!(null)] {
        cases.push(json!({"candidates":[{"index":bad,"finishReason":"STOP"}]}));
    }
    for part in [
        json!(null),
        json!("text"),
        json!({"text":null}),
        json!({"text":3}),
        json!({"thought":"true"}),
        json!({"thoughtSignature":[]}),
        json!({"functionCall":null}),
        json!({"text":"lost","functionCall":{"name":"run","args":{}}}),
        json!({"functionCall":{"name":"run","args":[]}}),
        json!({"functionCall":{"id":4}}),
        json!({"functionCall":{"willContinue":"true"}}),
    ] {
        cases.push(chunk(json!([part])));
    }
    for native in cases {
        assert!(
            convert_response(&native, &request(false), &codec(), "bad").is_err(),
            "{native}"
        );
        let mut stream = ResponseStream::new(request(false), "bad");
        assert!(stream.feed(&native).is_err(), "{native}");
        assert!(
            stream
                .feed(&json!({"candidates":[{"finishReason":"STOP"}]}))
                .is_err()
        );
        assert!(stream.finish(&codec()).is_err());
    }
}

#[test]
fn malformed_native_input_cannot_panic_when_merging_following_user_turn() {
    for content in [
        json!({"role":"user","parts":null}),
        json!({"role":"user"}),
        json!({"parts":[]}),
        json!({"role":"invalid","parts":[]}),
        json!({"role":"user","parts":[{"text":1}]}),
    ] {
        let input = json!({"model":"gemini/test","input":[{"type":"gemini_content","content":content},{"role":"user","content":"next"}]});
        assert!(convert_request(&input, &config(), &codec()).is_err());
    }
}

#[test]
fn tool_completion_requires_successful_finish_and_transport_eof() {
    for custom in [false, true] {
        for reason in [
            None,
            Some("STOP"),
            Some("MAX_TOKENS"),
            Some("SAFETY"),
            Some("MALFORMED_FUNCTION_CALL"),
        ] {
            let req = request(custom);
            let name = req.tools.keys().next().unwrap().clone();
            let mut native = chunk(
                json!([{"functionCall":{"name":name,"args":if custom {json!({"input":"echo hello"})} else {json!({"cmd":"echo hello"})}},"thoughtSignature":"signature"}]),
            );
            if let Some(reason) = reason {
                native["candidates"][0]["finishReason"] = json!(reason);
            }
            let unary = convert_response(&native, &req, &codec(), "tool").unwrap();
            assert_eq!(
                unary["output"][1]["status"],
                if reason == Some("STOP") {
                    "completed"
                } else {
                    "incomplete"
                }
            );
            let mut stream = ResponseStream::new(req, "tool");
            let early = stream.feed(&native).unwrap();
            assert!(!executable_done(&early));
            // Usage may arrive after the finish marker; retain it before completing.
            stream
                .feed(&json!({"usageMetadata":{"promptTokenCount":5,"thoughtsTokenCount":7}}))
                .unwrap();
            let final_events = stream.finish(&codec()).unwrap();
            assert_eq!(executable_done(&final_events), reason == Some("STOP"));
            assert_eq!(
                final_events.last().unwrap()["response"]["usage"]["output_tokens"],
                7
            );
            if reason == Some("STOP") {
                let carrier = final_events
                    .iter()
                    .position(|e| {
                        e["type"] == "response.output_item.done" && e["output_index"] == 0
                    })
                    .unwrap();
                let tool = final_events
                    .iter()
                    .position(|e| {
                        e["type"] == "response.function_call_arguments.done"
                            || e["type"] == "response.custom_tool_call_input.done"
                    })
                    .unwrap();
                assert!(carrier < tool);
            }
        }
    }
}

#[test]
fn error_after_tool_prefix_is_terminal_and_preserves_diagnostics() {
    let req = request(false);
    let name = req.tools.keys().next().unwrap().clone();
    let mut stream = ResponseStream::new(req, "broken");
    let prefix = chunk(json!([{"functionCall":{"name":name,"args":{"cmd":"unsafe-if-partial"}}}]));
    let early = stream.feed(&prefix).unwrap();
    assert!(!executable_done(&early));
    let failed = stream
        .feed(&json!({"error":{"message":"failure"}}))
        .unwrap()
        .remove(0);
    assert_eq!(failed["type"], "response.failed");
    assert_eq!(failed["response"]["output"], json!([]));
    assert_eq!(failed["response"]["error"]["code"], "server_error");
    assert_eq!(
        failed["response"]["gemini"]["candidates"][0]["content"]["parts"],
        prefix["candidates"][0]["content"]["parts"]
    );
    assert!(
        failed["sequence_number"].as_u64().unwrap()
            > early.last().unwrap()["sequence_number"].as_u64().unwrap()
    );
    assert!(stream.finish(&codec()).is_err());
}

#[test]
fn sparse_partial_arguments_have_a_cumulative_allocation_bound() {
    let req = request(false);
    let name = req.tools.keys().next().unwrap().clone();
    let mut stream = ResponseStream::new(req, "sparse");
    let mut path = String::from("$.x");
    for _ in 0..5 {
        path.push_str("[65535]");
    }
    let result = stream.feed(&chunk(json!([{"functionCall":{"name":name,"partialArgs":[{"jsonPath":path,"stringValue":"small input, huge allocation"}]}}])));
    assert!(result.unwrap_err().to_string().contains("allocated slots"));
    assert!(stream.finish(&codec()).is_err());
}

#[test]
fn ambiguous_partial_calls_are_rejected() {
    for mixed in [true, false] {
        let req = request(false);
        let name = req.tools.keys().next().unwrap().clone();
        let mut stream = ResponseStream::new(req, "ambiguous");
        let mut part = json!({"functionCall":{"name":name,"id":"duplicate","willContinue":true}});
        if mixed {
            part["functionCall"]["args"] = json!({"lost":"value"});
            assert!(stream.feed(&chunk(json!([part]))).is_err());
        } else {
            stream.feed(&chunk(json!([part.clone()]))).unwrap();
            assert!(stream.feed(&chunk(json!([part]))).is_err());
        }
    }
}

#[test]
fn many_small_text_fragments_preserve_full_output_and_signatures() {
    let mut stream = ResponseStream::new(request(false), "large");
    let fragment = "🌍λ".repeat(20);
    let part = chunk(json!([{"text":fragment}]));
    let start = std::time::Instant::now();
    for _ in 0..8192 {
        stream.feed(&part).unwrap();
    }
    stream.feed(&json!({"candidates":[{"content":{"parts":[{"thoughtSignature":"late-signature"}]},"finishReason":"STOP"}]})).unwrap();
    let events = stream.finish(&codec()).unwrap();
    let response = &events.last().unwrap()["response"];
    assert_eq!(
        response["output"][1]["content"][0]["text"],
        fragment.repeat(8192)
    );
    assert_eq!(
        response["gemini"]["candidates"][0]["content"]["parts"][0]["thoughtSignature"],
        "late-signature"
    );
    eprintln!(
        "8192 text chunks / {} bytes incl final encryption: {:?}",
        fragment.len() * 8192,
        start.elapsed()
    );
}

#[test]
fn interrupted_partial_calls_retain_the_exact_native_trace() {
    let req = request(false);
    let name = req.tools.keys().next().unwrap().clone();
    let part = json!({"functionCall":{"name":name,"id":"partial","willContinue":true,
        "partialArgs":[{"jsonPath":"$.command","stringValue":"unfinished"}]},
        "thoughtSignature":"signed-partial"});
    let mut stream = ResponseStream::new(req, "partial");
    stream.feed(&chunk(json!([part]))).unwrap();
    assert!(stream.finish(&codec()).is_err());
    let failed = stream.fail("interrupted", "incomplete arguments");
    assert_eq!(
        failed["response"]["gemini"]["streamFunctionCallParts"],
        json!([part])
    );
    assert_eq!(failed["response"]["output"], json!([]));
}

#[test]
fn incremental_projection_preserves_indices_across_mixed_signed_parts_and_calls() {
    let raw_request = json!({
        "model": "gemini/gemini-2.5-pro",
        "input": "Execute mixed turn",
        "tools": [
            {
                "type": "tool_search",
                "execution": "client",
                "description": "Search deferred tools",
                "parameters": {
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"]
                }
            },
            {
                "type": "function",
                "name": "check_file",
                "strict": true,
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"],
                    "additionalProperties": false
                }
            },
            {
                "type": "namespace",
                "name": "editor",
                "tools": [{
                    "type": "custom",
                    "name": "patch",
                    "description": "Apply patch"
                }]
            },
            {
                "type": "function",
                "name": "deferred_tool",
                "defer_loading": true,
                "parameters": {"type": "object"}
            }
        ]
    });
    let converted = convert_request(&raw_request, &config(), &codec()).unwrap();
    let search_native = converted
        .tools
        .iter()
        .find(|(_, t)| t.name == "tool_search")
        .unwrap()
        .0
        .clone();
    let fn_native = converted
        .tools
        .iter()
        .find(|(_, t)| t.name == "check_file")
        .unwrap()
        .0
        .clone();
    let custom_native = converted
        .tools
        .iter()
        .find(|(_, t)| t.custom && t.name == "patch")
        .unwrap()
        .0
        .clone();

    let stream_chunks = vec![
        chunk(json!([{"text": ""}])),
        chunk(json!([{"text": "α"}])),
        chunk(json!([{"text": "β"}])),
        chunk(json!([{"thoughtSignature": "sig-text-1"}])),
        chunk(json!([{"text": "", "thoughtSignature": "sig-empty-carrier"}])),
        chunk(json!([{"text": "γδ", "thoughtSignature": "sig-text-2"}])),
        chunk(json!([{"text": "ε"}])),
        chunk(json!([{"text": "plan-step", "thought": true, "thoughtSignature": "sig-thought"}])),
        chunk(json!([{"text": "ζ", "thoughtSignature": "sig-text-3"}])),
        chunk(json!([{
            "functionCall": {"name": search_native, "args": {"query": "deferred"}, "id": "search-1"}
        }])),
        chunk(json!([{"thoughtSignature": "sig-search"}])),
        chunk(json!([{
            "functionCall": {
                "name": fn_native,
                "id": "fn-1",
                "willContinue": true,
                "partialArgs": [{"jsonPath": "$.path", "stringValue": "src/", "willContinue": true}]
            }
        }])),
        chunk(json!([{
            "functionCall": {
                "id": "fn-1",
                "willContinue": false,
                "partialArgs": [{"jsonPath": "$.path", "stringValue": "main.rs", "willContinue": false}]
            },
            "thoughtSignature": "sig-fn"
        }])),
        chunk(json!([{
            "functionCall": {"name": custom_native, "args": {"input": "patch-body"}, "id": "custom-1"},
            "thoughtSignature": "sig-custom"
        }])),
        chunk(json!([{"inlineData": {"mimeType": "image/png", "data": "AA=="}}])),
        chunk(json!([{"thoughtSignature": "sig-inline"}])),
        json!({"candidates": [{"finishReason": "STOP"}], "usageMetadata": {"promptTokenCount": 20, "candidatesTokenCount": 10, "thoughtsTokenCount": 5, "totalTokenCount": 35}}),
    ];

    let mut stream = ResponseStream::new(converted.clone(), "mixed");
    let mut events = Vec::new();
    for c in &stream_chunks {
        events.extend(stream.feed(c).unwrap());
    }
    events.extend(stream.finish(&codec()).unwrap());

    for (seq, event) in events.iter().enumerate() {
        assert_eq!(event["sequence_number"], seq);
    }

    let added_indices: Vec<usize> = events
        .iter()
        .filter(|e| e["type"] == "response.output_item.added")
        .map(|e| e["output_index"].as_u64().unwrap() as usize)
        .collect();
    assert_eq!(added_indices, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);

    let done_indices: Vec<usize> = events
        .iter()
        .filter(|e| e["type"] == "response.output_item.done")
        .map(|e| e["output_index"].as_u64().unwrap() as usize)
        .collect();
    assert_eq!(done_indices, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);

    let final_response = &events.last().unwrap()["response"];
    let unary = convert_response(&final_response["gemini"], &converted, &codec(), "mixed").unwrap();
    assert_eq!(final_response["output"].as_array().unwrap().len(), 9);
    assert_eq!(
        final_response["output"].as_array().unwrap()[1..],
        unary["output"].as_array().unwrap()[1..]
    );
    assert_eq!(final_response["output"][1]["content"][0]["text"], "αβ");
    assert_eq!(final_response["output"][2]["content"][0]["text"], "γδ");
    assert_eq!(final_response["output"][3]["content"][0]["text"], "ε");
    assert_eq!(final_response["output"][4]["content"][0]["text"], "ζ");
    assert_eq!(final_response["output"][5]["type"], "tool_search_call");
    assert_eq!(final_response["output"][6]["type"], "function_call");
    assert_eq!(
        final_response["output"][6]["arguments"],
        "{\"path\":\"src/main.rs\"}"
    );
    assert_eq!(final_response["output"][7]["type"], "custom_tool_call");
    assert_eq!(final_response["output"][7]["input"], "patch-body");
    assert_eq!(
        final_response["output"][8]["content"][0]["part"]["thoughtSignature"],
        "sig-inline"
    );

    let mut replay_input: Vec<Value> = events
        .iter()
        .filter(|e| e["type"] == "response.output_item.done")
        .map(|e| e["item"].clone())
        .collect();
    replay_input.extend([
        json!({"type": "tool_search_output", "execution": "client", "call_id": "search-1", "status": "completed", "tools": []}),
        json!({"type": "function_call_output", "call_id": "fn-1", "output": "ok"}),
        json!({"type": "custom_tool_call_output", "call_id": "custom-1", "output": "applied"}),
    ]);
    let mut replay_req = raw_request;
    replay_req["input"] = json!(replay_input);
    let replayed = convert_request(&replay_req, &config(), &codec()).unwrap();
    let expected_native_parts =
        final_response["gemini"]["candidates"][0]["content"]["parts"].clone();
    assert_eq!(replayed.body["contents"][0]["parts"], expected_native_parts);
}

#[test]
fn detached_signature_must_not_leave_stale_native_projection() {
    let codec = codec();
    let req = convert_request(
        &json!({"model":"gemini/gemini-2.5-pro","input":"Check final projection"}),
        &config(),
        &codec,
    )
    .unwrap();
    let mut stream = ResponseStream::new(req.clone(), "projection");
    stream.feed(&chunk(json!([{}]))).unwrap();
    stream
        .feed(&json!({"candidates":[{"content":{"parts":[{"thoughtSignature":"detached"}]},"finishReason":"STOP"}]}))
        .unwrap();

    // The empty part becomes a signature-only carrier. Rejecting this stream
    // is acceptable; a successful response must match the final native parts
    // and remain replayable, rather than completing a stale visible item.
    if let Ok(events) = stream.finish(&codec) {
        let response = &events.last().unwrap()["response"];
        let unary = convert_response(&response["gemini"], &req, &codec, "projection").unwrap();
        assert_eq!(
            &response["output"].as_array().unwrap()[1..],
            &unary["output"].as_array().unwrap()[1..]
        );
        let replay: Vec<Value> = events
            .iter()
            .filter(|event| event["type"] == "response.output_item.done")
            .map(|event| event["item"].clone())
            .collect();
        convert_request(
            &json!({"model":"gemini/gemini-2.5-pro","input":replay}),
            &config(),
            &codec,
        )
        .unwrap();
    }
}

#[test]
#[ignore = "offline release benchmark; run with --release --ignored --nocapture"]
fn benchmark_stream_many_signed_parts_and_tool_calls() {
    let tools: Vec<Value> = (0..16)
        .map(|i| {
            json!({
                "type": "function",
                "name": format!("tool_{i}"),
                "strict": true,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "line": {"type": "integer"},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "line", "content"],
                    "additionalProperties": false
                }
            })
        })
        .collect();
    let req = convert_request(
        &json!({"model": "gemini/gemini-2.5-pro", "input": "benchmark", "tools": tools}),
        &config(),
        &codec(),
    )
    .unwrap();
    let tool_names: Vec<String> = req.tools.keys().cloned().collect();
    let codec = codec();
    let payload = "x".repeat(256);

    for count in [128usize, 256, 512, 1024] {
        let signed_text_chunks: Vec<Value> = (0..count)
            .map(|i| {
                chunk(json!([{
                    "text": format!("segment-{i}-{payload}"),
                    "thoughtSignature": format!("sig-text-{i}")
                }]))
            })
            .collect();
        let tool_call_chunks: Vec<Value> = (0..count)
            .map(|i| {
                let name = &tool_names[i % tool_names.len()];
                chunk(json!([{
                    "functionCall": {
                        "name": name,
                        "id": format!("call-{i}"),
                        "args": {
                            "path": format!("src/module_{i}.rs"),
                            "line": i,
                            "content": payload
                        }
                    },
                    "thoughtSignature": format!("sig-call-{i}")
                }]))
            })
            .collect();
        let stop = json!({"candidates": [{"finishReason": "STOP"}]});

        let iterations = 5u32;
        let mut text_feed_total = std::time::Duration::ZERO;
        let mut text_total = std::time::Duration::ZERO;
        for _ in 0..iterations {
            let mut stream = ResponseStream::new(req.clone(), "bench-text");
            let start = std::time::Instant::now();
            for c in &signed_text_chunks {
                stream.feed(c).unwrap();
            }
            stream.feed(&stop).unwrap();
            text_feed_total += start.elapsed();
            let events = stream.finish(&codec).unwrap();
            text_total += start.elapsed();
            assert_eq!(
                events.last().unwrap()["response"]["output"]
                    .as_array()
                    .unwrap()
                    .len(),
                count + 1
            );
        }

        let mut tool_feed_total = std::time::Duration::ZERO;
        let mut tool_total = std::time::Duration::ZERO;
        for _ in 0..iterations {
            let mut stream = ResponseStream::new(req.clone(), "bench-tool");
            let start = std::time::Instant::now();
            for c in &tool_call_chunks {
                stream.feed(c).unwrap();
            }
            stream.feed(&stop).unwrap();
            tool_feed_total += start.elapsed();
            let events = stream.finish(&codec).unwrap();
            tool_total += start.elapsed();
            assert_eq!(
                events.last().unwrap()["response"]["output"]
                    .as_array()
                    .unwrap()
                    .len(),
                count + 1
            );
        }

        eprintln!(
            "count={count:4} | signed_text feed={:?} total={:?} | tool_calls feed={:?} total={:?}",
            text_feed_total / iterations,
            text_total / iterations,
            tool_feed_total / iterations,
            tool_total / iterations,
        );
    }
}
