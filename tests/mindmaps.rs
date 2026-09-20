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
        let mut c = self.default_cmd(command, args);
        c.args(["--project", project]);
        c
    }
    fn default_cmd(&self, command: &str, args: &[&str]) -> Command {
        self.command(command, args, true)
    }
    fn command(&self, command: &str, args: &[&str], json: bool) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        c.current_dir(&self.root)
            .env("HEY_BOSS_ISSUE_DB", self.root.join("issues.db"))
            .env("HEY_BOSS_INBOX_SOCKET", self.root.join("absent.sock"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args([command, "--agent", "human:test"]);
        if json {
            c.arg("--json");
        }
        c.args(args);
        c
    }
    fn terminal(&self, project: &str, args: &[&str]) -> String {
        let output = self
            .command("mm", args, false)
            .args(["--project", project])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
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
    for node in ["api", "implementation"] {
        assert_eq!(f.run("Atlas", &["links", node])["links"], g["links"]);
    }
    f.fail("Atlas", &["edit", "api", "--body", "Copied issue"], 2);
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
    ).unwrap();
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
    )
    .unwrap();
    assert_eq!(graph["notifications"]["available"], false);
    assert_eq!(nodes(&graph).len(), 1);
    assert_eq!(graph["nodes"][0]["id"], "topic");
    assert!(graph["nodes"][0]["parent_id"].is_null());
}

