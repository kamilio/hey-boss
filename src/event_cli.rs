use clap::{Args, Subcommand};
use hey_boss::{
    database::Connection,
    event_log::{self, Filter, Record},
    issues::{self, Error, Result},
};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Args)]
pub struct Options {
    #[command(flatten)]
    query: Query,
    /// JSON pages, or newline-delimited records when following.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Args)]
struct Query {
    /// Exact event type or namespace, e.g. 'agent.*'.
    #[arg(long = "type", global = true)]
    kind: Option<String>,
    /// Fleet node ID, configured host name, or local.
    #[arg(long, global = true)]
    machine: Option<String>,
    /// Existing project name or ID; omitted means all projects and machines.
    #[arg(long, global = true)]
    project: Option<String>,
    /// Exact run, job, filesystem, or other subject identity.
    #[arg(long, global = true)]
    subject: Option<String>,
    /// Event time: RFC3339 or an age such as 30m, 2h, 7d.
    #[arg(long, global = true)]
    since: Option<String>,
    /// Resume from a cursor produced by this replica.
    #[arg(long, global = true)]
    after: Option<String>,
    /// Page size; default shows the latest 50, or reads forward with --since/--after.
    #[arg(long, global = true, default_value_t = 50)]
    limit: usize,
}

#[derive(Subcommand)]
enum Action {
    /// Read durable events available on this replica, including received fleet events.
    List,
    /// Print recent events, then newly arriving events. Ctrl+C stops cleanly.
    Follow,
    /// Read one complete event and its structured evidence.
    Show { id: String },
    /// List tracked event types, retained counts, and latest occurrence times.
    Types,
}

fn since(value: &str) -> Result<i64> {
    if let Ok(time) = value.parse::<jiff::Timestamp>() {
        return Ok(time.as_millisecond());
    }
    let (digits, scale) = [('m', 60_000), ('h', 3_600_000), ('d', 86_400_000)]
        .into_iter()
        .find_map(|(unit, scale)| value.strip_suffix(unit).map(|digits| (digits, scale)))
        .ok_or_else(|| Error::invalid("Use an RFC3339 time or an age such as 30m, 2h, 7d"))?;
    let delta = digits
        .parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .and_then(|v| v.checked_mul(scale))
        .ok_or_else(|| Error::invalid("Invalid event age"))?;
    Ok(jiff::Timestamp::now()
        .as_millisecond()
        .saturating_sub(delta)
        .max(0))
}

fn machines(db: &Connection) -> Result<Vec<(String, String, String)>> {
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='fleet_state')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(vec![]);
    }
    let saved: Option<String> = db
        .query_row(
            "SELECT value FROM fleet_state WHERE key='machines'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(saved) = saved else {
        return Ok(vec![]);
    };
    let saved: Value = serde_json::from_str(&saved)?;
    Ok(saved
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(host, value)| {
            Some((
                value["node"].as_str()?.into(),
                host.clone(),
                value["hostname"].as_str().unwrap_or(host).into(),
            ))
        })
        .collect())
}

