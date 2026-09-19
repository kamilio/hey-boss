use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read},
    path::Path,
};
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DocumentAttachment {
    pub name: String,
    pub mime: String,
    pub data: String,
}
pub fn read_file(path: &Path) -> io::Result<(String, Option<DocumentAttachment>)> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Review path must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(io::Error::other("Review image exceeds 4 MiB"));
    }
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect::<String>();
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    };
    if let Some(mime) = mime {
        return Ok((
            format!("Image review: {name}"),
            Some(DocumentAttachment {
                name,
                mime: mime.into(),
                data: STANDARD.encode(bytes),
            }),
        ));
    }
    if bytes.len() > 1024 * 1024 {
        return Err(io::Error::other("Review text file exceeds 1 MiB"));
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        io::Error::other(
            "Review file must be UTF-8 text or a supported PNG, JPEG, GIF, or WebP image",
        )
    })?;
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "md" | "markdown" | "mdown") {
        return Ok((text, None));
    }
    let language = match extension.as_str() {
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" => "typescript",
        "py" => "python",
        "yml" => "yaml",
        "rb" => "ruby",
        "sh" => "bash",
        "h" => "c",
        "cc" | "cxx" => "cpp",
        "swift" | "json" | "jsonc" | "yaml" | "toml" | "html" | "xml" | "svg" | "css" | "sql"
        | "go" | "java" | "kotlin" | "diff" => extension.as_str(),
        _ => "text",
    };
    let run = |marker| {
        let (mut max, mut current) = (0, 0);
        for c in text.chars() {
            if c == marker {
                current += 1;
                max = max.max(current);
            } else {
                current = 0;
            }
        }
        max
    };
    let backticks = run('`');
    let tildes = run('~');
    let (marker, length) = if backticks <= tildes {
        ('`', backticks)
    } else {
        ('~', tildes)
    };
    let fence = marker.to_string().repeat((length + 1).max(3));
    Ok((format!("{fence}{language}\n{text}\n{fence}"), None))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_files_are_literal_and_fences_cannot_break_out() {
        let root = std::env::temp_dir().join(format!("hb-document-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("example.rs");
        std::fs::write(&path, "let x = \"```\";\n<script>bad</script>").unwrap();
        let (text, attachment) = read_file(&path).unwrap();
        assert!(attachment.is_none());
        let html = crate::markdown::render_document(&text);
        assert!(html.contains("language-rust") && !html.contains("<script>"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
