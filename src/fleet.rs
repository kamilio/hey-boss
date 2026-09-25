//! Supervisor/companion command surface and owner-private local control transport.
use clap::Subcommand;
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

mod native;

/// Apply or inspect this machine's saved workers, retaining their stable IDs.
pub fn auto_workers(apply: bool, config_only: bool) -> std::io::Result<Value> {
    native::auto_workers(apply, config_only)
}
pub fn add_auto_worker(
    settings: &crate::issues::worker::Settings,
    id: Option<&str>,
) -> std::io::Result<Value> {
    native::add_auto_worker(settings, id)
}
pub fn remove_auto_worker(id: &str) -> std::io::Result<Value> {
    native::remove_auto_worker(id)
}

// Persisted roles and protocol-v1 frames keep their original names so existing
// installations can upgrade without losing state or starting a second service.
pub const SUPERVISOR_ROLE: &str = "controller";
pub const COMPANION_ROLE: &str = "agent";

#[derive(Subcommand)]
pub enum Action {
    /// Read a known local agent conversation; requests arrive on stdin.
    #[command(hide = true)]
    Conversation,
    /// Owner-private database driver using the CLI's bundled SQLite.
    #[command(hide = true)]
    Database {
        #[arg(long)]
        path: PathBuf,
    },
    /// Install and start the fleet supervisor using the saved machine inventory.
    Setup {
        #[arg(long)]
        source: Option<PathBuf>,
    },
    /// Run the fleet supervisor (normally managed by launchd/systemd).
    #[command(alias = "controller")]
    Supervisor,
    /// Run the fleet companion that synchronizes this machine and manages workers.
    #[command(name = "companion", alias = "agent")]
    Companion {
        #[arg(long, hide = true)]
        stdio: bool,
        #[arg(long, hide = true, conflicts_with = "stdio")]
        install: bool,
    },
    /// Show machines, workers, and connectivity through the local fleet connection.
    Status,
    /// Queue a durable worker signal, including while its machine is offline.
    Signal {
        host: String,
        worker: String,
        #[arg(value_parser = ["pause", "resume", "stop", "restart"])]
        signal: String,
    },
}

pub fn socket_path() -> std::io::Result<PathBuf> {
    if let Some(state) = std::env::var_os("HEY_BOSS_FLEET_STATE") {
        return Ok(PathBuf::from(state).join("fleet.sock"));
    }
    Ok(PathBuf::from(
        std::env::var_os("HOME").ok_or_else(|| std::io::Error::other("HOME is missing"))?,
    )
    .join(".local/share/hey-boss/fleet.sock"))
}

