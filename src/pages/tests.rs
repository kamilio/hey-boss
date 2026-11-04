use crate::{issues::Store, pages};
use std::{fs, io::Read, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::current_dir().unwrap().join("out").join(format!(
            "publish-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn publishing_is_global_atomic_and_reuses_unchanged_content() {
    let f = Fixture::new();
    let db = Store::open(&f.0.join("issues.db")).unwrap().into_database();
    let site = f.0.join("site");
    fs::create_dir_all(site.join("assets")).unwrap();
    fs::write(site.join("index.html"), "<h1>First</h1>").unwrap();
    fs::write(site.join("assets/app.js"), "window.loaded=true").unwrap();
    fs::write(site.join(".env"), "do not publish").unwrap();
    let first = pages::publish(&f.0.join("issues.db"), &site, "report").unwrap();
    assert_eq!(first.files, 2);
    assert_eq!(pages::list(&f.0.join("issues.db"), 0).unwrap().len(), 1);
    assert_eq!(
        pages::publish(&f.0.join("issues.db"), &site, "report")
            .unwrap()
            .deployment,
        first.deployment
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM published_chunks", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    fs::write(site.join("index.html"), "<h1>Second</h1>").unwrap();
    let second = pages::publish(&f.0.join("issues.db"), &site, "report").unwrap();
    assert_ne!(first.deployment, second.deployment);
    assert_eq!(
        db.query_row("SELECT count(*) FROM published_chunks", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    let mut text = String::new();
    pages::content(&f.0.join("issues.db"), &first.deployment, "index.html")
        .unwrap()
        .reader
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "<h1>First</h1>");
    fs::remove_file(site.join("index.html")).unwrap();
    assert!(pages::publish(&f.0.join("issues.db"), &site, "report").is_err());
    assert_eq!(
        pages::view(&f.0.join("issues.db"), "report")
            .unwrap()
            .deployment,
        second.deployment
    );
    assert!(pages::content(&f.0.join("issues.db"), &first.deployment, ".env").is_err());
    assert!(pages::content(&f.0.join("issues.db"), &first.deployment, "../issues.db").is_err());
    assert_eq!(
        db.query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn a_single_html_file_is_index_and_symlinks_and_invalid_names_are_rejected() {
    let f = Fixture::new();
    let source = f.0.join("report.html");
    fs::write(&source, "<h1>Hello</h1>").unwrap();
    assert_eq!(
        pages::publish(&f.0.join("issues.db"), &source, "report")
            .unwrap()
            .files,
        1
    );
    for name in ["../escape", "", "UPPER", "-bad", "a/b"] {
        assert!(pages::publish(&f.0.join("issues.db"), &source, name).is_err());
    }
    let link = f.0.join("link.html");
    std::os::unix::fs::symlink(&source, &link).unwrap();
    assert!(pages::publish(&f.0.join("issues.db"), &link, "linked").is_err());
}

#[test]
fn download_round_trips_assets_and_delete_revokes_the_named_page() {
    let f = Fixture::new();
    let database = f.0.join("issues.db");
    let source = f.0.join("site");
    fs::create_dir_all(source.join("assets")).unwrap();
    fs::write(source.join("index.html"), "hello").unwrap();
    let large = vec![42; pages::CHUNK_SIZE * 2 + 17];
    fs::write(source.join("assets/data.bin"), &large).unwrap();
    pages::publish(&database, &source, "site").unwrap();
    let output = f.0.join("downloaded");
    pages::download(&database, "site", &output).unwrap();
    assert_eq!(fs::read(output.join("assets/data.bin")).unwrap(), large);
    assert!(pages::download(&database, "site", &output).is_err());
    pages::delete(&database, "site").unwrap();
    assert!(pages::view(&database, "site").is_err());
    assert!(pages::list(&database, 0).unwrap().is_empty());
    assert!(pages::download(&database, "site", &f.0.join("deleted")).is_err());
}

#[test]
fn concurrent_publishers_share_chunks_and_leave_complete_named_pages() {
    let f = Fixture::new();
    let database = f.0.join("issues.db");
    Store::open(&database).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|worker| {
            let root = f.0.clone();
            let database = database.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let source = root.join(format!("source-{worker}"));
                fs::create_dir(&source).unwrap();
                fs::write(source.join("common.bin"), vec![9; pages::CHUNK_SIZE + 17]).unwrap();
                barrier.wait();
                for revision in 0..8 {
                    fs::write(source.join("index.html"), format!("{worker}:{revision}")).unwrap();
                    pages::publish(&database, &source, &format!("page-{}", worker % 3)).unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(pages::list(&database, 0).unwrap().len(), 3);
    for page in pages::list(&database, 0).unwrap() {
        assert!(!page.syncing);
        pages::download(&database, &page.name, &f.0.join(&page.name)).unwrap();
    }
}

#[test]
fn chunks_are_compressed_on_disk_and_legacy_chunks_remain_readable() {
    let f = Fixture::new();
    let database = f.0.join("issues.db");
    let bytes = b"<p>Repeated inventory and source evidence</p>".repeat(20_000);
    let hash = hey_boss_sync::digest(&bytes);
    pages::write_chunk(&database, &hash, &bytes).unwrap();
    let compressed = pages::root(&database).join(format!("{hash}.gz"));
    assert!(fs::metadata(&compressed).unwrap().len() < bytes.len() as u64 / 10);
    assert_eq!(pages::read_chunk(&database, &hash).unwrap(), bytes);
    fs::remove_file(compressed).unwrap();
    fs::write(pages::root(&database).join(&hash), &bytes).unwrap();
    assert_eq!(pages::read_chunk(&database, &hash).unwrap(), bytes);
    assert!(!pages::root(&database).join(&hash).exists());
}

#[test]
fn concurrent_reads_can_migrate_the_same_legacy_chunk() {
    let f = Fixture::new();
    let database = f.0.join("issues.db");
    let bytes = vec![b'x'; pages::CHUNK_SIZE];
    let hash = hey_boss_sync::digest(&bytes);
    fs::create_dir(pages::root(&database)).unwrap();
    fs::write(pages::root(&database).join(&hash), &bytes).unwrap();
    let start = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                start.wait();
                assert_eq!(pages::read_chunk(&database, &hash).unwrap(), bytes);
            });
        }
    });
    assert!(!pages::root(&database).join(hash).exists());
}
