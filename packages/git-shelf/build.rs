fn main() {
    println!("cargo:rerun-if-env-changed=GIT_SHELF_BUILD_ID");
    let build = std::env::var("GIT_SHELF_BUILD_ID").unwrap_or_else(|_| "standalone".into());
    println!(
        "cargo:rustc-env=GIT_SHELF_VERSION={} (build {build})",
        env!("CARGO_PKG_VERSION")
    );
}
