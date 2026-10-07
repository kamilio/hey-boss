// Embeds every crate source so `hey-proxy rollout` can rebuild the proxy on
// remote hosts without a hand-maintained file list drifting out of date.
use std::{
    env, fs,
    path::{Path, PathBuf},
};

const TOP_LEVEL: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "README.md",
    "LICENSE",
    "build.rs",
];
const TREES: &[&str] = &["src", "tests"];

fn collect(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy();
        if name.starts_with('.') || name == "__pycache__" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            files.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut files: Vec<PathBuf> = TOP_LEVEL.iter().map(PathBuf::from).collect();
    for tree in TREES {
        println!("cargo:rerun-if-changed={tree}");
        collect(&root, &root.join(tree), &mut files);
    }
    for file in TOP_LEVEL {
        println!("cargo:rerun-if-changed={file}");
    }
    let mut out = String::from("&[\n");
    for file in files {
        let name = file.to_str().unwrap().replace('\\', "/");
        let absolute = root.join(&file);
        out.push_str(&format!(
            "    ({name:?}, include_bytes!({:?}) as &[u8]),\n",
            absolute.to_str().unwrap()
        ));
    }
    out.push_str("]\n");
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("bundle_files.rs"),
        out,
    )
    .unwrap();
}
