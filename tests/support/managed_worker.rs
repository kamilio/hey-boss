// Seed the registration that the supervisor creates before launching its child.
pub fn register(path: &std::path::Path) -> String {
    drop(hey_boss::issues::Store::open(path).unwrap());
    let id: String = rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))
        .unwrap();
    rusqlite::Connection::open(path).unwrap().execute(
        "INSERT INTO issue_workers(id,kind,config,version,updated_at,machine) VALUES(?1,'cli','{}',1,0,?2)",
        rusqlite::params![id, hey_boss::issues::identity::machine().unwrap()],
    ).unwrap();
    id
}
