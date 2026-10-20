use super::*;
use crate::{
    database::Connection,
    issues::{Request, Store},
};
use serde_json::{Value, json};
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    store: Store,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hey-boss-search-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::open(&root.join("issues.db")).unwrap();
        let db = Connection::open(root.join("issues.db")).unwrap();
        db.execute(
            "INSERT INTO projects(id,name,next_number) VALUES('named:Search','Search',1)",
            [],
        )
        .unwrap();
        super::settings(&root.join("issues.db"), Some(true)).unwrap();
        Self { root, store }
    }
    fn run(&mut self, operation: Value) -> Value {
        let request:Request=serde_json::from_value(json!({"version":1,"project":{"id":"named:Search","name":"Search"},"operation":operation,"actor":{"id":"human:qa","kind":"human","machine":"test","host":"test","cwd":self.root,"source":"test"}})).unwrap();
        self.store.execute(&request).unwrap()
    }
    fn add(&mut self, title: &str, body: &str) -> i64 {
        self.run(json!({"action":"create","title":title,"body":body,"labels":[]}))["issue"]["number"].as_i64().unwrap()
    }
    fn index(&self) {
        let db = Connection::open(self.root.join("issues.db")).unwrap();
        let cache = index::open(&self.root.join("issues.db"), true).unwrap();
        for n in db
            .query_collect("SELECT number FROM issues", [], |r| r.get::<_, i64>(0))
            .unwrap()
        {
            let generation=db.query_row("SELECT generation FROM issue_search_dirty WHERE project='named:Search' AND number=?1",[n],|r|r.get(0)).unwrap();
            while !worker::index_step(&db, &cache, "named:Search", n, generation).unwrap() {}
        }
        db.execute("UPDATE issue_search_dirty SET dirty=0", [])
            .unwrap();
        cache
            .execute(
                "INSERT OR REPLACE INTO meta VALUES('backfill_done','1')",
                [],
            )
            .unwrap();
    }
    fn search(&mut self, query: &str) -> Value {
        self.run(json!({"action":"search","query":query,"semantic":false}))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn numbers(value: &Value) -> Vec<i64> {
    value["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_i64().unwrap())
        .collect()
}

#[test]
fn title_description_comment_order_and_noise_cannot_win() {
    let mut f = Fixture::new();
    let comment = f.add("Other work", "");
    let description = f.add("Network work", "Fix reconnect after sleep.");
    let title = f.add("Reconnect after sleep", "");
    for _ in 0..20 {
        f.run(json!({"action":"comment","number":comment,"body":"reconnect after sleep"}));
    }
    f.index();
    let found = f.search("reconnect after sleep");
    assert_eq!(numbers(&found), vec![title, description, comment]);
    assert_eq!(found["issues"][2]["match"]["field"], "comment");
    assert!(found["issues"][2]["match"]["comment_id"].as_i64().unwrap() > 0);
    assert_eq!(
        numbers(&f.search(&format!("#{description}")))[0],
        description
    );
}

#[test]
fn filters_pagination_and_live_deletion_are_authoritative() {
    let mut f = Fixture::new();
    let first = f.add("Reconnect", "network");
    let second = f.add("Reconnect again", "network");
    f.index();
    let page = f.run(json!({"action":"search","query":"reconnect","semantic":false,"limit":1}));
    assert_eq!(page["next_offset"], 1);
    assert_eq!(
        numbers(&f.run(
            json!({"action":"search","query":"reconnect","semantic":false,"limit":1,"offset":1})
        )),
        vec![second]
    );
    f.run(json!({"action":"delete","number":first,"force":true}));
    assert_eq!(numbers(&f.search("reconnect")), vec![second]);
    assert!(
        numbers(&f.run(
            json!({"action":"search","query":"reconnect","semantic":false,"labels":["absent"]})
        ))
        .is_empty()
    );
    assert_eq!(
        numbers(&f.run(
            json!({"action":"search","query":"reconnect","semantic":false,"state":"deleted"})
        )),
        vec![first]
    );
}

#[test]
fn edits_from_replication_invalidate_old_content_without_api_work() {
    let mut f = Fixture::new();
    let n = f.add("Network", "old_unique_marker");
    f.index();
    assert_eq!(numbers(&f.search("old_unique_marker")), vec![n]);
    let db = Connection::open(f.root.join("issues.db")).unwrap();
    // Incoming projection can preserve version and time; capture actual text changes.
    db.execute(
        "UPDATE issues SET body='new_unique_marker' WHERE number=?1",
        [n],
    )
    .unwrap();
    assert!(numbers(&f.search("old_unique_marker")).is_empty());
    f.index();
    assert_eq!(numbers(&f.search("new_unique_marker")), vec![n]);
    assert!(numbers(&f.search("old_unique_marker")).is_empty());
}

#[test]
fn archived_bodies_and_comments_are_indexed() {
    let mut f = Fixture::new();
    let n = f.add("Historical network fix", "archived_description_marker");
    f.run(json!({"action":"comment","number":n,"body":"archived_comment_marker"}));
    f.run(json!({"action":"close","number":n,"force":true}));
    let db = Connection::open(f.root.join("issues.db")).unwrap();
    crate::issues::archive::archive_issue(
        &db,
        "named:Search",
        n,
        crate::issues::worker::now() + 6 * 24 * 60 * 60 * 1000,
    )
    .unwrap();
    f.index();
    for query in ["archived_description_marker", "archived_comment_marker"] {
        assert_eq!(
            numbers(
                &f.run(json!({"action":"search","query":query,"state":"all","semantic":false}))
            ),
            vec![n]
        );
    }
}

#[test]
fn cached_semantics_find_paraphrases_without_network() {
    let mut f = Fixture::new();
    let n = f.add("Reconnect after waking", "");
    f.index();
    let cache = index::open(&f.root.join("issues.db"), true).unwrap();
    let mut v = vec![0.0; index::DIMENSIONS];
    v[0] = 1.0;
    cache
        .execute(
            "INSERT INTO embeddings SELECT DISTINCT hash,?1 FROM passages",
            [index::bytes(&v)],
        )
        .unwrap();
    cache
        .execute(
            "INSERT INTO queries VALUES('connection dies on resume',?1,0)",
            [index::bytes(&v)],
        )
        .unwrap();
    let result = f.run(json!({"action":"search","query":"connection dies on resume"}));
    assert_eq!(numbers(&result), vec![n]);
    assert_eq!(result["search"]["mode"], "hybrid");
    assert_eq!(result["issues"][0]["match"]["kind"], "semantic");
}

#[test]
fn fresh_install_is_opt_in_and_disabling_preserves_literal_search() {
    let mut f = Fixture::new();
    let n = f.add("Network", "reconnect after sleep");
    super::settings(&f.root.join("issues.db"), Some(false)).unwrap();
    let result = f.run(json!({"action":"search","query":"reconnect"}));
    assert_eq!(numbers(&result), vec![n]);
    assert_eq!(result["search"]["enabled"], false);
    let missing = f.root.join("fresh.db");
    assert!(!super::enabled(&missing));
    assert!(!index::path(&missing).exists());
}

#[test]
fn long_threads_resume_after_restart_and_edits_reset_the_cursor() {
    let mut f = Fixture::new();
    let n = f.add("Thread", "");
    for i in 0..21 {
        f.run(json!({"action":"comment","number":n,"body":format!("message {i}")}));
    }
    let db = Connection::open(f.root.join("issues.db")).unwrap();
    let generation: i64 = db
        .query_row(
            "SELECT generation FROM issue_search_dirty WHERE number=?1",
            [n],
            |r| r.get(0),
        )
        .unwrap();
    let cache = index::open(&f.root.join("issues.db"), true).unwrap();
    assert!(!worker::index_step(&db, &cache, "named:Search", n, generation).unwrap());
    assert_eq!(
        cache
            .query_row(
                "SELECT count(*) FROM passages WHERE field='comment'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        8
    );
    drop(cache);
    f.index();
    assert_eq!(numbers(&f.search("message 20")), vec![n]);
    db.execute(
        "UPDATE comments SET body='edited_tail' WHERE id=(SELECT max(id) FROM comments)",
        [],
    )
    .unwrap();
    f.index();
    assert_eq!(numbers(&f.search("edited_tail")), vec![n]);
}
