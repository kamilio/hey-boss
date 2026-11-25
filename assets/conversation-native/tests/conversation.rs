use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use hey_boss_conversation::{agent, context, media, plugins::Plugins, provider};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, UnixListener},
    sync::mpsc,
};
fn scratch() -> tempfile::TempDir {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out");
    fs::create_dir_all(&path).unwrap();
    tempfile::tempdir_in(path).unwrap()
}
fn resources() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_conversation-diagram"))
        .parent()
        .unwrap()
        .into()
}
fn msg(s: &str) -> Value {
    json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":s}]})
}
fn tool(name: &str, args: Value) -> Value {
    json!({"type":"function_call","call_id":"call_1","name":name,"arguments":args.to_string()})
}
async fn mock(items: Vec<Value>) -> Result<(String, tokio::task::JoinHandle<Result<Vec<Value>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/v1/responses", listener.local_addr()?);
    Ok((
        url,
        tokio::spawn(async move {
            let mut requests = vec![];
            for item in items {
                let (stream, _) = listener.accept().await?;
                let mut s = BufReader::new(stream);
                requests.push(read_request(&mut s).await?);
                respond(&mut s, item).await?;
            }
            Ok(requests)
        }),
    ))
}
async fn read_request(s: &mut BufReader<tokio::net::TcpStream>) -> Result<Value> {
    let mut line = String::new();
    let mut len = None;
    loop {
        line.clear();
        assert!(s.read_line(&mut line).await? > 0);
        if line == "\r\n" {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = Some(v.trim().parse::<usize>()?);
        }
    }
    let mut bytes = vec![0; len.unwrap()];
    s.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}
