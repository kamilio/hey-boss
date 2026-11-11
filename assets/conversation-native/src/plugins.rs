use crate::{emit, field, media};
use anyhow::{Context, Result, bail, ensure};
use futures_util::future::join_all;
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    process::Command,
    sync::mpsc,
    time::timeout,
};
use uuid::Uuid;

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
pub fn schemas() -> Vec<Value> {
    let s = json!({"type":"string"});
    let cmd = object(
        json!({"command":s,"description":s}),
        &["command", "description"],
    );
    let image = object(
        json!({"type":s,"mimeType":s,"data":s,"image_url":{"anyOf":[s,object(json!({"url":s}),&["url"])]}}),
        &[],
    );
    [
        ("shell",include_str!("../../conversation-tools/shell.md"),cmd.clone()),
        ("batch",include_str!("../../conversation-tools/batch.md"),object(json!({"commands":{"type":"array","items":cmd,"minItems":1,"maxItems":8}}),&["commands"])),
        ("inspect_window",include_str!("../../conversation-tools/inspect_window.md"),object(json!({"target":s,"mode":{"type":"string","enum":["accessibility","screenshot","both"]}}),&["target","mode"])),
        ("show_media",include_str!("../../conversation-tools/show_media.md"),object(json!({"title":s,"file":s,"url":s,"image":image,"svg":s,"graphviz":s,"mermaid":s}),&[])),
        ("hide_media",include_str!("../../conversation-tools/hide_media.md"),object(json!({}),&[])),
    ].into_iter().map(|(name,description,parameters)|json!({"type":"function","name":name,"description":description.trim(),"parameters":parameters,"strict":false})).collect()
}
async fn output(mut stream: impl AsyncRead + Unpin) -> Result<String> {
    let mut tail = poe_agent_rust::shell_tools::RetainedOutput::default();
    let mut buf = [0; 8192];
    let mut pending = Vec::new();
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        let keep = match std::str::from_utf8(&pending) {
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            _ => pending.len(),
        };
        tail.append(
            &String::from_utf8_lossy(&pending[..keep])
                .encode_utf16()
                .collect::<Vec<_>>(),
        );
        pending.drain(..keep);
    }
    tail.append(
        &String::from_utf8_lossy(&pending)
            .encode_utf16()
            .collect::<Vec<_>>(),
    );
    Ok(String::from_utf16_lossy(&tail.format()))
}
struct Group(i32);
impl Drop for Group {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}
async fn shell(cwd: &Path, login: &Path, args: &Value) -> Result<String> {
    let cmd = field(args, "command")?;
    field(args, "description")?;
    let id = Uuid::new_v4().to_string();
    emit(json!({"ToolStarted":{"id":id,"input":args}}))?;
    let result: Result<String> = async {
        let mut child = Command::new(login)
            .args(["-ilc", &format!("set +m; trap 'exit 143' TERM;\n{cmd}")])
            .current_dir(cwd)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let group = Group(child.id().context("Missing shell PID")? as i32);
        let out = child.stdout.take().unwrap();
        let err = child.stderr.take().unwrap();
        let (s, o, e) = timeout(Duration::from_secs(120), async {
            tokio::try_join!(async { Ok(child.wait().await?) }, output(out), output(err))
        })
        .await
        .context("Command timed out after 120 seconds")??;
        std::mem::forget(group);
        let combined = [o.trim_end(), e.trim_end()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        ensure!(s.success(), "{s}: {combined}");
        Ok(if combined.is_empty() {
            "Command completed with no output".into()
        } else {
            combined
        })
    }
    .await;
    let mut out = args.clone();
    out["aggregatedOutput"] = json!(match &result {
        Ok(s) => s.clone(),
        Err(e) => e.to_string(),
    });
    emit(json!({"ToolCompleted":{"id":id,"output":out,"failed":result.is_err()}}))?;
    result
}
pub struct Plugins<'a> {
    pub cwd: &'a Path,
    pub home: &'a Path,
    pub login: &'a Path,
    pub resources: &'a Path,
    pub http: &'a reqwest::Client,
    pub acks: &'a mut mpsc::UnboundedReceiver<Value>,
}
impl Plugins<'_> {
    pub async fn call(&mut self, name: &str, args: &Value) -> Result<Value> {
        match name {
            "shell" => Ok(json!(shell(self.cwd, self.login, args).await?)),
            "batch" => {
                let list = args["commands"].as_array().context("Missing commands")?;
                ensure!((1..=8).contains(&list.len()), "batch requires 1–8 commands");
                for a in list {
                    field(a, "command")?;
                    field(a, "description")?;
                }
                let outputs = join_all(list.iter().map(|a| async {
                    let mut out = a.clone();
                    match shell(self.cwd, self.login, a).await {
                        Ok(s) => out["output"] = json!(s),
                        Err(e) => out["error"] = json!(e.to_string()),
                    };
                    out
                }))
                .await;
                Ok(json!(serde_json::to_string(&outputs)?))
            }
            "show_media" | "hide_media" => {
                let mut data = if name == "hide_media" {
                    json!({})
                } else {
                    media::prepare(args, self.cwd, self.home, self.resources, self.http).await?
                };
                let id = Uuid::new_v4().to_string();
                data["id"] = json!(id);
                emit(json!({"ConversationMedia":data}))?;
                timeout(Duration::from_secs(15), async {
                    while let Some(a) = self.acks.recv().await {
                        if a["id"] == id {
                            if let Some(e) = a["error"].as_str() {
                                bail!("{e}");
                            }
                            return Ok(());
                        }
                    }
                    bail!("Conversation closed")
                })
                .await
                .context("Desktop did not acknowledge the image")??;
                Ok(json!("OK"))
            }
            "inspect_window" => {
                let target = field(args, "target")?;
                let mode = field(args, "mode")?;
                ensure!(
                    matches!(mode, "screenshot" | "accessibility" | "both"),
                    "Unknown inspection mode"
                );
                let id = Uuid::new_v4().to_string();
                let mut input = json!({"command":format!("{mode}: {target}"),"description":format!("Looking at {}.",if target=="active"{"the current window"}else{target})});
                emit(json!({"ToolStarted":{"id":id,"input":input}}))?;
                let result: Result<Value> = timeout(Duration::from_secs(105), async {
                    let mut sock = UnixStream::connect(
                        self.home
                            .join("Library/Application Support/hey-talk/control.sock"),
                    )
                    .await
                    .map_err(|e| match e.kind() {
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                            anyhow::anyhow!("Start Hey Talk on this Mac to inspect a window.")
                        }
                        _ => e.into(),
                    })?;
                    let req = json!({"command":"inspect_window","mode":mode,"target":target});
                    sock.write_all(format!("{req}\n").as_bytes()).await?;
                    let data: Value =
                        serde_json::from_slice(&media::limited(sock, 8 * 1024 * 1024).await?)?;
                    ensure!(data["ok"] == true, "{}", data["error"]);
                    Ok(data)
                })
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Hey Talk did not answer.")));
                let result = result.map(|v| {
                    let text = format!(
                        "{}{}",
                        v["source"].as_str().unwrap_or(""),
                        v["text"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(|s| format!("\n{s}"))
                            .unwrap_or_default()
                    );
                    let mut parts = vec![json!({"type":"text","text":text})];
                    let mut display = text;
                    if let Some(data) = v["jpeg"].as_str().filter(|s| !s.is_empty()) {
                        parts.push(json!({"type":"image","mimeType":"image/jpeg","data":data}));
                        display += "\nScreenshot attached.";
                    }
                    (json!(parts), display)
                });
                input["aggregatedOutput"] = json!(match &result {
                    Ok((_, s)) => s.clone(),
                    Err(e) => e.to_string(),
                });
                emit(json!({"ToolCompleted":{"id":id,"output":input,"failed":result.is_err()}}))?;
                Ok(result?.0)
            }
            _ => bail!("Unknown conversation tool: {name}"),
        }
    }
}
