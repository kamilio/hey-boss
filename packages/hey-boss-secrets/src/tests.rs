use super::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct Memory(Arc<Mutex<BTreeMap<String, Vec<u8>>>>);
impl backend::Backend for Memory {
    fn name(&self) -> &'static str {
        "memory-test"
    }
    fn get(&self, key: &str) -> Result<Option<Secret>> {
        Ok(self.0.lock().unwrap().get(key).cloned().map(Secret::new))
    }
    fn put(&self, key: &str, value: &[u8]) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
}
fn setup(label: &str) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(
        dir.path(),
        &dir.path().join("journal.db"),
        Box::new(Memory::default()),
    )
    .unwrap();
    store.initialize(label).unwrap();
    (dir, store)
}
fn sync(a: &Store, b: &mut Store) {
    let source = hey_boss_sync::Engine::open(a.journal_path()).unwrap();
    let mut target = hey_boss_sync::Engine::open(b.journal_path()).unwrap();
    loop {
        let batch = source.batch(target.received(source.id()).unwrap()).unwrap();
        if batch.records.is_empty() {
            break;
        }
        target.ingest(&batch).unwrap();
    }
    b.synchronize().unwrap();
}

#[test]
fn local_roundtrip_redaction_and_stale_updates() {
    let (dir, mut s) = setup("writer");
    let r = s
        .put(
            "google-personal",
            b"synthetic-sensitive-token",
            "bundle",
            None,
        )
        .unwrap();
    assert_eq!(
        s.get("google-personal").unwrap().expose(),
        b"synthetic-sensitive-token"
    );
    assert_eq!(
        format!("{:?}", s.get("google-personal").unwrap()),
        "Secret([redacted])"
    );
    assert_eq!(
        s.put("google-personal", b"replacement", "bundle", Some("stale"))
            .unwrap_err(),
        Error::StaleRevision
    );
    s.put(
        "google-personal",
        b"replacement",
        "bundle",
        Some(&r.revision),
    )
    .unwrap();
    let inventory = serde_json::to_string(&s.inventory().unwrap()).unwrap();
    assert!(!inventory.contains("synthetic-sensitive-token"));
    assert!(!inventory.contains("ciphertext"));
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            assert!(
                !std::fs::read(p)
                    .unwrap()
                    .windows(b"synthetic-sensitive-token".len())
                    .any(|x| x == b"synthetic-sensitive-token")
            );
        }
    }
}

#[test]
fn replication_requires_trust_and_grant_and_survives_writer_offline() {
    let (_a, mut a) = setup("writer");
    let (_b, mut b) = setup("reader");
    let (_c, mut c) = setup("outsider");
    a.trust(&b.enrollment().unwrap()).unwrap();
    a.put("cookies", b"synthetic-cookie-jar", "blob", None)
        .unwrap();
    sync(&a, &mut b);
    assert_eq!(b.get("cookies").unwrap_err(), Error::UntrustedWriter);
    b.trust(&a.enrollment().unwrap()).unwrap();
    assert_eq!(b.get("cookies").unwrap_err(), Error::NotGranted);
    a.grant("cookies", &b.enrollment().unwrap().machine.id, true)
        .unwrap();
    sync(&a, &mut b);
    sync(&b, &mut c);
    c.trust(&a.enrollment().unwrap()).unwrap();
    assert_eq!(c.get("cookies").unwrap_err(), Error::NotGranted);
    drop(a);
    assert_eq!(b.get("cookies").unwrap().expose(), b"synthetic-cookie-jar");
    let r = b.inventory().unwrap().records.remove(0);
    b.put(
        "cookies",
        b"updated-while-owner-offline",
        "blob",
        Some(&r.revision),
    )
    .unwrap();
    assert_eq!(
        b.get("cookies").unwrap().expose(),
        b"updated-while-owner-offline"
    );
}

#[test]
fn revocation_and_deletion_override_local_cached_values() {
    let (_a, mut a) = setup("writer");
    let (_b, mut b) = setup("reader");
    a.trust(&b.enrollment().unwrap()).unwrap();
    b.trust(&a.enrollment().unwrap()).unwrap();
    a.put("key", b"synthetic", "value", None).unwrap();
    let id = b.enrollment().unwrap().machine.id;
    a.grant("key", &id, true).unwrap();
    sync(&a, &mut b);
    b.get("key").unwrap();
    a.grant("key", &id, false).unwrap();
    sync(&a, &mut b);
    assert_eq!(b.get("key").unwrap_err(), Error::NotGranted);
    a.delete("key").unwrap();
    sync(&a, &mut b);
    assert_eq!(a.get("key").unwrap_err(), Error::NotFound);
    assert_eq!(b.get("key").unwrap_err(), Error::NotFound);
}

#[test]
fn oversized_and_unsafe_names_are_rejected_before_storage() {
    let (_d, mut s) = setup("writer");
    assert_eq!(
        s.put("../escape", b"x", "value", None).unwrap_err(),
        Error::InvalidInput
    );
    assert_eq!(
        s.put("safe", &vec![0; MAX_SECRET + 1], "blob", None)
            .unwrap_err(),
        Error::TooLarge
    );
}

