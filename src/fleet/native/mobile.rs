//! Authenticated HTTPS bridge. Pairing credentials never enter logs or argv.
use super::{
    Result,
    context::{Context, read_json},
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
        let actor = key.as_ref().map(|_| Actor {
            id: "human:boss".into(),
            kind: "human".into(),
            session_id: None,
            machine: self.ctx.node.clone(),
            host: crate::issues::identity::host(),
            pid: None,
            process_start: None,
            cwd: self.ctx.home.clone(),
            source: "phone".into(),
        });
        let request = Request {
            version: 1,
            project: project
                .map(|p| serde_json::from_value::<Project>(p.clone()))
                .transpose()?
                .unwrap_or(Project {
                    id: "named:Fleet".into(),
                    name: "Fleet".into(),
                }),
            project_override: None,
            actor,
            operation: serde_json::from_value::<Operation>(operation)?,
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
        response
            .take(crate::issues::WIRE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > crate::issues::WIRE_LIMIT {
            return Err(invalid("Mobile response exceeds limit"));
        }
        Ok(serde_json::from_slice(&bytes)?)
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
            projects.insert(id.to_owned(), json!({"id":p["id"],"name":p["name"]}));
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
            let project = projects.get(creation["project"].as_str().unwrap_or(""));
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
                    json!({"kind":if request["action"]=="takeover" {"takeover"} else {"conversation"},"host":request["host"],"run":request["run"],"cursor":request.get("cursor").cloned().unwrap_or(json!(0))}),
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
    let mobile = Mobile { ctx, client };
    while !mobile.ctx.stopped() {
        let _ = mobile.sync();
        mobile.ctx.wait(Duration::from_secs(5));
    }
}
