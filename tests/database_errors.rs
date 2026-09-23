use hey_boss::issues::Error;

#[test]
fn database_errors_keep_extended_codes_for_read_failures_corruption_and_contention() {
    for (extended, primary, category) in [
        (
            rusqlite::ffi::SQLITE_IOERR_SHORT_READ,
            rusqlite::ffi::SQLITE_IOERR,
            "database_error",
        ),
        (
            rusqlite::ffi::SQLITE_CORRUPT,
            rusqlite::ffi::SQLITE_CORRUPT,
            "database_error",
        ),
        (
            rusqlite::ffi::SQLITE_BUSY_SNAPSHOT,
            rusqlite::ffi::SQLITE_BUSY,
            "database_busy",
        ),
    ] {
        let error = Error::from(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(extended),
            Some("Synthetic database failure".into()),
        ));
        assert_eq!(error.code, category);
        let serialized = serde_json::to_value(&error).unwrap();
        assert_eq!(serialized["details"]["sqlite_code"], primary);
        assert_eq!(serialized["details"]["sqlite_extended_code"], extended);
        assert!(error.to_string().contains(&format!("SQLite {extended}")));
        assert!(error.message.contains("Synthetic database failure"));
    }
}

#[test]
fn non_sqlite_failures_do_not_invent_a_database_result_code() {
    let error = Error::from(rusqlite::Error::QueryReturnedNoRows);
    assert_eq!(error.code, "database_error");
    assert!(error.details.is_none());
}
