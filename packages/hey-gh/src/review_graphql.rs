//! Inspect the parsed GraphQL request, after gh has expanded --input/fields.
//! A mutation on an opted-in repository must never fall back to a live write.
use crate::review_intercept::Target;
use graphql_parser::query::{Definition, OperationDefinition, Selection, Value as G};
use serde_json::{Value, json};
use std::process::{Command, Stdio};
type R<T> = Result<T, String>;
fn val(v: &G<String>, vars: &Value) -> Value {
    match v {
        G::Variable(k) => vars[k].clone(),
        G::String(s) | G::Enum(s) => json!(s),
        G::Int(i) => json!(i.as_i64()),
        G::Boolean(b) => json!(b),
        G::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), val(v, vars))).collect()),
        G::List(l) => Value::Array(l.iter().map(|v| val(v, vars)).collect()),
        _ => Value::Null,
    }
}
fn selection(fields: &graphql_parser::query::SelectionSet<String>, v: &Value) -> R<Value> {
    if fields.items.is_empty() || v.is_null() {
        return Ok(v.clone());
    }
    if let Some(rows) = v.as_array() {
        return Ok(Value::Array(
            rows.iter()
                .map(|r| selection(fields, r))
                .collect::<R<_>>()?,
        ));
    }
    let mut out = json!({});
    for s in &fields.items {
        let Selection::Field(f) = s else {
            return Err("Unsupported selection in local review response".into());
        };
        let k = f.alias.as_ref().unwrap_or(&f.name);
        let child = v
            .get(&f.name)
            .ok_or_else(|| format!("Unsupported local review field: {}", f.name))?;
        out[k] = selection(&f.selection_set, child)?
    }
    Ok(out)
}
pub(super) enum Decision {
    Read,
    Forward(bool),
    Local(Value),
}
pub(super) fn handle(t: &Target, envelope: &Value) -> R<Decision> {
    let query = envelope["query"]
        .as_str()
        .ok_or("Expected a GraphQL query")?;
    let doc = graphql_parser::parse_query::<String>(query).map_err(|_| "Invalid GraphQL")?;
    let ops = doc
        .definitions
        .iter()
        .filter_map(|d| {
            if let Definition::Operation(OperationDefinition::Mutation(m)) = d {
                Some(m)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if ops.is_empty() {
        return Ok(Decision::Read);
    }
    let relevant = ops.iter().any(|m| {
        m.selection_set.items.iter().any(|s| match s {
            Selection::Field(f) => f.name.contains("PullRequestReview"),
            _ => true,
        })
    });
    if !relevant {
        return Ok(Decision::Forward(crate::cli_auth::graphql_comments(query)));
    }
    if ops.len() != 1 || ops[0].selection_set.items.len() != 1 {
        return Err("Cannot mix a local review mutation with other mutations".into());
    }
    let Selection::Field(f) = &ops[0].selection_set.items[0] else {
        return Err("Unsupported review mutation fragment".into());
    };
    let input = f
        .arguments
        .iter()
        .find(|(k, _)| k == "input")
        .map(|(_, v)| val(v, &envelope["variables"]))
        .ok_or("Missing review mutation input")?;
    let identifier = [
        "pullRequestId",
        "pullRequestReviewId",
        "pullRequestReviewThreadId",
        "pullRequestReviewCommentId",
        "threadId",
        "id",
    ]
    .iter()
    .find_map(|k| input[*k].as_str())
    .ok_or("Cannot identify review destination")?;
    let (repo, n, pending) = if let Some(local) = identifier.strip_prefix("HBPR:") {
        let mut parts = local.rsplitn(3, ':');
        let id: i64 = parts
            .next()
            .ok_or("Invalid local review")?
            .parse()
            .map_err(|_| "Invalid local id")?;
        let n: u64 = parts
            .next()
            .ok_or("Missing PR")?
            .parse()
            .map_err(|_| "Invalid PR")?;
        (parts.next().ok_or("Missing repo")?.to_owned(), n, Some(id))
    } else {
        let out=Command::new("gh").env("HEY_GH_NATIVE_CALL","1").args(["api","graphql","-f","query=query($id:ID!){node(id:$id){... on PullRequest{number repository{nameWithOwner}} ... on PullRequestReview{pullRequest{number repository{nameWithOwner}}} ... on PullRequestReviewComment{pullRequest{number repository{nameWithOwner}}} ... on PullRequestReviewThread{pullRequest{number repository{nameWithOwner}}}}}","-f",&format!("id={identifier}")]).stdin(Stdio::null()).output().map_err(|e|e.to_string())?;
        if !out.status.success() {
            return Err(
                "Cannot identify review destination using personal GitHub credentials".into(),
            );
        }
        let v: Value =
            serde_json::from_slice(&out.stdout).map_err(|_| "Invalid GitHub node response")?;
        let node = &v["data"]["node"];
        let pr = node.get("pullRequest").unwrap_or(node);
        (
            pr["repository"]["nameWithOwner"]
                .as_str()
                .ok_or("Missing reviewed repository")?
                .to_owned(),
            pr["number"].as_u64().ok_or("Missing PR number")?,
            None,
        )
    };
    if !t.enabled.iter().any(|e| e.eq_ignore_ascii_case(&repo)) {
        if t.repo.is_empty() {
            return Ok(Decision::Forward(true));
        }
        return Err("Review belongs to a different repository".into());
    }
    let t = Target {
        repo: repo.clone(),
        enabled: t.enabled.clone(),
        auth: t.auth,
    };
    let mut commit = input["commitOID"].clone();
    if let Some(id) = pending {
        let r = t
            .store(&json!({"command":"view","id":id}))
            .map_err(|e| e.to_string())?;
        if r["review"]["stage"] != "draft" || r["review"]["number"] != n {
            return Err("No matching pending review".into());
        }
        commit = r["review"]["head"].clone();
    }
    let field = f.alias.as_ref().unwrap_or(&f.name);
    if f.name == "addPullRequestReviewThread" {
        let template = json!({"clientMutationId":input["clientMutationId"],"thread":{"id":"","path":"","line":0,"isResolved":false,"comments":{"nodes":[{"id":"","databaseId":0,"body":""}],"totalCount":1}}});
        selection(&f.selection_set, &template)?;
        let mut c = json!({"path":input["path"],"body":input["body"],"line":input["line"],"side":input["side"]});
        for (a, b) in [("startLine", "start_line"), ("startSide", "start_side")] {
            if let Some(v) = input.get(a) {
                c[b] = v.clone()
            }
        }
        if !input["subjectType"].is_null() && input["subjectType"] != "LINE" {
            return Err("Expected a line comment".into());
        }
        let review = t
            .rest(
                "POST",
                &format!("/repos/{repo}/pulls/{n}/reviews"),
                &json!({"comments":[c],"commit_id":commit}),
            )
            .map_err(|e| e.to_string())?
            .ok_or("Thread was not stored")?;
        let v = t
            .store(&json!({"command":"view","id":review["id"]}))
            .map_err(|e| e.to_string())?;
        let comment = v["review"]["comments"]
            .as_array()
            .and_then(|a| a.last())
            .ok_or("No thread receipt")?;
        let cid = comment["id"].as_i64().ok_or("Invalid comment ID")?;
        let obj = json!({"clientMutationId":input["clientMutationId"],"thread":{"id":format!("HBThread:{repo}:{n}:{cid}"),"path":c["path"],"line":c["line"],"isResolved":false,"comments":{"totalCount":1,"nodes":[{"id":format!("HBComment:{repo}:{n}:{cid}"),"databaseId":cid,"body":c["body"]}]}}});
        return Ok(Decision::Local(
            json!({"data":{field:selection(&f.selection_set,&obj)?}}),
        ));
    }
    let template = json!({"pullRequestReview":{"id":"","state":"","body":"","databaseId":0},"clientMutationId":input["clientMutationId"]});
    selection(&f.selection_set, &template)?;
    let event = input["event"].clone();
    let mut payload = json!({"event":event,"body":input["body"],"commit_id":commit});
    match f.name.as_str() {
        "addPullRequestReview" => {
            if let Some(threads) = input["threads"].as_array() {
                payload["comments"] = Value::Array(
                    threads
                        .iter()
                        .map(|v| {
                            let mut v = v.clone();
                            if let Some(n) = v.as_object_mut().and_then(|o| o.remove("startLine")) {
                                v["start_line"] = n
                            }
                            if let Some(s) = v.as_object_mut().and_then(|o| o.remove("startSide")) {
                                v["start_side"] = s
                            }
                            v
                        })
                        .collect(),
                )
            }
            if !input["comments"].is_null() {
                return Err(
                    "Legacy position comments are not supported; use a path and line".into(),
                );
            }
        }
        "submitPullRequestReview" if pending.is_some() => {}
        _ => return Err("Unsupported review mutation; nothing was posted to GitHub".into()),
    }
    let r = t
        .rest(
            "POST",
            &format!("/repos/{repo}/pulls/{n}/reviews"),
            &payload,
        )
        .map_err(|e| e.to_string())?
        .ok_or("Review was not stored")?;
    let id = r["id"].as_i64().ok_or("Missing local review receipt")?;
    let result = json!({"clientMutationId":input["clientMutationId"],"pullRequestReview":{"id":format!("HBPR:{repo}:{n}:{id}"),"databaseId":id,"body":r["body"],"state":r["state"]}});
    Ok(Decision::Local(
        json!({"data":{field:selection(&f.selection_set,&result)?}}),
    ))
}
