//! Synthetic native-storage check. No real credentials are accessed.
use hey_boss_secrets::backend::{Backend, native};
fn main() {
    let backend: Box<dyn Backend> = native(std::path::Path::new(&std::env::var("HOME").unwrap()));
    let key = "synthetic-upgrade-check";
    match std::env::args().nth(1).as_deref() {
        Some("write") => match backend.create(key, b"synthetic-storage-check") {
            Ok(()) => (),
            Err(hey_boss_secrets::Error::Conflict) => assert_eq!(
                backend.get(key).unwrap().unwrap().expose(),
                b"synthetic-storage-check"
            ),
            Err(err) => panic!("fixture creation failed: {err}"),
        },
        Some("check") => assert_eq!(
            backend.get(key).unwrap().unwrap().expose(),
            b"synthetic-storage-check"
        ),
        _ => panic!("use write or check"),
    }
    println!("native storage check passed");
}
