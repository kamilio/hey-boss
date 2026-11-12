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
                requests.push(serde_json::from_slice(&bytes)?);
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
            }
            Ok(requests)
        }),
    ))
}
#[tokio::test]
async fn native_loop_resumes_tools_and_compacts_durably() -> Result<()> {
    let tmp = scratch();
    let home = tmp.path();
    assert_eq!(context::settings(home)?.0, home.join("Workspace/root"));
    fs::create_dir(home.join(".hey-boss"))?;
    fs::write(
        home.join(".hey-boss/config.yaml"),
        "conversation:\n  root: ~/work\n  shell: /bin/zsh\n",
    )?;
    let (cwd, login) = context::settings(home)?;
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
    assert_eq!(req[0]["tools"].as_array().unwrap().len(), 5);
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
    let task = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await?;
        let mut line = String::new();
        BufReader::new(&mut s).read_line(&mut line).await?;
        assert_eq!(serde_json::from_str::<Value>(&line)?["target"], "Gmail");
        s.write_all(br#"{"ok":true,"source":"Gmail","text":"AXButton: Compose","jpeg":"YWJj"}"#)
            .await?;
        Ok::<_, anyhow::Error>(())
    });
    let (_, mut acks) = mpsc::unbounded_channel();
    let mut p = Plugins {
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
