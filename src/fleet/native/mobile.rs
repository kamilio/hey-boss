//! Authenticated HTTPS bridge. Pairing credentials never enter logs or argv.
use super::{
    Result,
    context::{Context, atomic_json, read_json},
    replica::{self, invalid},
};
use crate::issues::{Actor, Operation, Project, Request, Store};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    time::Duration,
};
struct Mobile {
    ctx: Context,
    client: reqwest::blocking::Client,
}
impl Mobile {
    fn rpc(&self, operation: Value, project: Option<&Value>, key: Option<String>) -> Result<Value> {
        let operation = serde_json::from_value::<Operation>(operation)?;
        let actor = operation.needs_actor().then(|| Actor {
            id: "human:boss".into(),
            kind: "human".into(),
            session_id: None,
            machine: self.ctx.node.clone(),
            host: crate::issues::identity::host(),
            pid: None,
            process_start: None,
            cwd: self.ctx.home.clone(),
            source: "phone".into(),
            invocation: None,
            creation_run: None,
        });
        let request = Request {
            version: 1,
            project: project
                .map(|p| serde_json::from_value::<Project>(json!({"id":p["id"],"name":p["name"]})))
                .transpose()?
                .unwrap_or(Project {
                    id: "named:Fleet".into(),
                    name: "Fleet".into(),
                }),
            project_override: project.and_then(|p| p["id"].as_str()).map(str::to_owned),
            actor,
            operation,
            request_id: key,
        };
        match Store::open(&self.ctx.path)?.execute(&request) {
            Ok(value) => Ok(value),
            Err(e) if matches!(e.code.as_str(), "invalid_input" | "not_found" | "conflict") => {
                Ok(json!({"ok":false,"error":e}))
            }
            Err(_) => Err("Issue store temporarily unavailable".into()),
        }
    }
    fn call(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        let pairing = self
            .ctx
            .path
            .parent()
            .ok_or_else(|| invalid("Issue database has no parent"))?
            .join("mobile.json");
        let config = read_json(&pairing, Value::Null)?;
        if config.is_null() {
            return Err("Mobile pairing is not configured".into());
        }
        let origin = config["url"]
            .as_str()
            .ok_or_else(|| invalid("Invalid mobile service origin"))?;
        let url =
            reqwest::Url::parse(origin).map_err(|_| invalid("Invalid mobile service origin"))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid("Invalid mobile service origin"));
        }
        let token = config["token"]
            .as_str()
            .ok_or_else(|| invalid("Mobile pairing is not configured"))?;
        let url = format!("{}{path}", origin.trim_end_matches('/'));
        let request = if let Some(body) = body {
            self.client.post(url).json(body)
        } else {
            self.client.get(url)
        };
        let response = request
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .send()
            .map_err(|_| invalid("Mobile service temporarily unavailable"))?;
        if !response.status().is_success() {
            return Err(invalid("Mobile service temporarily unavailable"));
        }
        let mut bytes = vec![];
        let limit = if path.starts_with("/api/bridge/checkpoint") {
            64 * 1024 * 1024
        } else {
            crate::issues::WIRE_LIMIT
        };
        response.take(limit as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(invalid("Mobile response exceeds limit"));
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    fn checkpoint(&self) -> Result<()> {
        let state = self.call("/api/bridge/checkpoint", None)?;
        let path = self
            .ctx
            .path
            .parent()
            .ok_or_else(|| invalid("Missing mobile state directory"))?
            .join("mobile-relay.json");
        if state["ready"] == false {
            let snapshot = read_json(&path, Value::Null)?;
            self.call(
                "/api/bridge/checkpoint/restore",
                Some(&json!({"snapshot":snapshot})),
            )?;
        } else if let Some(snapshot) = state.get("snapshot") {
            // atomic_json fsyncs both the private file and its parent directory.
            // Only then may Fly acknowledge pairing, answers, or publication.
            atomic_json(&path, snapshot)?;
            self.call(
                "/api/bridge/checkpoint/ack",
                Some(&json!({"epoch":state["epoch"],"version":state["version"]})),
            )?;
        }
        Ok(())
    }
    fn accepted(&self, project: &str, key: &str) -> Result<bool> {
        Ok(!replica::rows(
            &self.ctx.db()?,
            "SELECT 1 FROM requests WHERE project_id=? AND actor='human:boss' AND request_id=?",
            &[json!(project), json!(key)],
        )?
        .is_empty())
    }
    fn sync(&self) -> Result<()> {
        let registry = self.rpc(
            json!({"action":"projects","include_hidden":true}),
            None,
            None,
        )?;
        if registry["ok"] != true {
            return Err("Issue project registry unavailable".into());
        }
        let mut projects = BTreeMap::new();
        let mut visible = BTreeSet::new();
        for p in registry["projects"]
            .as_array()
            .ok_or_else(|| invalid("Invalid project registry"))?
        {
            let id = p["id"]
                .as_str()
                .ok_or_else(|| invalid("Invalid project ID"))?;
            projects.insert(
                id.to_owned(),
                json!({"id":p["id"],"name":p["name"],"name_collisions":p["name_collisions"]}),
            );
            if p["hidden_at"].is_null() {
                visible.insert(id.to_owned());
            }
        }
        self.call("/api/bridge/issue-projects",Some(&json!({"projects":projects.iter().filter(|(id,_)|visible.contains(*id)).map(|(_,p)|p).collect::<Vec<_>>()})))?;
        let creations = self.call("/api/bridge/issues", None)?;
        for creation in creations["creations"]
            .as_array()
            .ok_or_else(|| invalid("Invalid mobile creation queue"))?
        {
            let request_id = transport_id(&creation["requestID"])?;
            let selector = creation["project"].as_str().unwrap_or("");
            let project = projects.get(selector).or_else(|| {
                projects.values().find(|p| {
                    p["name"]
                        .as_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case(selector))
                })
            });
            let key = format!("mobile:{request_id}");
            let accepted = if let Some(project) = project {
                !visible.contains(project["id"].as_str().unwrap())
                    && self.accepted(project["id"].as_str().unwrap(), &key)?
            } else {
                false
            };
            let outcome = if project.is_none()
                || !visible.contains(project.unwrap()["id"].as_str().unwrap()) && !accepted
            {
                json!({"status":"error","error":"This project is no longer registered or is hidden. Choose a registered project and submit again."})
            } else {
                let value=self.rpc(json!({"action":"create","title":creation["title"],"body":creation["body"],"labels":creation["labels"],"at_top":true}),project,Some(key))?;
                if value["ok"] == true {
                    json!({"status":"synced","number":value["issue"]["number"]})
                } else {
                    json!({"status":"error","error":value["error"]["message"].as_str().unwrap_or("Unable to create issue").chars().take(1000).collect::<String>()})
                }
            };
            self.call(
                &format!("/api/bridge/issues/{request_id}/result"),
                Some(&outcome),
            )?;
        }
        self.artifacts(&projects, &visible)?;
        self.agents(&visible)?;
        Ok(())
    }
    fn web(&self, projects: &BTreeMap<String, Value>, visible: &BTreeSet<String>) -> Result<()> {
        let queue = self.call("/api/bridge/web", None)?;
        for request in queue["requests"].as_array().into_iter().flatten() {
            let id = transport_id(&request["id"])?;
            let result = self.web_request(request, projects, visible).unwrap_or_else(|_| {
                json!({"ok":false,"error":{"code":"unavailable","message":"Supervisor could not complete this request. Reconnect and retry."}})
            });
            self.call(&format!("/api/bridge/web/{id}/result"), Some(&result))?;
        }
        Ok(())
    }
    fn web_request(
        &self,
        request: &Value,
        projects: &BTreeMap<String, Value>,
        visible: &BTreeSet<String>,
    ) -> Result<Value> {
        let payload = &request["payload"];
        let mut value = match request["kind"].as_str() {
            Some("bootstrap") => {
                let project = projects
                    .values()
                    .find(|p| visible.contains(p["id"].as_str().unwrap_or("")));
                let mut value = self.rpc(
                    json!({"action":"projects","include_hidden":true}),
                    project,
                    None,
                )?;
                value["csrf"] = json!("paired-device");
                value["actor"] = json!({"id":"human:boss","kind":"human"});
                value["backend_host"] = Value::Null;
                value
            }
            Some("preview") => {
                let body = payload["body"]
                    .as_str()
                    .ok_or_else(|| invalid("Expected Markdown"))?;
                if body.len() > crate::issues::BODY_LIMIT {
                    return Err(invalid("Markdown exceeds 1 MiB"));
                }
                json!({"ok":true,"html":crate::markdown::render_fragment(body)})
            }
            Some("inbox") => {
                let action = serde_json::from_value::<crate::notices::Action>(payload.clone())?;
                if let crate::notices::Action::Link {
                    issue: Some(reference),
                    ..
                } = &action
                {
                    if !visible.contains(&reference.project) {
                        return Err(invalid("Project is hidden"));
                    }
                    self.rpc(
                        json!({"action":"view","number":reference.number}),
                        projects.get(&reference.project),
                        None,
                    )?;
                }
                crate::notices::execute(&action)?
            }
            Some("action") => {
                let operation = mobile_web_operation(payload)?;
                let project_id = payload["project"]
                    .as_str()
                    .ok_or_else(|| invalid("Expected project"))?;
                let new_project = json!({"id":project_id,"name":project_id.strip_prefix("named:").unwrap_or(project_id)});
                let creating_project = matches!(operation, Operation::Create { .. })
                    && project_id.starts_with("named:");
                let history_project = projects.values().find_map(|p| {
                    p["name_collisions"]
                        .as_array()
                        .and_then(|warnings| {
                            warnings
                                .iter()
                                .find(|w| w["legacy"] == true && w["rejected_id"] == project_id)
                        })
                        .map(|_| json!({"id":project_id,"name":p["name"],"canonical_id":p["id"]}))
                });
                if history_project.is_some() && operation.writes() {
                    return Err(invalid(
                        "This is saved legacy history. Use the project name for new work.",
                    ));
                }
                let project = projects
                    .get(project_id)
                    .or_else(|| {
                        projects.values().find(|p| {
                            p["name"]
                                .as_str()
                                .is_some_and(|name| name.eq_ignore_ascii_case(project_id))
                        })
                    })
                    .or(history_project.as_ref())
                    .or_else(|| creating_project.then_some(&new_project))
                    .ok_or_else(|| invalid("Unknown project"))?;
                let resolved_id = project["canonical_id"]
                    .as_str()
                    .or_else(|| project["id"].as_str())
                    .unwrap_or(project_id);
                let writing = operation.writes();
                let key = if writing {
                    Some(format!(
                        "web-mobile:{}",
                        transport_id(&payload["request_id"])?
                    ))
                } else {
                    None
                };
                let registry = matches!(
                    operation,
                    Operation::Projects { .. } | Operation::RestoreProject
                );
                if !visible.contains(resolved_id)
                    && !registry
                    && !(!projects.contains_key(project_id) && creating_project)
                    && !(writing && self.accepted(resolved_id, key.as_deref().unwrap())?)
                {
                    return Ok(
                        json!({"ok":false,"error":{"code":"not_found","message":"Project is hidden; restore it before accessing issues"}}),
                    );
                }
                if let Operation::Transfer { destination, .. } = &operation
                    && !visible.contains(destination)
                {
                    return Err(invalid("Destination project is hidden"));
                }
                self.rpc(serde_json::to_value(operation)?, Some(project), key)?
            }
            _ => return Err(invalid("Unknown mobile web request")),
        };
        if let Some(issue) = value.get_mut("issue")
            && let Some(body) = issue["body"].as_str()
        {
            issue["body_html"] = json!(crate::markdown::render_fragment(body));
        }
        if let Some(comments) = value["comments"].as_array_mut() {
            for comment in comments {
                if let Some(body) = comment["body"].as_str() {
                    comment["body_html"] = json!(crate::markdown::render_fragment(body));
                }
            }
        }
        if crate::mindmap::needs_inbox(&value) {
            crate::mindmap::enrich_notifications(
                &mut value,
                crate::notices::execute(&crate::notices::Action::List),
            )?;
        }
        Ok(value)
    }
    fn artifacts(
        &self,
        projects: &BTreeMap<String, Value>,
        visible: &BTreeSet<String>,
    ) -> Result<()> {
        let queue = self.call("/api/bridge/artifacts", None)?;
        for request in queue["requests"].as_array().into_iter().flatten() {
            let request_id = transport_id(&request["id"])?;
            let project = projects.get(request["project"].as_str().unwrap_or(""));
            let operation = request.get("operation").cloned().unwrap_or(json!({}));
            let read = operation["action"] == "view"
                || operation["action"] == "status_history"
                || operation["action"] == "status_view"
                || operation["action"] == "mindmap"
                    && matches!(
                        operation["operation"]["command"].as_str(),
                        Some("view" | "show")
                    );
            let outcome = if operation["action"] != "artifact"
                && operation["action"] != "attachment"
                && !read
            {
                json!({"ok":false,"error":{"code":"invalid_input","message":"Only artifact operations are accepted"}})
            } else if let Some(project) = project {
                let writing = !read
                    && !matches!(
                        operation["operation"]["command"].as_str(),
                        Some("list" | "view" | "links" | "preview" | "download")
                    );
                let key = writing.then(|| format!("artifact-mobile:{request_id}"));
                let project_id = project["id"].as_str().unwrap();
                let accepted = writing
                    && !visible.contains(project_id)
                    && self.accepted(project_id, key.as_ref().unwrap())?;
                if !visible.contains(project_id) && !accepted {
                    json!({"ok":false,"error":{"code":"not_found","message":"Project is hidden; restore it before accessing artifacts"}})
                } else {
                    self.rpc(operation, Some(project), key)?
                }
            } else {
                json!({"ok":false,"error":{"code":"not_found","message":"Project is no longer registered"}})
            };
            self.call(
                &format!("/api/bridge/artifacts/{request_id}/result"),
                Some(&outcome),
            )?;
        }
        Ok(())
    }
    fn agents(&self, visible: &BTreeSet<String>) -> Result<()> {
        let status = super::local_request(&self.ctx, json!({"kind":"overview"}))?;
        let mut machines = vec![];
        for m in status["machines"].as_array().into_iter().flatten() {
            let workers=m["workers"].as_array().into_iter().flatten().map(|w|json!({"id":w["id"],"pid":w["pid"],"runs":w["runs"].as_array().into_iter().flatten().filter(|r|visible.contains(r["project_id"].as_str().unwrap_or(""))).collect::<Vec<_>>()})).collect::<Vec<_>>();
            machines.push(json!({"host":m["host"],"hostname":m["hostname"],"state":m["state"],"heartbeat":m["heartbeat"],"workers":workers}));
        }
        self.call(
            "/api/bridge/agents/status",
            Some(&json!({"ok":true,"machines":machines})),
        )?;
        let queue = self.call("/api/bridge/agents", None)?;
        for request in queue["requests"].as_array().into_iter().flatten() {
            let identifier = transport_id(&request["id"])?;
            let result = (|| -> Result<Value> {
                if !visible.contains(request["project"].as_str().unwrap_or("")) {
                    return Err(invalid("This project is no longer available"));
                }
                super::local_request(
                    &self.ctx,
                    json!({"kind":request["action"],"host":request["host"],"run":request["run"],"scope":request["scope"],"text":request["text"],"request_id":request["request_id"],"cursor":request.get("cursor").cloned().unwrap_or(json!(0)),"before":request["before"],"latest":request["latest"],"at":request["at"]}),
                )
            })();
            let outcome = result.unwrap_or_else(|e| json!({"ok":false,"error":e.to_string()}));
            self.call(
                &format!("/api/bridge/agents/{identifier}/result"),
                Some(&outcome),
            )?;
        }
        Ok(())
    }
}
fn mobile_web_operation(payload: &Value) -> Result<Operation> {
    if !payload["host"].is_null() {
        return Err(invalid("Remote hosts are not accepted"));
    }
    let operation: Operation = serde_json::from_value(payload["operation"].clone())?;
    if matches!(
        operation,
        Operation::ReadPlan { .. } | Operation::BindPlan { .. } | Operation::Status { .. }
    ) {
        return Err(invalid(
            "Plan files and status updates use the terminal workflow",
        ));
    }
    if let Operation::Mindmap { operation } = &operation
        && operation.writes()
    {
        return Err(invalid("Mindmaps are read-only on the web"));
    }
    Ok(operation)
}

