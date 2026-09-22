use hey_boss::issues::{Request, Store};
use serde_json::{Value, json};
use std::process::{Command, Output};

struct Fixture {
    directory: std::path::PathBuf,
    db: std::path::PathBuf,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("hey-boss-lookup-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        Self {
            db: directory.join("issues.db"),
            directory,
        }
    }
    fn execute(&self, operation: Value) -> Value {
        let request: Request = serde_json::from_value(json!({"version":1,"project":{"id":"github.com/poe-platform/poe-code","name":"poe-code"},"actor":{"id":"human:boss","kind":"human","session_id":null,"machine":"test","host":"test","pid":null,"process_start":null,"cwd":"/tmp","source":"test"},"operation":operation})).unwrap();
        Store::open(&self.db).unwrap().execute(&request).unwrap()
    }
    fn lookup(&self, url: &str, json: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hey-boss"));
        command
            .args(["lookup", url])
            .env("HEY_BOSS_ISSUE_DB", &self.db)
            .env("HEY_BOSS_ISSUE_PROJECT", "named:Wrong project")
            .env_remove("HEY_BOSS_ISSUE_HOST");
        if json {
            command.arg("--json");
        }
        command.output().unwrap()
    }
    fn value(&self, url: &str) -> Value {
        let output = self.lookup(url, true);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn lookup_resolves_issue_url_project_before_worker_default_without_mutating() {
    let f = Fixture::new("issue");
    f.execute(json!({"action":"create","title":"Lookup example","body":"# Details\nRead only","labels":[]}));
    let url = "http://127.0.0.1:4781/#project=github.com%2Fpoe-platform%2Fpoe-code&view=issues&inbox_state=unread&issue=1&state=closed&owner=all";
    let value = f.value(url);
    assert_eq!(value["route"]["entity"], "issue");
    assert_eq!(value["issue"]["title"], "Lookup example");
    assert_eq!(value["issue"]["body"], "# Details\nRead only");
    assert_eq!(value["issue"]["version"], 1);
    let phone = f.value("https://hey-boss-mobile-kamil.fly.dev/issues#project=github.com%2Fpoe-platform%2Fpoe-code&issue=1");
    assert_eq!(phone["route"]["entity"], "issue");
    assert_eq!(phone["issue"]["title"], "Lookup example");
    let text = f.lookup(url, false);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("poe-code#1"), "{text}");
    assert!(text.contains("Read only"));
    assert_eq!(
        f.execute(json!({"action":"view","number":1}))["issue"]["version"],
        1
    );
}

#[test]
fn lookup_reads_artifacts_and_mindmap_nodes_from_desktop_and_mobile_links() {
    let f = Fixture::new("resources");
    let artifact = f.execute(json!({"action":"artifact","operation":{"command":"create","title":"Design","body":"Document contents"}}));
    let id = artifact["artifact"]["id"].as_str().unwrap();
    let doc = f.value(&format!("https://example.invalid/artifacts#project=github.com%2Fpoe-platform%2Fpoe-code&artifact={id}&issue=91"));
    assert_eq!(doc["route"]["entity"], "artifact");
    assert_eq!(doc["artifact"]["body"], "Document contents");
    let added = f.execute(json!({"action":"mindmap","operation":{"command":"add","kind":"text","title":"Topic","body":"Topic contents","alias":"topic"}}));
    let id = added["node"]["id"].as_str().unwrap();
    for path in ["mm", "project-resource"] {
        let node = f.value(&format!(
            "http://localhost/{path}#project=github.com%2Fpoe-platform%2Fpoe-code&node={id}"
        ));
        assert_eq!(node["route"]["entity"], "node");
        assert_eq!(node["node"]["body"], "Topic contents");
    }
}

