//! One bounded, chronological stream over the existing audit log and comments.
use super::*;

pub(super) const INDEXES: &str = "
CREATE INDEX IF NOT EXISTS issue_timeline_events ON events(project_id,issue_number,created_at DESC,id DESC) WHERE action!='commented';
CREATE INDEX IF NOT EXISTS issue_timeline_comment_models ON events(project_id,issue_number,json_extract(data,'$.comment_id'),id DESC) WHERE action='commented';
CREATE INDEX IF NOT EXISTS issue_timeline_comments ON comments(project_id,issue_number,created_at DESC,id DESC);
";

const QUERY: &str = "SELECT created_at,kind,id FROM (
 SELECT created_at,0 AS kind,id FROM events
 WHERE project_id=?1 AND issue_number=?2 AND action!='commented' AND (created_at,0,id)<(?3,?4,?5)
 UNION ALL
 SELECT created_at,1 AS kind,id FROM comments
 WHERE project_id=?1 AND issue_number=?2 AND (created_at,1,id)<(?3,?4,?5)
) ORDER BY created_at DESC,kind DESC,id DESC LIMIT ?6";

pub(super) fn page(
    db: &Connection,
    project: &Project,
    number: i64,
    limit: u32,
    before: Option<[i64; 3]>,
) -> Result<Value> {
    get_issue(db, &project.id, number, true)?;
    let cold = crate::issues::archive::history_connection(db, &project.id, number)?;
    let db = cold.as_ref().unwrap_or(db);
    let [at, kind, id] = before.unwrap_or([i64::MAX; 3]);
    let mut stmt = db.prepare(QUERY)?;
    // Select keys first: neither sorting nor pagination reads hidden bodies.
    let keys = stmt
        .query_map(params![project.id, number, at, kind, id, limit + 1], |r| {
            Ok([r.get::<_, i64>(0)?, r.get(1)?, r.get(2)?])
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut more = keys.len() > limit as usize;
    let mut entries = Vec::new();
    let mut comments = Vec::new();
    let mut events = Vec::new();
    let mut bytes = 0;
    let mut cursor = None;
    for key @ [at, kind, id] in keys.into_iter().take(limit as usize) {
        let value = if kind == 1 {
            db.query_row("SELECT c.author,c.body,coalesce((SELECT action='comment_resolved' FROM events WHERE project_id=c.project_id AND issue_number=c.issue_number AND action IN ('comment_resolved','comment_unresolved') AND json_extract(data,'$.comment_id')=c.id ORDER BY created_at DESC,id DESC LIMIT 1),0),coalesce((SELECT json_extract(data,'$.actor_model') FROM events WHERE project_id=c.project_id AND issue_number=c.issue_number AND action='commented' AND json_extract(data,'$.comment_id')=c.id ORDER BY id DESC LIMIT 1),json_extract(a.metadata,'$.model')) FROM comments c LEFT JOIN agents a ON a.id=c.author WHERE c.id=?1", [id], |r| Ok(json!({"id":id,"author":r.get::<_,String>(0)?,"body":r.get::<_,String>(1)?,"resolved":r.get::<_,bool>(2)?,"actor_model":r.get::<_,Option<String>>(3)?,"created_at":at})))?
        } else {
            let (actor, action, data): (String, String, String) = db.query_row(
                "SELECT actor,action,data FROM events WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            json!({"id":id,"actor":actor,"action":action,"data":serde_json::from_str::<Value>(&data)?,"created_at":at})
        };
        bytes += serde_json::to_vec(&value)?.len();
        if bytes > PAGE_BYTES / 2 && !entries.is_empty() {
            more = true;
            break;
        }
        entries.push(json!({"kind":if kind==1 {"comment"} else {"event"},"id":id,"created_at":at}));
        if kind == 1 {
            comments.push(value);
        } else {
            events.push(value);
        }
        cursor = Some(key);
    }
    entries.reverse();
    Ok(
        json!({"ok":true,"project":project,"entries":entries,"comments":comments,"events":events,"next_before":if more {cursor} else {None}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeline_seeks_keys_without_scanning_hidden_history() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE events(id INTEGER PRIMARY KEY,project_id TEXT,issue_number INTEGER,created_at INTEGER,action TEXT,data TEXT);
            CREATE TABLE comments(id INTEGER PRIMARY KEY,project_id TEXT,issue_number INTEGER,created_at INTEGER);").unwrap();
        db.execute_batch(INDEXES).unwrap();
        db.execute_batch(
            "WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<20000)
            INSERT INTO events SELECT id,'p',1,id,'edited','{}' FROM n;
            INSERT INTO comments SELECT id,project_id,issue_number,created_at FROM events;",
        )
        .unwrap();
        for at in [i64::MAX, 10000, 10] {
            let mut stmt = db.prepare(QUERY).unwrap();
            let keys = stmt
                .query_map(params!["p", 1, at, 1, i64::MAX, 31], |r| r.get::<_, i64>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert!(!keys.is_empty());
            let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
            assert!(
                steps < 3000,
                "Cursor {at} scanned hidden history: {steps} VM steps"
            );
        }
    }

    #[test]
    fn timeline_orders_mixed_rows_and_pages_without_duplicates() {
        let root = std::env::temp_dir().join(format!(
            "hb-timeline-{}",
            super::super::super::worker::random_id().unwrap()
        ));
        let mut store = Store::open(&root.join("issues.db")).unwrap();
        store.db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('named:Timeline','Timeline',3);
          INSERT INTO agents VALUES('codex:test','{\"model\":\"gpt-6-astra\"}',0);
          INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES
            ('named:Timeline',1,'Task','','open','codex:test',0,0,1,'[]'),('named:Timeline',2,'Other','','open','codex:test',0,0,1,'[]');
          INSERT INTO comments(id,project_id,issue_number,author,body,created_at) VALUES
            (1,'named:Timeline',1,'codex:test','Earlier comment',10),(2,'named:Timeline',1,'codex:test','Later comment',30),(3,'named:Timeline',2,'codex:test','Private',20);
          INSERT INTO events(id,project_id,issue_number,actor,action,created_at,data) VALUES
            (1,'named:Timeline',1,'codex:test','assigned',20,'{\"target\":\"boss\",\"actor_model\":\"gpt-6-astra\"}'),
            (2,'named:Timeline',1,'codex:test','commented',10,'{\"comment_id\":1,\"actor_model\":\"gpt-6-sol\"}'),
            (3,'named:Timeline',1,'codex:test','edited',10,'{}'),
            (4,'named:Timeline',1,'codex:test','comment_resolved',30,'{\"comment_id\":2}');").unwrap();
        let request = |before: Value| -> Request {
            serde_json::from_value(json!({"version":1,"project":{"id":"named:Timeline","name":"Timeline"},"operation":{"action":"timeline","number":1,"limit":2,"before":before}})).unwrap()
        };
        let first = store.execute(&request(Value::Null)).unwrap();
        assert_eq!(
            first["entries"],
            json!([{"kind":"event","id":4,"created_at":30},{"kind":"comment","id":2,"created_at":30}])
        );
        assert_eq!(first["comments"][0]["resolved"], true);
        assert_eq!(first["comments"][0]["actor_model"], "gpt-6-astra");
        store.db.execute("INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES('named:Timeline',1,'codex:test','closed',40,'{}')",[]).unwrap();
        let second = store
            .execute(&request(first["next_before"].clone()))
            .unwrap();
        assert_eq!(
            second["entries"],
            json!([{"kind":"comment","id":1,"created_at":10},{"kind":"event","id":1,"created_at":20}])
        );
        assert_eq!(second["events"][0]["data"]["actor_model"], "gpt-6-astra");
        assert_eq!(second["comments"][0]["actor_model"], "gpt-6-sol");
        let third = store
            .execute(&request(second["next_before"].clone()))
            .unwrap();
        assert_eq!(
            third["entries"],
            json!([{"kind":"event","id":3,"created_at":10}])
        );
        assert!(third["next_before"].is_null());
        assert!(!request(Value::Null).operation.writes());
        let mut invalid = request(Value::Null);
        invalid.operation =
            serde_json::from_value(json!({"action":"timeline","number":1,"limit":101})).unwrap();
        assert!(store.execute(&invalid).is_err());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