fn transport_id(value: &Value) -> Result<&str> {
    let id = value
        .as_str()
        .ok_or_else(|| invalid("Invalid mobile transport ID"))?;
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err(invalid("Invalid mobile transport ID"));
    }
    Ok(id)
}
pub(super) fn run(ctx: Context) {
    let Ok(client) = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
    else {
        return;
    };
    let checkpoint = Mobile {
        ctx: ctx.clone(),
        client: client.clone(),
    };
    // Inbox actions can wait for cloud acknowledgment while the regular bridge
    // is serving them. Keep durable checkpoint acknowledgments independent.
    let checkpointer = std::thread::spawn(move || {
        while !checkpoint.ctx.stopped() {
            let _ = checkpoint.checkpoint();
            checkpoint.ctx.wait(Duration::from_secs(1));
        }
    });
    let web = Mobile {
        ctx: ctx.clone(),
        client: client.clone(),
    };
    let web_bridge = std::thread::spawn(move || {
        while !web.ctx.stopped() {
            let _ = (|| -> Result<()> {
                let registry = web.rpc(
                    json!({"action":"projects","include_hidden":true}),
                    None,
                    None,
                )?;
                let projects = registry["projects"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| {
                        p["id"]
                            .as_str()
                            .map(|id| (id.to_owned(), json!({"id":id,"name":p["name"],"name_collisions":p["name_collisions"]})))
                    })
                    .collect();
                let visible = registry["projects"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|p| p["hidden_at"].is_null())
                    .filter_map(|p| p["id"].as_str().map(str::to_owned))
                    .collect();
                web.web(&projects, &visible)
            })();
            web.ctx.wait(Duration::from_secs(1));
        }
    });
    let mobile = Mobile { ctx, client };
    while !mobile.ctx.stopped() {
        let _ = mobile.sync();
        mobile.ctx.wait(Duration::from_secs(5));
    }
    let _ = checkpointer.join();
    let _ = web_bridge.join();
}