#[test]
fn lookup_rejects_invalid_and_missing_targets_instead_of_returning_unrelated_items() {
    let f = Fixture::new("errors");
    f.execute(json!({"action":"create","title":"Existing","body":"","labels":[]}));
    for url in [
        "ftp://localhost/#issue=1",
        "https://localhost/unknown#issue=1",
        "http://localhost/#issue=-1",
        "http://localhost/#issue=01",
        "http://localhost/#issue=9007199254740992",
        "http://localhost/agents/session#run=missing",
        "http://localhost/#project=%ZZ&issue=1",
        "http://localhost/#project=%FF&issue=1",
        "http://localhost/#project=github.com%2Fpoe-platform%2Fpoe-code&issue=99",
    ] {
        let output = f.lookup(url, true);
        assert!(!output.status.success(), "{url}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], false, "{url}: {value}");
        assert!(value["error"]["code"].is_string());
    }
}

#[test]
fn lookup_notice_uses_read_only_inbox_view_and_query_task_links() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    let f = Fixture::new("notices");
    let socket = f.directory.join("inbox.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let thread = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            let payload: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(payload["command"], "inbox_view");
            assert_eq!(payload["task_id"], "notice-1");
            let result = json!({"task":{"taskID":"notice-1","title":"Read me","kind":"alert","question":"Notice contents","status":"pending"}}).to_string();
            stream
                .write_all(
                    json!({"status":"ok","result":result})
                        .to_string()
                        .as_bytes(),
                )
                .unwrap();
        }
    });
    for url in [
        "http://localhost/#view=inbox&notice=notice-1&issue=5&host=devbox",
        "https://example.invalid/?task=notice-1",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(["lookup", url, "--json"])
            .env("HEY_BOSS_INBOX_SOCKET", &socket)
            .env_remove("HEY_BOSS_ISSUE_HOST")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["route"]["entity"], "notice");
        assert_eq!(value["task"]["status"], "pending");
    }
    thread.join().unwrap();
}

#[test]
fn rust_router_matches_shared_browser_route_contract() {
    let fixtures: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/lookup-routes.json")).unwrap();
    for fixture in fixtures {
        let route = hey_boss::routes::resolve(fixture["url"].as_str().unwrap()).unwrap();
        let value = serde_json::to_value(route).unwrap();
        for key in ["entity", "id", "project", "host"] {
            assert_eq!(
                value[key].as_str().unwrap_or(""),
                fixture[key].as_str().unwrap(),
                "{fixture}: {key}"
            );
        }
    }
}

#[test]
fn lookup_collections_keep_web_issue_filters() {
    let f = Fixture::new("collections");
    f.execute(json!({"action":"create","title":"Match","body":"","labels":["ready"]}));
    f.execute(json!({"action":"create","title":"Other","body":"","labels":[]}));
    let value = f.value("http://localhost/#project=github.com%2Fpoe-platform%2Fpoe-code&search=Match&label=ready&owner=unassigned");
    assert_eq!(value["route"]["entity"], "issues");
    assert_eq!(value["issues"].as_array().unwrap().len(), 1);
    assert_eq!(value["issues"][0]["title"], "Match");
    assert_eq!(
        f.value("http://localhost/#project=github.com%2Fpoe-platform%2Fpoe-code&owner=mine")["issues"],
        json!([])
    );
    assert_eq!(
        f.value("http://localhost/artifacts#project=github.com%2Fpoe-platform%2Fpoe-code")["artifacts"],
        json!([])
    );
    assert_eq!(
        f.value("http://localhost/mm#project=github.com%2Fpoe-platform%2Fpoe-code")["nodes"],
        json!([])
    );
}

#[test]
fn lookup_collections_preserve_blocked_and_all_state_filters() {
    let f = Fixture::new("blocked-collections");
    f.execute(json!({"action":"create","title":"Runnable","body":"","labels":[]}));
    f.execute(json!({"action":"create","title":"Blocked","body":"","labels":[]}));
    f.execute(json!({"action":"block","number":2,"comment":"Needs external access","force":false}));
    let blocked = f.value("http://127.0.0.1:4781/#project=github.com%2Fpoe-platform%2Fpoe-code&view=issues&inbox_state=unread&state=blocked&owner=all");
    assert_eq!(blocked["issues"].as_array().unwrap().len(), 1);
    assert_eq!(blocked["issues"][0]["number"], 2);
    assert_eq!(blocked["issues"][0]["state"], "blocked");
    assert_eq!(f.value("http://localhost/#project=github.com%2Fpoe-platform%2Fpoe-code&state=all")["issues"].as_array().unwrap().len(), 2);
}

