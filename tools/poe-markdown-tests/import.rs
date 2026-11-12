//! Reproduce the reference suite, or refresh it from a local Poe Code checkout.
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

const SUITE: &str = "packages/toolcraft-design/src/terminal-markdown";
const REPOSITORY: &str = "https://github.com/poe-platform/poe-code";

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    repository: String,
    revision: String,
    test_files: Vec<PathBuf>,
    files: BTreeMap<String, String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/poe-markdown-tests");
    let manifest_path = base.join("upstream-manifest.json");
    let stage = tempfile::tempdir_in(&base)?;
    let checkout = std::env::args_os().nth(1).map(PathBuf::from);
    let dest = stage.path().join("upstream");
    let git = |args: &[&str]| -> io::Result<Vec<u8>> {
        let output = Command::new("git")
            .current_dir(checkout.as_deref().unwrap_or(stage.path()))
            .args(args)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(String::from_utf8_lossy(&output.stderr)));
        }
        Ok(output.stdout)
    };
    let expected: Option<Manifest> = if checkout.is_none() {
        Some(serde_json::from_slice(&fs::read(&manifest_path)?)?)
    } else {
        None
    };
    let revision = if let Some(expected) = &expected {
        git(&["init", "--bare", "--quiet"])?;
        git(&[
            "fetch",
            "--depth=1",
            "--no-tags",
            REPOSITORY,
            &expected.revision,
        ])?;
        expected.revision.clone()
    } else {
        String::from_utf8(git(&["rev-parse", "HEAD"])?)?
            .trim()
            .to_owned()
    };
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
        if files.contains_key(relative) {
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
    let manifest = Manifest {
        repository: REPOSITORY.into(),
        revision,
        test_files,
        files,
    };
    if expected
        .as_ref()
        .is_some_and(|expected| *expected != manifest)
    {
        return Err("upstream files or tests differ from upstream-manifest.json".into());
    }
    fs::copy(
        base.join("frontmatter-package.json"),
        dest.join("packages/frontmatter/package.json"),
    )?;
    let upstream = base.join("upstream");
    if upstream.exists() {
        fs::remove_dir_all(&upstream)?;
    }
    fs::rename(dest, upstream)?;
    if expected.is_none() {
        let mut json = serde_json::to_vec_pretty(&manifest)?;
        json.push(b'\n');
        fs::write(manifest_path, json)?;
    }
    println!(
        "Imported {} tests and {} supporting files.",
        manifest.test_files.len(),
        manifest.files.len() - manifest.test_files.len()
    );
    Ok(())
}
