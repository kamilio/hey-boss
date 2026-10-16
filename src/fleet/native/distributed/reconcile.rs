//! Recover append-only legacy history; retire stale intent only with evidence.
//! Original conflict payloads remain intact and every decision is recorded.
use super::super::context::Context;
use super::*;
use rusqlite::params;
use serde_json::json;

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS fleet_conflict_resolutions(id TEXT PRIMARY KEY REFERENCES fleet_conflicts(id),action TEXT NOT NULL,evidence TEXT NOT NULL CHECK(json_valid(evidence)),resolved_at INTEGER NOT NULL);";

fn decoded(change: &Value, name: &str) -> Result<Value> {
    match change.get(name) {
        Some(Value::String(s)) => Ok(serde_json::from_str(s)?),
        Some(v) => Ok(v.clone()),
        None => Ok(Value::Null),
    }
}

struct Decision {
    action: &'static str,
    evidence: Value,
    insert: Option<Value>,
}
fn decision(db: &Connection, conflict: &Value) -> Result<Decision> {
    let change: Value = serde_json::from_str(conflict["data"].as_str().unwrap())?;
    let before = decoded(&change, "before_json")?;
    let after = decoded(&change, "after_json")?;
    let table = conflict["table_name"].as_str().unwrap();
    let result = |action, evidence| Decision {
        action,
        evidence,
        insert: None,
    };
    let first = |sql: &str, values: &[Value]| -> Result<Option<Value>> {
        Ok(replica::rows(db, sql, values)?.into_iter().next())
    };
    if table == "issue_status_updates" && before.is_null() && after.is_object() {
        if !["id", "project_id", "author", "level", "comment"]
            .iter()
            .all(|key| after[*key].is_string())
            || !after["issue_number"].is_i64()
            || !after["created_at"].is_i64()
        {
            return Ok(result(
                "review",
                json!({"reason":"Invalid history payload"}),
            ));
        }
        if let Some(current) = first(
            "SELECT * FROM issue_status_updates WHERE id=?1",
            &[after["id"].clone()],
        )? {
            return Ok(result(
                if current == after {
                    "already_present"
                } else {
                    "review"
                },
                json!({"current":current}),
            ));
        }
        if let Some(history) = crate::issues::archive::history_connection(
            db,
            after["project_id"].as_str().unwrap(),
            after["issue_number"].as_i64().unwrap(),
        )? {
            if let Some(current) = replica::rows(
                &history,
                "SELECT * FROM issue_status_updates WHERE id=?1",
                &[after["id"].clone()],
            )?
            .into_iter()
            .next()
            {
                return Ok(result(
                    if current == after {
                        "already_present"
                    } else {
                        "review"
                    },
                    json!({"storage":"archive","current":current}),
                ));
            }
        }
        if first("SELECT value FROM distributed_fields WHERE table_name='issue_status_updates' AND entity=?1 AND field='@alive' AND value='false'",&[json!(json!({"id":after["id"]}).to_string())])?.is_some() {
            return Ok(result("superseded",json!({"reason":"A retained deletion must not be undone"})));
        }
        let parent = first(
            "SELECT state,updated_at,deleted_at FROM issues WHERE project_id=?1 AND number=?2",
            &[after["project_id"].clone(), after["issue_number"].clone()],
        )?;
        let author = first(
            "SELECT id FROM agents WHERE id=?1",
            &[after["author"].clone()],
        )?;
        if parent.is_some() && author.is_some() {
            return Ok(Decision {
                action: "recovered_history",
                evidence: json!({"parent":parent,"original_timestamp":after["created_at"]}),
                insert: Some(after),
            });
        }
        return Ok(result(
            "review",
            json!({"reason":"History parent or author is missing"}),
        ));
    }
    if table == "issues" && before.is_object() && after.is_object() {
        let Some(current) = first(
            "SELECT * FROM issues WHERE project_id=?1 AND number=?2",
            &[after["project_id"].clone(), after["number"].clone()],
        )?
        else {
            return Ok(result("review", json!({"reason":"Task is missing"})));
        };
        let changed: Vec<&str> = after
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|k| before[*k] != after[*k] && !matches!(*k, "version" | "updated_at"))
            .collect();
        if changed.iter().all(|k| current[*k] == after[*k]) {
            return Ok(result(
                "already_present",
                json!({"fields":changed,"current_updated_at":current["updated_at"]}),
            ));
        }
        if current["state"] == "closed"
            && changed
                .iter()
                .all(|field| matches!(*field, "state" | "assignee" | "closed_at" | "closed_by"))
            && (after["state"] == "closed" || changed == vec!["assignee"])
        {
            return Ok(result(
                "superseded",
                json!({"reason":"Task is already complete; stale ownership and duplicate completion cannot replace it","current_closed_at":current["closed_at"],"current_closed_by":current["closed_by"]}),
            ));
        }
        if current["state"] == "closed"
            && before["state"] != after["state"]
            && after["state"] != "closed"
            && after["updated_at"].as_i64() >= current["closed_at"].as_i64()
        {
            return Ok(result(
                "review",
                json!({"reason":"A post-completion reopen or blocker needs an explicit decision; later comments do not supersede it","current_closed_at":current["closed_at"]}),
            ));
        }
        // A later edit of the same field is stronger evidence than an unrelated
        // claim or heartbeat changing updated_at. Keep the matching event ID.
        let mut later_edits = serde_json::Map::new();
        for field in changed
            .iter()
            .filter(|field| current[**field] != after[**field])
        {
            if !matches!(
                *field,
                "title" | "body" | "labels" | "blockers" | "manual_blocked"
            ) {
                break;
            }
            let path = format!("$.after.{field}");
            let before_path = format!("$.before.{field}");
            let evidence = first(
                "SELECT id,created_at FROM events WHERE project_id=?1 AND issue_number=?2 AND action='edited' AND created_at>?3 AND json_extract(data,?4) IS NOT json_extract(data,?5) AND json_extract(data,?4)=?6 ORDER BY created_at DESC LIMIT 1",
                &[
                    after["project_id"].clone(),
                    after["number"].clone(),
                    after["updated_at"].clone(),
                    json!(path),
                    json!(before_path),
                    current[*field].clone(),
                ],
            )?;
            if let Some(evidence) = evidence {
                later_edits.insert((*field).into(), evidence);
            }
        }
        if changed
            .iter()
            .all(|field| current[*field] == after[*field] || later_edits.contains_key(*field))
        {
            return Ok(result("superseded", json!({"later_edits":later_edits})));
        }
        let newer = current["updated_at"].as_i64().unwrap_or(0)
            > after["updated_at"].as_i64().unwrap_or(i64::MAX);
        let content = changed.iter().any(|k| {
            !matches!(
                *k,
                "state"
                    | "assignee"
                    | "closed_at"
                    | "closed_by"
                    | "manual_blocked"
                    | "blockers"
                    | "assignment_target"
                    | "github_ack_event"
            )
        });
        let completed = current["state"] == "closed"
            && current["closed_at"].as_i64().unwrap_or(0)
                > after["updated_at"].as_i64().unwrap_or(i64::MAX);
        if newer && (!content || completed) {
            return Ok(result(
                "superseded",
                json!({"fields":changed,"current_updated_at":current["updated_at"],"current_state":current["state"],"current_closed_at":current["closed_at"],"reason":"A later task transition supersedes this offline intent; original edit retained"}),
            ));
        }
        return Ok(result(
            "review",
            json!({"fields":changed,"current_updated_at":current["updated_at"],"reason":"An unapplied content edit needs an explicit decision"}),
        ));
    }
    if table == "projects" && before.is_null() && after.is_object() {
        if let Some(current) = first(
            "SELECT id,name FROM projects WHERE id=?1",
            &[after["id"].clone()],
        )? {
            return Ok(result(
                if current["name"] == after["name"] {
                    "already_present"
                } else {
                    "review"
                },
                json!({"current":current}),
            ));
        }
    }
    if table == "issue_github_watches" {
        if let Some(current) = first(
            "SELECT status FROM issue_github_watches WHERE project_id=?1 AND issue_number=?2",
            &[after["project_id"].clone(), after["issue_number"].clone()],
        )? {
            if current["status"] == after["status"] {
                return Ok(result("already_present", json!({})));
            }
            let status: Value = serde_json::from_str(current["status"].as_str().unwrap())?;
            if let (Some(prs), Some(at)) =
                (status["prs"].as_object(), change["created_at"].as_i64())
            {
                if !prs.is_empty()
                    && prs.values().all(|pr| {
                        pr["checked_at"]
                            .as_i64()
                            .is_some_and(|checked| checked > at)
                    })
                {
                    return Ok(result(
                        "superseded",
                        json!({"reason":"A newer derived GitHub watch snapshot is retained","event":status["event"],"observations":prs.iter().map(|(url,pr)|json!({"url":url,"checked_at":pr["checked_at"]})).collect::<Vec<_>>()}),
                    ));
                }
            }
        }
    }
    if table == "issue_pull_requests" && after.is_object() {
        if let Some(current) = first(
            "SELECT * FROM issue_pull_requests WHERE project_id=?1 AND issue_number=?2 AND url=?3",
            &[
                after["project_id"].clone(),
                after["issue_number"].clone(),
                after["url"].clone(),
            ],
        )? {
            if before.is_null() && current["purpose"] == after["purpose"] {
                return Ok(result(
                    "already_present",
                    json!({"url":current["url"],"reason":"The same PR link and purpose already exist; original authorship is retained"}),
                ));
            }
            let changed: Vec<&str> = after
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .filter(|k| before.is_null() || before[*k] != after[*k])
                .collect();
            if changed.iter().all(|k| current[*k] == after[*k]) {
                return Ok(result("already_present", json!({"fields":changed})));
            }
            if changed
                .iter()
                .all(|k| matches!(*k, "checked_at" | "status" | "merged_at" | "pr_title"))
                && current["checked_at"].as_i64() > after["checked_at"].as_i64()
            {
                return Ok(result(
                    "superseded",
                    json!({"reason":"A later GitHub observation is retained","current_checked_at":current["checked_at"]}),
                ));
            }
        } else if before.is_object() {
            if let Some(event) = first(
                "SELECT id,action,created_at FROM events WHERE project_id=?1 AND issue_number=?2 AND action IN ('pr_removed','pr_attached') AND json_extract(data,'$.url')=?3 ORDER BY created_at DESC,id DESC LIMIT 1",
                &[
                    after["project_id"].clone(),
                    after["issue_number"].clone(),
                    after["url"].clone(),
                ],
            )? {
                if event["action"] == "pr_removed" {
                    return Ok(result(
                        "superseded",
                        json!({"reason":"An observation must not restore an explicitly removed PR link","event":event}),
                    ));
                }
            }
        }
    }
    Ok(result(
        "review",
        json!({"reason":"No automatic resolution is proven"}),
    ))
}

