use super::*;
use crate::proxy::logs::store::Entry;

fn entry(id: &str) -> Entry {
    Entry {
        request_id: id.into(),
        timestamp_ms: now_ms() as u64,
        ended_ms: Some(now_ms() as u64),
        method: "POST".into(),
        path: "/v1/responses".into(),
        provider: Some("openai".into()),
        account_id: Some("alpha".into()),
        billing: Some("api".into()),
        routed_model: Some("gpt-6-astra".into()),
        input_tokens: Some(100_000),
        cached_input_tokens: Some(80_000),
        output_tokens: Some(10_000),
        reasoning_tokens: Some(2000),
        ..Entry::default()
    }
}

#[tokio::test]
async fn aggregates_by_actual_transport_without_model_based_subscription_guessing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("spend.sqlite3");
    let tracker = SpendTracker::open(db.clone()).unwrap();
    tracker.record_entry(&entry("1"));
    let mut sub = entry("2");
    sub.provider = Some("codex".into());
    sub.account_id = Some("acct_one".into());
    sub.billing = Some("subscription".into());
    tracker.record_entry(&sub);
    tracker.flush().await.unwrap();
    let report = generate_spend_report(&db, None, None).unwrap();
    assert_eq!(report.summary.requests, 2);
    assert_eq!(report.summary.total_tokens, 220_000);
    assert_eq!(report.by_account.len(), 2);
    assert_eq!(
        report
            .by_provider
            .iter()
            .find(|p| p.provider == "openai")
            .unwrap()
            .totals
            .requests,
        1
    );
    assert!((report.summary.api_value_usd.unwrap() - 1.56).abs() < 1e-8);
    assert_eq!(report.by_day.len(), 2);
    let filtered = generate_spend_report(&db, None, Some("codex")).unwrap();
    assert_eq!(filtered.summary.requests, 1);
}

#[tokio::test]
async fn unknown_prices_missing_usage_and_client_relays_are_not_fabricated() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("spend.sqlite3");
    let tracker = SpendTracker::open(db.clone()).unwrap();
    let mut unknown = entry("1");
    unknown.routed_model = Some("future-model".into());
    tracker.record_entry(&unknown);
    let mut missing = entry("2");
    missing.input_tokens = None;
    missing.output_tokens = None;
    tracker.record_entry(&missing);
    let mut relay = entry("3");
    relay.mode = "client".into();
    tracker.record_entry(&relay);
    tracker.flush().await.unwrap();
    let report = generate_spend_report(&db, None, None).unwrap();
    assert_eq!(report.summary.requests, 2);
    assert_eq!(report.summary.missing_usage_requests, 1);
    assert_eq!(report.summary.unpriced_requests, 1);
    assert_eq!(report.summary.api_value_usd, None);
}

#[tokio::test]
async fn storage_grows_with_buckets_not_requests_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("spend.sqlite3");
    let tracker = SpendTracker::open(db.clone()).unwrap();
    for _ in 0..20 {
        for n in 0..100 {
            tracker.record_entry(&entry(&n.to_string()));
        }
        tracker.flush().await.unwrap();
    }
    let conn = Connection::open(&db).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM buckets", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(tracker);
    assert_eq!(
        generate_spend_report(&db, None, None)
            .unwrap()
            .summary
            .requests,
        2000
    );
    assert!(std::fs::metadata(&db).unwrap().len() < 1024 * 1024);
}

#[test]
fn legacy_sources_remain_separate_and_are_compacted_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("spend.sqlite3");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TABLE spend_ledger(source TEXT,provider TEXT,model TEXT,timestamp_ms INTEGER,input_tokens INTEGER,cached_input_tokens INTEGER,cache_write_tokens INTEGER,cache_write_1h_tokens INTEGER,output_tokens INTEGER,reasoning_tokens INTEGER,cost_nano_usd INTEGER); INSERT INTO spend_ledger VALUES('proxy','codex','gpt-6-astra',0,100,80,0,0,10,5,100),('codex_cli','codex','gpt-6-astra',0,100,80,0,0,10,5,100);").unwrap();
    drop(conn);
    let mut conn = open_writer(&db).unwrap();
    initialize(&mut conn).unwrap();
    let report = generate_spend_report(&db, None, None).unwrap();
    assert_eq!(report.summary.requests, 0);
    assert_eq!(report.legacy_unverified.len(), 2);
    assert_eq!(
        report
            .legacy_unverified
            .iter()
            .map(|r| r.totals.requests)
            .sum::<u64>(),
        2
    );
    initialize(&mut conn).unwrap();
    assert_eq!(
        generate_spend_report(&db, None, None)
            .unwrap()
            .legacy_unverified
            .len(),
        2
    );
}