#[cfg(test)]
mod web_tests {
    use super::*;
    #[test]
    fn web_operations_keep_desktop_restrictions() {
        assert!(mobile_web_operation(&json!({"operation":{"action":"view","number":77}})).is_ok());
        assert!(
            mobile_web_operation(
                &json!({"host":"devbox","operation":{"action":"view","number":77}})
            )
            .is_err()
        );
        assert!(mobile_web_operation(&json!({"operation":{"action":"status","number":77,"level":"green","comment":"Agent only"}})).is_err());
        assert!(mobile_web_operation(&json!({"operation":{"action":"mindmap","operation":{"command":"add","title":"No"}}})).is_err());
    }
    #[test]
    fn phone_issue_mutations_are_authoritative_idempotent_and_render_markdown() {
        let directory = std::env::temp_dir().join(format!(
            "hb-mobile-web-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let ctx = Context {
            home: directory.clone(),
            state: directory.clone(),
            desired: directory.join("fleet.json"),
            binary: std::env::current_exe().unwrap(),
            path: directory.join("issues.db"),
            node: "test-device".into(),
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let mobile = Mobile {
            ctx,
            client: reqwest::blocking::Client::new(),
        };
        let project = json!({"id":"named:Phone","name":"Phone"});
        let projects = BTreeMap::from([("named:Phone".to_owned(), project.clone())]);
        let visible = BTreeSet::from(["named:Phone".to_owned()]);
        let creation = json!({"kind":"action","payload":{"project":"named:Phone","request_id":"stable-key","operation":{"action":"create","title":"Phone issue","body":"**Private content**","labels":[]}}});
        let first = mobile.web_request(&creation, &projects, &visible).unwrap();
        assert_eq!(first["issue"]["created_by"], "human:boss");
        assert!(
            first["issue"]["body_html"]
                .as_str()
                .unwrap()
                .contains("<strong>Private content</strong>")
        );
        let repeated = mobile.web_request(&creation, &projects, &visible).unwrap();
        assert_eq!(first["issue"]["number"], repeated["issue"]["number"]);
        let mut by_name = creation.clone();
        by_name["payload"]["project"] = json!("Phone");
        assert_eq!(
            mobile.web_request(&by_name, &projects, &visible).unwrap()["issue"]["number"],
            first["issue"]["number"]
        );
        let hidden = mobile.web_request(&json!({"kind":"action","payload":{"project":"named:Phone","operation":{"action":"view","number":1}}}), &projects, &BTreeSet::new()).unwrap();
        assert_eq!(hidden["error"]["code"], "not_found");
        let read = mobile.web_request(&json!({"kind":"action","payload":{"project":"named:Phone","operation":{"action":"view","number":1}}}), &projects, &visible).unwrap();
        assert_eq!(read["issue"]["title"], "Phone issue");
        let db = mobile.ctx.db().unwrap();
        db.execute_batch(
            "UPDATE fleet_meta SET syncing=1;
            INSERT INTO projects(id,name,next_number) VALUES('local:legacy:/saved/Phone','Phone',1);
            UPDATE fleet_meta SET syncing=0;",
        )
        .unwrap();
        drop(db);
        let legacy = json!({"id":"local:legacy:/saved/Phone","name":"Phone"});
        mobile
            .rpc(
                json!({"action":"create","title":"Legacy issue","body":"","labels":[]}),
                Some(&legacy),
                None,
            )
            .unwrap();
        let mut with_history = projects.clone();
        with_history.get_mut("named:Phone").unwrap()["name_collisions"] =
            json!([{"legacy":true,"rejected_id":"local:legacy:/saved/Phone"}]);
        let historical = mobile.web_request(&json!({"kind":"action","payload":{"project":"local:legacy:/saved/Phone","operation":{"action":"view","number":1}}}), &with_history, &visible).unwrap();
        assert_eq!(historical["issue"]["title"], "Legacy issue");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
