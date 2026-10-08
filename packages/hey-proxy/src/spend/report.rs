use super::*;
use rusqlite::OpenFlags;
use std::{collections::BTreeMap, fmt::Write};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Totals {
    pub requests: u64,
    pub missing_usage_requests: u64,
    pub unpriced_requests: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_write_1h_tokens: u64,
    pub total_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// Sum of the priced portion. A partial estimate if any request is unpriced/missing.
    pub priced_api_value_usd: f64,
    /// None when any usage or price is missing.
    pub api_value_usd: Option<f64>,
}
impl Totals {
    fn add(&mut self, t: &Self) {
        self.requests += t.requests;
        self.missing_usage_requests += t.missing_usage_requests;
        self.unpriced_requests += t.unpriced_requests;
        self.input_tokens += t.input_tokens;
        self.cached_input_tokens += t.cached_input_tokens;
        self.cache_write_tokens += t.cache_write_tokens;
        self.cache_write_1h_tokens += t.cache_write_1h_tokens;
        self.output_tokens += t.output_tokens;
        self.total_tokens = self.input_tokens + self.output_tokens;
        self.reasoning_tokens += t.reasoning_tokens;
        self.priced_api_value_usd += t.priced_api_value_usd;
        self.api_value_usd = (self.missing_usage_requests == 0 && self.unpriced_requests == 0)
            .then_some(self.priced_api_value_usd);
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Breakdown {
    pub provider: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub account_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub account_label: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub billing: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source: String,
    #[serde(flatten)]
    pub totals: Totals,
}
#[derive(Serialize, Deserialize)]
pub struct Day {
    pub source: String,
    pub start_ms: i64,
    pub provider: String,
    pub billing: String,
    #[serde(flatten)]
    pub totals: Totals,
}
#[derive(Serialize, Deserialize)]
pub struct SpendReport {
    pub schema_version: u32,
    pub generated_at_ms: i64,
    pub collection_started_ms: i64,
    pub since_timestamp_ms: Option<i64>,
    pub bucket_width_hours: u32,
    pub retention: String,
    pub coverage: String,
    pub persisted_lost_records: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub writer: Option<WriterHealth>,
    pub summary: Totals,
    pub by_provider: Vec<Breakdown>,
    pub by_account: Vec<Breakdown>,
    pub by_model: Vec<Breakdown>,
    pub by_day: Vec<Day>,
    pub legacy_unverified: Vec<Breakdown>,
    pub legacy_by_day: Vec<Day>,
}
const SUMS: &str = "sum(b.requests),sum(b.requests-b.reported),sum(b.unpriced),sum(b.input),sum(b.cached),sum(b.writes),sum(b.writes_1h),sum(b.output),sum(b.reasoning),sum(b.cost)";
fn totals(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Totals> {
    let mut t = Totals {
        requests: row.get::<_, i64>(offset)? as u64,
        missing_usage_requests: row.get::<_, i64>(offset + 1)? as u64,
        unpriced_requests: row.get::<_, i64>(offset + 2)? as u64,
        input_tokens: row.get::<_, i64>(offset + 3)? as u64,
        cached_input_tokens: row.get::<_, i64>(offset + 4)? as u64,
        cache_write_tokens: row.get::<_, i64>(offset + 5)? as u64,
        cache_write_1h_tokens: row.get::<_, i64>(offset + 6)? as u64,
        output_tokens: row.get::<_, i64>(offset + 7)? as u64,
        reasoning_tokens: row.get::<_, i64>(offset + 8)? as u64,
        priced_api_value_usd: row.get::<_, i64>(offset + 9)? as f64 / 1e9,
        total_tokens: 0,
        api_value_usd: None,
    };
    t.total_tokens = t.input_tokens + t.output_tokens;
    if t.missing_usage_requests == 0 && t.unpriced_requests == 0 {
        t.api_value_usd = Some(t.priced_api_value_usd);
    }
    Ok(t)
}
pub fn generate_spend_report(
    path: &Path,
    since: Option<i64>,
    provider: Option<&str>,
) -> Result<SpendReport> {
    let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("No accounting database; start the updated proxy first")?;
    conn.busy_timeout(Duration::from_millis(250))?;
    let tx = conn.transaction()?;
    let now = now_ms();
    let width = match since {
        None => 0,
        Some(since) if since >= now / HOUR_MS * HOUR_MS - 90 * DAY_MS => 1,
        Some(_) => 24,
    };
    if let Some(since) = since {
        anyhow::ensure!(
            since >= now / DAY_MS * DAY_MS - 730 * DAY_MS,
            "Dated totals are retained for 730 days; use --all for lifetime totals"
        );
    }
    let cutoff = since
        .map(|s| s / (width * HOUR_MS) * (width * HOUR_MS))
        .unwrap_or(0);
    let started: String = tx
        .query_row(
            "SELECT value FROM accounting_meta WHERE key='started_ms'",
            [],
            |r| r.get(0),
        )
        .context("Accounting is initializing; retry shortly")?;
    let lost: String = tx.query_row(
        "SELECT value FROM accounting_meta WHERE key='lost_records'",
        [],
        |r| r.get(0),
    )?;
    let mut statement=tx.prepare(&format!("SELECT s.provider,s.account,s.label,s.billing,s.model,s.source,{SUMS} FROM buckets b JOIN series s ON s.id=b.series_id
        WHERE ((s.source='proxy' AND b.width=?1 AND b.start>=?2) OR (s.source!='proxy' AND b.width=0)) AND (?3 IS NULL OR s.provider=?3)
        GROUP BY s.id"))?;
    let rows = statement.query_map(params![width, cutoff, provider], |r| {
        Ok(Breakdown {
            provider: r.get(0)?,
            account_id: r.get(1)?,
            account_label: r.get(2)?,
            billing: r.get(3)?,
            model: r.get(4)?,
            source: r.get(5)?,
            totals: totals(r, 6)?,
        })
    })?;
    let mut summary = Totals {
        api_value_usd: Some(0.0),
        ..Totals::default()
    };
    let mut providers: BTreeMap<String, Breakdown> = BTreeMap::new();
    let mut accounts: BTreeMap<(String, String, String), Breakdown> = BTreeMap::new();
    let mut models: BTreeMap<(String, String, String), Breakdown> = BTreeMap::new();
    let mut legacy = Vec::new();
    for row in rows {
        let row = row?;
        if row.source != "proxy" {
            legacy.push(row);
            continue;
        }
        summary.add(&row.totals);
        providers
            .entry(row.provider.clone())
            .or_insert_with(|| Breakdown {
                provider: row.provider.clone(),
                ..Breakdown::default()
            })
            .totals
            .add(&row.totals);
        accounts
            .entry((
                row.provider.clone(),
                row.account_id.clone(),
                row.billing.clone(),
            ))
            .or_insert_with(|| Breakdown {
                provider: row.provider.clone(),
                account_id: row.account_id.clone(),
                account_label: row.account_label.clone(),
                billing: row.billing.clone(),
                ..Breakdown::default()
            })
            .totals
            .add(&row.totals);
        models
            .entry((row.provider.clone(), row.model.clone(), row.billing.clone()))
            .or_insert_with(|| Breakdown {
                provider: row.provider.clone(),
                model: row.model.clone(),
                billing: row.billing.clone(),
                ..Breakdown::default()
            })
            .totals
            .add(&row.totals);
    }
    let mut days=tx.prepare(&format!("SELECT b.start,s.provider,s.billing,s.source,{SUMS} FROM buckets b JOIN series s ON s.id=b.series_id WHERE b.width=24 AND b.start>=?1 AND (?2 IS NULL OR s.provider=?2) GROUP BY b.start,s.provider,s.billing,s.source ORDER BY b.start,s.provider,s.billing"))?;
    let daily_cutoff = since.unwrap_or(now - 730 * DAY_MS) / DAY_MS * DAY_MS;
    let by_day = days
        .query_map(params![daily_cutoff, provider], |r| {
            Ok(Day {
                start_ms: r.get(0)?,
                provider: r.get(1)?,
                billing: r.get(2)?,
                source: r.get(3)?,
                totals: totals(r, 4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (by_day, legacy_by_day) = by_day.into_iter().partition(|d| d.source == "proxy");
    Ok(SpendReport{schema_version:2,generated_at_ms:now,collection_started_ms:started.parse()?,since_timestamp_ms:since.map(|_|cutoff),bucket_width_hours:width as u32,
        retention:"90 days hourly; 730 days daily; lifetime totals. UTC bucket boundaries; current buckets are partial.".into(),
        coverage:"Completed requests observed by this proxy only, bucketed by completion time; in-flight requests and unflushed records at process termination are excluded; subscription transport is identified by credentials, not model. Provider-side retries, traffic elsewhere, and unreported usage are not inferred. API value is an estimate using the embedded price book, not subscription charges. Cached input and reasoning are subsets. Legacy totals are separate and unverified.".into(),
        persisted_lost_records:lost.parse()?,writer:None,summary,by_provider:providers.into_values().collect(),by_account:accounts.into_values().collect(),by_model:models.into_values().collect(),by_day,legacy_unverified:legacy,legacy_by_day})
}
pub fn render_spend_report(r: &SpendReport) -> String {
    let mut out = String::new();
    let value = |t: &Totals| {
        t.api_value_usd
            .map(|v| format!("${v:.2}"))
            .unwrap_or_else(|| format!("${:.2} priced portion", t.priced_api_value_usd))
    };
    let _ = writeln!(
        out,
        "Proxy usage · {}\n{} requests · {} input · {} output · API value {}",
        if r.since_timestamp_ms.is_none() {
            "lifetime"
        } else {
            "selected UTC buckets"
        },
        r.summary.requests,
        r.summary.input_tokens,
        r.summary.output_tokens,
        value(&r.summary)
    );
    let _ = writeln!(
        out,
        "\nProvider / account / billing                 Input       Output     API value"
    );
    for a in &r.by_account {
        let _ = writeln!(
            out,
            "{} / {} / {}: {} / {} / {}",
            a.provider,
            a.account_label,
            a.billing,
            a.totals.input_tokens,
            a.totals.output_tokens,
            value(&a.totals)
        );
    }
    let _ = writeln!(
        out,
        "\nCache reads: {} · cache writes: {} · reasoning: {}\nMissing usage: {} requests · unpriced: {} requests",
        r.summary.cached_input_tokens,
        r.summary.cache_write_tokens,
        r.summary.reasoning_tokens,
        r.summary.missing_usage_requests,
        r.summary.unpriced_requests
    );
    if !r.legacy_unverified.is_empty() {
        let _ = writeln!(
            out,
            "Legacy history preserved separately in JSON; excluded because sources overlap and billing identity is unknown."
        );
    }
    if r.persisted_lost_records > 0
        || r.writer
            .as_ref()
            .is_some_and(|w| !w.ready || w.failed_records + w.dropped_records > 0)
    {
        let _ = writeln!(
            out,
            "Accounting gaps detected; inspect --json writer health and persisted_lost_records."
        );
    }
    let _ = writeln!(out, "{}\n{}", r.retention, r.coverage);
    out
}