async fn respond(s: &mut BufReader<tokio::net::TcpStream>, item: Value) -> Result<()> {
    let mut events = vec![];
    if let Some(text) = item["content"][0]["text"].as_str() {
        events.push(json!({"type":"response.output_text.delta","delta":text}));
    }
    events.push(json!({"type":"response.completed","response":{"output":[item]}}));
    let body = events
        .iter()
        .map(|e| format!("data: {e}\r\n\r\n"))
        .collect::<String>();
    s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await?;
    Ok(())
}
#[tokio::test]
async fn attached_updates_cannot_act_and_user_turns_keep_the_fixed_thread() -> Result<()> {
    let tmp = scratch();
    let root = tmp.path();
    fs::write(root.join("AGENTS.md"), "")?;
    fs::write(
        root.join("hey-t3.mjs"),
        include_str!("../examples/t3-replay.mjs"),
    )?;
    // App resources are readable files, not executables.
    fs::set_permissions(root.join("hey-t3.mjs"), fs::Permissions::from_mode(0o644))?;
    fs::write(
        root.join("thread.json"),
        json!({"messages":[{"role":"assistant","text":"Current result"}]}).to_string(),
    )?;
    fs::write(root.join("sent.jsonl"), "")?;
    fs::write(root.join("delivery.json"), "{}")?;
    let (_, mut acks) = mpsc::unbounded_channel();
    let mut plugins = Plugins {
        delegates: None,
        cwd: root,
        focus: None,
        home: root,
        login: Path::new("/bin/sh"),
        resources: root,
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    let link = "t3code://threads/00000000-0000-0000-0000-000000000001/simulated";
    let read = hey_boss_conversation::t3::call(&plugins, link, "t3_read", &json!({})).await?;
    let thread: Value = serde_json::from_str(read.as_str().unwrap())?;
    assert_eq!(thread["messages"][0]["text"], "Current result");
    let state = root.join("session.json");
    // Even a misbehaving model cannot send during an incoming update.
    let (url, server) = mock(vec![
        tool("t3_send", json!({"text":"unrequested work"})),
        msg(""),
    ])
    .await?;
    agent::run(
        &state,
        &url,
        &json!({"turn":"progress","t3_thread":link,"automatic":true}),
        &mut plugins,
    )
    .await?;
    let requests = server.await??;
    assert!(requests[0]["tools"].as_array().unwrap().is_empty());
    assert!(
        requests[1]["input"]
            .to_string()
            .contains("Tools unavailable during automatic updates")
    );
    assert_eq!(fs::read_to_string(root.join("sent.jsonl"))?, "");
    let (url, server) = mock(vec![
        tool(
            "t3_send",
            json!({"text":"Keep the smaller fix.\nNo deployment.","queue":true}),
        ),
        msg("Queued."),
    ])
    .await?;
    agent::run(
        &state,
        &url,
        &json!({"turn":"Queue my direction","t3_thread":link}),
        &mut plugins,
    )
    .await?;
    let requests = server.await??;
    assert!(
        requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "t3_send")
    );
    let sent: Value = serde_json::from_str(&fs::read_to_string(root.join("sent.jsonl"))?)?;
    assert_eq!(
        sent,
        json!({"op":"queue","text":"Keep the smaller fix.\nNo deployment."})
    );
    assert!(requests[1]["input"].to_string().contains("queued"));
    let answered = hey_boss_conversation::t3::call(
        &plugins,
        link,
        "t3_answer",
        &json!({"request_id":"browser-request","answers":{"browser":"Safari"}}),
    )
    .await?;
    assert!(answered.as_str().unwrap().contains("answered"));
    Ok(())
}
#[test]
fn agent_requires_a_configured_root_before_opening_chat_history() -> Result<()> {
    let tmp = scratch();
    let home = tmp.path();
    let state = home.join("session.json");
    let journal = state.with_extension("jsonl");
    for resume in [false, true] {
        if resume {
            fs::create_dir(home.join(".hey-boss"))?;
            fs::write(home.join(".hey-boss/config.yaml"), "conversation: {}\n")?;
            fs::write(&journal, "existing history\n")?;
        }
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_conversation-agent"))
            .arg(&state)
            .env("HOME", home)
            .output()?;
        assert!(!output.status.success());
        let event: Value = serde_json::from_slice(&output.stdout)?;
        assert!(
            event["error"]
                .as_str()
                .unwrap()
                .contains("set conversation.root in ~/.hey-boss/config.yaml")
        );
        if resume {
            assert_eq!(fs::read_to_string(&journal)?, "existing history\n");
        } else {
            assert!(!journal.exists());
        }
    }
    Ok(())
}
#[tokio::test]
async fn native_loop_resumes_tools_and_compacts_durably() -> Result<()> {
    let tmp = scratch();
    let home = tmp.path();
    fs::create_dir(home.join(".hey-boss"))?;
    fs::write(
        home.join(".hey-boss/config.yaml"),
        "conversation:\n  root: ~/work\n  shell: /bin/zsh\n",
    )?;
    let (cwd, login) = context::settings(home, None)?;
    let skills = cwd.join(".agents/skills/notes");
    fs::create_dir_all(&skills)?;
    fs::write(
        skills.join("SKILL.md"),
        "---\nname: notes\ndescription: Organize notes\n---\nPrivate skill body.",
    )?;
    fs::write(cwd.join("AGENTS.md"), "@./rules.md")?;
    fs::write(cwd.join("rules.md"), "Preserve existing notes.")?;
    let (url,server)=mock(vec![tool("batch",json!({"commands":[{"command":"echo one > one.txt; cat one.txt","description":"Creating a note."},{"command":"exit 7","description":"Checking failure."}]})),msg("Done."),msg("One note.")]).await?;
    let state = home.join("history/session.json");
    let (_, mut acks) = mpsc::unbounded_channel();
    let mut p = Plugins {
        delegates: None,
        focus: None,
        home,
        cwd: &cwd,
        login: &login,
        resources: &resources(),
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    agent::run(&state, &url, &json!({"turn":"Create notes."}), &mut p).await?;
    assert_eq!(
        fs::metadata(state.with_extension("jsonl"))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    agent::run(&state, &url, &json!({"turn":"What happened?"}), &mut p).await?;
    let req = server.await??;
    assert_eq!(req[0]["reasoning"]["effort"], "low");
    assert_eq!(req[0]["store"], false);
    let prompt = req[0]["instructions"].as_str().unwrap();
    assert!(
        prompt.contains("Preserve existing notes.")
            && prompt.contains("Organize notes")
            && !prompt.contains("Private skill body")
    );
    assert!(
        req[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "delegate")
    );
    assert_eq!(fs::read_to_string(cwd.join("one.txt"))?, "one\n");
    assert!(
        req[2]["input"].to_string().contains("function_call_output")
            && req[2]["input"].to_string().contains("exit status: 7")
    );
    let interrupted = hey_boss_conversation::notebook::Notebook::open(
        &home.join("missing"),
        &json!([{"role":"assistant","tool_calls":[{"id":"pending","function":{"name":"shell"}}]}]),
    )?;
    assert_eq!(interrupted.messages[1]["toolCallId"], "pending");
    let long = vec![json!({"role":"user","content":"x".repeat(60000)}); 4];
    for summary in ["", "Earlier decisions."] {
        let (url, server) = mock(vec![msg("Done."), msg(summary)]).await?;
        let state = home.join(format!("compact-{}.json", summary.len()));
        let r = agent::run(
            &state,
            &url,
            &json!({"turn":"Continue.","history":long}),
            &mut p,
        )
        .await;
        assert_eq!(r.is_err(), summary.is_empty());
        server.await??;
        let saved = hey_boss_conversation::notebook::Notebook::open(&state, &Value::Null)?;
        assert_eq!(
            saved
                .messages
                .iter()
                .filter(|m| m["role"] == "user")
                .count(),
            if summary.is_empty() { 5 } else { 3 }
        );
    }
    let invalid = json!({"commands":[{"command":"touch must-not-run","description":"Creating."},{"command":"true"}]});
    assert!(p.call("batch", &invalid).await.is_err());
    assert!(!cwd.join("must-not-run").exists());
    Ok(())
}
#[tokio::test]
async fn media_formats_and_desktop_window_socket() -> Result<()> {
    let tmp = scratch();
    let cwd = tmp.path();
    let http = reqwest::Client::new();
    let resources = resources();
    fs::write(cwd.join("map.dot"), "digraph { Preview -> Publish }")?;
    fs::write(cwd.join("map.mmd"), "flowchart LR\n A-->B")?;
    for (source, kind, payload) in [
        (json!({"file":"map.dot"}), "Graphviz", "Publish"),
        (json!({"file":"map.mmd"}), "Mermaid", "<svg"),
        (
            json!({"mermaid":"sequenceDiagram\n Alice->>Bob: Hello"}),
            "Mermaid",
            "Hello",
        ),
        (json!({"svg":"<svg><circle/></svg>"}), "SVG", "<circle"),
        (
            json!({"image":{"type":"input_image","image_url":"data:image/png;base64,YWJj"}}),
            "Image",
            "abc",
        ),
        (
            json!({"image":{"type":"image","mimeType":"image/png","data":"YWJj"}}),
            "Image",
            "abc",
        ),
    ] {
        let m = media::prepare(&source, cwd, cwd, &resources, &http).await?;
        assert_eq!(m["kind"], kind);
        assert!(
            String::from_utf8(STANDARD.decode(m["data"].as_str().unwrap())?)?.contains(payload)
        );
    }
    assert!(
        media::prepare(&json!({"file":"missing.dot"}), cwd, cwd, &resources, &http)
            .await
            .is_err()
    );
    // A short relative Unix socket address also works when the checkout path is long.
    let endpoint = cwd.join("Library/Application Support/hey-talk");
    fs::create_dir_all(&endpoint)?;
    let short = tempfile::Builder::new().prefix("s").tempdir_in(".")?;
    let sock = Path::new(short.path().file_name().unwrap()).join("c");
    let listener = UnixListener::bind(&sock)?;
    std::os::unix::fs::symlink(sock.canonicalize()?, endpoint.join("control.sock"))?;
    let socket_home = Path::new(short.path().file_name().unwrap()).join("h");
    std::os::unix::fs::symlink(cwd.canonicalize()?, &socket_home)?;
    let paste = json!({"type":"function","name":"paste","description":"from the dictation service","strict":true,"parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}});
    let declared = paste.clone();
    let task = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await?;
        let mut line = String::new();
        BufReader::new(&mut s).read_line(&mut line).await?;
        assert_eq!(serde_json::from_str::<Value>(&line)?["target"], "Gmail");
        s.write_all(br#"{"ok":true,"source":"Gmail","text":"AXButton: Compose","jpeg":"YWJj"}"#)
            .await?;

        drop(s);
        let (mut s, _) = listener.accept().await?;
        let mut line = String::new();
        BufReader::new(&mut s).read_line(&mut line).await?;
        assert_eq!(
            serde_json::from_str::<Value>(&line)?,
            json!({"command":"spoken_context"})
        );
        s.write_all(json!({"ok":true,"id":"captured-field","observed":{"app":"Fixture","window":"Original window","field":"AXTextArea"},"paste_tool":declared}).to_string().as_bytes()).await?;
        drop(s);
        for reply in [
            r#"{"ok":true}"#,
            r#"{"ok":false,"error":"Allow Hey Talk in Accessibility"}"#,
        ] {
            let (mut s, _) = listener.accept().await?;
            let mut line = String::new();
            BufReader::new(&mut s).read_line(&mut line).await?;
            assert_eq!(
                serde_json::from_str::<Value>(&line)?,
                json!({"command":"paste_text","focus":"captured-field","text":"  Hi 🌍\n"})
            );
            s.write_all(reply.as_bytes()).await?;
        }
        Ok::<_, anyhow::Error>(())
    });
    let (_, mut acks) = mpsc::unbounded_channel();
    let mut p = Plugins {
        delegates: None,
        focus: None,
        home: &socket_home,
        cwd,
        login: Path::new("/bin/zsh"),
        resources: &resources,
        http: &http,
        acks: &mut acks,
    };
    let output = p
        .call("inspect_window", &json!({"target":"Gmail","mode":"both"}))
        .await?;
    let args = json!({"text":"  Hi 🌍\n"});
    let (url, server) = mock(vec![tool("paste", args.clone()), msg("Pasted.")]).await?;
    agent::run(
        &cwd.join("paste-session.json"),
        &url,
        &json!({"turn":"Paste this."}),
        &mut p,
    )
    .await?;
    let requests = server.await??;
    assert!(requests[0]["tools"].as_array().unwrap().contains(&paste));
    assert!(
        !requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "paste_text")
    );
    assert!(requests[0]["input"].to_string().contains("Original window"));
    assert!(
        requests[1]["input"]
            .to_string()
            .contains("Pasted without submitting.")
    );
    assert!(
        p.call("paste", &args)
            .await
            .unwrap_err()
            .to_string()
            .contains("Allow Hey Talk in Accessibility")
    );
    task.await??;
    let wire = provider::input(&[json!({"role":"tool","toolCallId":"cap","content":output})])?;
    assert_eq!(
        wire[0]["output"][1]["image_url"],
        "data:image/jpeg;base64,YWJj"
    );
    assert!(
        wire[0]["output"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Compose")
    );
    Ok(())
}
#[tokio::test]
async fn cancelling_shell_kills_descendants() -> Result<()> {
    let tmp = scratch();
    let cwd = tmp.path();
    let (_, mut acks) = mpsc::unbounded_channel();
    let mut p = Plugins {
        delegates: None,
        focus: None,
        home: cwd,
        cwd,
        login: Path::new("/bin/zsh"),
        resources: &resources(),
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    let args =
        json!({"command":"echo started > waiting; sleep 1; touch orphan","description":"Waiting."});
    tokio::select! {r=p.call("shell",&args)=>panic!("finished early: {r:?}"),_=async{while !cwd.join("waiting").exists(){tokio::time::sleep(Duration::from_millis(10)).await;}}=>{}}
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(!cwd.join("orphan").exists());
    Ok(())
}
#[tokio::test]
async fn login_shell_loads_interactive_startup_and_preserves_quoting() -> Result<()> {
    let tmp = scratch();
    let cwd = tmp.path().canonicalize()?;
    fs::write(
        cwd.join(".zprofile"),
        "export CONVERSATION_LOGIN_READY=yes\n",
    )?;
    fs::write(
        cwd.join(".zshrc"),
        "wire_fixture() { printf 'terminal-%s' \"$CONVERSATION_LOGIN_READY\"; }\n",
    )?;
    let shell = cwd.join("login");
    fs::write(
        &shell,
        "#!/bin/sh\nexport ZDOTDIR=\"$PWD\"\nexec /bin/zsh \"$@\"\n",
    )?;
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700))?;
    let (_, mut acks) = mpsc::unbounded_channel();
    let mut p = Plugins {
        delegates: None,
        focus: None,
        home: &cwd,
        cwd: &cwd,
        login: &shell,
        resources: &resources(),
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    let output=p.call("shell",&json!({"command":"wire_fixture; printf '%s' ' quoted $HOME; $(false)'","description":"Checking terminal commands."})).await?;
    assert_eq!(output, "terminal-yes quoted $HOME; $(false)");
    Ok(())
}

#[test]
fn notebook_extends_the_same_file_and_recovers_an_interrupted_write() -> Result<()> {
    use hey_boss_conversation::notebook::Notebook;
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;
    let dir = scratch();
    let state = dir.path().join("chat.agent.json");
    let journal = state.with_extension("jsonl");
    let old = json!({"role":"user","content":"old\nmessage"});
    fs::write(&state, json!({"messages":[old]}).to_string())?;
    let mut book = Notebook::open(&state, &Value::Null)?;
    let prefix = fs::read(&journal)?;
    let inode = journal.metadata()?.ino();
    book.push(json!({"role":"assistant","tool_calls":[{"id":"x","function":{"name":"shell"}}]}))?;
    assert!(fs::read(&journal)?.starts_with(&prefix));
    assert_eq!(journal.metadata()?.ino(), inode);
    assert!(Notebook::open(&state, &Value::Null).is_err());
    drop(book);
    fs::OpenOptions::new()
        .append(true)
        .open(&journal)?
        .write_all(b"{\"role\":\"to")?;
    let mut book = Notebook::open(&state, &Value::Null)?;
    assert_eq!(book.messages[0], old);
    assert_eq!(book.messages[2]["toolCallId"], "x");
    book.push(json!({"role":"user","content":"next"}))?;
    let summary = json!({"role":"system","content":"Earlier conversation"});
    book.compact(3, summary.clone())?;
    drop(book);
    let book = Notebook::open(&state, &Value::Null)?;
    assert_eq!(
        book.messages,
        vec![summary, json!({"role":"user","content":"next"})]
    );
    assert!(fs::read(&journal)?.starts_with(&prefix));
    drop(book);
    fs::OpenOptions::new()
        .append(true)
        .open(&journal)?
        .write_all(b"broken\n")?;
    assert!(Notebook::open(&state, &Value::Null).is_err());
    Ok(())
}

#[tokio::test]
async fn explicit_t3_delegation_journals_the_exact_exchange() -> Result<()> {
    let tmp = scratch();
    let dir = tmp.path();
    fs::write(
        dir.join("hey-t3.mjs"),
        r#"
import {createInterface} from 'node:readline';
const input = createInterface({input:process.stdin});
input.once('line', line => {
  console.log(JSON.stringify({Session:{link:"t3code://test"}}));
  console.log(JSON.stringify({TurnCompleted:{status:"Completed",output:"Answer: "+JSON.parse(line).turn}}));
  input.close();
});
"#,
    )?;
    let request = "Compare 'a' and $b.\nKeep both lines.";
    let description = "Compare the two values";
    let (url, server) = mock(vec![
        tool(
            "delegate",
            json!({"provider":"t3","request":request,"description":description}),
        ),
        msg("Handed off."),
        msg("Finished."),
    ])
    .await?;
    let (_ack, mut acks) = mpsc::unbounded_channel();
    let path = dir.join("chat.agent.json");
    let (delegates, mut ready) = hey_boss_conversation::delegate::Delegates::open(&path)?;
    let mut p = Plugins {
        delegates: Some(&delegates),
        focus: None,
        home: dir,
        cwd: dir,
        login: Path::new("/bin/sh"),
        resources: dir,
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    // Leave the acknowledgement channel open and silent: playback must not gate delegation.
    tokio::time::timeout(
        Duration::from_secs(5),
        agent::run(&path, &url, &json!({"turn":"Check."}), &mut p),
    )
    .await??;
    assert!(delegates.pending());
    let result = tokio::time::timeout(Duration::from_secs(5), ready.recv())
        .await?
        .unwrap();
    assert_eq!(result["aggregatedOutput"], format!("Answer: {request}"));
    agent::run(
        &path,
        &url,
        &hey_boss_conversation::delegate::follow_up(result),
        &mut p,
    )
    .await?;
    assert!(!delegates.pending());
    let requests = server.await??;
    let schema = requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "delegate")
        .unwrap();
    assert_eq!(
        schema["parameters"]["properties"]["description"]["type"],
        "string"
    );
    assert_eq!(schema["parameters"]["required"], json!(["request"]));
    let result = requests[1]["input"].as_array().unwrap().last().unwrap();
    assert_eq!(result["call_id"], "call_1");
    let receipt: Value = serde_json::from_str(result["output"].as_str().unwrap())?;
    assert_eq!(receipt["status"], "running");
    assert!(requests[2]["input"].to_string().contains("Answer: "));
    let book = hey_boss_conversation::notebook::Notebook::open(&path, &Value::Null)?;
    let args: Value = serde_json::from_str(
        book.messages[1]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap(),
    )?;
    assert_eq!(args["description"], description);
    assert_eq!(args["request"], request);
    assert_eq!(book.messages[2]["content"], result["output"]);
    assert_eq!(
        book.messages
            .iter()
            .filter(|m| m["name"] == "delegate_result")
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn background_turn_uses_checkout_and_no_desktop_context() -> Result<()> {
    let tmp = scratch();
    let home = tmp.path();
    let cwd = home.join("project");
    fs::create_dir(&cwd)?;
    let (url, server) = mock(vec![
        tool(
            "shell",
            json!({"command":"pwd > workspace","description":"Record working directory"}),
        ),
        msg("Done."),
    ])
    .await?;
    let (_, mut acks) = mpsc::unbounded_channel();
    let mut p = Plugins {
        delegates: None,
        focus: None,
        home,
        cwd: &cwd,
        login: Path::new("/bin/sh"),
        resources: &resources(),
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    let text = include_str!("../../../src/jobs/fixtures/original.md");
    agent::run(
        &home.join("job.json"),
        &url,
        &json!({"turn":text,"job":true}),
        &mut p,
    )
    .await?;
    let requests = server.await??;
    assert_eq!(
        fs::canonicalize(fs::read_to_string(cwd.join("workspace"))?.trim())?,
        cwd.canonicalize()?
    );
    let req = &requests[0];
    assert!(
        !req["input"]
            .to_string()
            .contains("Observed pre-focus desktop context")
    );
    assert_eq!(
        req["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["delegate", "shell", "batch"]
    );
    Ok(())
}

#[tokio::test]
async fn queued_follow_up_is_saved_before_inference() -> Result<()> {
    let tmp = scratch();
    let root = tmp.path();
    fs::write(root.join("AGENTS.md"), "")?;
    let (url, server) = mock(vec![msg("Redirected.")]).await?;
    let (_, mut acks) = mpsc::unbounded_channel();
    let mut plugins = Plugins {
        delegates: None,
        focus: None,
        home: root,
        cwd: root,
        login: Path::new("/bin/sh"),
        resources: root,
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    let (input, mut pending) = mpsc::unbounded_channel();
    input.send(json!({"turn":"Correction one"}))?;
    input.send(json!({"turn":"Correction two"}))?;
    let state = root.join("agent.json");
    agent::run_with_input(
        &state,
        &url,
        &json!({"turn":"First", "job":true}),
        &mut plugins,
        &mut pending,
    )
    .await?;
    let requests = server.await??;
    assert!(!root.join("should-not-run").exists());
    let next = requests[0]["input"].to_string();
    assert!(next.contains("Correction one") && next.contains("Correction two"));
    let book = hey_boss_conversation::notebook::Notebook::open(&state, &Value::Null)?;
    let users: Vec<_> = book
        .messages
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    assert_eq!(users, ["First", "Correction one", "Correction two"]);
    Ok(())
}

#[tokio::test]
async fn poe_delegates_spawn_fork_recover_and_deduplicate() -> Result<()> {
    use hey_boss_conversation::delegate::{Delegates, follow_up};
    let dir = scratch();
    let root = dir.path();
    fs::write(root.join("AGENTS.md"), "")?;
    let (child_url, child_server) = mock(vec![msg("child answer"), msg("fresh answer")]).await?;
    let state = root.join("parent.json");
    let (tasks, mut ready) = Delegates::open(&state)?;
    let (_, mut acks) = mpsc::unbounded_channel();
    let tools = Plugins {
        delegates: Some(&tasks),
        focus: None,
        home: root,
        cwd: root,
        login: Path::new("/bin/sh"),
        resources: root,
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    let history = vec![
        json!({"role":"user","content":"parent context"}),
        json!({"role":"assistant","tool_calls":[{"id":"pending","function":{"name":"delegate","arguments":"{}"}}]}),
    ];
    let args = json!({"provider":"poe","request":"child task","description":"Child","fork":true});
    let receipt = tasks.start(&tools, &child_url, &args, &history)?;
    assert_eq!(receipt["status"], "running");
    let result = tokio::time::timeout(Duration::from_secs(5), ready.recv())
        .await?
        .unwrap();
    let child_state = state
        .with_extension("delegates")
        .join(format!("{}.agent.json", receipt["id"].as_str().unwrap()));
    let captured = hey_boss_conversation::notebook::Notebook::open(&child_state, &Value::Null)?;
    assert_eq!(captured.messages[0], history[0]);
    assert_eq!(captured.messages[2]["toolCallId"], "pending");
    drop(captured);
    assert_eq!(history.len(), 2);
    assert_eq!(result["aggregatedOutput"], "child answer");
    // A completed result is recovered until the notebook has durably accepted it.
    drop(tools);
    drop(tasks);
    let (tasks, mut ready) = Delegates::open(&state)?;
    let mut tools = Plugins {
        delegates: Some(&tasks),
        focus: None,
        home: root,
        cwd: root,
        login: Path::new("/bin/sh"),
        resources: root,
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    assert_eq!(ready.try_recv()?, result);
    let (url, server) = mock(vec![msg("Handled")]).await?;
    let request = follow_up(result);
    agent::run(&state, &url, &request, &mut tools).await?;
    agent::run(&state, &url, &request, &mut tools).await?;
    assert_eq!(
        server.await??.len(),
        1,
        "A repeated receipt must not start another inference"
    );
    assert!(!tasks.pending());
    let mut book = hey_boss_conversation::notebook::Notebook::open(&state, &Value::Null)?;
    assert_eq!(
        book.messages
            .iter()
            .filter(|m| m["delegation"] == receipt["id"])
            .count(),
        1
    );
    book.compact(
        book.messages.len(),
        json!({"role":"user","content":"Summary"}),
    )?;
    drop(book);
    agent::run(&state, &url, &request, &mut tools).await?;
    let fresh = tasks.start(&tools, &child_url, &json!({"request":"fresh"}), &history)?;
    let result = tokio::time::timeout(Duration::from_secs(5), ready.recv())
        .await?
        .unwrap();
    assert_eq!(result["id"], fresh["id"]);
    assert_eq!(result["provider"], "poe");
    assert_eq!(result["description"], "");
    let requests = child_server.await??;
    assert!(requests[0]["input"].to_string().contains("parent context"));
    assert!(!requests[1]["input"].to_string().contains("parent context"));
    tasks.cancel()?;
    assert!(!tasks.pending());
    drop(tools);
    drop(tasks);
    let (recovered, mut ready) = Delegates::open(&state)?;
    assert!(!recovered.pending());
    assert!(ready.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn ending_the_owner_closes_delegate_input_without_restarting_the_task() -> Result<()> {
    use hey_boss_conversation::delegate::Delegates;
    let dir = scratch();
    let root = dir.path();
    fs::write(
        root.join("hey-t3.mjs"),
        r#"
import fs from 'node:fs';
import {createInterface} from 'node:readline';
const input = createInterface({input:process.stdin});
input.once('line', () => fs.writeFileSync('started', ''));
input.on('close', () => fs.writeFileSync('cancelled', ''));
"#,
    )?;
    let state = root.join("parent.json");
    let (tasks, _ready) = Delegates::open(&state)?;
    let (_, mut acks) = mpsc::unbounded_channel();
    let tools = Plugins {
        delegates: Some(&tasks),
        focus: None,
        home: root,
        cwd: root,
        login: Path::new("/bin/sh"),
        resources: root,
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    tasks.start(
        &tools,
        "unused",
        &json!({"provider":"t3","request":"wait","description":"Wait"}),
        &[],
    )?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !root.join("started").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    drop(tools);
    drop(tasks);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !root.join("cancelled").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let (_tasks, mut ready) = Delegates::open(&state)?;
    let result = ready.try_recv()?;
    assert_eq!(result["failed"], true);
    assert!(
        result["aggregatedOutput"]
            .as_str()
            .unwrap()
            .contains("may have run")
    );
    Ok(())
}

#[tokio::test]
async fn newer_input_cancels_stale_inference_before_it_can_speak() -> Result<()> {
    let tmp = scratch();
    let root = tmp.path();
    fs::write(root.join("AGENTS.md"), "")?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/v1/responses", listener.local_addr()?);
    let (input, mut pending) = mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await?;
        let mut stale = BufReader::new(socket);
        read_request(&mut stale).await?;
        input.send(json!({"turn":"Design doc, not design log."}))?;
        let (socket, _) = listener.accept().await?;
        let mut current = BufReader::new(socket);
        let request = read_request(&mut current).await?;
        assert!(
            request["input"]
                .to_string()
                .contains("Design doc, not design log.")
        );
        // Even a late response from the old request must not enter the transcript.
        let _ = respond(
            &mut stale,
            msg("Back to the design log: done or carry over?"),
        )
        .await;
        respond(&mut current, msg("Design doc. Carried over.")).await?;
        Ok::<_, anyhow::Error>(())
    });
    let (_ack, mut acks) = mpsc::unbounded_channel();
    let mut tools = Plugins {
        delegates: None,
        focus: None,
        home: root,
        cwd: root,
        login: Path::new("/bin/sh"),
        resources: root,
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    let (events, mut output) = mpsc::unbounded_channel();
    let state = root.join("parent.json");
    tokio::time::timeout(
        Duration::from_secs(5),
        hey_boss_conversation::EVENTS.scope(
            events,
            agent::run_with_input(
                &state,
                &url,
                &json!({"turn":"Carry over the filesystem work.","job":true}),
                &mut tools,
                &mut pending,
            ),
        ),
    )
    .await??;
    server.await??;
    let events: Vec<_> = std::iter::from_fn(|| output.try_recv().ok()).collect();
    assert!(!json!(events).to_string().contains("Back to the design log"));
    assert_eq!(
        events.last().unwrap()["TurnCompleted"]["output"],
        "Design doc. Carried over."
    );
    let book = hey_boss_conversation::notebook::Notebook::open(&state, &Value::Null)?;
    assert_eq!(book.messages.len(), 3);
    Ok(())
}

#[tokio::test]
async fn in_process_delegates_are_nonblocking_quiet_and_cancel_with_the_owner() -> Result<()> {
    use hey_boss_conversation::delegate::Delegates;
    let tmp = scratch();
    let root = tmp.path();
    fs::write(root.join("AGENTS.md"), "")?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/v1/responses", listener.local_addr()?);
    let state = root.join("parent.json");
    let (tasks, mut ready) = Delegates::open(&state)?;
    let (_ack, mut acks) = mpsc::unbounded_channel();
    let tools = Plugins {
        delegates: Some(&tasks),
        focus: None,
        home: root,
        cwd: root,
        login: Path::new("/bin/sh"),
        resources: root,
        http: &reqwest::Client::new(),
        acks: &mut acks,
    };
    let (events, mut output) = mpsc::unbounded_channel();
    let receipts = hey_boss_conversation::EVENTS
        .scope(events, async {
            Ok::<_, anyhow::Error>([
                tasks.start(&tools, &url, &json!({"request":"slow research"}), &[])?,
                tasks.start(
                    &tools,
                    &url,
                    &json!({"request":"fast research","description":""}),
                    &[],
                )?,
            ])
        })
        .await?;
    // Both launches return before either provider request has received a response.
    let (slow, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await??;
    let mut slow = BufReader::new(slow);
    read_request(&mut slow).await?;
    let (fast, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await??;
    let mut fast = BufReader::new(fast);
    read_request(&mut fast).await?;
    respond(&mut fast, msg("Fast research result")).await?;
    let result = tokio::time::timeout(Duration::from_secs(5), ready.recv())
        .await?
        .unwrap();
    assert_eq!(result["id"], receipts[1]["id"]);
    assert_eq!(result["aggregatedOutput"], "Fast research result");
    let events: Vec<_> = std::iter::from_fn(|| output.try_recv().ok()).collect();
    assert_eq!(
        events.len(),
        2,
        "Child messages and completion must not leak into the parent's events"
    );
    assert!(
        events
            .iter()
            .all(|e| e["ToolStarted"]["input"]["description"] == "")
    );
    assert!(!root.join("conversation-job").exists());
    drop(tools);
    drop(tasks);
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), slow.read(&mut byte)).await??,
        0
    );
    assert!(ready.try_recv().is_err());
    let (recovered, mut ready) = Delegates::open(&state)?;
    let results = [ready.try_recv()?, ready.try_recv()?];
    assert!(
        results
            .iter()
            .any(|r| r["id"] == receipts[0]["id"] && r["failed"] == true)
    );
    assert!(
        results
            .iter()
            .any(|r| r["id"] == receipts[1]["id"] && r["failed"] == false)
    );
    recovered.cancel()?;
    Ok(())
}

#[tokio::test]
async fn spoken_follow_up_preserves_automatic_delegate_delivery() -> Result<()> {
    let tmp = scratch();
    let root = tmp.path();
    fs::write(root.join("AGENTS.md"), "")?;
    fs::create_dir(root.join(".hey-boss"))?;
    fs::write(
        root.join(".hey-boss/config.json"),
        json!({"conversation":{"root":root,"shell":"/bin/sh"}}).to_string(),
    )?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy = format!("http://{}", listener.local_addr()?);
    let mut process = tokio::process::Command::new(env!("CARGO_BIN_EXE_conversation-agent"))
        .arg(root.join("parent.json"))
        .env("HOME", root)
        .env("HTTP_PROXY", &proxy)
        .env("http_proxy", &proxy)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut input = process.stdin.take().unwrap();
    let mut events = BufReader::new(process.stdout.take().unwrap()).lines();
    input
        .write_all(b"{\"turn\":\"Research the task.\"}\n")
        .await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        let (socket, _) = listener.accept().await?;
        let mut first = BufReader::new(socket);
        read_request(&mut first).await?;
        respond(
            &mut first,
            tool("delegate", json!({"request":"child research"})),
        )
        .await?;
        let mut child = None;
        let mut parent = None;
        for _ in 0..2 {
            let (socket, _) = listener.accept().await?;
            let mut socket = BufReader::new(socket);
            let request = read_request(&mut socket).await?;
            if request["input"][0]["content"] == "child research" {
                child = Some(socket);
            } else {
                parent = Some(socket);
            }
        }
        let mut child = child.unwrap();
        let _parent = parent.unwrap();
        input
            .write_all(b"{\"turn\":\"Carry over the design doc.\",\"mode\":\"follow-up\"}\n")
            .await?;
        let (socket, _) = listener.accept().await?;
        let mut follow_up = BufReader::new(socket);
        let request = read_request(&mut follow_up).await?;
        assert!(
            request["input"]
                .to_string()
                .contains("Carry over the design doc.")
        );
        respond(&mut follow_up, msg("Design doc carried over.")).await?;
        loop {
            let event: Value = serde_json::from_str(&events.next_line().await?.unwrap())?;
            assert!(event.get("error").is_none(), "{event}");
            if event["TurnCompleted"]["status"] == "Completed" {
                break;
            }
        }
        respond(&mut child, msg("Research arrived after the interruption")).await?;
        let result = loop {
            let event: Value = serde_json::from_str(&events.next_line().await?.unwrap())?;
            assert!(event.get("error").is_none(), "{event}");
            assert!(
                event.get("Message").is_none(),
                "Child speech leaked to parent"
            );
            if let Some(item) = event.get("DelegateReady") {
                break item.clone();
            }
        };
        assert_eq!(result["failed"], false);
        assert_eq!(
            result["aggregatedOutput"],
            "Research arrived after the interruption"
        );
        let follow = hey_boss_conversation::delegate::follow_up(result);
        input.write_all(format!("{follow}\n").as_bytes()).await?;
        let (socket, _) = listener.accept().await?;
        let mut resumed = BufReader::new(socket);
        let request = read_request(&mut resumed).await?;
        assert!(
            request["input"]
                .to_string()
                .contains("Research arrived after the interruption")
        );
        respond(&mut resumed, msg("Here is the research result.")).await?;
        loop {
            let event: Value = serde_json::from_str(&events.next_line().await?.unwrap())?;
            if event["TurnCompleted"]["status"] == "Completed" {
                break;
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    drop(input);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), process.wait())
            .await??
            .success()
    );
    Ok(())
}
