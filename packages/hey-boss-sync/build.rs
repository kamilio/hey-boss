use std::{env, path::PathBuf, process::Command};

fn checked(command: &mut Command) {
    let output = command.output().expect("start cr-sqlite build");
    assert!(
        output.status.success(),
        "cr-sqlite build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn main() {
    println!("cargo:rerun-if-changed=vendor");
    let source = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("vendor/cr-sqlite");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let host = env::var("HOST").unwrap();
    assert_eq!(
        host, target,
        "Build the native extension on the deployment architecture"
    );
    checked(
        Command::new(env::var_os("CARGO").unwrap())
            .current_dir(&source)
            .args([
                "build",
                "--locked",
                "--release",
                "-p",
                "crsql_bundle_static",
                "--features",
                "loadable_extension",
            ])
            .arg("--target-dir")
            .arg(out.join("rust"))
            .env("RUSTC_BOOTSTRAP", "1")
            .env(
                "CRSQLITE_COMMIT_SHA",
                "8e32195704ccaea8508f7d378e11dc2dc0bb3d2e",
            )
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTFLAGS"),
    );
    let library = out.join(if target.contains("apple") {
        "crsqlite.dylib"
    } else {
        "crsqlite.so"
    });
    checked(
        Command::new(env::var("CC").unwrap_or_else(|_| "cc".into()))
            .current_dir(source.join("core"))
            .args([
                "-O2",
                "-std=c99",
                "-fPIC",
                "-shared",
                "-Isrc",
                "-Isrc/sqlite",
                "src/crsqlite.c",
                "src/changes-vtab.c",
                "src/ext-data.c",
                "src/version-guard.c",
            ])
            .arg(out.join("rust/release/libcrsql_bundle_static.a"))
            .arg("-o")
            .arg(&library),
    );
    println!("cargo:rustc-env=CRSQLITE_LIBRARY={}", library.display());
}
