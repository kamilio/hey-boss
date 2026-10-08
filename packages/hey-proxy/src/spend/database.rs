use super::*;
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounting_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS series(
 id INTEGER PRIMARY KEY, source TEXT NOT NULL, provider TEXT NOT NULL, account TEXT NOT NULL,
 label TEXT NOT NULL, billing TEXT NOT NULL, model TEXT NOT NULL, price_model TEXT NOT NULL, price_version TEXT NOT NULL,
 UNIQUE(source,provider,account,billing,model,price_model,price_version));
CREATE TABLE IF NOT EXISTS buckets(
 series_id INTEGER NOT NULL REFERENCES series(id), width INTEGER NOT NULL, start INTEGER NOT NULL,
 requests INTEGER NOT NULL, reported INTEGER NOT NULL, unpriced INTEGER NOT NULL,
 input INTEGER NOT NULL, cached INTEGER NOT NULL, writes INTEGER NOT NULL, writes_1h INTEGER NOT NULL,
 output INTEGER NOT NULL, reasoning INTEGER NOT NULL, cost INTEGER NOT NULL,
 PRIMARY KEY(width,start,series_id)) WITHOUT ROWID;
";
pub(super) fn open_writer(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(2))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA journal_size_limit=4194304; PRAGMA wal_autocheckpoint=256;")?;
    Ok(conn)
}
pub(super) fn initialize(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch(SCHEMA)?;
    tx.execute(
        "INSERT OR IGNORE INTO accounting_meta VALUES('started_ms',?1)",
        [now_ms().to_string()],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO accounting_meta VALUES('lost_records','0')",
        [],
    )?;
    let legacy: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='spend_ledger')",
        [],
        |r| r.get(0),
    )?;
    if legacy {
        // Preserve old totals separately; model-name guesses and overlapping CLI/proxy
        // sources cannot retrospectively establish which subscription paid for them.
        tx.execute_batch("INSERT OR IGNORE INTO series(source,provider,account,label,billing,model,price_model,price_version)
            SELECT 'legacy_'||source,provider,'unknown','unknown','unknown','all','unknown','legacy' FROM spend_ledger GROUP BY source,provider;")?;
        for width in [0, 24] {
            let cutoff = if width == 0 {
                0
            } else {
                now_ms() / DAY_MS * DAY_MS - 730 * DAY_MS
            };
            tx.execute("INSERT INTO buckets SELECT s.id,?1,CASE WHEN ?1=0 THEN 0 ELSE (l.timestamp_ms/86400000)*86400000 END,
                count(*),count(*),count(*),sum(input_tokens),sum(cached_input_tokens),sum(cache_write_tokens),sum(cache_write_1h_tokens),sum(output_tokens),sum(reasoning_tokens),sum(cost_nano_usd)
                FROM spend_ledger l JOIN series s ON s.source='legacy_'||l.source AND s.provider=l.provider AND s.model='all'
                WHERE l.timestamp_ms>=?2 GROUP BY s.id,3
                ON CONFLICT(width,start,series_id) DO UPDATE SET requests=requests+excluded.requests,reported=reported+excluded.reported,unpriced=unpriced+excluded.unpriced,input=input+excluded.input,cached=cached+excluded.cached,writes=writes+excluded.writes,writes_1h=writes_1h+excluded.writes_1h,output=output+excluded.output,reasoning=reasoning+excluded.reasoning,cost=cost+excluded.cost",params![width,cutoff])?;
        }
        tx.execute_batch("DROP TABLE spend_ledger; DROP TABLE IF EXISTS sync_cursors; DROP TABLE IF EXISTS subscription_snapshots; DROP TABLE IF EXISTS spend_meta;
            INSERT OR REPLACE INTO accounting_meta VALUES('compact_pending','1');")?;
    }
    tx.commit()?;
    let compact: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounting_meta WHERE key='compact_pending')",
        [],
        |r| r.get(0),
    )?;
    if compact {
        // Once only, on the background writer. Reclaim the old per-request ledger.
        conn.execute_batch(
            "PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
        )?;
        conn.execute(
            "DELETE FROM accounting_meta WHERE key='compact_pending'",
            [],
        )?;
    }
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    conn.pragma_update(None, "max_page_count", MAX_DB_BYTES as i64 / page_size)?;
    Ok(())
}
pub(super) fn prune(conn: &Connection, now: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM buckets WHERE (width=1 AND start<?1) OR (width=24 AND start<?2)",
        params![
            now / HOUR_MS * HOUR_MS - 90 * DAY_MS,
            now / DAY_MS * DAY_MS - 730 * DAY_MS
        ],
    )?;
    Ok(())
}
pub(super) fn record(conn: &Connection, r: &Record) -> Result<()> {
    let price = r.price();
    let key = params![
        "proxy",
        r.provider,
        r.account,
        r.label,
        r.billing,
        r.model,
        price.price_model.as_deref().unwrap_or("unknown"),
        price.price_version
    ];
    conn.prepare_cached("INSERT OR IGNORE INTO series(source,provider,account,label,billing,model,price_model,price_version)
        SELECT ?1,?2,?3,?4,?5,?6,?7,?8 WHERE (SELECT count(*) FROM series) < 2048")?.execute(key)?;
    let id:i64=conn.prepare_cached("SELECT id FROM series WHERE source=?1 AND provider=?2 AND account=?3 AND billing=?5 AND model=?6 AND price_model=?7 AND price_version=?8")?.query_row(key,|row|row.get(0)).context("Accounting series limit reached")?;
    for value in [
        r.input.unwrap_or(0),
        r.output.unwrap_or(0),
        r.cached,
        r.writes,
        r.writes_1h,
        r.reasoning,
    ] {
        anyhow::ensure!(value <= i64::MAX as u64, "Token count is out of range");
    }
    let reported = r.input.is_some() && r.output.is_some();
    for width in [0, 1, 24] {
        let start = if width == 0 {
            0
        } else {
            r.timestamp / (width * HOUR_MS) * (width * HOUR_MS)
        };
        conn.prepare_cached("INSERT INTO buckets VALUES(?1,?2,?3,1,?4,?5,?6,?7,?8,?9,?10,?11,?12)
            ON CONFLICT(width,start,series_id) DO UPDATE SET requests=requests+1,reported=reported+excluded.reported,unpriced=unpriced+excluded.unpriced,input=input+excluded.input,cached=cached+excluded.cached,writes=writes+excluded.writes,writes_1h=writes_1h+excluded.writes_1h,output=output+excluded.output,reasoning=reasoning+excluded.reasoning,cost=cost+excluded.cost")?
            .execute(params![id,width,start,reported,reported && price.cost_nano_usd.is_none(),r.input.unwrap_or(0) as i64,r.cached as i64,r.writes as i64,r.writes_1h as i64,r.output.unwrap_or(0) as i64,r.reasoning as i64,price.cost_nano_usd.unwrap_or(0)])?;
    }
    Ok(())
}
