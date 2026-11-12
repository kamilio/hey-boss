//! Offline regression through the installed Pi CLI. Optional PI_COMPACTION_SESSION is read-only.
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdout},
    time::timeout,
};

const MODEL: &str = "gemini-test";
const REPLY: &str = "Offline fixture summary.";
type Budgets = Arc<Mutex<Vec<u64>>>;

async fn provider() -> (String, Budgets) {
    let budgets = Budgets::default();
    let requests = budgets.clone();
    let server=Router::new().route("/v1/responses",post(move |Json(body):Json<Value>| {
        let requests=requests.clone();
        async move {
            requests.lock().unwrap().push(body["max_output_tokens"].as_u64().expect("Pi output budget"));
            let item=json!({"id":"msg_fixture","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":REPLY,"annotations":[]}]});
            let response=json!({"id":"resp_fixture","object":"response","model":MODEL,"status":"completed","output":[item],"usage":{"input_tokens":10,"output_tokens":4,"total_tokens":14}});
            let events=[json!({"type":"response.created","response":{"id":"resp_fixture","model":MODEL}}),
                json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_fixture","type":"message","role":"assistant","content":[]}}),
                json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
                json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":REPLY}),
                json!({"type":"response.output_item.done","output_index":0,"item":item}),
                json!({"type":"response.completed","response":response})];
            ([("content-type","text/event-stream")],events.iter().map(|e|format!("event: {}\ndata: {e}\n\n",e["type"].as_str().unwrap())).collect::<String>())
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
    (address.to_string(), budgets)
}

struct Pi {
    child: Child,
    lines: Lines<BufReader<ChildStdout>>,
}
impl Pi {
    fn start(root: &Path, session: &Path, model: &str) -> Self {
        let mut child =
            tokio::process::Command::new(env::var_os("PI_BIN").unwrap_or_else(|| "pi".into()))
                .current_dir(root)
                .env_clear()
                .env("HOME", root)
                .env("TMPDIR", root)
                .env("PATH", env::var_os("PATH").unwrap())
                .env("PI_CODING_AGENT_DIR", root)
                .args([
                    "--offline",
                    "--no-tools",
                    "--no-extensions",
                    "--no-skills",
                    "--no-prompt-templates",
                    "--no-context-files",
                    "--no-themes",
                    "--mode",
                    "rpc",
                    "--provider",
                    "hey-proxy",
                    "--model",
                    model,
                    "--session",
                ])
                .arg(session)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .expect("install Pi or set PI_BIN");
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self { child, lines }
    }
    async fn send(&mut self, msg: Value) {
        self.child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn wait(&mut self, key: &str, value: &str) -> Value {
        timeout(Duration::from_secs(30), async {
            while let Some(line) = self.lines.next_line().await.unwrap() {
                let event: Value = serde_json::from_str(&line).unwrap();
                if event[key] == value {
                    return event;
                }
            }
            panic!("Pi exited before {key}={value}");
        })
        .await
        .expect("Pi RPC deadline")
    }
    async fn rpc(&mut self, cmd: &str) -> Value {
        self.send(json!({"type":cmd,"id":cmd})).await;
        self.wait("id", cmd).await
    }
    async fn prompt(&mut self) {
        self.send(json!({"type":"prompt","message":"Return the fixture reply."}))
            .await;
        let end = self.wait("type", "agent_end").await;
        let message = end["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(
            message["stopReason"], "stop",
            "Pi failed at the loopback provider: {}",
            message["errorMessage"]
        );
        assert_eq!(message["content"][0]["text"], REPLY);
    }
    async fn stop(mut self) {
        self.child.kill().await.unwrap();
    }
}

fn configure(root: &Path, address: &str, registry: Option<Value>) {
    let mut cfg = json!({"listen":address,"aliases":[{"from":MODEL,"to":"gemini/gemini-early-exp"},{"from":"coding","to":"unknown"}]});
    if let Some(registry) = registry {
        cfg["model_registry"] = registry;
    }
    fs::write(root.join("proxy.json"), cfg.to_string()).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_hey-proxy"))
        .current_dir(root)
        .env_clear()
        .env("HOME", root)
        .env("TMPDIR", root)
        .env("PATH", env::var_os("PATH").unwrap())
        .env("PI_CODING_AGENT_DIR", root)
        .args(["--config", "proxy.json", "configure-pi"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}
fn branches() -> Vec<Vec<Value>> {
    if let Some(path) = env::var_os("PI_COMPACTION_SESSION") {
        use std::collections::{HashMap, HashSet};
        let text = fs::read_to_string(path).unwrap();
        let entries: Vec<Value> = text
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        let by_id: HashMap<_, _> = entries
            .iter()
            .filter_map(|e| Some((e["id"].as_str()?, e)))
            .collect();
        entries
            .iter()
            .filter(|e| {
                e["type"] == "message"
                    && e["message"]["role"] == "assistant"
                    && e["message"]["model"] == MODEL
                    && e["message"]["stopReason"] == "length"
            })
            .map(|e| {
                let mut cursor = e["parentId"].as_str();
                let mut seen = HashSet::new();
                let mut branch = vec![];
                while let Some(id) = cursor {
                    assert!(seen.insert(id), "cyclic session");
                    let parent = by_id[id];
                    branch.push(parent.clone());
                    cursor = parent["parentId"].as_str();
                }
                branch.reverse();
                branch
            })
            .collect()
    } else {
        [122133,123070,125008].map(|total| (0..18).map(|n|{
            let mut msg=json!({"role":"user","timestamp":0,"content":[{"type":"text","text":"x".repeat(20000)}]});
            if n%2==1 {msg["role"]="assistant".into();msg["api"]="openai-responses".into();msg["provider"]="hey-proxy".into();msg["model"]=MODEL.into();msg["stopReason"]="stop".into();msg["usage"]=json!({"input":total-100,"output":100,"cacheRead":0,"cacheWrite":0,"totalTokens":total,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}});}
            json!({"type":"message","id":format!("entry-{n}"),"parentId":(n>0).then(||format!("entry-{}",n-1)),"timestamp":"1970-01-01T00:00:00.000Z","message":msg})
        }).collect()).to_vec()
    }
}
fn session(root: &Path, branch: &[Value]) -> PathBuf {
    let sessions = root.join("test-sessions");
    fs::create_dir_all(&sessions).unwrap();
    let path = sessions.join("fixture.jsonl");
    let mut file = fs::File::create(&path).unwrap();
    writeln!(file,"{}",json!({"type":"session","version":3,"id":"a745a55c-aba9-4911-b088-3077c7258e00","timestamp":"2026-01-01T00:00:00.000Z","cwd":root})).unwrap();
    for entry in branch {
        writeln!(file, "{entry}").unwrap();
    }
    path
}
#[tokio::test]
#[ignore = "requires an installed Pi CLI"]
async fn installed_pi_recovers_output_budget_after_compaction() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("out");
    fs::create_dir_all(&out).unwrap();
    let tmp = tempfile::tempdir_in(out).unwrap();
    let root = tmp.path();
    let (addr, budgets) = provider().await;
    let settings = root.join("settings.json");
    let old =
        json!({"compaction":{"enabled":true,"keepRecentTokens":100000,"reserveTokens":200000}});
    let branches = branches();
    assert!(
        branches.len() >= 3,
        "expected at least three regression branches"
    );
    for branch in &branches {
        fs::write(&settings, old.to_string()).unwrap();
        configure(root, &addr, None);
        let fixed = fs::read(&settings).unwrap();
        fs::write(&settings, old.to_string()).unwrap();
        let path = session(root, branch);
        let mut pi = Pi::start(root, &path, MODEL);
        let state = pi.rpc("get_state").await;
        assert_eq!(
            state["data"]["model"]["baseUrl"],
            format!("http://{addr}/v1")
        );
        assert!(
            state["data"]["messageCount"].as_u64().unwrap() > 0,
            "Pi must load the fixture history"
        );
        let old_plan = pi.rpc("compact").await;
        assert_eq!(old_plan["success"], false);
        assert!(
            old_plan["error"]
                .as_str()
                .unwrap()
                .contains("Nothing to compact"),
            "unexpected compact error: {}",
            old_plan["error"]
        );
        budgets.lock().unwrap().clear();
        pi.prompt().await;

        assert!(
            *budgets.lock().unwrap().first().unwrap() < 2000,
            "fixture must reproduce output starvation"
        );
        pi.stop().await;
        fs::write(&settings, fixed).unwrap();
        let path = session(root, branch);
        let mut pi = Pi::start(root, &path, MODEL);
        let state = pi.rpc("get_state").await;
        assert_eq!(state["data"]["model"]["contextWindow"], 128000);
        assert_eq!(state["data"]["model"]["maxTokens"], 16384);
        budgets.lock().unwrap().clear();
        // Context edits in saved forks can invalidate usage estimates. Their
        // length-error recovery requests compaction explicitly. Synthetic
        // histories must trigger Pi's ordinary preflight compaction on prompt.
        if env::var_os("PI_COMPACTION_SESSION").is_some() {
            let result = pi.rpc("compact").await;
            assert_eq!(
                result["success"], true,
                "Pi could not compact the replay branch"
            );
        }
        pi.prompt().await;
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .lines()
                .filter_map(|s| serde_json::from_str::<Value>(s).ok())
                .any(|e| e["type"] == "compaction"
                    && e["summary"].as_str().is_some_and(|s| s.contains(REPLY))
                    && e["firstKeptEntryId"].is_string())
        );
        assert_eq!(*budgets.lock().unwrap().last().unwrap(), 16384);
        pi.stop().await;
    }
    // Repair the malformed catalog produced by the earlier registry writer.
    let catalog_path = root.join("models.json");
    let mut catalog: Value = serde_json::from_slice(&fs::read(&catalog_path).unwrap()).unwrap();
    catalog["providers"]["hey-proxy"]["modelOverrides"] = Value::Null;
    fs::write(&catalog_path, catalog.to_string()).unwrap();
    for max in [65536, 32768] {
        configure(
            root,
            &addr,
            Some(
                json!({"defaults":{"context_window":128000,"max_tokens":16384,"keep_recent_tokens":20000,"reserve_tokens":16384},"models":{"gemini/gemini-early-exp":{"context_window":1048576,"max_tokens":max,"keep_recent_tokens":100000,"reserve_tokens":200000}}}),
            ),
        );
        for (id, context, limit, keep, reserve) in [
            (MODEL, 1048576, max, 100000, 200000),
            ("gemini/gemini-early-exp", 1048576, max, 100000, 200000),
            ("coding", 128000, 16384, 20000, 16384),
            ("unknown", 128000, 16384, 20000, 16384),
        ] {
            let path = session(root, &[]);
            let mut pi = Pi::start(root, &path, id);
            let model = pi.rpc("get_state").await["data"]["model"].clone();
            assert_eq!(model["id"], id);
            assert_eq!(model["contextWindow"], context);
            assert_eq!(model["maxTokens"], limit);
            let settings: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
            let c = &settings["compaction"];
            assert_eq!(c["enabled"], true);
            let o = &c["modelOverrides"][format!("hey-proxy/{id}")];
            assert_eq!(
                o.get("keepRecentTokens").unwrap_or(&c["keepRecentTokens"]),
                keep
            );
            assert_eq!(
                o.get("reserveTokens").unwrap_or(&c["reserveTokens"]),
                reserve
            );
            budgets.lock().unwrap().clear();
            pi.prompt().await;
            assert_eq!(budgets.lock().unwrap()[0], limit);
            pi.stop().await;
        }
    }
}
