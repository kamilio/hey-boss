use hey_boss::issues::{Project, Request, Store};
use serde_json::{Value, json};
static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Fixture {
    root: std::path::PathBuf,
    store: Store,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-vocabulary-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(&root.join("issues.db")).unwrap();
        Self { root, store }
    }
    fn run(&mut self, operation: Value) -> hey_boss::issues::Result<Value> {
        let actor = hey_boss::issues::identity::resolve(Some("human:boss"), "test", &self.root)?;
        self.store.execute(&Request {
            version: 1,
            project: Project {
                id: "named:Unregistered".into(),
                name: "Unregistered".into(),
            },
            project_override: None,
            actor: Some(actor),
            operation: serde_json::from_value(
                json!({"action":"dictation_vocabulary","operation":operation}),
            )
            .unwrap(),
            request_id: None,
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn vocabulary_is_global_persistent_and_case_insensitive() {
    let mut f = Fixture::new();
    assert_eq!(
        f.run(json!({"action":"show"})).unwrap()["initialized"],
        false
    );
    assert_eq!(
        f.run(json!({"action":"initialize","words":["Kamil","Poe"]}))
            .unwrap()["vocabulary"],
        json!(["Kamil", "Poe"])
    );
    assert_eq!(
        f.run(json!({"action":"add","words":["POE"," T3   Code ","ans"]}))
            .unwrap()["vocabulary"],
        json!(["Kamil", "POE", "T3 Code", "ans"])
    );
    assert_eq!(
        f.run(json!({"action":"remove","words":["poe","kamil"]}))
            .unwrap()["vocabulary"],
        json!(["T3 Code", "ans"])
    );
    f.store = Store::open(&f.root.join("issues.db")).unwrap();
    assert_eq!(
        f.run(json!({"action":"show"})).unwrap()["vocabulary"],
        json!(["T3 Code", "ans"])
    );
}

#[test]
fn late_migration_never_resurrects_removed_words_or_replaces_an_empty_list() {
    let mut f = Fixture::new();
    f.run(json!({"action":"remove","words":["Poe"]})).unwrap();
    assert_eq!(
        f.run(json!({"action":"initialize","words":["Poe","Kamil"]}))
            .unwrap()["vocabulary"],
        json!(["Kamil"])
    );
    f.run(json!({"action":"remove","words":["Kamil"]})).unwrap();
    assert_eq!(
        f.run(json!({"action":"initialize","words":["Poe","Kamil","New default"]}))
            .unwrap()["vocabulary"],
        json!([])
    );
    assert_eq!(
        f.run(json!({"action":"show"})).unwrap()["initialized"],
        true
    );
}

#[test]
fn invalid_batches_do_not_partially_change_the_shared_list() {
    let mut f = Fixture::new();
    f.run(json!({"action":"initialize","words":["Poe"]}))
        .unwrap();
    for words in [
        json!([]),
        json!(["Valid", "bad/term"]),
        json!(["Valid", "one\ntwo"]),
        json!(["Poe :99"]),
        json!([""]),
        json!(["x".repeat(257)]),
    ] {
        assert!(f.run(json!({"action":"add","words":words})).is_err());
        assert_eq!(
            f.run(json!({"action":"show"})).unwrap()["vocabulary"],
            json!(["Poe"])
        );
    }
    let oversized: Vec<_> = (0..257).map(|i| format!("Word {i}")).collect();
    assert!(f.run(json!({"action":"add","words":oversized})).is_err());
    assert_eq!(
        f.run(json!({"action":"show"})).unwrap()["vocabulary"],
        json!(["Poe"])
    );
}

#[test]
fn unicode_spelling_uses_the_same_identity_as_the_mac_client() {
    let mut f = Fixture::new();
    f.run(json!({"action":"initialize","words":[]})).unwrap();
    assert_eq!(
        f.run(json!({"action":"add","words":["Cafe\u{301}","CAFÉ"]}))
            .unwrap()["vocabulary"],
        json!(["CAFÉ"])
    );
    assert_eq!(
        f.run(json!({"action":"remove","words":["cafe\u{301}"]}))
            .unwrap()["vocabulary"],
        json!([])
    );
}

#[test]
fn an_oversized_offline_merge_stays_bounded_and_can_be_reduced_gradually() {
    let mut f = Fixture::new();
    f.run(json!({"action":"initialize","words":[]})).unwrap();
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    db.execute_batch(
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<600)
        INSERT INTO dictation_vocabulary SELECT 1,'word '||x,'Word '||x,1 FROM n;",
    )
    .unwrap();
    let result = f
        .run(json!({"action":"remove","words":["Word 1"]}))
        .unwrap();
    assert_eq!(result["total"], 599);
    assert_eq!(result["vocabulary"].as_array().unwrap().len(), 256);
}

#[test]
fn settings_cli_manages_vocabulary_without_registering_a_project() {
    let f = Fixture::new();
    let file = f.root.join("vocabulary.md");
    std::fs::write(&file, "Kamil\nPoe\n").unwrap();
    let run = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&f.root)
            .env("HEY_BOSS_ISSUE_DB", f.root.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args(["settings", "vocabulary", "--json"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    assert_eq!(
        run(&["initialize", "--file", file.to_str().unwrap()])["initialized"],
        true
    );
    assert_eq!(
        run(&["add", "ans", "T3 Code"])["vocabulary"],
        json!(["Kamil", "Poe", "T3 Code", "ans"])
    );
    assert_eq!(
        run(&["remove", "pOE"])["vocabulary"],
        json!(["Kamil", "T3 Code", "ans"])
    );
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
