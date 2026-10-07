use super::super::*;
use super::store::now_ms;
use anyhow::Result;
use rusqlite::Connection;

const DAY_MS: u64 = 86_400_000;
const MINUTE_MS: u64 = 60_000;

// Fixed space, including when persistence is disabled or the writer is stalled.
// Only one bucket increment runs on arrival; summation runs on dashboard reads.
pub(super) struct Traffic([std::sync::atomic::AtomicU64; 60]);
impl Default for Traffic {
    fn default() -> Self {
        Self(std::array::from_fn(|_| {
            std::sync::atomic::AtomicU64::new(0)
        }))
    }
}
impl Traffic {
    pub fn record(&self, timestamp: u64) {
        use std::sync::atomic::Ordering::Relaxed;
        let second = timestamp / 1000;
        // One atomic word keeps rollover and increment indivisible. A delayed
        // recording cannot overwrite a newer bucket after the ring wraps.
        let slot = &self.0[(second % 60) as usize];
        // Rust 1.99 renamed this to try_update; retain the identical operation
        // so existing Rust 1.98 installations can still build the proxy.
        #[allow(deprecated)]
        let _ = slot.fetch_update(Relaxed, Relaxed, |value| {
            let recorded = value >> 32;
            if recorded > second {
                return None;
            }
            let count = if recorded == second {
                (value as u32).saturating_add(1)
            } else {
                1
            };
            Some((second << 32) | u64::from(count))
        });
    }
    pub fn count(&self, timestamp: u64) -> u64 {
        use std::sync::atomic::Ordering::Relaxed;
        let second = timestamp / 1000;
        self.0
            .iter()
            .map(|slot| slot.load(Relaxed))
            .filter(|value| {
                let recorded = value >> 32;
                recorded <= second && second - recorded < 60
            })
            .map(|value| u64::from(value as u32))
            .sum()
    }
}

fn eligible(row: &str) -> String {
    // Discovery, token counting and WebSocket handshakes do not generate billable
    // tokens. Keep these out of pricing coverage while RPM still counts HTTP calls.
    format!(
        "{row}.mode!='client' AND {row}.method IN ('POST','SEND') AND rtrim({row}.path,'/') NOT LIKE '%/count_tokens'"
    )
}

