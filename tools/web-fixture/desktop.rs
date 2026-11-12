//! Stand in for the desktop app without reading or writing anyone's real inbox.
use super::*;
use std::{io::Read, os::unix::net::UnixListener};

pub(super) fn seed(f: &Fixture, scenario: Scenario, project: &str) -> Result<Value> {
    let root = f.root.path().to_owned();
    let mut info = json!({"root":root});
    if matches!(scenario, Scenario::NativeArtifactEditor) {
        let doc = run(f.command(&[
            "artifact",
            "--project",
            project,
            "--agent",
            "human:boss",
            "--json",
            "create",
            "--title",
            "Focused writing",
            "--body",
            "# Focus\n\nDraft safely.",
        ]))?;
        info["id"] = doc["artifact"]["id"]
            .as_str()
            .ok_or("missing artifact ID")?
            .into();
    }
    let listener = UnixListener::bind(root.join("inbox.sock"))?;
    let project = format!("named:{project}");
    let id = info["id"].clone();
    thread::spawn(move || {
        let mut launches = 0;
        for mut stream in listener.incoming().map_while(std::result::Result::ok) {
            let result = (|| -> Result<Value> {
                stream.set_read_timeout(Some(Duration::from_secs(3)))?;
                stream.set_write_timeout(Some(Duration::from_secs(3)))?;
                let msg: Value = serde_json::from_reader((&stream).take(64 * 1024))?;
                match msg["command"].as_str() {
                    Some("inbox_list" | "inbox_unread") => Ok(json!({"tasks":[],"unread":0})),
                    Some("artifact_editor") if !id.is_null() => {
                        let ctx: Value = serde_json::from_str(
                            msg["question"].as_str().ok_or("missing editor context")?,
                        )?;
                        if ctx["project"] != project || ctx["id"] != id || !ctx["body"].is_null() {
                            return Err("incorrect native editor context".into());
                        }
                        launches += 1;
                        let log = root.join("launches.new");
                        fs::write(
                            &log,
                            serde_json::to_vec(&json!({"count":launches,"context":ctx}))?,
                        )?;
                        fs::rename(log, root.join("launches.json"))?;
                        Ok(json!({"ok":true}))
                    }
                    _ => Err("unsupported fixture command".into()),
                }
            })();
            let reply = match result {
                Ok(value) => json!({"status":"ok","result":value.to_string()}),
                Err(err) => json!({"status":"error","error":err.to_string()}),
            };
            let _ = serde_json::to_writer(&mut stream, &reply);
        }
    });
    Ok(info)
}
