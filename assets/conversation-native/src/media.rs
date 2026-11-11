use crate::field;
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};
const LIMIT: usize = 12 * 1024 * 1024;
pub(crate) async fn limited(reader: impl AsyncRead + Unpin, max: usize) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    reader.take(max as u64 + 1).read_to_end(&mut data).await?;
    ensure!(data.len() <= max, "Media exceeds the size limit.");
    Ok(data)
}
pub async fn prepare(
    args: &Value,
    cwd: &Path,
    home: &Path,
    resources: &Path,
    http: &reqwest::Client,
) -> Result<Value> {
    let mut sources = args
        .as_object()
        .context("Expected a media source")?
        .iter()
        .filter(|(k, v)| k.as_str() != "title" && !v.is_null());
    let (key, source) = sources.next().context("Missing media source")?;
    ensure!(sources.next().is_none(), "Provide one media source.");
    let mut kind = key.as_str();
    let mut name = None;
    let mut extension = None;
    let mut value = source.as_str().unwrap_or("");
    let mut bytes = Vec::new();
    if kind == "image" {
        if source["type"] == "image" && field(source, "mimeType")?.starts_with("image/") {
            let data = field(source, "data")?;
            ensure!(
                data.len() <= LIMIT.div_ceil(3) * 4,
                "Media exceeds the size limit."
            );
            bytes = STANDARD.decode(data)?;
            if source["mimeType"] == "image/svg+xml" {
                kind = "svg";
            }
        } else {
            value = source["image_url"]
                .as_str()
                .or_else(|| source["image_url"]["url"].as_str())
                .context("Missing image URL")?;
            kind = "url";
        }
    }
    match kind {
        "file" => {
            let path = if let Some(p) = value.strip_prefix("~/") {
                home.join(p)
            } else {
                cwd.join(value)
            };
            let meta = path.metadata()?;
            ensure!(
                meta.is_file() && meta.len() <= LIMIT as u64,
                "Expected an image or diagram file within the size limit."
            );
            bytes = limited(tokio::fs::File::open(&path).await?, LIMIT).await?;
            name = path.file_name().map(|s| s.to_string_lossy().into_owned());
            extension = ext(&path);
        }
        "url" if value.starts_with("data:image/") => {
            ensure!(
                value.len() <= LIMIT * 14 / 10,
                "Media exceeds the size limit."
            );
            let data = data_url::DataUrl::process(value)
                .map_err(|_| anyhow::anyhow!("Invalid image data URL"))?;
            if data.mime_type().matches("image", "svg+xml") {
                kind = "svg";
            }
            bytes = data
                .decode_to_vec()
                .map_err(|_| anyhow::anyhow!("Invalid image data"))?
                .0;
        }
        "url" => {
            let url = reqwest::Url::parse(value)?;
            ensure!(
                matches!(url.scheme(), "https" | "http"),
                "Expected an HTTP(S) image URL."
            );
            let mut response = http
                .get(url)
                .timeout(Duration::from_secs(20))
                .send()
                .await?
                .error_for_status()?;
            extension = ext(Path::new(response.url().path()));
            if response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with("image/svg+xml"))
            {
                kind = "svg";
            }
            while let Some(chunk) = response.chunk().await? {
                ensure!(
                    bytes.len() + chunk.len() <= LIMIT,
                    "Media exceeds the size limit."
                );
                bytes.extend(chunk);
            }
        }
        "graphviz" | "mermaid" | "svg" if key != "image" => {
            bytes = field(args, key)?.as_bytes().to_vec()
        }
        "svg" | "image" => {}
        _ => bail!("Unknown media source"),
    }
    kind = match extension.as_deref() {
        Some("dot" | "gv") => "graphviz",
        Some("mmd" | "mermaid") => "mermaid",
        Some("svg") => "svg",
        _ => kind,
    };
    ensure!(bytes.len() <= LIMIT, "Media exceeds the size limit.");
    if matches!(kind, "mermaid" | "graphviz") {
        ensure!(
            bytes.len() <= 1024 * 1024,
            "Diagram exceeds the size limit."
        );
        let mut cmd = Command::new(resources.join("conversation-diagram"));
        cmd.arg(kind);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        bytes = timeout(Duration::from_secs(10), async {
            let write = async {
                let r = stdin.write_all(&bytes).await;
                drop(stdin);
                match r {
                    Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
                    _ => Ok(()),
                }
            };
            let (_, out, err, status) = tokio::try_join!(
                write,
                limited(stdout, LIMIT),
                limited(stderr, LIMIT),
                async { Ok(child.wait().await?) }
            )?;
            ensure!(
                status.success(),
                "Diagram renderer failed: {}",
                String::from_utf8_lossy(&err).trim()
            );
            Ok::<_, anyhow::Error>(out)
        })
        .await??;
    }
    let kind = match kind {
        "mermaid" => "Mermaid",
        "graphviz" => "Graphviz",
        "svg" => "SVG",
        _ => "Image",
    };
    let title = args["title"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(name.as_deref())
        .unwrap_or(if kind == "Image" { "Image" } else { "Diagram" });
    Ok(json!({"kind":kind,"title":title,"data":STANDARD.encode(bytes)}))
}
fn ext(p: &Path) -> Option<String> {
    p.extension()?.to_str().map(str::to_ascii_lowercase)
}
