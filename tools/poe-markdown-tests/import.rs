//! Refresh the compatibility suite from a local Poe Code checkout.
use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

const SUITE: &str = "packages/toolcraft-design/src/terminal-markdown";
const ENTRYPOINT: &str = "packages/toolcraft-design/src/index.ts";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    repository: &'static str,
    revision: String,
    test_files: Vec<PathBuf>,
    files: BTreeMap<String, String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let checkout = std::env::args_os()
        .nth(1)
        .ok_or("expected a Poe Code checkout path")?;
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/poe-markdown-tests");
    let dest = base.join("upstream");
    let git = |args: &[&str]| -> io::Result<Vec<u8>> {
        let output = Command::new("git")
            .current_dir(&checkout)
            .args(args)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(String::from_utf8_lossy(&output.stderr)));
        }
        Ok(output.stdout)
    };
    let revision = String::from_utf8(git(&["rev-parse", "HEAD"])?)?
        .trim()
        .to_owned();
    let tree = String::from_utf8(git(&[
        "ls-tree",
        "-r",
        "--name-only",
        &revision,
        "--",
        SUITE,
    ])?)?;
    let test_files: Vec<PathBuf> = tree
        .lines()
        .filter(|s| s.ends_with(".test.ts"))
        .map(PathBuf::from)
        .collect();
    if test_files.is_empty() {
        return Err("no upstream Markdown tests found".into());
    }
    let mut todo = test_files.clone();
    todo.extend([
        format!("{SUITE}/testing/theme-render-fixture.ts").into(),
        PathBuf::from("packages/frontmatter/src/index.ts"),
        "packages/toolcraft-design/LICENSE".into(),
        "packages/frontmatter/LICENSE".into(),
    ]);
    let imports = Regex::new(r#"(?:from\s*|import\s*)["'](\.[^"']+)["']"#)?;
    let mut files = BTreeMap::new();
    while let Some(file) = todo.pop() {
        let relative = file.to_str().ok_or("upstream path is not UTF-8")?;
        if relative == ENTRYPOINT || files.contains_key(relative) {
            continue;
        }
        let data = git(&["show", &format!("{revision}:{relative}")])?;
        if file.extension().is_some_and(|ext| ext == "ts") {
            for import in imports.captures_iter(std::str::from_utf8(&data)?) {
                let mut dependency = file.parent().unwrap().to_owned();
                for component in Path::new(&import[1]).components() {
                    match component {
                        std::path::Component::CurDir => (),
                        std::path::Component::ParentDir if dependency.pop() => (),
                        std::path::Component::Normal(part) => dependency.push(part),
                        _ => {
                            return Err(
                                format!("invalid relative import in {}", file.display()).into()
                            );
                        }
                    }
                }
                if dependency.extension().is_some_and(|ext| ext == "js") {
                    dependency.set_extension("ts");
                }
                todo.push(dependency);
            }
        }
        let output = dest.join(&file);
        fs::create_dir_all(output.parent().unwrap())?;
        fs::write(output, &data)?;
        files.insert(relative.to_owned(), format!("{:x}", Sha256::digest(&data)));
    }
    // The tracked test-only entrypoint belongs to the harness, not the upstream snapshot.
    let manifest = Manifest {
        repository: "https://github.com/poe-platform/poe-code",
        revision,
        test_files,
        files,
    };
    let mut json = serde_json::to_vec_pretty(&manifest)?;
    json.push(b'\n');
    fs::write(base.join("upstream-manifest.json"), json)?;
    println!(
        "Imported {} tests and {} supporting files.",
        manifest.test_files.len(),
        manifest.files.len() - manifest.test_files.len()
    );
    Ok(())
}
