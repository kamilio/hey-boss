//! Controller/agent command surface and owner-private local control transport.
use clap::Subcommand;
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum Action {
    /// Owner-private database driver using the CLI's bundled SQLite.
    #[command(hide = true)]
    Database {
        #[arg(long)]
        path: PathBuf,
    },
    /// Install and start the automatic controller using the saved machine inventory.
    Setup {
        #[arg(long)]
        source: Option<PathBuf>,
    },
    /// Run the controller (normally managed by launchd/systemd).
    Controller,
    /// Run the durable local agent (normally managed automatically).
    Agent {
        #[arg(long, hide = true)]
        stdio: bool,
        #[arg(long, hide = true, conflicts_with = "stdio")]
        install: bool,
    },
    /// Show all machines, workers, connectivity, and pending changes.
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

pub fn call(value: &Value) -> crate::issues::Result<Value> {
    let mut stream = UnixStream::connect(socket_path()?).map_err(|e| {
        crate::issues::Error::new(
            "fleet_unavailable",
            format!("Fleet controller is unavailable: {e}. Run hey-boss fleet setup."),
        )
    })?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(15)))?;
    serde_json::to_writer(&mut stream, value)?;
    stream.write_all(b"\n")?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut bytes = Vec::new();
    stream
        .take(crate::issues::WIRE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > crate::issues::WIRE_LIMIT {
        return Err(crate::issues::Error::invalid(
            "Fleet response exceeds 16 MiB",
        ));
    }
    let result: Value = serde_json::from_slice(&bytes)?;
    if result["ok"] == false {
        return Err(crate::issues::Error::new(
            "fleet_error",
            result["error"].as_str().unwrap_or("Fleet request failed"),
        ));
    }
    Ok(result)
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
                if value["role"] == "agent" || value["role"] == "controller" {
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
    if let Action::Database { path } = action {
        return database(path);
    }
    let mut command = std::process::Command::new("python3");
    command.args(["-c", include_str!("../tools/fleet_hey_boss.py")]);
    command.env(
        "HEY_BOSS_FLEET_BINARY",
        std::env::current_exe()?.canonicalize()?,
    );
    match action {
        Action::Database { .. } => unreachable!(),
        Action::Setup { source } => {
            command.arg("setup");
            if let Some(source) = source {
                command.arg("--source").arg(source);
            }
        }
        Action::Controller => {
            command.arg("controller");
        }
        Action::Agent { stdio, install } => {
            command.arg("agent");
            if *stdio {
                command.arg("--stdio");
            }
            if *install {
                command.arg("--install");
            }
        }
        Action::Status => {
            command.arg("status");
        }
        Action::Signal {
            host,
            worker,
            signal,
        } => {
            command.args(["signal", host, worker, signal]);
        }
    }
    if matches!(
        action,
        Action::Controller | Action::Agent { install: false, .. }
    ) {
        use std::os::unix::process::CommandExt;
        return Err(command.exec());
    }
    let status = command.status()?;
    if !status.success() {
        return Err(std::io::Error::other("Fleet command failed"));
    }
    Ok(())
}

fn database(path: &std::path::Path) -> std::io::Result<()> {
    use rusqlite::types::{Value as SqlValue, ValueRef};
    use std::io::BufRead;
    let connection = rusqlite::Connection::open(path).map_err(std::io::Error::other)?;
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
                statement.execute(rusqlite::params_from_iter(values))?;
            } else {
                let mut cursor = statement.query(rusqlite::params_from_iter(values))?;
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