#[test]
fn survivors_converge_without_owner_and_regardless_of_delivery_order() {
    let (_da, mut a) = setup("owner");
    let (_db, mut b) = setup("east");
    let (_dc, mut c) = setup("west");
    for peer in [&mut b, &mut c] {
        a.trust(&peer.enrollment().unwrap()).unwrap();
        peer.trust(&a.enrollment().unwrap()).unwrap();
    }
    a.put("shared", b"initial", "value", None).unwrap();
    for peer in [&b, &c] {
        a.grant("shared", &peer.enrollment().unwrap().machine.id, true)
            .unwrap();
    }
    sync(&a, &mut b);
    sync(&a, &mut c);
    drop(a);
    b.put("shared", b"east-offline", "value", None).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    c.put("shared", b"west-latest", "value", None).unwrap();
    sync(&b, &mut c);
    sync(&c, &mut b);
    assert_eq!(b.get("shared").unwrap().expose(), b"west-latest");
    assert_eq!(c.get("shared").unwrap().expose(), b"west-latest");
    assert_eq!(
        b.inventory().unwrap().records[0].revision,
        c.inventory().unwrap().records[0].revision
    );
    b.put("shared", b"after-reconnect", "value", None).unwrap();
    sync(&b, &mut c);
    assert_eq!(c.get("shared").unwrap().expose(), b"after-reconnect");
}

#[test]
fn later_stale_policy_write_cannot_undo_revocation() {
    let (_da, mut a) = setup("owner");
    let (_db, mut b) = setup("revoked");
    a.trust(&b.enrollment().unwrap()).unwrap();
    b.trust(&a.enrollment().unwrap()).unwrap();
    a.put("shared", b"initial", "value", None).unwrap();
    a.grant("shared", &b.enrollment().unwrap().machine.id, true)
        .unwrap();
    sync(&a, &mut b);
    a.grant("shared", &b.enrollment().unwrap().machine.id, false)
        .unwrap();
    b.put("shared", b"stale-policy-data", "value", None)
        .unwrap();
    sync(&b, &mut a);
    sync(&a, &mut b);
    assert_eq!(a.get("shared").unwrap().expose(), b"initial");
    assert_eq!(b.get("shared").unwrap_err(), Error::NotGranted);
}

#[test]
fn tampered_ciphertext_and_policy_never_materialize() {
    let (_da, mut a) = setup("owner");
    let (_db, mut b) = setup("peer");
    a.put("valid", b"synthetic", "value", None).unwrap();
    let source = hey_boss_sync::Engine::open(a.journal_path()).unwrap();
    let payload = source.pending(0).unwrap().pop().unwrap().2;
    let mut wire: serde_json::Value = serde_json::from_str(&payload).unwrap();
    wire["revision"]["body"]["ciphertext"] = "forged".into();
    let mut target = hey_boss_sync::Engine::open(b.journal_path()).unwrap();
    target.put(&wire.to_string()).unwrap();
    let mut wire: serde_json::Value = serde_json::from_str(&payload).unwrap();
    wire["revision"]["body"]["access"]["policy"]["name"] = "spoofed".into();
    target.put(&wire.to_string()).unwrap();
    assert!(b.inventory().unwrap().records.is_empty());
    sync(&a, &mut b);
    assert_eq!(b.inventory().unwrap().records.len(), 1);
}

#[test]
fn binary_max_size_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let memory = Memory::default();
    let open = || {
        Store::open(
            dir.path(),
            &dir.path().join("journal.db"),
            Box::new(memory.clone()),
        )
        .unwrap()
    };
    let mut a = open();
    a.initialize("test").unwrap();
    let bytes: Vec<u8> = (0..MAX_SECRET).map(|i| (i % 256) as u8).collect();
    a.put("binary", &bytes, "cookies", None).unwrap();
    drop(a);
    let mut b = open();
    assert_eq!(b.get("binary").unwrap().expose(), bytes);
}

#[test]
fn sdk_protocol_preserves_bytes_and_returns_fixed_errors() {
    use base64::Engine;
    let (_dir, mut store) = setup("sdk");
    let request = serde_json::json!({"op":"put","name":"sdk-key","kind":"blob","expected":null,"data":base64::engine::general_purpose::STANDARD.encode(b"synthetic\0binary\n")});
    let reply = client::serve(&mut store, request.to_string().as_bytes());
    let reply: serde_json::Value = serde_json::from_slice(reply.expose()).unwrap();
    assert_eq!(reply["ok"], true);
    assert!(reply.get("data").is_none());
    let reply = client::serve(&mut store, br#"{"op":"get","name":"sdk-key"}"#);
    let reply: serde_json::Value = serde_json::from_slice(reply.expose()).unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(reply["data"].as_str().unwrap())
            .unwrap(),
        b"synthetic\0binary\n"
    );
    let reply = client::serve(&mut store, b"synthetic-secret-malformed-request");
    assert_eq!(reply.expose(), br#"{"ok":false,"error":"invalid_input"}"#);
}
