use std::{fs, path::Path};

fn collect(root: &Path, path: &Path, files: &mut Vec<String>) {
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("read harvester sources") {
            collect(root, &entry.expect("source entry").path(), files);
        }
    } else if path.is_file() {
        files.push(path.strip_prefix(root).unwrap().to_string_lossy().into());
    }
}

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let root = Path::new(&manifest);
    let mut files = Vec::new();
    for name in ["Cargo.toml", "build.rs", "src"] {
        println!("cargo:rerun-if-changed={name}");
        collect(root, &root.join(name), &mut files);
    }
    // Match the workspace's content fingerprint convention. Paths are relative
    // so SSH installation staging and the original checkout identify alike.
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
    for name in ["../../Cargo.toml", "../../Cargo.lock"] {
        println!("cargo:rerun-if-changed={name}");
        // Cargo also builds this as a standalone published package, without
        // the parent workspace. Its normalized package manifest is hashed above.
        let bytes = match fs::read(root.join(name)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("read workspace build input: {error}"),
        };
        for byte in name.bytes().chain([0]).chain(bytes).chain([0]) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    println!("cargo:rustc-env=HEY_HARVESTER_BUILD_ID={hash:016x}");
}
