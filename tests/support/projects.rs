//! Explicit legacy registry setup for fixtures that test existing project behavior.
use std::path::Path;

pub fn seed(db: &Path, projects: &[&str]) {
    drop(hey_boss::issues::Store::open(db).unwrap());
    let db = rusqlite::Connection::open(db).unwrap();
    for id in projects {
        let name = id.rsplit('/').next().unwrap().trim_start_matches("named:");
        db.execute(
            "INSERT OR IGNORE INTO projects(id,name,next_number) VALUES(?1,?2,1)",
            [id, &name],
        )
        .unwrap();
    }
}
