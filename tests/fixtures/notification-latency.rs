//! Slow loopback relay for testing that the desktop stays responsive during notification actions.
//! cargo run --quiet --example notification-latency-fixture
use serde_json::{Value, json};
use std::{thread, time::Duration};
use tiny_http::{Header, Request, Response, Server};

fn reply(path: &str, body: &Value) -> (u16, Value) {
    let unavailable = || (503, json!({"error":"Synthetic relay unavailable"}));
    if path.ends_with("/clear") {
        let Some(ids) = body["taskIDs"].as_array() else {
            return (400, json!({"error":"Expected taskIDs"}));
        };
        if ids.iter().any(|id| id == "dismiss-fail") {
            return unavailable();
        }
        let Some(tasks) = body["tasks"].as_array() else {
            return (400, json!({"error":"Expected tasks"}));
        };
        let tasks: Vec<_> = tasks
            .iter()
            .map(|task| {
                let cancel = matches!(task["kind"].as_str(), Some("approval" | "prompt"))
                    || task["commentsEnabled"] == true;
                json!({"taskID":task["taskID"],"status":if cancel {"cancelled"} else {"ok"}})
            })
            .collect();
        return (200, json!({"tasks":tasks}));
    }
    let id = path
        .strip_suffix("/resolve")
        .and_then(|p| p.rsplit_once("/tasks/"))
        .map(|(_, id)| id);
    let Some(id) = id.filter(|id| !id.is_empty() && !id.contains('/')) else {
        return (404, json!({"error":"Unknown fixture path"}));
    };
    if id.ends_with("-fail") {
        return unavailable();
    }
    let mut task = json!({"taskID":id,"status":"ok"});
    if id == "phone-won" {
        task["result"] = json!("Phone answer");
        return (409, json!({"task":task}));
    }
    if let Some(result) = body.get("result") {
        task["result"] = result.clone();
    }
    (200, json!({"task":task}))
}

fn handle(mut req: Request) {
    let mut bytes = Vec::new();
    let body = req.as_reader().read_to_end(&mut bytes).ok().and_then(|_| {
        if bytes.is_empty() {
            Some(Value::Null)
        } else {
            serde_json::from_slice(&bytes).ok()
        }
    });
    thread::sleep(Duration::from_millis(700));
    let (code, value) = body.map_or_else(
        || (400, json!({"error":"Invalid JSON"})),
        |body| reply(req.url(), &body),
    );
    let _ = req.respond(
        Response::from_string(value.to_string())
            .with_status_code(code)
            .with_header(Header::from_bytes("Content-Type", "application/json").unwrap()),
    );
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = Server::http("127.0.0.1:0")?;
    println!("http://{}", server.server_addr());
    for req in server.incoming_requests() {
        thread::spawn(move || handle(req));
    }
    Ok(())
}