// Backfill once on the writer, never while binding the listener or forwarding.
// Triggers keep counts and integer costs atomic with request snapshots, including
// late usage, corrections and deletions. Bucket -1 is the constant-time all-time row.
pub(super) fn initialize(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS dashboard_totals (
        bucket INTEGER PRIMARY KEY, requests INTEGER NOT NULL, priced INTEGER NOT NULL,
        cost_nano_usd INTEGER NOT NULL);",
    )?;
    let ready: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='dashboard_minutes_v1')",
        [],
        |r| r.get(0),
    )?;
    if !ready {
        let include = eligible("r");
        transaction.execute_batch(&format!("DELETE FROM dashboard_totals;
            INSERT INTO dashboard_totals SELECT timestamp_ms/60000,count(*),
                sum(cost_nano_usd IS NOT NULL),coalesce(sum(cost_nano_usd),0)
                FROM requests r WHERE {include} GROUP BY 1;
            INSERT INTO dashboard_totals SELECT -1,count(*),coalesce(sum(cost_nano_usd IS NOT NULL),0),
                coalesce(sum(cost_nano_usd),0) FROM requests r WHERE {include};
            INSERT INTO metadata VALUES('dashboard_minutes_v1','1');"))?;
    }
    for (name, action, condition, changes) in [
        (
            "insert",
            "AFTER INSERT",
            eligible("NEW"),
            vec![("NEW", 1)],
        ),
        (
            "delete",
            "AFTER DELETE",
            eligible("OLD"),
            vec![("OLD", -1)],
        ),
        (
            "update",
            "AFTER UPDATE OF timestamp_ms,mode,cost_nano_usd,method,path",
            "OLD.timestamp_ms!=NEW.timestamp_ms OR OLD.mode!=NEW.mode OR OLD.method!=NEW.method OR OLD.path!=NEW.path OR OLD.cost_nano_usd IS NOT NEW.cost_nano_usd".into(),
            vec![("OLD", -1), ("NEW", 1)],
        ),
    ] {
        let mut body = String::new();
        for (row, sign) in changes {
            let include = eligible(row);
            for bucket in [format!("{row}.timestamp_ms/60000"), "-1".into()] {
                body.push_str(&format!("INSERT INTO dashboard_totals(bucket,requests,priced,cost_nano_usd)
                    SELECT {bucket},{sign},{sign}*({row}.cost_nano_usd IS NOT NULL),{sign}*coalesce({row}.cost_nano_usd,0)
                    WHERE {include}
                    ON CONFLICT(bucket) DO UPDATE SET requests=requests+excluded.requests,
                        priced=priced+excluded.priced,cost_nano_usd=cost_nano_usd+excluded.cost_nano_usd;"));
            }
        }
        transaction.execute_batch(&format!(
            "CREATE TRIGGER IF NOT EXISTS dashboard_{name}_v1
            {action} ON requests WHEN {condition} BEGIN {body} END;"
        ))?;
    }
    transaction.commit()?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Window {
    day_start_ms: u64,
    week_start_ms: u64,
}
impl Window {
    fn utc(now: u64) -> Self {
        let day = now / DAY_MS;
        Self {
            day_start_ms: day * DAY_MS,
            week_start_ms: day.saturating_sub((day + 3) % 7) * DAY_MS,
        }
    }
    fn validate(self, now: u64) -> Result<Self> {
        // Browser calendar arithmetic preserves local midnight across DST and
        // fractional-hour zones. A week read is always bounded to eight days.
        anyhow::ensure!(
            self.day_start_ms <= now
                && now - self.day_start_ms <= 27 * 3_600_000
                && self.week_start_ms <= self.day_start_ms
                && now - self.week_start_ms <= 8 * DAY_MS
                && self.day_start_ms.is_multiple_of(MINUTE_MS)
                && self.week_start_ms.is_multiple_of(MINUTE_MS),
            "Invalid calendar boundaries"
        );
        Ok(self)
    }
}
pub(super) type Cache = std::collections::VecDeque<(Window, Instant, Value)>;

#[cfg(test)]
pub(super) fn totals(connection: &Connection, now: u64) -> Result<Value> {
    totals_for(connection, now, Window::utc(now))
}
fn totals_for(connection: &Connection, now: u64, window: Window) -> Result<Value> {
    let mut result = json!({"snapshot_ms":now,"day_start_ms":window.day_start_ms,
        "week_start_ms":window.week_start_ms});
    for (name, start, end) in [
        (
            "today",
            (window.day_start_ms / MINUTE_MS) as i64,
            (now / MINUTE_MS) as i64,
        ),
        (
            "week",
            (window.week_start_ms / MINUTE_MS) as i64,
            (now / MINUTE_MS) as i64,
        ),
        ("all_time", -1, -1),
    ] {
        let (requests, priced, cost): (i64, i64, f64) = connection.query_row(
            "SELECT coalesce(sum(requests),0),coalesce(sum(priced),0),
                coalesce(total(cost_nano_usd),0)/1000000000.0 FROM dashboard_totals WHERE bucket>=?1 AND bucket<=?2",
            [start, end], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        result[name] = json!({"requests":requests,"priced_requests":priced,
            "unpriced_requests":requests-priced,
            "estimated_cost_usd":if requests == 0 || priced > 0 {Some(cost)} else {None}});
    }
    Ok(result)
}

pub(crate) async fn data(
    State(service): State<Arc<Service>>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, u64>>,
) -> Response {
    let now = now_ms();
    let window = if query.is_empty() {
        Window::utc(now)
    } else {
        let (Some(&day_start_ms), Some(&week_start_ms)) =
            (query.get("day_start_ms"), query.get("week_start_ms"))
        else {
            return error(
                StatusCode::BAD_REQUEST,
                "Both calendar boundaries are required",
            );
        };
        let Ok(window) = (Window {
            day_start_ms,
            week_start_ms,
        })
        .validate(now) else {
            return error(StatusCode::BAD_REQUEST, "Invalid calendar boundaries");
        };
        if query.len() != 2 {
            return error(StatusCode::BAD_REQUEST, "Unknown dashboard parameter");
        }
        window
    };
    let proxy = service.snapshot();
    if proxy.config.mode == Mode::Client {
        return relay(&proxy, window).await;
    }
    let logging = service
        .logs
        .database
        .as_ref()
        .map(|db| db.health())
        .unwrap_or_else(|| json!({"enabled":false,"status":"disabled"}));
    let mut value =
        json!({"rpm":service.logs.rpm(),"logging":logging,"spend":null,"source":"local"});
    if let Some(database) = &service.logs.database {
        // Independent of the request cache. Single flight and bounded to eight
        // calendar windows even when readers are in different time zones.
        let mut cache = service.logs.dashboard.lock().await;
        if let Some((_, _, spend)) = cache
            .iter()
            .find(|(key, at, _)| *key == window && at.elapsed() < Duration::from_secs(5))
        {
            value["spend"] = spend.clone();
        } else {
            match database
                .read(move |connection| totals_for(connection, now, window))
                .await
            {
                Ok(spend) => {
                    cache.retain(|(key, _, _)| *key != window);
                    if cache.len() == 8 {
                        cache.pop_front();
                    }
                    cache.push_back((window, Instant::now(), spend.clone()));
                    value["spend"] = spend;
                }
                Err(_) => {
                    // Retain the same calendar window's last successful values.
                    // The error and original snapshot time make staleness clear.
                    if let Some((_, _, spend)) = cache.iter().find(|(key, _, _)| *key == window) {
                        value["spend"] = spend.clone();
                    }
                    value["error"] =
                        json!("Spend history is temporarily unavailable. Retrying automatically.")
                }
            }
        }
    }
    ([(header::CACHE_CONTROL, "no-store")], axum::Json(value)).into_response()
}

async fn relay(proxy: &Proxy, window: Window) -> Response {
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let source = proxy
            .config
            .api_keys
            .get(&proxy.config.default.api_key)
            .ok_or_else(|| anyhow::anyhow!("Host credential is missing"))?;
        let token = proxy
            .service
            .credentials
            .resolve(
                source,
                Duration::from_secs(proxy.config.credential_cache_seconds),
            )
            .await?;
        let mut response = proxy
            .client
            .get(format!("{}/logs/api/dashboard", proxy.config.upstream_url))
            .bearer_auth(token.to_str()?)
            .query(&[
                ("day_start_ms", window.day_start_ms),
                ("week_start_ms", window.week_start_ms),
            ])
            .send()
            .await?
            .error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= 65_536,
                "Host dashboard is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let mut value: Value = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            value["rpm"].is_u64() && value["logging"].is_object(),
            "Invalid host dashboard"
        );
        value["source"] = json!("host");
        Ok::<_, anyhow::Error>(value)
    })
    .await;
    match result {
        Ok(Ok(value)) => ([(header::CACHE_CONTROL, "no-store")], axum::Json(value)).into_response(),
        _ => error(
            StatusCode::BAD_GATEWAY,
            "Host dashboard unavailable; retry shortly",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpm_is_bounded_by_time_not_the_recent_request_limit() {
        let traffic = Traffic::default();
        for _ in 0..10_000 {
            traffic.record(1_000);
        }
        traffic.record(60_999);
        assert_eq!(traffic.count(60_999), 10_001);
        assert_eq!(traffic.count(61_000), 1);
        traffic.record(61_000);
        assert_eq!(traffic.count(61_000), 2);
        assert_eq!(traffic.count(121_000), 0);
    }

    #[test]
    fn local_calendar_boundaries_cover_dst_and_fractional_hour_offsets() {
        for (now, day_start_ms, week_start_ms) in [
            (1_793_597_400_000, 1_793_509_200_000, 1_792_990_800_000), // Chicago, 25-hour day
            (1_773_030_600_000, 1_772_949_600_000, 1_772_431_200_000), // Chicago, 23-hour day
            (1_790_942_400_000, 1_790_878_500_000, 1_790_532_900_000), // Kathmandu, UTC+05:45
        ] {
            let window = Window {
                day_start_ms,
                week_start_ms,
            }
            .validate(now)
            .unwrap();
            let mut connection = Connection::open_in_memory().unwrap();
            connection.execute_batch("CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT);
                CREATE TABLE requests(id INTEGER PRIMARY KEY,timestamp_ms INTEGER,mode TEXT,cost_nano_usd INTEGER,method TEXT DEFAULT 'POST',path TEXT DEFAULT '/v1/responses');").unwrap();
            initialize(&mut connection).unwrap();
            for (id, at, cost) in [
                (1, day_start_ms - 1, 1),
                (2, day_start_ms, 2),
                (3, now, 4),
                (4, week_start_ms - 1, 8),
                (5, week_start_ms, 16),
            ] {
                connection
                    .execute(
                        "INSERT INTO requests(id,timestamp_ms,mode,cost_nano_usd) VALUES(?1,?2,'host',?3)",
                        rusqlite::params![id, at as i64, cost * 1_000_000_000i64],
                    )
                    .unwrap();
            }
            let value = totals_for(&connection, now, window).unwrap();
            assert_eq!(value["today"]["estimated_cost_usd"], 6.0);
            assert_eq!(value["week"]["estimated_cost_usd"], 23.0);
            assert_eq!(value["all_time"]["estimated_cost_usd"], 31.0);
            assert!(
                Window {
                    day_start_ms: now + 1,
                    week_start_ms
                }
                .validate(now)
                .is_err()
            );
            assert!(
                Window {
                    day_start_ms,
                    week_start_ms: now - 9 * DAY_MS
                }
                .validate(now)
                .is_err()
            );
        }
    }

    #[test]
    fn concurrent_rpm_recording_has_no_lost_increments_or_stale_rollover() {
        let traffic = std::sync::Arc::new(Traffic::default());
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let traffic = traffic.clone();
                scope.spawn(move || {
                    for _ in 0..10_000 {
                        traffic.record(120_000);
                    }
                });
            }
        });
        assert_eq!(traffic.count(120_999), 80_000);
        traffic.record(60_000); // Delayed old arrival must not overwrite the current slot.
        assert_eq!(traffic.count(120_999), 80_000);
        traffic.record(180_000);
        assert_eq!(traffic.count(180_000), 1);
    }

    #[test]
    fn migration_and_late_usage_keep_calendar_totals_exact() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT);
            CREATE TABLE requests(id INTEGER PRIMARY KEY,timestamp_ms INTEGER,mode TEXT,cost_nano_usd INTEGER,method TEXT DEFAULT 'POST',path TEXT DEFAULT '/v1/responses');
            INSERT INTO requests(id,timestamp_ms,mode,cost_nano_usd) VALUES(1,0,'standalone',1000000000),(2,345600000,'host',NULL),
                (3,432000000,'standalone',0),(4,432000000,'client',9000000000);").unwrap();
        initialize(&mut connection).unwrap();
        let check =
            |connection: &Connection, day: u64, today: i64, week: i64, all: i64, cost: f64| {
                let value = totals(connection, day * DAY_MS).unwrap();
                assert_eq!(value["today"]["requests"], today);
                assert_eq!(value["week"]["requests"], week);
                assert_eq!(value["all_time"]["requests"], all);
                assert_eq!(value["all_time"]["estimated_cost_usd"], cost);
            };
        check(&connection, 5, 1, 2, 3, 1.0); // Tuesday, Monday boundary excludes Thursday.
        check(&connection, 11, 0, 0, 3, 1.0); // Next Monday.
        let unknown = totals(&connection, 4 * DAY_MS).unwrap();
        assert_eq!(unknown["today"]["estimated_cost_usd"], Value::Null);
        assert_eq!(unknown["today"]["unpriced_requests"], 1);
        connection
            .execute_batch(
                "UPDATE requests SET cost_nano_usd=2500000000 WHERE id=2;
            UPDATE requests SET cost_nano_usd=2500000000 WHERE id=2;",
            )
            .unwrap();
        check(&connection, 5, 1, 2, 3, 3.5);
        initialize(&mut connection).unwrap(); // Restart is idempotent, no repricing.
        check(&connection, 5, 1, 2, 3, 3.5);
        connection
            .execute_batch(
                "UPDATE requests SET timestamp_ms=432000000 WHERE id=2;
            UPDATE requests SET mode='client' WHERE id=1;
            DELETE FROM requests WHERE id=3;",
            )
            .unwrap();
        check(&connection, 5, 1, 1, 1, 2.5);
        connection
            .execute_batch(
                "BEGIN; INSERT INTO requests(id,timestamp_ms,mode,cost_nano_usd) VALUES(5,432000000,'host',7000000000); ROLLBACK;",
            )
            .unwrap();
        check(&connection, 5, 1, 1, 1, 2.5);
        // Backfill from an older database must produce the same retained totals.
        connection
            .execute_batch("DELETE FROM metadata WHERE key='dashboard_minutes_v1';")
            .unwrap();
        initialize(&mut connection).unwrap();
        check(&connection, 5, 1, 1, 1, 2.5);
        let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT sum(requests) FROM dashboard_totals WHERE bucket>=4 AND bucket<=5", [], |r| r.get(3)).unwrap();
        assert!(plan.contains("SEARCH"), "{plan}");
    }
}