fn configuration_decision(conflict: &Value, authority: &Value) -> Result<Decision> {
    let change: Value = serde_json::from_str(conflict["data"].as_str().unwrap())?;
    let machine = &authority["document"]["machines"][conflict["node"].as_str().unwrap_or("")];
    let workers = machine["workers"]
        .as_array()
        .ok_or_else(|| replica::invalid("No verified authoritative machine configuration"))?;
    let worker = workers.iter().find(|w| w["id"] == change["id"]);
    Ok(Decision {
        action: "superseded",
        insert: None,
        evidence: json!({"reason":"The main YAML is authoritative; retain its current worker settings instead of replaying rejected offline controls","configuration_revision":authority["revision"],"worker":change["id"],"current":worker,"removed":worker.is_none()}),
    })
}

pub(in crate::fleet::native) fn run(ctx: &Context, apply: bool) -> Result<Value> {
    let db = ctx.db()?;
    if !db.query_row(
        "SELECT active=1 FROM distributed_meta WHERE id=1",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(replica::invalid(
            "Activate distributed sync before reconciling legacy conflicts",
        ));
    }
    if apply {
        db.execute_batch(SCHEMA)?;
    }
    // Only the controller can retire obsolete configuration requests. Verify
    // the cached, validated document still matches the file without rewriting it.
    let mut authority = Value::Null;
    if ctx.service_role()? == "controller" {
        let cached = ctx.read_json(&ctx.state.join("fleet-config-cache.json"), Value::Null)?;
        if let Ok(text) = std::fs::read_to_string(&ctx.desired) {
            if cached["error"].is_null()
                && cached["revision"] == super::super::context::hash(&json!(text))
            {
                authority = cached;
            }
        }
    }
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut review = vec![];
    // Read one record per transaction. Recovery never holds the application
    // writer while walking unrelated historical task bodies.
    let ids = replica::rows(
        &db,
        "SELECT id FROM fleet_conflicts WHERE resolved=0 ORDER BY created_at,id",
        &[],
    )?;
    for id in ids {
        let tx = crate::database::Transaction::new_unchecked(
            &db,
            if apply {
                rusqlite::TransactionBehavior::Immediate
            } else {
                rusqlite::TransactionBehavior::Deferred
            },
        )?;
        let Some(conflict) = replica::rows(
            &tx,
            "SELECT * FROM fleet_conflicts WHERE id=?1 AND resolved=0",
            &[id["id"].clone()],
        )?
        .into_iter()
        .next() else {
            continue;
        };
        let result = if conflict["table_name"] == "worker_configuration" {
            configuration_decision(&conflict, &authority)
        } else {
            decision(&tx, &conflict)
        }
        .unwrap_or_else(|error| Decision {
            action: "review",
            evidence: json!({"reason":error.to_string()}),
            insert: None,
        });
        *counts.entry(result.action.into()).or_default() += 1;
        if result.action == "review" {
            review.push(json!({"id":conflict["id"],"table":conflict["table_name"],"reason":result.evidence}));
        } else if apply {
            if let Some(row) = result.insert {
                replica::execute(
                    &tx,
                    "INSERT INTO issue_status_updates(id,project_id,issue_number,author,level,comment,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    &[
                        row["id"].clone(),
                        row["project_id"].clone(),
                        row["issue_number"].clone(),
                        row["author"].clone(),
                        row["level"].clone(),
                        row["comment"].clone(),
                        row["created_at"].clone(),
                    ],
                )?;
            }
            tx.execute(
                "INSERT INTO fleet_conflict_resolutions VALUES(?1,?2,?3,?4)",
                params![
                    conflict["id"].as_str(),
                    result.action,
                    result.evidence.to_string(),
                    crate::issues::worker::now()
                ],
            )?;
            tx.execute(
                "UPDATE fleet_conflicts SET resolved=1 WHERE id=?1",
                [conflict["id"].as_str()],
            )?;
        }
        tx.commit()?;
    }
    Ok(json!({"applied":apply,"counts":counts,"review":review}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (crate::admin::Temporary, Context, Connection) {
        let root = crate::admin::Temporary::new().unwrap();
        let ctx = Context {
            home: root.0.clone(),
            state: root.0.clone(),
            desired: root.0.join("fleet.yaml"),
            path: root.0.join("issues.db"),
            binary: std::env::current_exe().unwrap(),
            node: "a".repeat(32),
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let db = crate::issues::Store::open(&ctx.path)
            .unwrap()
            .into_database();
        install(&db, &ctx.node).unwrap();
        db.execute_batch("UPDATE distributed_meta SET active=1; INSERT INTO projects(id,name,next_number) VALUES('p','Project',2); INSERT INTO agents(id,metadata,last_seen) VALUES('human:test','{}',1); INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,closed_at,closed_by,version,labels) VALUES('p',1,'Completed','Body','closed','human:test',1,20,20,'human:test',1,'[]');").unwrap();
        (root, ctx, db)
    }
    fn status_conflict() -> Value {
        json!({"table_name":"issue_status_updates","data":json!({"before_json":null,"after_json":json!({"id":"offline-status","project_id":"p","issue_number":1,"author":"human:test","level":"green","comment":"Original progress","created_at":10}).to_string()}).to_string()})
    }
    #[test]
    fn recovery_is_atomic_idempotent_and_keeps_original_evidence() {
        let (_root, ctx, db) = fixture();
        let conflict = status_conflict();
        db.execute("INSERT INTO fleet_conflicts(id,node,seq,table_name,data,reason,created_at) VALUES('conflict','peer',1,'issue_status_updates',?1,'stale allocation',1)",[conflict["data"].as_str()]).unwrap();
        let issue = replica::rows(&db, "SELECT * FROM issues", &[]).unwrap();
        assert_eq!(run(&ctx, false).unwrap()["counts"]["recovered_history"], 1);
        assert_eq!(
            db.query_row("SELECT count(*) FROM issue_status_updates", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(run(&ctx, true).unwrap()["counts"]["recovered_history"], 1);
        assert_eq!(run(&ctx, true).unwrap()["counts"], json!({}));
        assert_eq!(
            replica::rows(&db, "SELECT * FROM issues", &[]).unwrap(),
            issue
        );
        assert_eq!(
            db.query_row(
                "SELECT data FROM fleet_conflicts WHERE resolved=1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            conflict["data"].as_str().unwrap()
        );
        assert_eq!(
            db.query_row("SELECT action FROM fleet_conflict_resolutions", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "recovered_history"
        );
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM distributed_outbox WHERE table_name='issue_status_updates' AND json_extract(after_json,'$.id')='offline-status')",[],|r|r.get::<_,bool>(0)).unwrap());
    }
    #[test]
    fn archived_history_is_not_duplicated_and_deleted_history_is_not_revived() {
        let (_root, _ctx, db) = fixture();
        let conflict = status_conflict();
        db.execute_batch("INSERT INTO issue_status_updates VALUES('offline-status','p',1,'human:test','green','Original progress',10);").unwrap();
        assert!(
            crate::issues::archive::archive_issue(
                &db,
                "p",
                1,
                crate::issues::archive::GRACE_MS + 20
            )
            .unwrap()
        );
        while crate::issues::archive::cleanup_history(&db).unwrap() > 0 {}
        assert_eq!(decision(&db, &conflict).unwrap().action, "already_present");
        let mut row:Value=serde_json::from_str(serde_json::from_str::<Value>(conflict["data"].as_str().unwrap()).unwrap()["after_json"].as_str().unwrap()).unwrap();
        row["id"] = json!("deleted-status");
        let deleted = json!({"table_name":"issue_status_updates","data":json!({"before_json":null,"after_json":row.to_string()}).to_string()});
        db.execute("INSERT INTO distributed_fields VALUES('issue_status_updates',?1,'@alive',1,'peer','false')",[json!({"id":"deleted-status"}).to_string()]).unwrap();
        assert_eq!(decision(&db, &deleted).unwrap().action, "superseded");
        row["id"] = json!("missing-status");
        let missing = json!({"table_name":"issue_status_updates","data":json!({"before_json":null,"after_json":row.to_string()}).to_string()});
        assert_eq!(decision(&db, &missing).unwrap().action, "recovered_history");
    }
    #[test]
    fn rejected_status_recovers_as_history_without_reopening_completed_work() {
        let root = crate::admin::Temporary::new().unwrap();
        let db = crate::issues::Store::open(&root.0.join("issues.db"))
            .unwrap()
            .into_database();
        db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('p','Project',2); INSERT INTO agents(id,metadata,last_seen) VALUES('human:test','{}',1); INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('p',1,'Completed','Body','closed','human:test',1,20,1,'[]');").unwrap();
        install(&db, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let row = json!({"id":"offline-status","project_id":"p","issue_number":1,"author":"human:test","level":"green","comment":"Original progress","created_at":10});
        let conflict = json!({"table_name":"issue_status_updates","data":json!({"before_json":null,"after_json":row.to_string()}).to_string()});
        let plan = decision(&db, &conflict).unwrap();
        assert_eq!(plan.action, "recovered_history");
        assert_eq!(plan.insert.unwrap()["created_at"], 10);
        db.execute("INSERT INTO issue_status_updates VALUES('offline-status','p',1,'human:test','green','Original progress',10)",[]).unwrap();
        assert_eq!(decision(&db, &conflict).unwrap().action, "already_present");
        assert_eq!(
            db.query_row("SELECT state FROM issues", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "closed"
        );
    }
    #[test]
    fn unsupported_or_ambiguous_edits_remain_unresolved() {
        let db = Connection::open_in_memory().unwrap();
        let conflict = json!({"table_name":"worker_configuration","data":"{\"id\":\"worker\",\"intent\":\"stop\"}"});
        assert_eq!(decision(&db, &conflict).unwrap().action, "review");
    }
    #[test]
    fn derived_observations_cannot_replace_newer_checks_or_removed_links() {
        let (_root, _ctx, db) = fixture();
        let old = json!({"project_id":"p","issue_number":1,"status":json!({"prs":{},"event":"old"}).to_string()});
        let conflict = json!({"table_name":"issue_github_watches","data":json!({"after_json":old.to_string(),"created_at":10}).to_string()});
        db.execute(
            "INSERT INTO issue_github_watches VALUES('p',1,?1)",
            [json!({"prs":{"url":{"checked_at":20}},"event":"new"}).to_string()],
        )
        .unwrap();
        assert_eq!(decision(&db, &conflict).unwrap().action, "superseded");
        db.execute(
            "UPDATE issue_github_watches SET status=?1",
            [json!({"prs":{"url":{"checked_at":5}},"event":"older"}).to_string()],
        )
        .unwrap();
        assert_eq!(decision(&db, &conflict).unwrap().action, "review");
        db.execute_batch("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('p',1,'human:test','pr_removed',30,'{\"url\":\"url\"}');").unwrap();
        let row = json!({"project_id":"p","issue_number":1,"url":"url"});
        let conflict = json!({"table_name":"issue_pull_requests","data":json!({"before_json":row.to_string(),"after_json":row.to_string()}).to_string()});
        assert_eq!(decision(&db, &conflict).unwrap().action, "superseded");
    }
    #[test]
    fn rejected_configuration_keeps_the_verified_main_document() {
        let conflict =
            json!({"node":"peer","data":json!({"id":"worker","intent":"stop"}).to_string()});
        assert!(configuration_decision(&conflict, &Value::Null).is_err());
        let authority = json!({"revision":"verified","document":{"machines":{"peer":{"workers":[{"id":"worker","intent":"running"}]}}}});
        let result = configuration_decision(&conflict, &authority).unwrap();
        assert_eq!(result.action, "superseded");
        assert_eq!(result.evidence["current"]["intent"], "running");
        assert!(result.insert.is_none());
    }
    #[test]
    fn a_later_comment_is_not_evidence_to_discard_an_explicit_reopen() {
        let (_root, _ctx, db) = fixture();
        db.execute("UPDATE issues SET updated_at=40", []).unwrap();
        let before = json!({"project_id":"p","number":1,"state":"closed","updated_at":20});
        let mut after = before.clone();
        after["state"] = json!("open");
        after["updated_at"] = json!(30);
        let conflict = json!({"table_name":"issues","data":json!({"before_json":before.to_string(),"after_json":after.to_string()}).to_string()});
        assert_eq!(decision(&db, &conflict).unwrap().action, "review");
    }
}