#[test]
fn lookup_agent_links_load_real_saved_conversation_and_filter_overview() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    let f = Fixture::new("agents");
    f.execute(json!({"action":"create","title":"Agent task","body":"","labels":[]}));
    let session = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let db = rusqlite::Connection::open(&f.db).unwrap();
    db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,session_id,state,owner_pid,owner_start,machine,started_at,updated_at) VALUES('run-1','github.com/poe-platform/poe-code',1,'issue','human:boss',?1,'completed',1,'test','test',1,1)", [session]).unwrap();
    let sessions = f.directory.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join(format!("rollout-{session}.jsonl")), json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Saved reply"}]}}).to_string()+"\n").unwrap();
    let listener = UnixListener::bind(f.directory.join("fleet.sock")).unwrap();
    let thread = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut input = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut input)
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&input).unwrap()["kind"],
                "status"
            );
            stream.write_all(json!({"ok":true,"machines":[{"host":"local","state":"connected","workers":[{"id":"worker-1","pid":1,"runs":[{"id":"run-1","project_id":"github.com/poe-platform/poe-code","title":"Agent task"}]}]}]}).to_string().as_bytes()).unwrap();
        }
    });
    for (url, entity) in [
        (
            "http://localhost/agents#project=github.com%2Fpoe-platform%2Fpoe-code",
            "agents",
        ),
        (
            "http://localhost/agents/session#host=local&run=run-1",
            "agent",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
            .args(["lookup", url, "--json"])
            .env("HEY_BOSS_ISSUE_DB", &f.db)
            .env("HEY_BOSS_FLEET_STATE", &f.directory)
            .env("CODEX_HOME", &f.directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["route"]["entity"], entity);
        if entity == "agent" {
            assert_eq!(value["messages"][0]["text"], "Saved reply");
        } else {
            assert_eq!(
                value["machines"][0]["workers"][0]["runs"][0]["title"],
                "Agent task"
            );
        }
    }
    thread.join().unwrap();
}

#[test]
fn lookup_remote_host_uses_existing_rpc_and_never_falls_back() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("remote");
    let bin = f.directory.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(&ssh, "#!/bin/sh\ncat > \"$HEY_BOSS_TEST_REQUEST\"\nprintf '%s' '{\"ok\":true,\"project\":{\"name\":\"poe-code\"},\"issue\":{\"number\":114,\"title\":\"Remote item\"}}'\n").unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let captured = f.directory.join("request.json");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let url =
        "http://localhost/#project=github.com%2Fpoe-platform%2Fpoe-code&host=devbox&issue=114";
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args(["lookup", url, "--json"])
        .env("PATH", &path)
        .env("HEY_BOSS_TEST_REQUEST", &captured)
        .env("HEY_BOSS_ISSUE_DB", &f.db)
        .output()
        .unwrap();
    assert!(output.status.success());
    let request: Value = serde_json::from_slice(&std::fs::read(captured).unwrap()).unwrap();
    assert_eq!(
        request["project_override"],
        "github.com/poe-platform/poe-code"
    );
    assert_eq!(request["operation"]["number"], 114);
    assert!(request["actor"].is_null());
    assert!(!f.db.exists());
    std::fs::write(&ssh, "#!/bin/sh\nexit 1\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hey-boss"))
        .args(["lookup", url, "--json"])
        .env("PATH", path)
        .env("HEY_BOSS_ISSUE_DB", &f.db)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["error"]["code"],
        "transport_error"
    );
    assert!(!f.db.exists());
}