fn project(db: &Connection, value: &str) -> Result<String> {
    let ids=db.prepare("SELECT id FROM projects WHERE id=?1 OR name=?1 COLLATE NOCASE ORDER BY id=?1 DESC LIMIT 2")?.query_map([value],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if ids.len() > 1 && ids[0] != value {
        return Err(Error::invalid("Project name is ambiguous; use its full ID"));
    }
    Ok(ids.into_iter().next().unwrap_or_else(|| value.into()))
}

fn write(text: &str) -> Result<()> {
    let mut output = io::stdout().lock();
    if let Err(error) = writeln!(output, "{text}").and_then(|_| output.flush()) {
        if error.kind() == io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        return Err(error.into());
    }
    Ok(())
}

fn time(value: i64) -> String {
    jiff::Timestamp::from_millisecond(value)
        .map(|t| t.to_string())
        .unwrap_or_else(|_| value.to_string())
}

fn text(record: &Record, machines: &[(String, String, String)]) -> String {
    let event = &record.event;
    let machine = machines
        .iter()
        .find(|(node, _, _)| node == &event.origin)
        .map(|(_, host, _)| host.as_str())
        .unwrap_or(&event.origin);
    let detail = event.data.to_string();
    let short: String = detail.chars().take(160).collect();
    format!(
        "{}  {}  {}  {}  {}{}",
        time(event.time),
        machine,
        event.kind,
        event.subject,
        short,
        if detail.chars().count() > 160 {
            "…"
        } else {
            ""
        }
    )
}

pub fn run(options: &Options) -> Result<()> {
    let db = event_log::open(&issues::database_path()?)?;
    let machines = machines(&db)?;
    let query = &options.query;
    let machine = query
        .machine
        .as_ref()
        .map(|wanted| {
            if wanted == "local" {
                return issues::identity::machine();
            }
            let matches: Vec<_> = machines
                .iter()
                .filter(|(node, host, hostname)| {
                    wanted == node || wanted == host || wanted == hostname
                })
                .collect();
            if matches.len() > 1 {
                return Err(Error::invalid(
                    "Machine name is ambiguous; use its fleet node ID",
                ));
            }
            Ok(matches
                .first()
                .map(|m| m.0.clone())
                .unwrap_or_else(|| wanted.clone()))
        })
        .transpose()?;
    let filter = Filter {
        kind: query.kind.clone(),
        machine,
        project: query
            .project
            .as_deref()
            .map(|p| project(&db, p))
            .transpose()?,
        subject: query.subject.clone(),
        since: query.since.as_deref().map(since).transpose()?,
    };
    match options.action.as_ref().unwrap_or(&Action::List) {
        Action::Show { id } => {
            let event = event_log::get(&db, id)?;
            write(&serde_json::to_string_pretty(&event)?)?;
        }
        Action::Types => {
            if query.after.is_some() {
                return Err(Error::invalid(
                    "Use --after with events list or events follow",
                ));
            }
            let rows = event_log::types(&db, &filter)?;
            if options.json {
                write(&serde_json::to_string(&rows)?)?;
            } else if rows.is_empty() {
                write("No event types observed on this replica yet.")?;
            } else {
                for row in rows {
                    write(&format!(
                        "{}  {} retained  last {} · {}",
                        row["type"].as_str().unwrap(),
                        row["count"],
                        row["last_seen"]
                            .as_i64()
                            .map(time)
                            .unwrap_or_else(|| "never".into()),
                        row["description"].as_str().unwrap_or("")
                    ))?;
                }
            }
        }
        action => {
            let follow = matches!(action, Action::Follow);
            let stopping = Arc::new(AtomicBool::new(false));
            if follow {
                let stopped = stopping.clone();
                ctrlc::set_handler(move || stopped.store(true, Ordering::Relaxed))
                    .map_err(|e| Error::new("signal", e.to_string()))?;
            }
            let mut after = query.after.clone();
            let mut first = true;
            loop {
                let page = event_log::read(&db, &filter, after.as_deref(), query.limit)?;
                if options.json {
                    if follow {
                        for event in &page.events {
                            write(&serde_json::to_string(event)?)?;
                        }
                        // An empty/filtered page still advances the reader. A
                        // checkpoint record also reports replica coverage.
                        if first || after.as_deref() != Some(page.cursor.as_str()) {
                            write(&json!({"checkpoint":page.cursor,"coverage":page.coverage,"expired_through":page.expired_through}).to_string())?;
                        }
                    } else {
                        write(&serde_json::to_string(&page)?)?;
                    }
                } else {
                    if first {
                        write(&format!(
                            "Local replica · {} awaiting publication · disconnected peers may have newer events",
                            page.coverage["pending_publication"]
                        ))?;
                    }
                    for event in &page.events {
                        write(&text(event, &machines))?;
                    }
                    if page.events.is_empty() && first {
                        write("No matching events.")?;
                    }
                    if !follow {
                        write(&format!(
                            "Cursor: {}{}",
                            page.cursor,
                            if page.has_more {
                                " · more available with --after"
                            } else {
                                ""
                            }
                        ))?;
                    }
                    if first && page.expired_through.is_some() {
                        write("Older event payloads have expired; this is retained history.")?;
                    }
                }
                if !follow || stopping.load(Ordering::Relaxed) {
                    break;
                }
                after = Some(page.cursor);
                first = false;
                if !page.has_more {
                    for _ in 0..10 {
                        if stopping.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                if stopping.load(Ordering::Relaxed) {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn age_and_timestamp_inputs_reject_invalid_or_overflowing_values() {
        assert_eq!(super::since("2026-01-01T00:00:00Z").unwrap(), 1767225600000);
        assert!(super::since("2h").unwrap() < jiff::Timestamp::now().as_millisecond());
        for value in ["", "0h", "-3h", "999999999999999999d", "tomorrow", "🦀"] {
            assert!(super::since(value).is_err(), "{value}");
        }
    }
}
