//! Fill transport revision guards from the same backend that receives the write.
use hey_boss::issues::{self, Error, Operation, Request, Result, Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn missing(value: &Value) -> bool {
    value.is_null() || value.as_i64() == Some(0)
}

fn artifact_edit(value: &mut Value) -> &mut Value {
    if value["command"] == "import" {
        artifact_edit(&mut value["operation"])
    } else {
        value
    }
}

fn needs_snapshot(value: &Value, supervisor: bool) -> bool {
    match value["action"].as_str().unwrap_or_default() {
        "assign" | "transfer" | "hold_attempt" | "reconcile_attempt" | "set_yolo" | "bind_plan" => {
            missing(&value["if_version"])
        }
        "edit" => value["if_version"].is_null() && (supervisor || value["draft"] == true),
        "reopen" | "set_blockers" => supervisor && value["if_version"].is_null(),
        "ready" => value["guard"].is_null(),
        "close" => supervisor && value["guard"].is_null(),
        "batch" => value["edits"]
            .as_array()
            .is_some_and(|edits| edits.iter().any(|edit| missing(&edit["if_version"]))),
        "artifact" => {
            let mut inner = &value["operation"];
            while inner["command"] == "import" {
                inner = &inner["operation"];
            }
            matches!(
                inner["command"].as_str(),
                Some("edit" | "archive" | "delete")
            ) && missing(&inner["if_version"])
        }
        _ => false,
    }
}

fn logical_command(value: &mut Value, template: &Value) {
    if let (Some(value), Some(template)) = (value.as_object_mut(), template.as_object()) {
        for (key, expected) in template {
            if (key == "if_version" && missing(expected)) || (key == "guard" && expected.is_null())
            {
                value.remove(key);
            } else if let Some(actual) = value.get_mut(key) {
                logical_command(actual, expected);
            }
        }
    } else if let (Some(value), Some(template)) = (value.as_array_mut(), template.as_array()) {
        for (actual, expected) in value.iter_mut().zip(template) {
            logical_command(actual, expected);
        }
    }
}

// Artifact receipts retain content hashes instead of uploaded bytes.
fn receipt_payload(operation: &Value) -> Result<Value> {
    let mut value = operation.clone();
    if value["action"] == "artifact" && value["operation"]["command"] == "import" {
        for file in value["operation"]["files"].as_array_mut().unwrap() {
            let bytes = hey_boss::attachments::decode(file["data"].as_str().unwrap())?;
            let object = file.as_object_mut().unwrap();
            object.remove("data");
            object.insert(
                "sha256".into(),
                json!(format!("{:x}", Sha256::digest(bytes))),
            );
        }
    }
    Ok(value)
}

fn replay_receipt(
    request: &Request,
    operation: &Value,
    authority: bool,
    supervisor: bool,
    call: &mut impl FnMut(&Request, bool) -> Result<Value>,
) -> Result<Option<Value>> {
    if let Some(id) = &request.request_id {
        let mut read = request.clone();
        read.request_id = None;
        read.operation = Operation::RequestStatus { id: id.clone() };
        let receipt = call(&read, authority || operation["action"] == "artifact")?;
        if receipt["request"]["state"] == "recorded" {
            let mut saved_operation = receipt["request"]["operation"].clone();
            let mut saved = saved_operation.clone();
            let mut wanted = receipt_payload(operation)?;
            logical_command(&mut saved, operation);
            logical_command(&mut wanted, operation);
            if saved != wanted {
                return Err(Error::conflict(
                    "Request ID was already used for a different command",
                ));
            }
            if operation["action"] == "artifact" && operation["operation"]["command"] == "import" {
                saved_operation["operation"]["files"] = operation["operation"]["files"].clone();
            }
            // Replay the saved guards through the normal route so its receipt
            // lookup also preserves companion routing and response metadata.
            let mut request = request.clone();
            request.operation = serde_json::from_value(saved_operation)?;
            return call(&request, supervisor).map(Some);
        }
    }
    Ok(None)
}

fn prepare(
    request: &Request,
    supervisor: bool,
    mut call: impl FnMut(&Request, bool) -> Result<Value>,
) -> Result<Value> {
    if !request.operation.writes() {
        return call(request, supervisor);
    }
    let mut request = request.clone();
    let mut operation = serde_json::to_value(&request.operation)?;
    // Keep the absent wire field for compatibility, but normalize the CLI
    // template so receipt comparison ignores only automatically captured guards.
    if operation["action"] == "ready" || supervisor && operation["action"] == "close" {
        if operation["action"] == "close" && operation["force"] == true {
            return Err(Error::invalid(
                "--supervisor close does not support --force",
            ));
        }
        operation
            .as_object_mut()
            .unwrap()
            .entry("guard")
            .or_insert(Value::Null);
    }
    let automatic = needs_snapshot(&operation, supervisor);
    let template = operation.clone();
    let authority = supervisor
        || matches!(operation["action"].as_str(), Some("assign" | "ready"))
        || (operation["action"] == "edit" && operation["draft"] == true);
    if automatic {
        let mut read = request.clone();
        read.request_id = None;
        if let Some(result) =
            replay_receipt(&request, &operation, authority, supervisor, &mut call)?
        {
            return Ok(result);
        }
        if operation["action"] == "artifact" {
            let edit = artifact_edit(&mut operation["operation"]);
            read.operation = Operation::Artifact {
                operation: hey_boss::artifacts::Operation::View {
                    id: edit["id"].as_str().unwrap().into(),
                },
            };
            edit["if_version"] = call(&read, authority)?["artifact"]["version"].clone();
        } else if operation["action"] == "batch" {
            for edit in operation["edits"].as_array_mut().unwrap() {
                if missing(&edit["if_version"]) {
                    read.operation = Operation::View {
                        number: edit["number"].as_i64().unwrap(),
                    };
                    edit["if_version"] = call(&read, authority)?["issue"]["version"].clone();
                }
            }
        } else {
            read.operation = Operation::View {
                number: operation["number"].as_i64().unwrap(),
            };
            let current = call(&read, authority)?;
            if matches!(operation["action"].as_str(), Some("ready" | "close")) {
                operation["guard"] = current["ready_guard"].clone();
            } else {
                operation["if_version"] = current["issue"]["version"].clone();
            }
        }
        request.operation = serde_json::from_value(operation)?;
    }
    if request.request_id.is_none()
        && request.operation.writes()
        && (supervisor
            || matches!(
                request.operation,
                Operation::Ready { guard: Some(_), .. } | Operation::Batch { .. }
            ))
    {
        let key = serde_json::to_vec(&json!([
            request.project,
            request.project_override,
            request.actor.as_ref().map(|a| &a.id),
            request.operation
        ]))?;
        request.request_id = Some(format!("cli-{:x}", Sha256::digest(key)));
    }
    let result = call(&request, supervisor);
    // Another identical caller may commit between our receipt lookup and
    // snapshot. Replay only its proven receipt, never refresh a failed guard.
    if automatic
        && result.as_ref().is_err_and(|error| error.code == "conflict")
        && let Some(saved) = replay_receipt(&request, &template, authority, supervisor, &mut call)?
    {
        return Ok(saved);
    }
    result
}

pub(crate) fn execute(request: &Request, host: Option<&str>, supervisor: bool) -> Result<Value> {
    let mut store = if host.is_none() {
        Some(Store::open(&issues::database_path()?)?)
    } else {
        None
    };
    prepare(request, supervisor, |request, authority| match host {
        Some(host) => issues::remote::call(host, request),
        None if authority => store.as_mut().unwrap().execute_supervisor(request),
        None => store.as_mut().unwrap().execute(request),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(operation: Value) -> Request {
        serde_json::from_value(json!({"version":1,"project":{"id":"named:Test","name":"Test"},"actor":{"id":"codex:owner","kind":"codex","session_id":"owner","machine":"local","host":"local","pid":null,"process_start":null,"cwd":"/tmp","source":"test"},"operation":operation})).unwrap()
    }

    #[test]
    fn assignment_reads_authority_and_preserves_conflict_without_retrying() {
        let request =
            request(json!({"action":"assign","number":1,"target":"github","if_version":0}));
        let mut calls = 0;
        let error = prepare(&request, false, |r, authority| {
            calls += 1;
            match &r.operation {
                Operation::View { number: 1 } => {
                    assert!(authority);
                    Ok(json!({"issue":{"version":7}}))
                }
                Operation::Assign { if_version: 7, .. } => Err(Error::conflict("Concurrent edit")),
                other => panic!("Unexpected request {other:?}"),
            }
        })
        .unwrap_err();
        assert_eq!(error.code, "conflict");
        assert_eq!(calls, 2);
    }

    #[test]
    fn failed_authoritative_read_never_falls_back_to_local_write() {
        let request =
            request(json!({"action":"assign","number":1,"target":"github","if_version":0}));
        let mut calls = 0;
        let error = prepare(&request, false, |_, authority| {
            calls += 1;
            assert!(authority);
            Err(Error::new("offline", "Disconnected"))
        })
        .unwrap_err();
        assert_eq!(error.code, "offline");
        assert_eq!(calls, 1);
    }

    #[test]
    fn lifecycle_retry_recovers_a_receipt_committed_before_its_snapshot() {
        for action in ["ready", "close"] {
            let mut request = request(if action == "ready" {
                json!({"action":action,"number":1,"force":false})
            } else {
                json!({"action":action,"number":1,"force":false,"comment":"Verified"})
            });
            request.request_id = Some("lifecycle-once".into());
            let mut original = serde_json::to_value(&request.operation).unwrap();
            original["guard"] =
                json!({"if_version":7,"expected_assignee":null,"expected_reservation":"before"});
            let mut calls = 0;
            let result = prepare(&request, true, |r, authority| {
                assert!(authority);
                calls += 1;
                match calls {
                    1 => Ok(json!({"request":{"state":"not_recorded"}})),
                    2 => Ok(json!({"ready_guard":{"if_version":8,"expected_assignee":null,"expected_reservation":"after"}})),
                    3 => Err(Error::conflict("Request ID used with different guards")),
                    4 => Ok(json!({"request":{"state":"recorded","operation":original}})),
                    5 => {
                        assert_eq!(serde_json::to_value(&r.operation).unwrap(), original);
                        Ok(json!({"ok":true,"issue":{"state":if action == "ready" {"ready"} else {"closed"}}}))
                    }
                    _ => panic!("Unexpected retry"),
                }
            }).unwrap();
            assert_eq!(result["ok"], true);
            assert_eq!(calls, 5);
        }
    }

    #[test]
    fn close_retries_a_lost_acknowledgement_without_repeating_the_write() {
        let mut request =
            request(json!({"action":"close","number":1,"force":false,"comment":"Verified"}));
        request.request_id = Some("lost-close".into());
        let mut saved = None;
        let mut writes = 0;
        let mut call = |r: &Request, authority: bool| {
            assert!(authority);
            match &r.operation {
                Operation::RequestStatus { .. } => Ok(match &saved {
                    Some(operation) => {
                        json!({"request":{"state":"recorded","operation":operation}})
                    }
                    None => json!({"request":{"state":"not_recorded"}}),
                }),
                Operation::View { .. } => Ok(
                    json!({"ready_guard":{"if_version":1,"expected_assignee":null,"expected_reservation":"snapshot"}}),
                ),
                Operation::Close { .. } if saved.is_none() => {
                    writes += 1;
                    saved = Some(serde_json::to_value(&r.operation).unwrap());
                    Err(Error::new(
                        "fleet_unavailable",
                        "Acknowledgement lost after commit",
                    ))
                }
                Operation::Close { .. } => Ok(json!({"ok":true,"issue":{"state":"closed"}})),
                _ => panic!("Unexpected request"),
            }
        };
        assert_eq!(
            prepare(&request, true, &mut call).unwrap_err().code,
            "fleet_unavailable"
        );
        assert_eq!(
            prepare(&request, true, &mut call).unwrap()["issue"]["state"],
            "closed"
        );
        assert_eq!(writes, 1);
    }

    #[test]
    fn legacy_explicit_guards_are_not_replaced() {
        let request =
            request(json!({"action":"assign","number":1,"target":"github","if_version":2}));
        let mut calls = 0;
        prepare(&request, false, |r, _| {
            calls += 1;
            assert!(matches!(
                r.operation,
                Operation::Assign { if_version: 2, .. }
            ));
            Ok(json!({"ok":true}))
        })
        .unwrap();
        assert_eq!(calls, 1);
    }
}
