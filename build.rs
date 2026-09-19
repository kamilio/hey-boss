use std::{fs, path::Path};

fn collect(root: &Path, path: &Path, files: &mut Vec<String>) {
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("read build sources") {
            collect(root, &entry.expect("source entry").path(), files);
        }
    } else if path.is_file() {
        files.push(path.strip_prefix(root).unwrap().to_string_lossy().into());
    }
}

fn main() {
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let root = Path::new(&root);
    let mut files = Vec::new();
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "src",
        "worker-tui/Cargo.toml",
        "worker-tui/src",
        "skills/hey-boss",
        "tools/upgrade_hey_boss.py",
        "tools/fleet_hey_boss.py",
        "tools/drain_github_issues.py",
        "hey_boss_daemon.swift",
        "package_hey_boss.swift",
        "setup_hey_boss.swift",
        "assets",
    ] {
        println!("cargo:rerun-if-changed={name}");
        collect(root, &root.join(name), &mut files);
    }
    files.sort();
    let mut hash = 0xcbf29ce484222325u64;
    for name in files {
        for byte in name
            .bytes()
            .chain([0])
            .chain(fs::read(root.join(&name)).unwrap())
            .chain([0])
        {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    println!("cargo:rustc-env=HEY_BOSS_BUILD_ID={hash:016x}");
}
