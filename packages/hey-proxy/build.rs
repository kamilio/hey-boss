// Rollouts retain the workspace layout and its single lockfile. Only source
// trees are embedded; build output, local configuration and credentials never are.
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn collect(root: &Path, path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for child in entries {
            let name = child.file_name().unwrap().to_string_lossy();
            if name.starts_with('.')
                || matches!(
                    name.as_ref(),
                    "target" | "output" | "out" | "dist" | "node_modules" | "__pycache__"
                )
            {
                continue;
            }
            collect(root, &child, files);
        }
    } else if path.is_file() {
        files.push(path.strip_prefix(root).unwrap().to_path_buf());
    }
}

fn main() {
    let package = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace = package.parent().and_then(Path::parent).filter(|root| {
        root.join("Cargo.toml").is_file() && root.join("packages/hey-proxy").is_dir()
    });
    // Cargo's published package is standalone; its generated manifest/lockfile
    // must remain usable too, without requiring a surrounding Boss checkout.
    let root = workspace.unwrap_or(&package);
    let mut files = Vec::new();
    let inputs: &[&str] = if workspace.is_some() {
        &[
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            "README.md",
            "LICENSE",
            "src",
            "tests",
            "packages",
            "skills/hey-boss",
            "assets",
            "hey_boss_daemon.swift",
            "package_hey_boss.swift",
            "setup_hey_boss.swift",
            "tools/upgrade_hey_boss.py",
            "tools/drain_github_issues.py",
        ]
    } else {
        &[
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            "README.md",
            "LICENSE",
            "src",
            "tests",
            "examples",
        ]
    };
    for name in inputs {
        let path = root.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        collect(root, &path, &mut files);
    }
    let mut out = String::from("&[\n");
    for file in files {
        let name = file.to_str().unwrap().replace('\\', "/");
        out.push_str(&format!(
            "    ({name:?}, include_bytes!({:?}) as &[u8]),\n",
            root.join(file).to_str().unwrap()
        ));
    }
    out.push_str("]\n");
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("bundle_files.rs"),
        out,
    )
    .unwrap();
    println!(
        "cargo:rustc-env=HEY_PROXY_BUNDLE_PACKAGE={}",
        if workspace.is_some() {
            "packages/hey-proxy"
        } else {
            "."
        }
    );
}
