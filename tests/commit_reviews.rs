#[path = "support/projects.rs"]
mod projects;
use hey_boss::issues::{Request, Store};
use serde_json::{Value, json};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const PATCH: &str = "diff --git a/example.rs b/example.rs\nindex 1111111..2222222 100644\n--- a/example.rs\n+++ b/example.rs\n@@ -1,2 +1,3 @@\n fn example() {\n+    run();\n }\n";

struct Fixture {
    root: std::path::PathBuf,
    store: Store,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hb-review-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::open(&root.join("issues.db")).unwrap();
        projects::seed(&root.join("issues.db"), &["named:Review"]);
        let mut f = Self { root, store };
        f.call(json!({"action":"create","title":"Review task","body":"","labels":[]}));
        f.call(json!({"action":"add_commit","number":1,"commit":SHA,"review":{
            "repository":"github.com/example/repo","parent":"1111111111111111111111111111111111111111",
            "message":"Run the example\n\nKeep the explanatory body.\n","author":"Reviewer","email":"review@example.invalid",
            "authored_at":"2026-11-02T12:00:00-06:00","patch":PATCH
        }}));
        f
    }
    fn request(&self, operation: Value) -> Request {
        serde_json::from_value(json!({"version":1,"project":{"id":"named:Review","name":"Review"},"actor":{"id":"human:boss","kind":"human","machine":"test","host":"test","cwd":self.root,"source":"test"},"operation":operation})).unwrap()
    }
    fn call(&mut self, operation: Value) -> Value {
        self.store.execute(&self.request(operation)).unwrap()
    }
    fn review(&mut self, operation: Value) -> Value {
        self.call(json!({"action":"commit_review","number":1,"commit":SHA,"operation":operation}))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn stored_review_survives_without_git_and_comments_keep_exact_source() {
    let mut f = Fixture::new();
    let review = f.review(json!({"command":"view"}));
    assert_eq!(review["review"]["patch"], PATCH);
    assert_eq!(
        review["review"]["message"],
        "Run the example\n\nKeep the explanatory body.\n"
    );
    let saved = f.review(json!({"command":"comment","location":{"file":"example.rs","side":"new","line":1,"endLine":2},"body":"Keep this explanation."}));
    let id = saved["comment"]["id"].as_str().unwrap();
    let edited = f.review(json!({"command":"edit_comment","id":id,"body":"Check this call.","previous_body":"Keep this explanation."}));
    assert_eq!(edited["comment"]["body"], "Check this call.");
    let comments = f.review(json!({"command":"comments"}));
    assert_eq!(
        comments["comments"][0]["source"],
        "fn example() {\n    run();"
    );
    assert_eq!(comments["comments"][0]["location"]["side"], "new");
    let issue = f.call(json!({"action":"view","number":1}));
    assert_eq!(issue["issue"]["commits"][0]["comment_count"], 1);
    assert_eq!(issue["issue"]["commits"][0]["review_available"], true);
    assert!(!issue.to_string().contains("diff --git"));
    f.review(json!({"command":"delete_comment","id":id,"previous_body":"Check this call."}));
    assert_eq!(
        f.review(json!({"command":"comments"}))["comments"],
        json!([])
    );
}

#[test]
fn message_edits_export_with_patch_and_stale_edits_do_not_overwrite() {
    let mut f = Fixture::new();
    f.review(json!({"command":"message","message":"Updated subject\n\nUpdated details.","previous_message":"Run the example\n\nKeep the explanatory body.\n"}));
    let exported = f.review(json!({"command":"patch"}));
    assert!(
        exported["patch"]
            .as_str()
            .unwrap()
            .contains("Subject: [PATCH] Updated subject\n\nUpdated details.")
    );
    assert!(exported["patch"].as_str().unwrap().ends_with(PATCH));
    let request = f.request(json!({"action":"commit_review","number":1,"commit":SHA,"operation":{"command":"message","message":"Stale","previous_message":"Run the example\n\nKeep the explanatory body.\n"}}));
    assert_eq!(f.store.execute(&request).unwrap_err().code, "conflict");
    assert_eq!(
        f.review(json!({"command":"view"}))["review"]["message"],
        "Updated subject\n\nUpdated details."
    );
}

#[test]
fn comment_locations_are_checked_and_rework_assigns_boss_atomically() {
    let mut f = Fixture::new();
    for location in [
        json!({"file":"missing","side":"new","line":2}),
        json!({"file":"example.rs","side":"old","line":3}),
        json!({"file":"example.rs","side":"new","line":1,"endLine":99999999}),
    ] {
        let r = f.request(json!({"action":"commit_review","number":1,"commit":SHA,"operation":{"command":"comment","location":location,"body":"Invalid"}}));
        assert!(f.store.execute(&r).is_err());
    }
    f.call(json!({"action":"ready","number":1,"force":false}));
    let before = f.call(json!({"action":"view","number":1}));
    let changed =
        f.call(json!({"action":"rework","number":1,"if_version":before["issue"]["version"]}));
    assert_eq!(changed["issue"]["state"], "open");
    assert_eq!(changed["issue"]["assignee"], "human:boss");
    let stale =
        f.request(json!({"action":"rework","number":1,"if_version":before["issue"]["version"]}));
    assert_eq!(f.store.execute(&stale).unwrap_err().code, "conflict");
}

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
fn captured_binary_rename_and_quoted_paths_apply_in_another_checkout_with_edited_message() {
    let mut f = Fixture::new();
    let repo = f.root.join("source");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.name", "Review Test"]);
    git(&repo, &["config", "user.email", "review@example.invalid"]);
    std::fs::write(
        repo.join("old name.txt"),
        "original\n--- literal diff text\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "Base"]);
    let base = git(&repo, &["rev-parse", "HEAD"]).trim().to_string();
    git(&repo, &["mv", "old name.txt", "new name.txt"]);
    std::fs::write(
        repo.join("new name.txt"),
        "original\n--- literal diff text\n+++ another literal\n",
    )
    .unwrap();
    std::fs::write(repo.join("zażółć.txt"), "Unicode path\n").unwrap();
    std::fs::write(repo.join("binary.dat"), [0, 1, 2, 255, 3, 0, 4]).unwrap();
    git(&repo, &["add", "."]);
    git(
        &repo,
        &["commit", "-qm", "Initial subject\n\nOriginal body"],
    );
    let sha = git(&repo, &["rev-parse", "HEAD"]).trim().to_string();
    let snapshot = hey_boss::issues::commit_reviews::capture(&repo, &sha, "named:Review")
        .unwrap()
        .unwrap();
    assert!(snapshot.patch.contains("GIT binary patch"));
    f.call(json!({"action":"add_commit","number":1,"commit":sha,"review":snapshot}));
    let comment=f.call(json!({"action":"commit_review","number":1,"commit":sha,"operation":{"command":"comment","location":{"file":"zażółć.txt","side":"new","line":1},"body":"Read this on laptop"}}));
    assert_eq!(comment["comment"]["source"], "Unicode path");
    f.call(json!({"action":"commit_review","number":1,"commit":sha,"operation":{"command":"message","message":"Reviewed subject\n\nReviewed body","previous_message":snapshot.message}}));
    let patch = f.call(
        json!({"action":"commit_review","number":1,"commit":sha,"operation":{"command":"patch"}}),
    );
    let patch_path = f.root.join("review.patch");
    std::fs::write(&patch_path, patch["patch"].as_str().unwrap()).unwrap();
    let target = f.root.join("delivery");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            target.to_str().unwrap(),
            &base,
        ],
    );
    git(&target, &["am", patch_path.to_str().unwrap()]);
    assert_eq!(
        git(&target, &["rev-parse", "HEAD^{tree}"]),
        git(&repo, &["rev-parse", "HEAD^{tree}"])
    );
    assert_eq!(
        git(&target, &["log", "-1", "--format=%B"]).trim(),
        "Reviewed subject\n\nReviewed body"
    );
    // Detaching and reattaching does not discard review feedback or message edits.
    f.call(json!({"action":"remove_commit","number":1,"commit":sha}));
    f.call(json!({"action":"add_commit","number":1,"commit":sha,"review":snapshot}));
    let saved = f.call(
        json!({"action":"commit_review","number":1,"commit":sha,"operation":{"command":"view"}}),
    );
    assert_eq!(
        saved["review"]["message"],
        "Reviewed subject\n\nReviewed body"
    );
    assert_eq!(saved["comments"].as_array().unwrap().len(), 1);
}

