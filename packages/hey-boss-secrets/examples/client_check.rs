//! Run with an isolated HEY_BOSS_STATE_DIR and an explicitly supplied CLI path.
use hey_boss_secrets::{Error, client::Client};
fn main() {
    let client = Client::new(std::env::args_os().nth(1).expect("CLI path"));
    let saved = client
        .put(
            "synthetic-rust-sdk",
            b"synthetic\0bytes\xff\n",
            "blob",
            None,
        )
        .unwrap();
    let (record, secret) = client.get("synthetic-rust-sdk").unwrap();
    assert_eq!(record.revision, saved.revision);
    assert_eq!(secret.expose(), b"synthetic\0bytes\xff\n");
    assert_eq!(
        client
            .put("synthetic-rust-sdk", b"new", "blob", Some("stale"))
            .unwrap_err(),
        Error::StaleRevision
    );
    println!("Rust SDK native CLI roundtrip and stale-write checks passed");
}