#[test]
fn completed_notice_children_keep_their_original_outline_location() {
    let mut graph = json!({"nodes":[{"id":"after","kind":"text","position":3,"parent_id":null},{"id":"child","kind":"text","position":0,"parent_id":"notice"},{"id":"before","kind":"text","position":1,"parent_id":null},{"id":"notice","kind":"notification","reference":"done","position":2,"parent_id":null}],"external_nodes":[],"links":[]});
    hey_boss::mindmap::enrich_notifications(&mut graph, Ok(json!({"tasks":[]}))).unwrap();
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
    assert_eq!(f.run("Atlas", &["links", "api-pr"])["links"], g["links"]);
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
        db.execute("INSERT INTO mindmap_nodes(id,project_id,alias,parent_id,position,kind,title,body,reference,reference_project,created_at,updated_at) VALUES(?1,'named:Atlas',NULL,NULL,?2,'markdown',?3,?4,NULL,NULL,1,1)",rusqlite::params![format!("n-large-{i}"),i,format!("Large topic {i}"),"Existing map content. ".repeat(1000)]).unwrap();
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

#[test]
fn large_markdown_maps_have_small_utf8_safe_previews_and_full_single_node_reads() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Root", "--id", "root"]);
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    let body = "🧭 Planning the release. ".repeat(20000);
    for i in 0..40 {
        db.execute("INSERT INTO mindmap_nodes(id,project_id,alias,parent_id,position,kind,title,body,reference,reference_project,created_at,updated_at) VALUES(?1,'named:Atlas',?2,NULL,?3,'markdown',?4,?5,NULL,NULL,1,1)",rusqlite::params![format!("n-body-{i}"),format!("long-{i}"),i,format!("Long planning note {i}"),body]).unwrap();
    }
    let preview = f.run("Atlas", &["show", "--bodies", "preview"]);
    assert!(preview.to_string().len() < 120000);
    let note = alias(&preview, "long-0");
    assert_eq!(note["body"].as_str().unwrap().chars().count(), 512);
    assert_eq!(note["body_truncated"], true);
    assert_eq!(note["has_body"], true);
    let omitted = f.run("Atlas", &["show", "--bodies", "none"]);
    assert_eq!(alias(&omitted, "long-0")["body"], "");
    assert_eq!(alias(&omitted, "long-0")["has_body"], true);
    let oversized = f.fail("Atlas", &["show"], 2);
    assert!(
        oversized["error"]["message"]
            .as_str()
            .unwrap()
            .contains("show --bodies preview")
    );
    let link_view = f.run("Atlas", &["links", "long-0"]);
    assert!(link_view.to_string().len() < 40000);
    assert_eq!(alias(&link_view, "long-0")["body"], "");
    let full = f.run("Atlas", &["view", "long-0"]);
    assert_eq!(full["node"]["body"], body);
    assert_eq!(full["node"]["body_truncated"], false);
    assert_eq!(nodes(&full).len(), 1);
    let one_preview = f.run("Atlas", &["view", "long-0", "--bodies", "preview"]);
    assert_eq!(one_preview["body_mode"], "preview");
    assert_eq!(
        one_preview["node"]["body"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        512
    );
    assert_eq!(one_preview["node"]["body_truncated"], true);
    let one_omitted = f.run("Atlas", &["view", "long-0", "--bodies", "none"]);
    assert_eq!(one_omitted["node"]["body"], "");
    assert_eq!(one_omitted["node"]["has_body"], true);
    assert_eq!(
        db.query_row(
            "SELECT body FROM mindmap_nodes WHERE alias='long-0'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        body
    );
}

#[test]
fn node_view_resolves_current_cross_project_issues_and_pending_only_notices() {
    let f = Fixture::new();
    f.issue(
        "Platform",
        &[
            "create",
            "--title",
            "Shared API",
            "--body",
            "Current issue details",
        ],
    );
    f.run("Platform", &["issue", "1", "--id", "api"]);
    let g = f.run("Atlas", &["view", "Platform::api"]);
    assert_eq!(g["project"]["id"], "named:Platform");
    assert_eq!(g["node"]["body"], "Current issue details");
    let issue_body = "🧭 Current issue details. ".repeat(1000);
    f.issue("Platform", &["edit", "1", "--body", &issue_body]);
    let preview_issue = f.run("Atlas", &["view", "Platform::api", "--bodies", "preview"]);
    assert_eq!(
        preview_issue["node"]["body"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        512
    );
    assert_eq!(preview_issue["node"]["resource_version"], 2);
    f.run("Atlas", &["notice", "review", "--id", "review"]);
    let inbox = inbox_fixture::Inbox::start(
        f.root.join("inbox.sock"),
        json!([{"taskID":"review","status":"pending","title":"Review","summary":"🧭 Review context. ".repeat(1000)}]),
    );
    let view = || {
        success(
            f.cmd("Atlas", "mm", &["view", "review"])
                .env("HEY_BOSS_INBOX_SOCKET", inbox.path())
                .output()
                .unwrap(),
        )
    };
    assert_eq!(view()["node"]["state"], "pending");
    let preview_notice = success(
        f.cmd("Atlas", "mm", &["view", "review", "--bodies", "preview"])
            .env("HEY_BOSS_INBOX_SOCKET", inbox.path())
            .output()
            .unwrap(),
    );
    assert_eq!(
        preview_notice["node"]["body"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        512
    );
    let omitted_notice = success(
        f.cmd("Atlas", "mm", &["view", "review", "--bodies", "none"])
            .env("HEY_BOSS_INBOX_SOCKET", inbox.path())
            .output()
            .unwrap(),
    );
    assert_eq!(omitted_notice["node"]["body"], "");
    assert_eq!(omitted_notice["node"]["has_body"], true);
    inbox.tasks(json!([{"taskID":"review","status":"completed"}]));
    let g = view();
    assert!(g.get("node").is_none());
    assert!(nodes(&g).is_empty());
    assert!(
        inbox
            .requests()
            .iter()
            .all(|r| r["command"] == "inbox_list")
    );
}

#[test]
fn native_issue_body_modes_preserve_null_bytes_unicode_and_live_revisions() {
    let f = Fixture::new();
    f.issue("Atlas", &["create", "--title", "Native planning"]);
    f.run("Atlas", &["issue", "1", "--id", "native"]);
    f.issue("Atlas", &["assign-to-boss", "1"]);
    for body in [
        String::new(),
        "\0".to_owned(),
        "Small\0native body".to_owned(),
        "🧭".repeat(512),
        "🧭".repeat(513),
        format!("é{}", "🧭".repeat(600)),
        format!("{}\0more", "a".repeat(511)),
    ] {
        let mut child = f
            .cmd("Atlas", "issue", &["edit", "1", "--body", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        let edited = success(child.wait_with_output().unwrap());
        for mode in ["none", "preview", "full"] {
            let result = f.run("Atlas", &["view", "native", "--bodies", mode]);
            let expected = match mode {
                "none" => String::new(),
                "preview" => body.chars().take(512).collect(),
                _ => body.clone(),
            };
            assert_eq!(result["node"]["body"], expected);
            assert_eq!(result["node"]["has_body"], !body.is_empty());
            assert_eq!(
                result["node"]["body_truncated"],
                if mode == "none" {
                    !body.is_empty()
                } else {
                    mode == "preview" && body.chars().count() > 512
                }
            );
            assert_eq!(
                result["node"]["resource_version"],
                edited["issue"]["version"]
            );
            assert_eq!(result["node"]["title"], "Native planning");
            assert_eq!(result["node"]["state"], "open");
            assert_eq!(result["node"]["assignee"], "human:boss");
        }
    }
    let closed = f.issue("Atlas", &["close", "1", "--force"]);
    let omitted = f.run("Atlas", &["view", "native", "--bodies", "none"]);
    assert_eq!(omitted["node"]["state"], "closed");
    assert!(omitted["node"]["assignee"].is_null());
    assert_eq!(
        omitted["node"]["resource_version"],
        closed["issue"]["version"]
    );
}

#[test]
fn terminal_outline_defaults_to_titles_but_view_and_export_keep_bodies() {
    let f = Fixture::new();
    f.run(
        "Atlas",
        &[
            "add",
            "Planning",
            "--id",
            "plan",
            "--body",
            "A complete planning note.",
        ],
    );
    let terminal = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .current_dir(&f.root)
            .env("HEY_BOSS_ISSUE_DB", f.root.join("issues.db"))
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .env_remove("HEY_BOSS_ISSUE_PROJECT")
            .args(["mm", "--project", "Atlas", "--agent", "human:test"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    for args in [&[][..], &["show"][..]] {
        let outline = terminal(args);
        assert!(outline.contains("Planning"));
        assert!(!outline.contains("A complete planning note."));
    }
    for args in [
        &["show", "--bodies", "full"][..],
        &["show", "--bodies", "preview"][..],
        &["view", "plan"][..],
        &["export"][..],
    ] {
        assert!(terminal(args).contains("A complete planning note."));
    }
    assert_eq!(
        alias(&f.run("Atlas", &["show"]), "plan")["body"],
        "A complete planning note."
    );
}

#[test]
fn repository_defaults_share_maps_across_worktrees_and_honor_worker_override() {
    let f = Fixture::new();
    let repository = f.root.join("repository");
    let worktree = f.root.join("worktree");
    std::fs::create_dir_all(&repository).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(&repository)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "--quiet"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "--quiet",
        "--allow-empty",
        "-m",
        "Fixture",
    ]);
    git(&[
        "remote",
        "add",
        "origin",
        "git@github.com:example/mindmap-default.git",
    ]);
    git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        "fixture-worktree",
        worktree.to_str().unwrap(),
    ]);
    let added = success(
        f.default_cmd("mm", &["add", "Release", "--id", "release"])
            .current_dir(&repository)
            .output()
            .unwrap(),
    );
    let other = success(
        f.default_cmd("mm", &["show"])
            .current_dir(&worktree)
            .output()
            .unwrap(),
    );
    assert_eq!(other["project"]["id"], added["project"]["id"]);
    assert_eq!(nodes(&other).len(), 1);
    assert_eq!(alias(&other, "release")["title"], "Release");
    let overridden = success(
        f.default_cmd("mm", &["add", "Worker plan", "--id", "worker"])
            .current_dir(&worktree)
            .env("HEY_BOSS_ISSUE_PROJECT", "WorkerProject")
            .output()
            .unwrap(),
    );
    assert_eq!(overridden["project"]["id"], "named:WorkerProject");
    let explicit = success(
        f.cmd("ExplicitProject", "mm", &["add", "Explicit plan"])
            .current_dir(&worktree)
            .env("HEY_BOSS_ISSUE_PROJECT", "WorkerProject")
            .output()
            .unwrap(),
    );
    assert_eq!(explicit["project"]["id"], "named:ExplicitProject");
    let after = success(
        f.default_cmd("mm", &["show"])
            .current_dir(&repository)
            .output()
            .unwrap(),
    );
    assert_eq!(nodes(&after).len(), 1);
}

#[test]
fn ambiguous_project_names_require_full_ids_for_maps_and_link_endpoints() {
    let f = Fixture::new();
    let first = "github.com/first/shared";
    let second = "github.com/second/shared";
    f.run(first, &["add", "First API", "--id", "api"]);
    f.run(second, &["add", "Second API", "--id", "api"]);
    f.run("Atlas", &["add", "Release", "--id", "release"]);
    f.fail("shared", &["show"], 4);
    f.fail("Atlas", &["link", "release", "shared::api"], 4);
    assert!(
        f.run("Atlas", &["show"])["links"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.run("Atlas", &["link", "release", &format!("{second}::api")]);
    assert_eq!(
        f.run("Atlas", &["show"])["external_nodes"][0]["title"],
        "Second API"
    );
}

#[test]
fn concurrent_versions_and_request_retries_commit_one_mutation() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Root", "--id", "root"]);
    let first = f
        .cmd(
            "Atlas",
            "mm",
            &["edit", "root", "--title", "First", "--if-version", "1"],
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second = f
        .cmd(
            "Atlas",
            "mm",
            &["edit", "root", "--title", "Second", "--if-version", "1"],
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let outputs = [
        first.wait_with_output().unwrap(),
        second.wait_with_output().unwrap(),
    ];
    assert_eq!(outputs.iter().filter(|o| o.status.success()).count(), 1);
    assert_eq!(
        outputs
            .iter()
            .filter(|o| o.status.code() == Some(4))
            .count(),
        1
    );
    assert_eq!(f.run("Atlas", &["show"])["version"], 2);
    let args = [
        "add",
        "Retried plan",
        "--id",
        "retry",
        "--request-id",
        "concurrent-retry",
        "--if-version",
        "2",
    ];
    let first = f
        .cmd("Atlas", "mm", &args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second = f
        .cmd("Atlas", "mm", &args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_eq!(
        success(first.wait_with_output().unwrap()),
        success(second.wait_with_output().unwrap())
    );
    let map = f.run("Atlas", &["show"]);
    assert_eq!(map["version"], 3);
    assert_eq!(nodes(&map).len(), 2);
}

#[test]
fn recursive_deletion_advances_each_cross_project_neighbor_once() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Release", "--id", "release"]);
    f.run(
        "Atlas",
        &["add", "API", "--id", "api", "--under", "release"],
    );
    f.run("Atlas", &["add", "UI", "--id", "ui", "--under", "release"]);
    f.run("Platform", &["add", "Shared API", "--id", "shared"]);
    f.run("Client", &["add", "Client", "--id", "client"]);
    f.run("Atlas", &["link", "api", "Platform::shared"]);
    f.run("Platform", &["link", "shared", "Atlas::ui"]);
    f.run("Client", &["link", "client", "Atlas::ui"]);
    let before: Vec<_> = ["Atlas", "Platform", "Client"]
        .iter()
        .map(|p| f.run(p, &["show"])["version"].as_i64().unwrap())
        .collect();
    f.run("Atlas", &["remove", "release", "--recursive"]);
    for (project, version) in ["Atlas", "Platform", "Client"].iter().zip(before) {
        let map = f.run(project, &["show"]);
        assert_eq!(map["version"], version + 1);
        assert!(map["links"].as_array().unwrap().is_empty());
    }
    assert_eq!(nodes(&f.run("Platform", &["show"])).len(), 1);
    assert_eq!(nodes(&f.run("Client", &["show"])).len(), 1);
}

#[test]
fn alias_changes_keep_node_identity_and_cross_links_and_allow_resource_nodes() {
    let f = Fixture::new();
    let created = f.run("Atlas", &["add", "Release", "--id", "release"]);
    let id = created["node"]["id"].as_str().unwrap();
    f.run("Platform", &["add", "API", "--id", "api"]);
    f.run(
        "Atlas",
        &["link", "release", "Platform::api", "--kind", "depends-on"],
    );
    let before = f.run("Atlas", &["show"]);
    let renamed = f.run(
        "Atlas",
        &["alias", "release", "roadmap", "--request-id", "rename"],
    );
    assert_eq!(renamed["node"]["id"], id);
    assert_eq!(renamed["node"]["alias"], "roadmap");
    // An identical retry succeeds even though the original selector no longer exists.
    assert_eq!(
        f.run(
            "Atlas",
            &["alias", "release", "roadmap", "--request-id", "rename"]
        ),
        renamed
    );
    let after = f.run("Atlas", &["show"]);
    assert_eq!(after["links"], before["links"]);
    assert_eq!(
        after["version"].as_i64(),
        before["version"].as_i64().map(|v| v + 1)
    );
    assert_eq!(
        f.run("Platform", &["show"])["external_nodes"][0]["alias"],
        "roadmap"
    );
    f.fail("Atlas", &["view", "release"], 3);
    let unchanged = f.run("Atlas", &["alias", "roadmap", "roadmap"]);
    assert_eq!(unchanged["changed"], false);
    assert_eq!(unchanged["version"], after["version"]);
    f.run("Atlas", &["add", "Other", "--id", "occupied"]);
    f.fail("Atlas", &["alias", "roadmap", "occupied"], 4);
    for alias in ["n-reserved", "other::topic", "issue:1"] {
        f.fail("Atlas", &["alias", "roadmap", alias], 2);
    }
    f.run("Atlas", &["alias", "roadmap", "--clear"]);
    assert!(f.run("Atlas", &["view", id])["node"]["alias"].is_null());
    assert_eq!(f.run("Atlas", &["show"])["links"], before["links"]);
    f.issue("Atlas", &["create", "--title", "Live issue"]);
    f.run("Atlas", &["issue", "1"]);
    f.run("Atlas", &["alias", "issue:1", "implementation"]);
    assert_eq!(
        f.run("Atlas", &["view", "implementation"])["node"]["title"],
        "Live issue"
    );
    f.fail("Atlas", &["alias", "Platform::api", "renamed"], 2);
    f.fail(
        "Atlas",
        &["alias", "implementation", "work", "--if-version", "0"],
        4,
    );
}

#[test]
fn schema_nine_migrates_and_map_reads_do_not_wait_for_an_existing_writer() {
    let f = Fixture::new();
    f.issue("Atlas", &["create", "--title", "Existing issue"]);
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    db.execute_batch("ALTER TABLE issues DROP COLUMN draft; ALTER TABLE issues DROP COLUMN plan; ALTER TABLE project_settings DROP COLUMN drafts_enabled; ALTER TABLE project_settings DROP COLUMN plan_template; DROP TABLE mindmap_links; DROP TABLE mindmap_nodes; DROP TABLE mindmaps; PRAGMA user_version=9;").unwrap();
    assert!(nodes(&f.run("Atlas", &["show"])).is_empty());
    assert_eq!(
        f.issue("Atlas", &["view", "1"])["issue"]["title"],
        "Existing issue"
    );
    f.run("Atlas", &["issue", "1", "--id", "implementation"]);
    db.execute_batch(
        "BEGIN IMMEDIATE; UPDATE issues SET title='Uncommitted title' WHERE number=1;",
    )
    .unwrap();
    let started = std::time::Instant::now();
    let map = f.run("Atlas", &["show", "--bodies", "preview"]);
    assert_eq!(alias(&map, "implementation")["title"], "Existing issue");
    assert_eq!(
        f.run("Atlas", &["view", "implementation"])["node"]["title"],
        "Existing issue"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "Mindmap reads waited for the SQLite writer lock"
    );
    db.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn oversized_relationship_maps_keep_mutations_and_focused_link_reads_available() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Release", "--id", "release"]);
    let mut db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    let tx = db.transaction().unwrap();
    for i in 0..66 {
        tx.execute("INSERT INTO mindmap_nodes(id,project_id,alias,parent_id,position,kind,title,body,reference,reference_project,created_at,updated_at) VALUES(?1,'named:Atlas',NULL,NULL,?2,'text',?3,'',NULL,NULL,1,1)",rusqlite::params![format!("n-description-{i}"),i,format!("Topic {i}")]).unwrap();
    }
    let description = "x".repeat(16384);
    for i in 0..2100 {
        let from = i / 65;
        let to = (from + i % 65 + 1) % 66;
        tx.execute(
            "INSERT INTO mindmap_links VALUES(?1,?2,'related',?3,1)",
            rusqlite::params![
                format!("n-description-{from}"),
                format!("n-description-{to}"),
                description
            ],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    f.fail("Atlas", &["show", "--bodies", "preview"], 2);
    f.run(
        "Atlas",
        &[
            "link",
            "release",
            "n-description-0",
            "--description",
            "The release's important relationship",
        ],
    );
    let focused = f.run("Atlas", &["links", "release"]);
    assert_eq!(focused["links"].as_array().unwrap().len(), 1);
    assert_eq!(
        focused["links"][0]["description"],
        "The release's important relationship"
    );
    f.run("Atlas", &["unlink", "release", "n-description-0"]);
    assert!(
        f.run("Atlas", &["links", "release"])["links"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM mindmap_links", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2100
    );
}

#[test]
fn large_pending_notice_bodies_are_bounded_without_losing_preview_or_single_node_reads() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Root", "--id", "root"]);
    let mut db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    let tx = db.transaction().unwrap();
    for i in 0..30 {
        tx.execute("INSERT INTO mindmap_nodes(id,project_id,alias,parent_id,position,kind,title,body,reference,reference_project,created_at,updated_at) VALUES(?1,'named:Atlas',?2,NULL,?3,'notification','Saved notice','',?2,'',1,1)",rusqlite::params![format!("n-notice-{i}"),format!("notice-{i}"),i]).unwrap();
    }
    tx.commit().unwrap();
    let summary = "Pending review context. ".repeat(25000);
    let inbox = inbox_fixture::Inbox::start(f.root.join("inbox.sock"), Value::Array((0..30).map(|i| json!({"taskID":format!("notice-{i}"),"status":"pending","title":format!("Review {i}"),"summary":summary})).collect()));
    let run = |args: &[&str]| {
        f.cmd("Atlas", "mm", args)
            .env("HEY_BOSS_INBOX_SOCKET", inbox.path())
            .output()
            .unwrap()
    };
    let full = run(&["show"]);
    assert_eq!(full.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&full.stdout).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("32 MiB")
    );
    let preview = success(run(&["show", "--bodies", "preview"]));
    assert_eq!(nodes(&preview).len(), 31);
    assert!(preview.to_string().len() < 100000);
    assert_eq!(success(run(&["view", "notice-0"]))["node"]["body"], summary);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM mindmap_nodes WHERE kind='notification'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        30
    );
    assert!(
        inbox
            .requests()
            .iter()
            .all(|request| request["command"] == "inbox_list")
    );
}

#[test]
fn foreign_endpoint_links_advance_only_the_maps_that_changed() {
    let f = Fixture::new();
    let affected_version = |receipt: &Value, project: &str| {
        receipt["affected_projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["project"] == project)
            .and_then(|entry| entry["version"].as_i64())
    };
    f.run("Atlas", &["add", "Unrelated outline", "--id", "outline"]);
    f.run("Platform", &["add", "API", "--id", "api"]);
    f.run("Client", &["add", "Release", "--id", "release"]);
    let linked = f.run(
        "Atlas",
        &[
            "link",
            "Platform::api",
            "Client::release",
            "--kind",
            "depends-on",
        ],
    );
    assert_eq!(linked["version"], 1);
    assert_eq!(affected_version(&linked, "named:Platform"), Some(2));
    assert_eq!(affected_version(&linked, "named:Client"), Some(2));
    assert_eq!(affected_version(&linked, "named:Atlas"), None);
    assert_eq!(f.run("Atlas", &["show"])["version"], 1);
    assert!(
        f.run("Atlas", &["show"])["links"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let unchanged = f.run(
        "Atlas",
        &[
            "link",
            "Platform::api",
            "Client::release",
            "--kind",
            "depends-on",
        ],
    );
    assert_eq!(unchanged["changed"], false);
    assert_eq!(f.run("Platform", &["show"])["version"], 2);
    f.run(
        "Atlas",
        &[
            "unlink",
            "Platform::api",
            "Client::release",
            "--kind",
            "depends-on",
        ],
    );
    assert_eq!(f.run("Platform", &["show"])["version"], 3);
    assert_eq!(f.run("Client", &["show"])["version"], 3);
    assert_eq!(f.run("Atlas", &["show"])["version"], 1);
    let created = f.run(
        "Atlas",
        &[
            "link",
            "Platform::pr:https://github.com/org/repo/pull/1",
            "Client::release",
        ],
    );
    assert_eq!(affected_version(&created, "named:Platform"), Some(4));
    assert_eq!(affected_version(&created, "named:Client"), Some(4));
    assert_eq!(created["version"], 1);
}

#[test]
fn readable_pr_labels_preserve_native_references_dependencies_and_export_links() {
    let f = Fixture::new();
    let url = "https://github.com/org/repo/pull/88";
    f.issue("Platform", &["create", "--title", "Navigation"]);
    f.issue("Platform", &["pr", "add", "1", url]);
    let added = f.run(
        "Atlas",
        &[
            "pr",
            url,
            "--title",
            "Navigation [polish] <draft> & **scope**",
            "--id",
            "navigation",
        ],
    );
    let id = added["node"]["id"].clone();
    f.run(
        "Atlas",
        &[
            "pr",
            "https://github.com/org/repo/pull/89",
            "--title",
            "Rollout",
            "--id",
            "rollout",
        ],
    );
    f.run(
        "Atlas",
        &[
            "link",
            "rollout",
            "navigation",
            "--kind",
            "depends-on",
            "--why",
            "Navigation must land first",
        ],
    );
    let before = f.run("Atlas", &["show"]);
    let exported = f.terminal("Atlas", &["export"]);
    let html = hey_boss::markdown::render_fragment(&exported);
    assert!(html.contains("href=\"https://github.com/org/repo/pull/88\""));
    assert!(html.contains("Navigation [polish] &lt;draft&gt; &amp; **scope**</a>"));
    let viewed = f.terminal("Atlas", &["view", "navigation"]);
    assert!(viewed.contains("Navigation [polish]"));
    assert!(viewed.contains(url));
    f.run("Atlas", &["edit", "navigation", "--title", "Navigation v2"]);
    let after = f.run("Atlas", &["show"]);
    let node = alias(&after, "navigation");
    assert_eq!(node["id"], id);
    assert_eq!(node["kind"], "pr");
    assert_eq!(node["reference"], url);
    assert_eq!(node["title"], "Navigation v2");
    assert_eq!(after["links"], before["links"]);
    assert!(
        after["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link["automatic"] == true)
    );
    f.fail(
        "Atlas",
        &["edit", "navigation", "--body", "Copied PR content"],
        2,
    );
    let unchanged = f.run("Atlas", &["edit", "navigation", "--title", "Navigation v2"]);
    assert_eq!(unchanged["changed"], false);
    assert_eq!(unchanged["node"]["updated_at"], node["updated_at"]);
}

#[test]
fn long_pr_urls_work_for_explicit_and_typed_creation_with_bounded_custom_labels() {
    let f = Fixture::new();
    let url = format!(
        "https://github.com/org/repo/pull/88?context={}",
        "x".repeat(1500)
    );
    let explicit = f.run("Atlas", &["pr", &url, "--id", "long-pr"]);
    let typed = f.run(
        "Platform",
        &[
            "link",
            &format!("pr:{url}"),
            "pr:https://github.com/org/repo/pull/89",
        ],
    );
    assert_eq!(explicit["node"]["reference"], url);
    let platform = f.run("Platform", &["show"]);
    assert!(
        nodes(&platform)
            .iter()
            .any(|node| node["reference"] == url && node["title"] == url)
    );
    assert_eq!(typed["changed"], true);
    f.run("Atlas", &["edit", "long-pr", "--title", "Readable PR"]);
    f.run("Atlas", &["edit", "long-pr", "--title", &url]);
    f.run("Atlas", &["edit", "long-pr", "--title", &format!("{url}/")]);
    assert!(f.terminal("Atlas", &["view", "long-pr"]).contains(&url));
    let long_label = "x".repeat(513);
    f.fail("Atlas", &["edit", "long-pr", "--title", &long_label], 2);
    f.fail("Atlas", &["add", &long_label], 2);
    f.run("Atlas", &["add", "Ordinary topic", "--id", "topic"]);
    f.fail("Atlas", &["edit", "topic", "--title", &url], 2);
    f.fail("Client", &["pr", &url, "--title", &long_label], 2);
    let trailing = f.run("Client", &["pr", &format!("{url}/"), "--id", "trailing"]);
    assert_eq!(trailing["node"]["reference"], url);
    f.run(
        "Client",
        &["edit", "trailing", "--title", &format!("{url}/")],
    );
    f.run("Client", &["remove", "trailing"]);
    let too_long = format!(
        "https://github.com/org/repo/pull/1?context={}",
        "x".repeat(2048)
    );
    f.fail("Client", &["pr", &too_long], 2);
    f.fail(
        "Client",
        &[
            "link",
            &format!("pr:{too_long}"),
            "pr:https://github.com/org/repo/pull/2",
        ],
        2,
    );
    assert!(nodes(&f.run("Client", &["show"])).is_empty());
}

#[test]
fn mirrored_issue_views_identify_the_resource_project_and_preserve_unavailable_references() {
    let f = Fixture::new();
    f.issue(
        "Platform",
        &["create", "--title", "Shared API", "--body", "Live resource"],
    );
    f.run(
        "Atlas",
        &["issue", "1", "--issue-project", "Platform", "--id", "api"],
    );
    let view = f.run("Atlas", &["view", "api"]);
    assert_eq!(view["node"]["reference_project_name"], "Platform");
    assert_eq!(view["node"]["reference_project"], "named:Platform");
    assert!(
        f.terminal("Atlas", &["view", "api"])
            .contains("Platform · issue #1 · named:Platform")
    );
    f.issue("Platform", &["assign-to-boss", "1"]);
    f.issue("Platform", &["settings", "set", "--boss-name", "Maya"]);
    let assigned = f.run("Atlas", &["view", "api"]);
    assert_eq!(assigned["node"]["assignee"], "human:boss");
    assert_eq!(assigned["boss"]["name"], "Maya");
    assert!(
        f.terminal("Atlas", &["view", "api"])
            .contains("Assigned to Maya (human:boss)")
    );
    f.issue("Platform", &["unassign", "1", "--force"]);
    assert!(
        !f.terminal("Atlas", &["view", "api"])
            .contains("Assigned to")
    );
    f.issue("Platform", &["delete", "1"]);
    let unavailable = f.run("Atlas", &["view", "api"]);
    assert_eq!(unavailable["node"]["available"], false);
    assert_eq!(unavailable["node"]["reference_project_name"], "Platform");
    assert_eq!(unavailable["node"]["id"], view["node"]["id"]);
    assert!(
        f.terminal("Atlas", &["view", "api"])
            .contains("(unavailable)")
    );
}

#[test]
fn markdown_export_keeps_relationship_labels_and_descriptions_literal() {
    let f = Fixture::new();
    let project = "Atlas **release**";
    let source = f.run(project, &["add", "Topic *literal* [draft]"]);
    let target = f.run(project, &["add", "Review <scope> & `plan`"]);
    f.run(
        project,
        &[
            "link",
            source["node"]["id"].as_str().unwrap(),
            target["node"]["id"].as_str().unwrap(),
            "--why",
            "Wait for **approval**, then [ship](https://example.com)",
        ],
    );
    let exported = f.terminal(project, &["export"]);
    let html = hey_boss::markdown::render_fragment(&exported);
    assert!(html.contains("Atlas **release**</h1>"));
    assert!(html.contains("Topic *literal* [draft] → Review &lt;scope&gt; &amp; `plan` [related] — Wait for **approval**, then [ship](<a href=\"https://example.com\">https://example.com</a>)"), "{html}");
    assert!(!html.contains("<em>literal</em>"));
    assert!(!html.contains("<strong>approval</strong>"));
    assert!(!html.contains(">ship</a>"));
    assert!(
        f.terminal(project, &["links"])
            .contains("Wait for **approval**, then [ship](https://example.com)")
    );
}

#[test]
fn fleet_replicas_require_the_authoritative_host_without_changing_local_maps() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Preserved local note", "--id", "note"]);
    let before = f.run("Atlas", &["show"]);
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    db.execute(
        "UPDATE fleet_meta SET role='agent',node='synthetic-replica' WHERE id=1",
        [],
    )
    .unwrap();
    for args in [
        vec!["show"],
        vec!["view", "note"],
        vec!["projects"],
        vec!["add", "Unsynchronized topic"],
        vec!["edit", "note", "--title", "Unsynchronized edit"],
        vec!["remove", "note"],
    ] {
        let error = f.fail("Atlas", &args, 2);
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("--host SUPERVISOR")
        );
    }
    // Exercise the actual CLI/RPC transport against a separate authoritative DB.
    use std::os::unix::fs::PermissionsExt;
    let supervisor = Fixture::new();
    supervisor.run("Atlas", &["add", "Authoritative outline", "--id", "root"]);
    let bin = f.root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(&ssh, "#!/bin/sh\nexport HEY_BOSS_ISSUE_DB=\"$MM_SUPERVISOR_DB\"\nexec \"$MM_TEST_BINARY\" issue rpc\n").unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let remote = |args: &[&str]| {
        let mut cmd = f.cmd("Atlas", "mm", args);
        cmd.args(["--host", "supervisor.test"])
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("MM_SUPERVISOR_DB", supervisor.root.join("issues.db"))
            .env("MM_TEST_BINARY", env!("CARGO_BIN_EXE_hey-boss"));
        cmd.output().unwrap()
    };
    let graph = success(remote(&["show"]));
    assert_eq!(nodes(&graph).len(), 1);
    assert_eq!(nodes(&graph)[0]["title"], "Authoritative outline");
    success(remote(&[
        "add",
        "Remote authorship",
        "--under",
        "root",
        "--id",
        "remote",
        "--request-id",
        "remote-one",
    ]));
    assert_eq!(nodes(&supervisor.run("Atlas", &["show"])).len(), 2);
    std::fs::write(&ssh, "#!/bin/sh\nexit 255\n").unwrap();
    let failed = remote(&["add", "Failed remote write"]);
    assert_eq!(failed.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("No local fallback")
    );
    db.execute("UPDATE fleet_meta SET role='controller' WHERE id=1", [])
        .unwrap();
    let after = f.run("Atlas", &["show"]);
    assert_eq!(after["nodes"], before["nodes"]);
    assert_eq!(after["version"], before["version"]);
    f.run(
        "Atlas",
        &["edit", "note", "--title", "Supervisor authorship"],
    );
}

#[test]
fn issue_initial_labels_are_atomic_and_preserve_cross_project_resources() {
    let f = Fixture::new();
    f.issue(
        "Platform",
        &[
            "create",
            "--title",
            "Technical title",
            "--body",
            "Live body",
            "--label",
            "ready",
        ],
    );
    f.issue("Platform", &["claim", "1"]);
    f.issue("Platform", &["subtask", "create", "1", "--title", "Child"]);
    f.issue(
        "Platform",
        &["pr", "add", "1", "https://github.com/example/repo/pull/36"],
    );
    f.issue("Platform", &["close", "1"]);
    let resource = f.issue("Platform", &["view", "1"]);
    let children = f.issue("Platform", &["subtask", "list", "1", "--all"]);
    let root = f.run("Atlas", &["add", "Root", "--id", "root"]);
    let args = [
        "issue",
        "1",
        "--issue-project",
        "Platform",
        "--id",
        "api",
        "--under",
        "root",
        "--title",
        "Simple API",
        "--if-version",
        "1",
        "--request-id",
        "initial-label",
    ];
    let saved = f.run("Atlas", &args);
    assert_eq!(saved["version"], 2);
    assert_eq!(saved["node"]["display_label"], "Simple API");
    assert_eq!(saved["node"]["parent_id"], root["node"]["id"]);
    assert_eq!(f.run("Atlas", &args), saved);
    let view = f.run("Atlas", &["view", "api"]);
    assert_eq!(view["node"]["title"], "Simple API");
    assert_eq!(view["node"]["original_title"], "Technical title");
    assert_eq!(view["node"]["body"], "Live body");
    assert_eq!(view["node"]["state"], "closed");
    assert_eq!(view["node"]["labels"], json!(["ready"]));
    assert!(
        f.run("Atlas", &["links", "api"])["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link["automatic"] == true)
    );
    let no_op = f.run(
        "Atlas",
        &[
            "issue",
            "1",
            "--issue-project",
            "Platform",
            "--title",
            "Simple API",
            "--if-version",
            "2",
        ],
    );
    assert_eq!(no_op["changed"], false);
    assert_eq!(no_op["version"], 2);
    assert_eq!(no_op["node"]["updated_at"], saved["node"]["updated_at"]);
    f.fail(
        "Atlas",
        &[
            "issue",
            "1",
            "--issue-project",
            "Platform",
            "--title",
            "Stale",
            "--if-version",
            "1",
        ],
        4,
    );
    f.fail(
        "Atlas",
        &[
            "issue",
            "1",
            "--issue-project",
            "Platform",
            "--title",
            "Do not save",
            "--id",
            "other",
        ],
        4,
    );
    assert_eq!(f.run("Atlas", &["show"])["version"], 2);
    let changed = f.run(
        "Atlas",
        &[
            "issue",
            "1",
            "--issue-project",
            "Platform",
            "--title",
            "New label",
            "--under",
            "root",
            "--id",
            "api",
            "--if-version",
            "2",
        ],
    );
    assert_eq!(changed["version"], 3);
    assert_eq!(changed["node"]["id"], saved["node"]["id"]);
    assert_eq!(f.run("Atlas", &args), saved);
    let before_conflict = f.run("Atlas", &["show"]);
    f.fail(
        "Atlas",
        &[
            "issue",
            "1",
            "--issue-project",
            "Platform",
            "--title",
            "Do not save",
            "--under",
            "api",
        ],
        4,
    );
    f.fail("Atlas", &["issue", "1", "--issue-project", "Platform"], 4);
    assert_eq!(f.run("Atlas", &["show"]), before_conflict);
    assert_eq!(f.issue("Platform", &["view", "1"]), resource);
    assert_eq!(
        f.issue("Platform", &["subtask", "list", "1", "--all"]),
        children
    );
    f.issue("Atlas", &["create", "--title", "Local issue"]);
    let before = f.run("Atlas", &["show"]);
    f.fail("Atlas", &["issue", "1", "--title", &"x".repeat(513)], 2);
    f.fail("Atlas", &["issue", "1", "--title", ""], 2);
    f.fail(
        "Atlas",
        &["issue", "1", "--title", "Local label", "--under", "missing"],
        3,
    );
    assert_eq!(f.run("Atlas", &["show"]), before);
    let local = f.run(
        "Atlas",
        &["issue", "1", "--title", "Local label", "--if-version", "3"],
    );
    assert_eq!(local["version"], 4);
    assert_eq!(
        f.run("Atlas", &["view", "issue:1"])["node"]["title"],
        "Local label"
    );
}

#[test]
fn issue_map_labels_preserve_live_resources_versions_and_retries() {
    let f = Fixture::new();
    f.issue(
        "Platform",
        &[
            "create",
            "--title",
            "Technical API title",
            "--body",
            "Live details",
            "--label",
            "ready",
        ],
    );
    f.run(
        "Atlas",
        &["issue", "1", "--issue-project", "Platform", "--id", "api"],
    );
    f.issue("Platform", &["claim", "1"]);
    let url = "https://github.com/example/repo/pull/30";
    f.issue("Platform", &["pr", "add", "1", url]);
    let before = f.issue("Platform", &["view", "1"]);
    let map = f.run("Atlas", &["show"]);
    let version = map["version"].to_string();
    let args = [
        "edit",
        "api",
        "--title",
        "Simple API",
        "--if-version",
        &version,
        "--request-id",
        "short-label",
    ];
    let edited = f.run("Atlas", &args);
    let labeled = f.run("Atlas", &["view", "api"]);
    assert_eq!(labeled["node"]["title"], "Simple API");
    assert_eq!(labeled["node"]["original_title"], "Technical API title");
    assert_eq!(edited["node"]["display_label"], "Simple API");
    assert_eq!(labeled["node"]["labels"], json!(["ready"]));
    assert_eq!(labeled["node"]["assignee"], "human:test");
    let no_op = f.run("Atlas", &["edit", "api", "--title", "Simple API"]);
    assert_eq!(no_op["changed"], false);
    assert_eq!(no_op["version"], edited["version"]);
    assert!(
        f.run("Atlas", &["links", "api"])["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link["automatic"] == true)
    );
    assert_eq!(f.run("Atlas", &args), edited);
    assert_eq!(f.issue("Platform", &["view", "1"]), before);
    assert!(f.terminal("Atlas", &["show"]).contains("Simple API"));
    assert!(f.terminal("Atlas", &["export"]).contains("Simple API"));
    assert!(
        f.terminal("Atlas", &["view", "api"])
            .contains("Technical API title")
    );
    f.fail(
        "Atlas",
        &["edit", "api", "--clear-label", "--if-version", &version],
        4,
    );
    f.issue(
        "Platform",
        &[
            "edit",
            "1",
            "--title",
            "Updated technical title",
            "--body",
            "Updated details",
        ],
    );
    for mode in ["none", "preview", "full"] {
        let shown = f.run("Atlas", &["show", "--bodies", mode]);
        assert_eq!(alias(&shown, "api")["title"], "Simple API");
        assert_eq!(
            alias(&shown, "api")["original_title"],
            "Updated technical title"
        );
    }
    assert_eq!(
        f.run("Atlas", &["view", "api"])["node"]["body"],
        "Updated details"
    );
    let before_clear = f.issue("Platform", &["view", "1"]);
    let cleared = f.run("Atlas", &["edit", "api", "--clear-label"]);
    assert_eq!(
        f.run("Atlas", &["view", "api"])["node"]["title"],
        "Updated technical title"
    );
    assert!(cleared["node"]["display_label"].is_null());
    assert_eq!(f.issue("Platform", &["view", "1"]), before_clear);
    assert_eq!(
        f.run("Atlas", &["edit", "api", "--clear-label"])["changed"],
        false
    );
    f.fail("Atlas", &["edit", "api", "--title", &"x".repeat(513)], 2);
    f.run("Atlas", &["add", "Topic", "--id", "topic"]);
    f.fail("Atlas", &["edit", "topic", "--clear-label"], 2);
    f.issue("Atlas", &["create", "--title", "Local technical title"]);
    f.run("Atlas", &["issue", "1"]);
    assert_eq!(
        f.run(
            "Atlas",
            &["edit", "issue:1", "--title", "Local short title"]
        )["node"]["display_label"],
        "Local short title"
    );
}

#[test]
fn issue_label_migration_preserves_existing_map_and_live_title() {
    let f = Fixture::new();
    f.issue("Atlas", &["create", "--title", "Existing issue"]);
    let map = f.run("Atlas", &["issue", "1", "--id", "existing"]);
    let db = rusqlite::Connection::open(f.root.join("issues.db")).unwrap();
    db.execute_batch(
        "ALTER TABLE mindmap_nodes DROP COLUMN display_label; PRAGMA user_version=11;",
    )
    .unwrap();
    drop(db);
    let migrated = f.run("Atlas", &["show"]);
    assert_eq!(migrated["version"], map["version"]);
    assert_eq!(alias(&migrated, "existing")["id"], map["node"]["id"]);
    assert_eq!(alias(&migrated, "existing")["title"], "Existing issue");
    assert!(alias(&migrated, "existing")["display_label"].is_null());
    assert_eq!(
        f.run("Atlas", &["edit", "existing", "--title", "Short label"])["node"]["display_label"],
        "Short label"
    );
}

fn batch_file(f: &Fixture, edits: Value) -> String {
    let path = f.root.join("batch.json");
    std::fs::write(&path, edits.to_string()).unwrap();
    path.to_str().unwrap().to_owned()
}

#[test]
fn batch_preview_atomic_commit_retry_and_resource_preservation() {
    let f = Fixture::new();
    f.issue(
        "Atlas",
        &[
            "create",
            "--title",
            "Original issue",
            "--body",
            "Resource body",
        ],
    );
    f.issue(
        "Atlas",
        &["pr", "add", "1", "https://github.com/org/repo/pull/1"],
    );
    f.run("Atlas", &["add", "Root", "--id", "root"]);
    f.run("Atlas", &["issue", "1", "--id", "followup"]);
    f.issue(
        "Atlas",
        &["subtask", "create", "1", "--title", "Child issue"],
    );
    f.issue("Atlas", &["claim", "1"]);
    let resource = f.issue("Atlas", &["view", "1"]);
    let before = f.run("Atlas", &["show"]);
    let version = before["version"].to_string();
    let file = batch_file(
        &f,
        json!([
            {"command":"edit","node":"issue:1","title":"Short label"},
            {"command":"alias","node":"followup","alias":"short"},
            {"command":"move","node":"followup","under":"root"}
        ]),
    );
    let preview = f.run(
        "Atlas",
        &[
            "batch",
            "--file",
            &file,
            "--dry-run",
            "--if-version",
            &version,
        ],
    );
    assert_eq!(preview["dry_run"], true);
    assert_eq!(preview["version"], before["version"].as_i64().unwrap() + 1);
    assert_eq!(preview["changed_nodes"].as_array().unwrap().len(), 1);
    assert_eq!(f.run("Atlas", &["show"]), before);
    let args = [
        "batch",
        "--file",
        &file,
        "--if-version",
        &version,
        "--request-id",
        "batch-1",
    ];
    let saved = f.run("Atlas", &args);
    assert_eq!(saved["version"], preview["version"]);
    assert_eq!(f.run("Atlas", &args), saved);
    let graph = f.run("Atlas", &["show"]);
    assert_eq!(alias(&graph, "short")["title"], "Short label");
    assert_eq!(
        alias(&graph, "short")["parent_id"],
        alias(&graph, "root")["id"]
    );
    assert_eq!(f.issue("Atlas", &["view", "1"]), resource);
    assert!(saved.to_string().len() < 4096);
    let noop = batch_file(
        &f,
        json!([
            {"command":"edit","node":"short","title":"Short label"},
            {"command":"move","node":"short","under":"root"}
        ]),
    );
    assert_eq!(
        f.run("Atlas", &["batch", "--file", &noop])["changed"],
        false
    );
    assert_eq!(f.run("Atlas", &["show"]), graph);
}

#[test]
fn invalid_batches_roll_back_every_edit_and_revision() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Root", "--id", "root"]);
    f.run(
        "Atlas",
        &["add", "Child", "--id", "child", "--under", "root"],
    );
    f.run("Other", &["add", "Foreign", "--id", "foreign"]);
    let before = f.run("Atlas", &["show"]);
    for (edits, code) in [
        (
            json!([{ "command":"edit","node":"root","title":"Changed" }, {"command":"edit","node":"missing","title":"Missing"}]),
            3,
        ),
        (
            json!([{ "command":"edit","node":"root","title":"Changed" }, {"command":"move","node":"root","under":"child"}]),
            4,
        ),
        (
            json!([{ "command":"edit","node":"root","title":"Changed" }, {"command":"alias","node":"child","alias":"root"}]),
            4,
        ),
        (
            json!([{ "command":"edit","node":"root","title":"Same" }, {"command":"edit","node":"child","title":"Same"}]),
            4,
        ),
        (
            json!([{ "command":"edit","node":"root","title":"Changed" }, {"command":"move","node":"child","under":"Other::foreign"}]),
            2,
        ),
        (
            json!([{ "command":"remove","node":"root","recursive":true}]),
            2,
        ),
        (
            json!([{ "command":"edit","node":"root","title":"Changed","typo":true}]),
            2,
        ),
    ] {
        let file = batch_file(&f, edits);
        f.fail("Atlas", &["batch", "--file", &file], code);
        assert_eq!(f.run("Atlas", &["show"]), before);
    }
    let file = batch_file(
        &f,
        json!([{ "command":"edit","node":"root","title":"Changed" }]),
    );
    f.fail("Atlas", &["batch", "--file", &file, "--if-version", "0"], 4);
    f.fail(
        "Atlas",
        &[
            "batch",
            "--file",
            &file,
            "--dry-run",
            "--request-id",
            "preview",
        ],
        2,
    );
    assert_eq!(f.run("Atlas", &["show"]), before);
}

#[test]
fn batch_reads_stdin_and_empty_batches_are_harmless() {
    let f = Fixture::new();
    let mut command = f.cmd("Atlas", "mm", &["batch", "--file", "-"]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"[]").unwrap();
    let result = success(child.wait_with_output().unwrap());
    assert_eq!(result["changed"], false);
    assert_eq!(result["version"], 0);
    assert_eq!(result["changed_nodes"], json!([]));
}

#[test]
fn batch_reorders_siblings_clears_labels_and_reports_net_changes_only() {
    let f = Fixture::new();
    f.run("Atlas", &["add", "Root", "--id", "root"]);
    f.run("Atlas", &["add", "Other", "--id", "other"]);
    f.issue("Atlas", &["create", "--title", "Live title"]);
    f.run("Atlas", &["issue", "1", "--id", "issue"]);
    f.run("Atlas", &["edit", "issue", "--title", "Old label"]);
    let before = f.run("Atlas", &["show"]);
    let file = batch_file(
        &f,
        json!([
            {"command":"edit","node":"issue","clear_label":true},
            {"command":"move","node":"issue","before":"root"}
        ]),
    );
    let result = f.run("Atlas", &["batch", "--file", &file]);
    assert_eq!(result["changed_nodes"].as_array().unwrap().len(), 3);
    assert_eq!(result["version"], before["version"].as_i64().unwrap() + 1);
    let after = f.run("Atlas", &["show"]);
    assert_eq!(alias(&after, "issue")["title"], "Live title");
    assert_eq!(alias(&after, "issue")["position"], 0);
    let noop = batch_file(
        &f,
        json!([
            {"command":"edit","node":"root","title":"Temporary"},
            {"command":"edit","node":"root","title":"Root"}
        ]),
    );
    let result = f.run("Atlas", &["batch", "--file", &noop]);
    assert_eq!(result["changed"], false);
    assert_eq!(result["changed_nodes"], json!([]));
    assert_eq!(f.run("Atlas", &["show"]), after);
}

#[test]
fn batch_many_labels_have_one_revision_and_compact_receipts() {
    let f = Fixture::new();
    let mut edits = Vec::new();
    for n in 1..=47 {
        f.issue(
            "Atlas",
            &[
                "create",
                "--title",
                &format!("Original issue {n}"),
                "--body",
                "Large body. ".repeat(1000).as_str(),
            ],
        );
        f.run("Atlas", &["issue", &n.to_string()]);
        edits.push(
            json!({"command":"edit","node":format!("issue:{n}"),"title":format!("Label {n}")}),
        );
    }
    let file = batch_file(&f, json!(edits));
    let result = f.run(
        "Atlas",
        &[
            "batch",
            "--file",
            &file,
            "--if-version",
            "47",
            "--request-id",
            "47-labels",
        ],
    );
    assert_eq!(result["version"], 48);
    assert_eq!(result["changed_nodes"].as_array().unwrap().len(), 47);
    assert!(result.to_string().len() < 32768);
    assert!(!result.to_string().contains("Large body"));
    f.fail(
        "Atlas",
        &[
            "batch",
            "--file",
            &batch_file(&f, json!([])),
            "--request-id",
            "47-labels",
        ],
        4,
    );
}