#[test]
fn review_and_feedback_follow_task_transfer_and_stale_comments_are_rejected() {
    let mut f = Fixture::new();
    let saved=f.review(json!({"command":"comment","location":{"file":"example.rs","side":"new","line":2},"body":"Before"}));
    let id = saved["comment"]["id"].as_str().unwrap();
    f.review(json!({"command":"edit_comment","id":id,"body":"After","previous_body":"Before"}));
    let stale=f.request(json!({"action":"commit_review","number":1,"commit":SHA,"operation":{"command":"delete_comment","id":id,"previous_body":"Before"}}));
    assert_eq!(f.store.execute(&stale).unwrap_err().code, "conflict");
    projects::seed(&f.root.join("issues.db"), &["named:Destination"]);
    let issue = f.call(json!({"action":"view","number":1}));
    let moved=f.call(json!({"action":"transfer","number":1,"destination":"named:Destination","if_version":issue["issue"]["version"]}));
    let mut request=f.request(json!({"action":"commit_review","number":moved["issue"]["number"],"commit":SHA,"operation":{"command":"view"}}));
    request.project_override = Some("named:Destination".into());
    let review = f.store.execute(&request).unwrap();
    assert_eq!(review["review"]["patch"], PATCH);
    assert_eq!(review["comments"][0]["body"], "After");
    assert_eq!(review["comments"][0]["id"], id);
}