#[tokio::test]
async fn streaming_snapshots_and_repeated_completion_count_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let config = crate::config::Config::default();
    let store = crate::proxy::logs::Store::open(&config, &path).unwrap();
    let id = store.begin("POST", "/v1/messages", "HTTP");
    store.attribution(id, "claude", "acct_test", "work", "subscription");
    store.route(
        id,
        Some("claude-sonnet-4-6".into()),
        Some("claude-sonnet-4-6".into()),
        "claude",
    );
    store.usage(id,&serde_json::json!({"usage":{"input_tokens":100,"cache_read_input_tokens":500,"cache_creation_input_tokens":200,"output_tokens":1}}));
    store.usage(id, &serde_json::json!({"usage":{"output_tokens":30}}));
    store.complete(id, "succeeded", "stream_end", None, 0);
    store.complete(id, "succeeded", "client_disconnect", None, 0);
    store.spend.as_ref().unwrap().flush().await.unwrap();
    let report = generate_spend_report(&default_db_path(Some(&path)), None, None).unwrap();
    assert_eq!(report.summary.requests, 1);
    assert_eq!(report.summary.input_tokens, 800);
    assert_eq!(report.summary.output_tokens, 30);
    assert_eq!(report.summary.cached_input_tokens, 500);
    assert_eq!(report.by_account[0].billing, "subscription");
}

#[test]
fn retention_preserves_lifetime_totals_and_daily_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spend.sqlite3");
    let mut conn = open_writer(&path).unwrap();
    initialize(&mut conn).unwrap();
    for age in [0, 91, 731] {
        let mut e = entry("old");
        e.ended_ms = Some((now_ms() - age * DAY_MS) as u64);
        database::record(&conn, &Record::from_entry(&e).unwrap()).unwrap();
    }
    database::prune(&conn, now_ms()).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM buckets WHERE width=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM buckets WHERE width=24", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        generate_spend_report(&path, None, None)
            .unwrap()
            .summary
            .requests,
        3
    );
    assert_eq!(
        generate_spend_report(&path, Some(now_ms() - 100 * DAY_MS), None)
            .unwrap()
            .summary
            .requests,
        2
    );
}

#[test]
fn full_queue_is_nonblocking_and_reports_drops() {
    let (sender, _rx) = mpsc::sync_channel(1);
    let tracker = SpendTracker {
        sender,
        health: Arc::new(Health::default()),
    };
    let e = entry("busy");
    tracker.record_entry(&e);
    let start = std::time::Instant::now();
    for _ in 0..10_000 {
        tracker.record_entry(&e);
    }
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(tracker.health().queued_records, 1);
    assert_eq!(tracker.health().dropped_records, 10_000);
}

#[tokio::test]
async fn sqlite_lock_does_not_block_forwarding_and_losses_are_visible() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spend.sqlite3");
    let tracker = SpendTracker::open(path.clone()).unwrap();
    tracker.flush().await.unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let start = std::time::Instant::now();
    tracker.record_entry(&entry("locked"));
    assert!(start.elapsed() < Duration::from_millis(50));
    assert!(tracker.flush().await.is_err());
    assert_eq!(tracker.health().failed_records, 1);
    conn.execute_batch("ROLLBACK").unwrap();
    tracker.record_entry(&entry("recovered"));
    tracker.flush().await.unwrap();
    let report = generate_spend_report(&path, None, None).unwrap();
    assert_eq!(report.summary.requests, 1);
    assert_eq!(report.persisted_lost_records, 1);
}

#[tokio::test]
async fn native_gemini_accounts_for_cache_and_thinking() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let store = crate::proxy::logs::Store::open(&crate::config::Config::default(), &path).unwrap();
    let id = store.begin(
        "POST",
        "/v1beta/models/gemini-3.1-pro-preview:generateContent",
        "HTTP",
    );
    store.attribution(id, "gemini", "default", "default", "api");
    store.route(
        id,
        None,
        Some("gemini/gemini-3.1-pro-preview".into()),
        "gemini",
    );
    store.usage(id,&serde_json::json!({"usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":60,"candidatesTokenCount":5,"thoughtsTokenCount":15}}));
    store.complete(id, "succeeded", "stream_end", None, 0);
    store.spend.as_ref().unwrap().flush().await.unwrap();
    let r = generate_spend_report(&default_db_path(Some(&path)), None, None).unwrap();
    assert_eq!(r.summary.input_tokens, 100);
    assert_eq!(r.summary.output_tokens, 20);
    assert_eq!(r.summary.reasoning_tokens, 15);
    assert_eq!(r.summary.cached_input_tokens, 60);
    assert!(r.summary.api_value_usd.is_some());
}

#[test]
fn oldest_supported_window_rounds_to_utc_days_without_expiring_during_query() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spend.sqlite3");
    let mut conn = open_writer(&path).unwrap();
    initialize(&mut conn).unwrap();
    let since = now_ms() - 730 * DAY_MS - 1;
    let report = generate_spend_report(&path, Some(since), None).unwrap();
    assert_eq!(report.bucket_width_hours, 24);
    assert_eq!(report.since_timestamp_ms, Some(since / DAY_MS * DAY_MS));
}
