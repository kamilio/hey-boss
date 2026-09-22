use hey_boss::issues::{Request, Store};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    store: Store,
    db: Connection,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "hey-boss-list-origins-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("issues.db");
        Self {
            store: Store::open(&path).unwrap(),
            db: Connection::open(path).unwrap(),
            root,
        }
    }

    fn run(&mut self, operation: Value) -> Value {
        self.run_as("human:boss", operation)
    }

    fn run_as(&mut self, actor: &str, mut operation: Value) -> Value {
        if operation["action"] == "list" {
            let mut defaults = json!({"state":"open","mine":false,"unassigned":false,"labels":[],"limit":50,"offset":0});
            defaults
                .as_object_mut()
                .unwrap()
                .extend(operation.as_object().unwrap().clone());
            operation = defaults;
        }
        let request: Request = serde_json::from_value(json!({"version":1,
            "project":{"id":"named:List origins","name":"List origins"},
            "actor":{"id":actor,"kind":"human","session_id":null,"machine":"test",
                "host":"test-host","pid":null,"process_start":null,"cwd":"/tmp","source":"test"},
            "operation":operation}))
        .unwrap();
        self.store.execute(&request).unwrap()
    }

    fn create(&mut self, title: &str) {
        self.run(
            json!({"action":"create","title":title,"body":"Keep the body","labels":["regression"]}),
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn issue_lists_keep_null_and_populated_origins_with_filters_and_pagination() {
    let mut f = Fixture::new("mixed");
    for title in [
        "Legacy unassigned",
        "Claimed with origin",
        "Closed legacy",
        "Boss with origin",
    ] {
        f.create(title);
    }
    f.run_as(
        "codex:other",
        json!({"action":"claim","number":2,"force":false}),
    );
    f.run(json!({"action":"assign_boss","number":4,"force":false}));
    f.run(json!({"action":"close","number":3,"force":false}));
    f.run(json!({"action":"comment","number":2,"body":"First finding"}));
    f.run(json!({"action":"comment","number":2,"body":"Second finding"}));
    f.run(json!({"action":"add_subtask","number":2,"child":1}));
    f.run(json!({"action":"move","number":4,"before":1}));
    f.db.execute("UPDATE issues SET origin=NULL WHERE number IN (1,3)", [])
        .unwrap();
    let before: Vec<Value> = (1..=4)
        .map(|number| f.run(json!({"action":"view","number":number}))["issue"].clone())
        .collect();
    for (filter, expected) in [
        (json!({"state":"all"}), vec![4, 1, 2, 3]),
        (json!({"state":"open"}), vec![4, 1]),
        (json!({"state":"blocked"}), vec![2]),
        (json!({"state":"closed"}), vec![3]),
        (json!({"state":"all","unassigned":true}), vec![1, 2, 3]),
        (json!({"state":"all","assignee":"codex:other"}), vec![]),
        (json!({"state":"all","mine":true}), vec![4]),
        (json!({"state":"all","assignee":"human:boss"}), vec![4]),
        (
            json!({"state":"all","labels":["regression"],"search":"legacy"}),
            vec![1, 3],
        ),
    ] {
        let mut offset = 0;
        let mut numbers = Vec::new();
        loop {
            let mut operation = filter.clone();
            operation["action"] = json!("list");
            operation["limit"] = json!(1);
            operation["offset"] = json!(offset);
            let page = f.run(operation);
            for issue in page["issues"].as_array().unwrap() {
                let number = issue["number"].as_i64().unwrap();
                numbers.push(number);
                let mut summary = issue.clone();
                assert_eq!(
                    summary
                        .as_object_mut()
                        .unwrap()
                        .remove("comment_count")
                        .unwrap(),
                    json!(if number == 2 { 2 } else { 0 })
                );
                let mut detail = before[number as usize - 1].clone();
                assert_eq!(
                    detail.as_object_mut().unwrap().remove("body").unwrap(),
                    "Keep the body"
                );
                assert_eq!(summary, detail);
            }
            match page["next_offset"].as_u64() {
                Some(next) => offset = next,
                None => break,
            }
        }
        assert_eq!(numbers, expected);
    }
    let all = f.run(json!({"action":"list","state":"all","all":true,"limit":1,"offset":3}));
    assert_eq!(all["issues"].as_array().unwrap().len(), 4);
    assert!(all["next_offset"].is_null());
    assert!(all["issues"][1]["origin"].is_null());
    assert!(all["issues"][2]["origin"].is_object());
    for number in 1..=4 {
        assert_eq!(
            f.run(json!({"action":"view","number":number}))["issue"],
            before[number as usize - 1]
        );
    }
}

#[test]
fn invalid_origins_report_per_issue_errors_without_losing_rows() {
    let mut f = Fixture::new("invalid");
    for title in [
        "Malformed JSON",
        "Unsupported JSON",
        "Healthy neighbor",
        "JSON null",
    ] {
        f.create(title);
    }
    // Corruption is injected only into this isolated fixture. Expression indexes
    // otherwise prevent invalid JSON from being stored at all.
    f.db.execute_batch("DROP INDEX issues_origin_session; DROP INDEX issues_origin_run; PRAGMA ignore_check_constraints=ON;").unwrap();
    for (number, raw) in [(1, "{broken"), (2, "[1,2]"), (4, "null")] {
        f.db.execute(
            "UPDATE issues SET origin=?1 WHERE number=?2",
            rusqlite::params![raw, number],
        )
        .unwrap();
    }
    let list = f.run(json!({"action":"list","state":"all"}));
    assert_eq!(list["issues"].as_array().unwrap().len(), 4);
    for number in 1..=4 {
        let issue = &list["issues"][number as usize - 1];
        let view = f.run(json!({"action":"view","number":number}));
        assert_eq!(issue["origin"], view["issue"]["origin"]);
        assert_eq!(issue["origin_error"], view["issue"]["origin_error"]);
        if number <= 2 {
            assert!(issue["origin"].is_null());
            assert_eq!(issue["origin_error"]["code"], "invalid_origin");
            assert!(
                issue["origin_error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("origin")
            );
        } else {
            assert!(issue.get("origin_error").is_none());
        }
    }
}