/// Read the fleet's existing heartbeat without contacting the supervisor.
/// Missing or unreadable status is unknown, never evidence of a live connection.
pub fn worker_connection(role: &str, database: &rusqlite::Connection) -> Value {
    worker_connection_path(role, database.path())
}
pub(crate) fn worker_connection_path(role: &str, database: Option<&str>) -> Value {
    use serde_json::json;
    if role != COMPANION_ROLE && role != "companion" {
        return json!({"state": if role == SUPERVISOR_ROLE || role == "supervisor" { "local" } else { "standalone" }});
    }
    let status = socket_path()
        .ok()
        .and_then(|path| {
            let path = path.with_file_name("fleet-agent-status.json");
            if let Some(database) = database.filter(|path| !path.is_empty()) {
                crate::issues::planning::protect_database_paths(
                    std::path::Path::new(database),
                    [&path],
                )
                .ok()?;
            }
            std::fs::read(path).ok()
        })
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let Some(status) = status else {
        return json!({"state":"unknown"});
    };
    let Some(heartbeat) = status["connected_at"].as_f64() else {
        return json!({"state":"unknown"});
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or_default();
    // The supervisor pings every five seconds and times out after fifteen.
    let connected = (0.0..=15.0).contains(&(now - heartbeat));
    json!({"state": if connected { "connected" } else { "disconnected" }, "last_sync":status["last_sync"]})
}

pub fn call(value: &Value) -> crate::issues::Result<Value> {
    // Keep the supervisor's frequently polled path free of database discovery.
    // Companions have no supervisor socket; resolve their authority route there.
    let mut stream = match UnixStream::connect(socket_path()?) {
        Ok(stream) => stream,
        Err(_) => return native::request(value.clone()),
    };
    stream.set_read_timeout(Some(std::time::Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(15)))?;
    serde_json::to_writer(&mut stream, value)?;
    stream.write_all(b"\n")?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let bytes = native::read_control_body(&mut stream)?
        .ok_or_else(|| crate::issues::Error::invalid("Fleet response exceeds 16 MiB"))?;
    let result: Value = serde_json::from_slice(&bytes)?;
    if result["ok"] == false {
        return Err(crate::issues::Error::new(
            "fleet_error",
            result["error"].as_str().unwrap_or("Fleet request failed"),
        ));
    }
    if result["ok"] != true {
        return Err(crate::issues::Error::new(
            "fleet_unavailable",
            "Fleet supervisor returned an incomplete response",
        ));
    }
    Ok(result)
}

pub(crate) fn authoritative_resource(
    request: &crate::issues::Request,
    database: &std::path::Path,
) -> crate::issues::Result<Value> {
    native::resource(request, database)
}

pub fn subscribe() -> crate::issues::Result<UnixStream> {
    let mut stream = UnixStream::connect(socket_path()?)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(20)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
    stream.write_all(b"{\"kind\":\"subscribe\"}\n")?;
    stream.shutdown(std::net::Shutdown::Write)?;
    Ok(stream)
}

pub fn record_local_worker(
    id: &str,
    config: Option<&crate::issues::worker::Settings>,
    intent: &str,
) -> std::io::Result<()> {
    // Private issue databases must not write intent into the user's live fleet.
    if std::env::var_os("HEY_BOSS_ISSUE_DB").is_some()
        && std::env::var_os("HEY_BOSS_FLEET_STATE").is_none()
    {
        return Ok(());
    }
    if std::env::var("HEY_BOSS_FLEET_MANAGED").as_deref() == Ok("1") {
        return Ok(());
    }
    let mut found = None;
    for name in ["fleet-agent.json", "fleet-main.json"] {
        let path = socket_path()?.with_file_name(name);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let value: Value = serde_json::from_slice(&bytes)?;
                if value["role"] == COMPANION_ROLE || value["role"] == SUPERVISOR_ROLE {
                    found = Some((path, value));
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let Some((path, mut value)) = found else {
        return Ok(());
    };
    let base = value["revision"].clone();
    let Some(workers) = value["workers"].as_array_mut() else {
        return Ok(());
    };
    if !workers.iter().any(|w| w["id"] == id) {
        if let Some(config) = config {
            workers.push(serde_json::json!({"id":id,"config":config}));
        } else {
            return Ok(());
        }
    }
    let worker = workers.iter_mut().find(|w| w["id"] == id).unwrap();
    if let Some(config) = config {
        worker["config"] = serde_json::to_value(config)?;
    }
    worker["intent"] = serde_json::json!(intent);
    worker["local_revision"] = serde_json::json!(crate::issues::worker::now());
    worker["base_revision"] = base;
    let temporary = path.with_extension(format!("{}.new", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    serde_json::to_writer(&mut file, &value)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

pub fn run(action: &Action) -> std::io::Result<()> {
    // Match the previous fleet service: private replicas, journals and backups.
    unsafe {
        libc::umask(0o077);
    }
    if matches!(action, Action::Conversation) {
        let request: Value = serde_json::from_reader(std::io::stdin().take(65536))?;
        let run = request["run"]
            .as_str()
            .ok_or_else(|| std::io::Error::other("Missing agent"))?;
        let window: crate::agent_conversations::Window = serde_json::from_value(request.clone())?;
        let result = crate::agent_conversations::local_window(run, &window)
            .map_err(std::io::Error::other)?;
        println!("{result}");
        return Ok(());
    }
    if let Action::Database { path } = action {
        return database(path);
    }
    native::run(action)
}

fn database(path: &std::path::Path) -> std::io::Result<()> {
    use rusqlite::types::{Value as SqlValue, ValueRef};
    use std::io::BufRead;
    let connection = crate::database::maintenance(path).map_err(std::io::Error::other)?;
    connection
        .busy_timeout(std::time::Duration::from_secs(10))
        .map_err(std::io::Error::other)?;
    connection
        .execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
        .map_err(std::io::Error::other)?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut bytes = Vec::new();
        let length = input
            .by_ref()
            .take(crate::issues::WIRE_LIMIT as u64 + 1)
            .read_until(b'\n', &mut bytes)?;
        if length == 0 {
            break;
        }
        if length > crate::issues::WIRE_LIMIT {
            return Err(std::io::Error::other("Database frame exceeds limit"));
        }
        let result = (|| -> Result<Value, Box<dyn std::error::Error>> {
            let request: Value = serde_json::from_slice(&bytes)?;
            if let Some(destination) = request["backup"].as_str() {
                connection.backup("main", destination, None)?;
                return Ok(
                    serde_json::json!({"ok":true,"columns":[],"rows":[],"transaction":!connection.is_autocommit()}),
                );
            }
            if request.get("replica").is_some() {
                let value = native::replica_request(&connection, &request)
                    .map_err(|e| -> Box<dyn std::error::Error> { e })?;
                return Ok(
                    serde_json::json!({"ok":true,"columns":["result"],"rows":[[value.to_string()]],"transaction":!connection.is_autocommit()}),
                );
            }
            let sql = request["sql"].as_str().ok_or("SQL is missing")?;
            let values = request["args"]
                .as_array()
                .ok_or("Arguments are missing")?
                .iter()
                .map(|v| match v {
                    Value::Null => Ok(SqlValue::Null),
                    Value::Bool(v) => Ok(SqlValue::Integer(i64::from(*v))),
                    Value::Number(n) => n
                        .as_i64()
                        .map(SqlValue::Integer)
                        .or_else(|| n.as_f64().map(SqlValue::Real))
                        .ok_or("Invalid number"),
                    Value::String(v) => Ok(SqlValue::Text(v.clone())),
                    _ => Err("Unsupported database parameter"),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut statement = connection.prepare(sql)?;
            let columns: Vec<String> = statement
                .column_names()
                .into_iter()
                .map(str::to_owned)
                .collect();
            let mut rows = Vec::new();
            if columns.is_empty() {
                statement.execute(crate::database::params_from_iter(values))?;
            } else {
                let mut cursor = statement.query(crate::database::params_from_iter(values))?;
                while let Some(row) = cursor.next()? {
                    let mut values = Vec::new();
                    for index in 0..columns.len() {
                        values.push(match row.get_ref(index)? {
                            ValueRef::Null => Value::Null,
                            ValueRef::Integer(v) => Value::from(v),
                            ValueRef::Real(v) => Value::from(v),
                            ValueRef::Text(v) => Value::from(std::str::from_utf8(v)?),
                            ValueRef::Blob(_) => {
                                return Err("Fleet database blobs are unsupported".into());
                            }
                        });
                    }
                    rows.push(values);
                }
            }
            Ok(
                serde_json::json!({"ok":true,"columns":columns,"rows":rows,"transaction":!connection.is_autocommit()}),
            )
        })();
        let value = result.unwrap_or_else(|error| {
            let constraint = error.downcast_ref::<rusqlite::Error>().is_some_and(|e| matches!(e, rusqlite::Error::SqliteFailure(code, _) if code.code == rusqlite::ErrorCode::ConstraintViolation));
            serde_json::json!({"ok":false,"error":error.to_string(),"constraint":constraint,"transaction":!connection.is_autocommit()})
        });
        serde_json::to_writer(&mut output, &value)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}
