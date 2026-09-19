use serde_json::{Value, json};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("out")
            .join(format!(
                "mindmaps-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }
    fn cmd(&self, project: &str, command: &str, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        c.current_dir(&self.root)
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .env("HEY_BOSS_INBOX_SOCKET", self.root.join("absent.sock"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args([
                command,
                "--project",
                project,
                "--agent",
                "human:test",
                "--json",
            ])
            .args(args);
        c
    }
    fn run(&self, p: &str, args: &[&str]) -> Value {
        success(self.cmd(p, "mm", args).output().unwrap())
    }
    fn issue(&self, p: &str, args: &[&str]) -> Value {
        success(self.cmd(p, "issue", args).output().unwrap())
    }
    fn fail(&self, p: &str, args: &[&str], code: i32) -> Value {
        let o = self.cmd(p, "mm", args).output().unwrap();
        assert_eq!(
            o.status.code(),
            Some(code),
            "{} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn success(o: Output) -> Value {
    assert!(
        o.status.success(),
        "{} {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_slice(&o.stdout).unwrap()
}
fn nodes(g: &Value) -> &Vec<Value> {
    g["nodes"].as_array().unwrap()
}
fn alias<'a>(g: &'a Value, a: &str) -> &'a Value {
    nodes(g).iter().find(|n| n["alias"] == a).unwrap()
}

#[test]
fn outline_markdown_move_cycles_and_resource_safe_deletion() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Release", "--id", "release"]);
    f.run(
        "Atlas",
        &[
            "add",
            "Design",
            "--id",
            "design",
            "--under",
            "release",
            "--body",
            "## Scope\n**Ship it**",
        ],
    );
    let g = f.run("Atlas", &["show"]);
    assert_eq!(alias(&g, "design")["kind"], "markdown");
    assert!(
        alias(&g, "design")["body_html"]
            .as_str()
            .unwrap()
            .contains("<strong>Ship it</strong>")
    );
    f.fail("Atlas", &["move", "release", "--under", "design"], 4);
    f.fail("Atlas", &["remove", "release"], 4);
    f.run(
        "Atlas",
        &["add", "Testing", "--id", "testing", "--under", "release"],
    );
    f.run("Atlas", &["move", "testing", "--before", "design"]);
    let g = f.run("Atlas", &["show"]);
    assert_eq!(
        alias(&g, "testing")["parent_id"],
        alias(&g, "release")["id"]
    );
    assert!(
        alias(&g, "testing")["position"].as_i64().unwrap()
            < alias(&g, "design")["position"].as_i64().unwrap()
    );
    f.run("Atlas", &["move", "design"]);
    let g = f.run("Atlas", &["show"]);
    assert!(alias(&g, "design")["parent_id"].is_null());
    f.issue("Atlas", &["create", "--title", "Ship"]);
    f.run("Atlas", &["issue", "1", "--under", "release"]);
    f.run("Atlas", &["remove", "release", "--recursive"]);
    let g = f.run("Atlas", &["show"]);
    assert_eq!(nodes(&g).len(), 1);
    assert_eq!(f.issue("Atlas", &["view", "1"])["issue"]["title"], "Ship");
}
#[test]
fn cross_project_dependencies_optional_descriptions_and_atomic_shorthands() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Release", "--id", "release"]);
    f.run("Platform", &["add", "Shared API", "--id", "api"]);
    f.run(
        "Atlas",
        &[
            "link",
            "release",
            "Platform::api",
            "--kind",
            "depends-on",
            "--why",
            "API must land first",
        ],
    );
    let g = f.run("Atlas", &["show"]);
    assert_eq!(g["links"][0]["description"], "API must land first");
    assert_eq!(g["external_nodes"][0]["alias"], "api");
    let other = f.run("Platform", &["show"]);
    assert_eq!(other["links"], g["links"]);
    assert_eq!(other["version"], 2);
    f.run(
        "Atlas",
        &["link", "release", "Platform::api", "--kind", "depends-on"],
    );
    let g = f.run("Atlas", &["show"]);
    assert!(g["links"][0]["description"].is_null());
    f.issue("Atlas", &["create", "--title", "Ship"]);
    f.run(
        "Atlas",
        &[
            "link",
            "issue:1",
            "pr:https://github.com/org/repo/pull/2",
            "--description",
            "Implementation",
        ],
    );
    let g = f.run("Atlas", &["show"]);
    assert!(nodes(&g).iter().any(|n| n["kind"] == "issue"));
    assert!(nodes(&g).iter().any(|n| n["kind"] == "pr"));
    let count = nodes(&g).len();
    f.fail(
        "Atlas",
        &["link", "pr:https://github.com/org/repo/pull/3", "missing"],
        3,
    );
    assert_eq!(nodes(&f.run("Atlas", &["show"])).len(), count);
    f.fail("Atlas", &["link", "release", "release"], 2);
    f.fail("Atlas", &["move", "release", "--under", "Platform::api"], 2);
    f.run(
        "Atlas",
        &[
            "link",
            "pr:https://github.com/org/repo/pull/2",
            "pr:https://github.com/org/repo/pull/1",
            "--kind",
            "depends-on",
        ],
    );
    let g = f.run("Atlas", &["show"]);
    assert!(
        g["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["kind"] == "depends-on")
    );
}
#[test]
fn live_issues_and_automatic_pr_links_follow_store_updates() {
    let f = Fixture::new();
    f.issue("Platform", &["create", "--title", "Old API"]);
    f.run(
        "Atlas",
        &["issue", "1", "--issue-project", "Platform", "--id", "api"],
    );
    f.issue(
        "Platform",
        &["edit", "1", "--title", "New API", "--body", "API details"],
    );
    let url = "https://github.com/org/repo/pull/7";
    f.issue("Platform", &["pr", "add", "1", url]);
    let g = f.run("Atlas", &["show"]);
    assert_eq!(alias(&g, "api")["title"], "New API");
    assert_eq!(alias(&g, "api")["body"], "API details");
    assert_eq!(nodes(&g).len(), 2);
    assert_eq!(g["links"][0]["automatic"], true);
    f.run("Atlas", &["pr", url, "--id", "implementation"]);
    let g = f.run("Atlas", &["show"]);
    assert_eq!(nodes(&g).len(), 2);
    assert_eq!(g["links"][0]["to"], alias(&g, "implementation")["id"]);
    f.fail("Atlas", &["edit", "api", "--title", "Copied issue"], 2);
    f.issue("Platform", &["close", "1"]);
    assert_eq!(alias(&f.run("Atlas", &["show"]), "api")["state"], "closed");
    f.issue("Platform", &["pr", "remove", "1", url]);
    assert!(
        f.run("Atlas", &["show"])["links"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn retry_versions_alias_conflicts_and_stdin() {
    let f = Fixture::new();
    let g = f.run(
        "Atlas",
        &[
            "add",
            "Release",
            "--id",
            "release",
            "--request-id",
            "create-release",
            "--if-version",
            "0",
        ],
    );
    let retry = f.run(
        "Atlas",
        &[
            "add",
            "Release",
            "--id",
            "release",
            "--request-id",
            "create-release",
            "--if-version",
            "0",
        ],
    );
    assert_eq!(g["node"]["id"], retry["node"]["id"]);
    assert_eq!(retry["version"], 1);
    f.fail(
        "Atlas",
        &["add", "Different", "--request-id", "create-release"],
        4,
    );
    f.fail("Atlas", &["add", "Duplicate", "--id", "release"], 4);
    f.fail(
        "Atlas",
        &["edit", "release", "--title", "New", "--if-version", "0"],
        4,
    );
    let mut child = f
        .cmd(
            "Atlas",
            "mm",
            &["add", "Notes", "--body", "-", "--id", "notes"],
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"# Notes\n- One\n- Two")
        .unwrap();
    let g = success(child.wait_with_output().unwrap());
    assert_eq!(g["node"]["body"], "# Notes\n- One\n- Two");
    f.fail("Atlas", &["add", "Bad", "--id", "issue:1"], 2);
}
#[test]
fn notification_projection_is_pending_only_preserves_children_and_links() {
    let mut graph = json!({"nodes":[{"id":"root","kind":"notification","reference":"completed","parent_id":null},{"id":"pending","kind":"notification","reference":"pending","parent_id":"root"},{"id":"topic","kind":"text","title":"Keep me","parent_id":"root"}],"external_nodes":[],"links":[{"from":"root","to":"topic"},{"from":"pending","to":"topic"}]});
    hey_boss::mindmap::enrich_notifications(
        &mut graph,
        Ok(
            json!({"tasks":[{"taskID":"completed","status":"completed","title":"Done"},{"taskID":"pending","status":"pending","title":"Review","summary":"Needs review"}]}),
        ),
    );
    assert_eq!(nodes(&graph).len(), 2);
    assert!(
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["parent_id"].is_null())
    );
    assert_eq!(graph["nodes"][0]["state"], "pending");
    assert_eq!(graph["links"].as_array().unwrap().len(), 1);
}

#[test]
fn unavailable_inbox_hides_unverified_notifications_without_losing_children() {
    let mut graph = json!({"nodes":[{"id":"notice","kind":"notification","reference":"unknown","parent_id":null},{"id":"topic","kind":"text","parent_id":"notice"}],"external_nodes":[],"links":[]});
    hey_boss::mindmap::enrich_notifications(
        &mut graph,
        Err(hey_boss::issues::Error::new(
            "inbox_unavailable",
            "Synthetic unavailable Inbox",
        )),
    );
    assert_eq!(graph["notifications"]["available"], false);
    assert_eq!(nodes(&graph).len(), 1);
    assert_eq!(graph["nodes"][0]["id"], "topic");
    assert!(graph["nodes"][0]["parent_id"].is_null());
}

#[test]
fn completed_notice_children_keep_their_original_outline_location() {
    let mut graph = json!({"nodes":[{"id":"after","kind":"text","position":3,"parent_id":null},{"id":"child","kind":"text","position":0,"parent_id":"notice"},{"id":"before","kind":"text","position":1,"parent_id":null},{"id":"notice","kind":"notification","reference":"done","position":2,"parent_id":null}],"external_nodes":[],"links":[]});
    hey_boss::mindmap::enrich_notifications(&mut graph, Ok(json!({"tasks":[]})));
    let ids: Vec<_> = nodes(&graph)
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["before", "child", "after"]);
}

#[path = "support/mindmap_inbox.rs"]
mod inbox_fixture;

#[test]
fn actual_cli_inbox_refresh_completion_failure_and_recovery_are_read_only() {
    let f = Fixture::new();
    f.run("Atlas", &["notice", "review", "--id", "review"]);
    f.run(
        "Atlas",
        &["add", "Follow-up", "--id", "follow-up", "--under", "review"],
    );
    let pending = json!([{"taskID":"review","status":"pending","title":"Review release","summary":"**Needs review** <script>alert(1)</script>"}]);
    let inbox = inbox_fixture::Inbox::start(f.root.join("inbox.sock"), pending.clone());
    let show = || {
        success(
            f.cmd("Atlas", "mm", &["show"])
                .env("HEY_BOSS_INBOX_SOCKET", inbox.path())
                .output()
                .unwrap(),
        )
    };
    let g = show();
    assert_eq!(alias(&g, "review")["title"], "Review release");
    assert_eq!(alias(&g, "review")["state"], "pending");
    assert!(
        alias(&g, "review")["body_html"]
            .as_str()
            .unwrap()
            .contains("<strong>Needs review</strong>")
    );
    assert!(
        !alias(&g, "review")["body_html"]
            .as_str()
            .unwrap()
            .contains("<script>")
    );
    let version = g["version"].clone();
    let links = || {
        success(
            f.cmd("Atlas", "mm", &["links", "review"])
                .env("HEY_BOSS_INBOX_SOCKET", inbox.path())
                .output()
                .unwrap(),
        )
    };
    assert_eq!(links()["node"]["title"], "Review release");
    inbox.tasks(json!([{"taskID":"review","status":"completed","title":"Done"}]));
    let g = show();
    assert_eq!(nodes(&g).len(), 1);
    assert!(alias(&g, "follow-up")["parent_id"].is_null());
    assert_eq!(g["version"], version);
    let completed_links = links();
    assert!(completed_links.get("node").is_none());
    assert!(completed_links["selected_node_id"].is_string());
    inbox.unavailable();
    let g = show();
    assert_eq!(g["notifications"]["available"], false);
    assert_eq!(nodes(&g).len(), 1);
    inbox.tasks(pending);
    let g = show();
    assert_eq!(nodes(&g).len(), 2);
    assert_eq!(
        alias(&g, "follow-up")["parent_id"],
        alias(&g, "review")["id"]
    );
    assert_eq!(inbox.requests().len(), 6);
    assert!(
        inbox
            .requests()
            .iter()
            .all(|r| r["command"] == "inbox_list")
    );
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM mindmap_nodes WHERE kind='notification'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn explicit_pr_reveals_attached_issues_without_manual_issue_placement() {
    let f = Fixture::new();
    let url = "https://github.com/org/repo/pull/9";
    f.issue(
        "Platform",
        &[
            "create",
            "--title",
            "Publish shared API",
            "--body",
            "Live details",
        ],
    );
    f.issue("Platform", &["pr", "add", "1", url]);
    f.run("Atlas", &["pr", url, "--id", "api-pr"]);
    let g = f.run("Atlas", &["show"]);
    assert_eq!(nodes(&g).len(), 1);
    assert_eq!(g["links"].as_array().unwrap().len(), 1);
    let issue = &g["external_nodes"][0];
    assert_eq!(issue["kind"], "issue");
    assert_eq!(issue["title"], "Publish shared API");
    assert_eq!(issue["reference_project"], "named:Platform");
    assert_eq!(issue["resource_only"], true);
    assert_eq!(g["links"][0]["from"], issue["id"]);
    assert_eq!(g["links"][0]["to"], alias(&g, "api-pr")["id"]);
    f.issue("Platform", &["edit", "1", "--title", "API v2"]);
    assert_eq!(
        f.run("Atlas", &["show"])["external_nodes"][0]["title"],
        "API v2"
    );
    f.issue("Platform", &["pr", "remove", "1", url]);
    assert!(
        f.run("Atlas", &["show"])["links"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn automatic_pr_projection_deduplicates_url_variants_and_prefers_local_issue_nodes() {
    let f = Fixture::new();
    let url = "https://github.com/org/repo/pull/11";
    f.issue("Platform", &["create", "--title", "Shared API"]);
    f.issue("Platform", &["pr", "add", "1", url]);
    f.issue("Platform", &["pr", "add", "1", &format!("{url}/")]);
    f.run("Platform", &["issue", "1", "--id", "source"]);
    f.run(
        "Atlas",
        &[
            "issue",
            "1",
            "--issue-project",
            "Platform",
            "--id",
            "mirror",
        ],
    );
    let g = f.run("Atlas", &["show"]);
    assert_eq!(nodes(&g).len(), 2);
    assert_eq!(g["links"].as_array().unwrap().len(), 1);
    f.run("Atlas", &["pr", url, "--id", "implementation"]);
    let g = f.run("Atlas", &["show"]);
    assert_eq!(nodes(&g).len(), 2);
    assert_eq!(g["links"].as_array().unwrap().len(), 1);
    assert_eq!(g["links"][0]["from"], alias(&g, "mirror")["id"]);
    assert!(g["external_nodes"].as_array().unwrap().is_empty());
    f.issue("Platform", &["create", "--title", "Integration tests"]);
    f.issue("Platform", &["pr", "add", "2", url]);
    let g = f.run("Atlas", &["show"]);
    assert_eq!(g["links"].as_array().unwrap().len(), 2);
    assert_eq!(g["external_nodes"].as_array().unwrap().len(), 1);
    assert_eq!(g["external_nodes"][0]["title"], "Integration tests");
}

#[test]
fn mutation_and_idempotency_receipt_sizes_do_not_include_the_existing_map() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Root", "--id", "root"]);
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    for i in 0..200 {
        db.execute("INSERT INTO mindmap_nodes VALUES(?1,'named:Atlas',NULL,NULL,?2,'markdown',?3,?4,NULL,NULL,1,1)",rusqlite::params![format!("n-large-{i}"),i,format!("Large topic {i}"),"Existing map content. ".repeat(1000)]).unwrap();
    }
    let result = f.run(
        "Atlas",
        &[
            "add",
            "One more topic",
            "--id",
            "more",
            "--request-id",
            "compact-receipt",
        ],
    );
    assert!(result.get("nodes").is_none());
    assert_eq!(result["node"]["alias"], "more");
    assert!(result.to_string().len() < 4096);
    let bytes: i64 = db
        .query_row(
            "SELECT length(response) FROM requests WHERE request_id='compact-receipt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        bytes < 4096,
        "Receipt unexpectedly copied the full map: {bytes} bytes"
    );
    let retry = f.run(
        "Atlas",
        &[
            "add",
            "One more topic",
            "--id",
            "more",
            "--request-id",
            "compact-receipt",
        ],
    );
    assert_eq!(retry, result);
    assert_eq!(
        db.query_row("SELECT count(*) FROM mindmap_nodes", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        202
    );
    let link = f.run(
        "Atlas",
        &[
            "link",
            "root",
            "more",
            "--description",
            "Additional planning",
        ],
    );
    assert_eq!(
        link["link"]["from"],
        f.run("Atlas", &["links", "root"])["node"]["id"]
    );
    assert_eq!(link["link"]["to"], result["node"]["id"]);
}

#[test]
fn qualified_links_inspection_includes_every_relationship_of_the_selected_node() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Release", "--id", "release"]);
    f.run("Platform", &["add", "Shared API", "--id", "api"]);
    f.run("Delivery", &["add", "Rollout", "--id", "rollout"]);
    f.run("Atlas", &["link", "release", "Platform::api"]);
    f.run(
        "Platform",
        &["link", "api", "Delivery::rollout", "--kind", "depends-on"],
    );
    let g = f.run("Atlas", &["links", "Platform::api"]);
    assert_eq!(g["project"]["id"], "named:Platform");
    assert_eq!(g["node"]["alias"], "api");
    assert_eq!(g["links"].as_array().unwrap().len(), 2);
}

#[test]
fn typed_resource_selectors_preserve_embedded_double_colons() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Release", "--id", "release"]);
    let url = "https://[::1]/repo/pull/1";
    f.run("Atlas", &["link", "release", &format!("pr:{url}")]);
    f.run("Atlas", &["link", "release", "notice:task::review"]);
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT reference FROM mindmap_nodes WHERE kind='pr'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        url
    );
    assert_eq!(
        db.query_row(
            "SELECT reference FROM mindmap_nodes WHERE kind='notification'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "task::review"
    );
}

#[test]
fn existing_issue_shorthands_remain_manageable_after_the_issue_is_deleted() {
    let f = Fixture::new();
    f.issue("Atlas", &["create", "--title", "Old work"]);
    f.run("Atlas", &["issue", "1", "--id", "old-work"]);
    f.run("Atlas", &["add", "Release", "--id", "release"]);
    f.run("Atlas", &["link", "issue:1", "release"]);
    f.issue("Atlas", &["delete", "1"]);
    let g = f.run("Atlas", &["links", "issue:1"]);
    assert_eq!(g["node"]["state"], "unavailable");
    assert_eq!(g["links"].as_array().unwrap().len(), 1);
    f.run("Atlas", &["unlink", "issue:1", "release"]);
    f.run("Atlas", &["remove", "issue:1"]);
    assert_eq!(nodes(&f.run("Atlas", &["show"])).len(), 1);
    f.fail("Atlas", &["link", "issue:1", "release"], 3);
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    assert!(
        db.query_row(
            "SELECT deleted_at IS NOT NULL FROM issues WHERE project_id='named:Atlas' AND number=1",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
}
