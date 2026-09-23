//! Transparent SQLite sessions hosted by the existing local service.
//! Transactions retain exclusive access to one writer until commit, rollback,
//! or caller disconnect. Independent queries use a read-only WAL connection.
pub(crate) mod owner;
mod wire;
pub use owner::Owner;
pub use rusqlite::{OpenFlags, StatementStatus, TransactionBehavior};
use rusqlite::{
    Result, ToSql,
    types::{FromSql, FromSqlError, ToSqlOutput, Value, ValueRef},
};
use std::{
    cell::{Cell, RefCell},
    io::BufReader,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};
use wire::{Command, Reply, SqlValue};

static CLIENTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
thread_local! { static LOCAL: Cell<bool> = const { Cell::new(false) }; }
/// Executable entry point opts into service-owned access. Library fixtures retain
/// ordinary local SQLite connections and can exercise the underlying lock guards.
pub fn use_service() {
    CLIENTS.store(true, std::sync::atomic::Ordering::Release);
}
fn local<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            LOCAL.set(self.0);
        }
    }
    let _restore = Restore(LOCAL.replace(true));
    f()
}

/// Explicit low-level maintenance also supports arbitrary private SQLite files.
/// A live issue owner is always used; standalone maintenance preserves the
/// historical driver without creating an issue schema in unrelated databases.
pub(crate) fn maintenance(path: &Path) -> crate::issues::Result<Connection> {
    crate::issues::Store::create_database_if_missing(path)?;
    crate::issues::Store::validate_database_path(path)?;
    if let Ok(connection) = Connection::connect(path) {
        return Ok(connection);
    }
    local(|| crate::issues::Store::open_connection(path))
}
pub(crate) fn remote_enabled() -> bool {
    CLIENTS.load(std::sync::atomic::Ordering::Acquire) && !LOCAL.get()
}
fn error(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(message.into())))
}
fn permission_denied(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::ToSqlConversionFailure(source)
        if source.downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied))
}

pub trait Params {
    fn values(self) -> Result<Vec<Value>>;
}
fn value(v: &dyn ToSql) -> Result<Value> {
    match v.to_sql()? {
        ToSqlOutput::Borrowed(v) => Value::try_from(v).map_err(|e| error(e.to_string())),
        ToSqlOutput::Owned(v) => Ok(v),
        _ => Err(error("Unsupported database parameter")),
    }
}
impl Params for [&(dyn ToSql + Send + Sync); 0] {
    fn values(self) -> Result<Vec<Value>> {
        Ok(vec![])
    }
}
impl Params for () {
    fn values(self) -> Result<Vec<Value>> {
        Ok(vec![])
    }
}
impl<T: ToSql> Params for &[T] {
    fn values(self) -> Result<Vec<Value>> {
        self.iter().map(|v| value(v)).collect()
    }
}
macro_rules! arrays { ($($n:literal),*) => {$ (impl<T: ToSql> Params for [T; $n] { fn values(self) -> Result<Vec<Value>> { self.iter().map(|v| value(v)).collect() } })*}; }
arrays!(
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32
);
pub struct IterParams<I>(I);
pub fn params_from_iter<I: IntoIterator>(values: I) -> IterParams<I> {
    IterParams(values)
}
impl<I: IntoIterator> Params for IterParams<I>
where
    I::Item: ToSql,
{
    fn values(self) -> Result<Vec<Value>> {
        self.0.into_iter().map(|v| value(&v)).collect()
    }
}

#[derive(Debug)]
enum Backend {
    Local(rusqlite::Connection),
    Remote(Remote),
}
#[derive(Debug)]
struct Remote {
    path: PathBuf,
    stream: RefCell<Option<BufReader<UnixStream>>>,
    transaction: Cell<bool>,
    last_id: Cell<i64>,
}
#[derive(Debug)]
pub struct Connection {
    backend: Backend,
}
impl Connection {
    pub fn owner_pid(&self) -> Result<u32> {
        match &self.backend {
            Backend::Local(_) => Ok(std::process::id()),
            Backend::Remote(remote) => Ok(remote.call(Command::Hello)?.pid),
        }
    }
    pub(crate) fn check_schema(&self) -> Result<(i64, i64)> {
        let Backend::Remote(remote) = &self.backend else {
            return Err(error("Schema checks require a service session"));
        };
        let reply = remote.call(Command::CheckSchema)?;
        Ok((reply.application, reply.schema))
    }
    pub(crate) fn exclusive_session(&self) -> Result<()> {
        if let Backend::Remote(remote) = &self.backend {
            remote.call(Command::ExclusiveSession)?;
        }
        Ok(())
    }
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_flags(path, OpenFlags::default())
    }
    pub fn open_with_flags(path: impl AsRef<Path>, flags: OpenFlags) -> Result<Self> {
        if remote_enabled() {
            match Self::connect(path.as_ref()) {
                Ok(connection) => Ok(connection),
                Err(e) if permission_denied(&e) => Err(e),
                Err(_) => {
                    owner::ensure(path.as_ref())?;
                    Self::connect(path.as_ref())
                }
            }
        } else {
            Ok(Self {
                backend: Backend::Local(rusqlite::Connection::open_with_flags(path, flags)?),
            })
        }
    }
    pub fn open_in_memory() -> Result<Self> {
        Ok(Self {
            backend: Backend::Local(rusqlite::Connection::open_in_memory()?),
        })
    }
    pub fn connect(path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(owner::socket_path(path)).map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                e.kind(),
                format!("Database service unavailable: {e}"),
            )))
        })?;
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| error(e.to_string()))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(15)))
            .map_err(|e| error(e.to_string()))?;
        let remote = Remote {
            path: path.to_owned(),
            stream: RefCell::new(Some(BufReader::new(stream))),
            transaction: Cell::new(false),
            last_id: Cell::new(0),
        };
        let reply = remote.call(Command::Hello)?;
        if reply.version != wire::VERSION {
            return Err(error(
                "Database service protocol mismatch; service is restarting after upgrade",
            ));
        }
        Ok(Self {
            backend: Backend::Remote(remote),
        })
    }
    pub(super) fn into_local(self) -> rusqlite::Connection {
        match self.backend {
            Backend::Local(db) => db,
            _ => panic!("owner must initialize locally"),
        }
    }
    pub fn busy_timeout(&self, duration: Duration) -> Result<()> {
        match &self.backend {
            Backend::Local(db) => db.busy_timeout(duration),
            Backend::Remote(_) => Ok(()),
        }
    }
    pub fn execute<P: Params>(&self, sql: &str, params: P) -> Result<usize> {
        let values = params.values()?;
        match &self.backend {
            Backend::Local(db) => db.execute(sql, rusqlite::params_from_iter(values)),
            Backend::Remote(remote) => Ok(remote
                .call(Command::Execute {
                    sql: sql.into(),
                    values: values.into_iter().map(SqlValue::from).collect(),
                })?
                .changes),
        }
    }
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        match &self.backend {
            Backend::Local(db) => db.execute_batch(sql),
            Backend::Remote(remote) => {
                remote.call(Command::Batch { sql: sql.into() })?;
                Ok(())
            }
        }
    }
    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        match &self.backend {
            Backend::Local(db) => Ok(Statement {
                backend: StatementBackend::Local(db.prepare(sql)?),
            }),
            Backend::Remote(remote) => {
                let metadata = remote.call(Command::Prepare { sql: sql.into() })?;
                Ok(Statement {
                    backend: StatementBackend::Remote {
                        connection: self,
                        sql: sql.into(),
                        metadata,
                        steps: Cell::new(0),
                    },
                })
            }
        }
    }
    pub fn prepare_cached(&self, sql: &str) -> Result<Statement<'_>> {
        self.prepare(sql)
    }
    pub fn query_row<T, P: Params>(
        &self,
        sql: &str,
        params: P,
        f: impl FnOnce(&Row<'_>) -> Result<T>,
    ) -> Result<T> {
        self.prepare(sql)?.query_row(params, f)
    }
    pub fn pragma_update<V: ToSql>(&self, schema: Option<&str>, name: &str, v: V) -> Result<()> {
        match &self.backend {
            Backend::Local(db) => db.pragma_update(schema, name, v),
            Backend::Remote(_) => {
                if schema.is_some() {
                    return Err(error("Attached database pragmas are unsupported"));
                }
                let text = match value(&v)? {
                    Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
                    Value::Integer(n) => n.to_string(),
                    Value::Real(n) => n.to_string(),
                    _ => return Err(error("Invalid pragma value")),
                };
                self.execute_batch(&format!("PRAGMA {name}={text}"))
            }
        }
    }
    pub fn pragma_query_value<T>(
        &self,
        schema: Option<&str>,
        name: &str,
        f: impl FnOnce(&Row<'_>) -> Result<T>,
    ) -> Result<T> {
        if schema.is_some() {
            return Err(error("Attached database pragmas are unsupported"));
        }
        self.query_row(&format!("PRAGMA {name}"), [], f)
    }
    pub fn path(&self) -> Option<&str> {
        match &self.backend {
            Backend::Local(db) => db.path(),
            Backend::Remote(remote) => remote.path.to_str(),
        }
    }
    pub fn is_autocommit(&self) -> bool {
        match &self.backend {
            Backend::Local(db) => db.is_autocommit(),
            Backend::Remote(remote) => !remote.transaction.get(),
        }
    }
    pub fn last_insert_rowid(&self) -> i64 {
        match &self.backend {
            Backend::Local(db) => db.last_insert_rowid(),
            Backend::Remote(remote) => remote.last_id.get(),
        }
    }
    pub fn transaction_with_behavior(
        &mut self,
        behavior: TransactionBehavior,
    ) -> Result<Transaction<'_>> {
        Transaction::new_unchecked(self, behavior)
    }
    pub fn unchecked_transaction(&self) -> Result<Transaction<'_>> {
        Transaction::new_unchecked(self, TransactionBehavior::Deferred)
    }
    pub fn read_transaction(&self) -> Result<Transaction<'_>> {
        match &self.backend {
            Backend::Local(_) => Transaction::new_unchecked(self, TransactionBehavior::Deferred),
            Backend::Remote(remote) => {
                remote.call(Command::ReadTransaction)?;
                Ok(Transaction {
                    connection: self,
                    finished: false,
                })
            }
        }
    }
    pub fn backup(
        &self,
        name: impl rusqlite::Name,
        path: impl AsRef<Path>,
        progress: Option<fn(rusqlite::backup::Progress)>,
    ) -> Result<()> {
        match &self.backend {
            Backend::Local(db) => db.backup(name, path, progress),
            Backend::Remote(remote) => {
                remote.call(Command::Backup {
                    path: path.as_ref().to_owned(),
                })?;
                Ok(())
            }
        }
    }
    /// Raw SQLite instrumentation is restricted to local regression fixtures.
    pub unsafe fn handle(&self) -> *mut rusqlite::ffi::sqlite3 {
        match &self.backend {
            Backend::Local(db) => unsafe { db.handle() },
            Backend::Remote(_) => panic!("Raw SQLite handles are unavailable for service sessions"),
        }
    }
}
impl Remote {
    fn call(&self, command: Command) -> Result<Reply> {
        let mut state = self.stream.borrow_mut();
        if !self.transaction.get()
            && state
                .as_ref()
                .is_some_and(|stream| owner::disconnected(stream.get_ref()))
        {
            *state = None;
        }
        if state.is_none() {
            if self.transaction.get() {
                if matches!(&command, Command::Batch { sql } if sql == "ROLLBACK") {
                    self.transaction.set(false);
                }
                return Err(error(
                    "Database transaction lost its connection; its outcome cannot be replayed automatically",
                ));
            }
            owner::ensure(&self.path)?;
            let Backend::Remote(replacement) = Connection::connect(&self.path)?.backend else {
                unreachable!()
            };
            *state = replacement.stream.into_inner();
        }
        let stream = state.as_mut().unwrap();
        let result: std::io::Result<Reply> = (|| {
            wire::write(stream.get_mut(), &command)?;
            let mut reply = wire::read::<Reply>(stream)?
                .ok_or_else(|| std::io::Error::other("Database service disconnected"))?;
            let mut rows = Vec::new();
            while reply.more {
                rows.append(&mut reply.rows);
                reply = wire::read::<Reply>(stream)?
                    .ok_or_else(|| std::io::Error::other("Database result stream disconnected"))?;
            }
            if !rows.is_empty() {
                rows.append(&mut reply.rows);
                reply.rows = rows;
            }
            Ok(reply)
        })();
        match result {
            Ok(reply) => {
                self.transaction.set(reply.transaction);
                self.last_id.set(reply.last_id);
                if let Some(failure) = &reply.error {
                    return Err(failure.restore());
                }
                Ok(reply)
            }
            Err(e) => {
                *state = None;
                Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
                    std::io::Error::new(
                        e.kind(),
                        format!(
                            "Database service transport failed: {e}. Write outcome may be unknown; mutations are never automatically replayed."
                        ),
                    ),
                )))
            }
        }
    }
}

pub struct Transaction<'a> {
    connection: &'a Connection,
    finished: bool,
}
impl<'a> Transaction<'a> {
    pub fn new_unchecked(
        connection: &'a Connection,
        behavior: TransactionBehavior,
    ) -> Result<Self> {
        connection.execute_batch(match behavior {
            TransactionBehavior::Deferred => "BEGIN DEFERRED",
            TransactionBehavior::Immediate => "BEGIN IMMEDIATE",
            TransactionBehavior::Exclusive => "BEGIN EXCLUSIVE",
            _ => return Err(error("Unsupported transaction behavior")),
        })?;
        Ok(Self {
            connection,
            finished: false,
        })
    }
    pub fn commit(mut self) -> Result<()> {
        self.connection.execute_batch("COMMIT")?;
        self.finished = true;
        Ok(())
    }
    pub fn rollback(mut self) -> Result<()> {
        self.connection.execute_batch("ROLLBACK")?;
        self.finished = true;
        Ok(())
    }
}
impl std::ops::Deref for Transaction<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection
    }
}
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.finished && !self.connection.is_autocommit() {
            let _ = self.connection.execute_batch("ROLLBACK");
        }
    }
}

enum StatementBackend<'a> {
    Local(rusqlite::Statement<'a>),
    Remote {
        connection: &'a Connection,
        sql: String,
        metadata: Reply,
        steps: Cell<i32>,
    },
}
pub struct Statement<'a> {
    backend: StatementBackend<'a>,
}
impl Statement<'_> {
    pub fn column_names(&self) -> Vec<&str> {
        match &self.backend {
            StatementBackend::Local(stmt) => stmt.column_names(),
            StatementBackend::Remote { metadata, .. } => {
                metadata.columns.iter().map(String::as_str).collect()
            }
        }
    }
    pub fn parameter_count(&self) -> usize {
        match &self.backend {
            StatementBackend::Local(stmt) => stmt.parameter_count(),
            StatementBackend::Remote { metadata, .. } => metadata.parameters,
        }
    }
    pub fn get_status(&self, status: StatementStatus) -> i32 {
        match &self.backend {
            StatementBackend::Local(stmt) => stmt.get_status(status),
            StatementBackend::Remote { steps, .. } => steps.get(),
        }
    }
    pub fn query<P: Params>(&mut self, params: P) -> Result<Rows<'_>> {
        let values = params.values()?;
        match &mut self.backend {
            StatementBackend::Local(stmt) => Ok(Rows {
                backend: RowsBackend::Local(stmt.query(rusqlite::params_from_iter(values))?),
            }),
            StatementBackend::Remote {
                connection,
                sql,
                steps,
                ..
            } => {
                let Backend::Remote(remote) = &connection.backend else {
                    unreachable!()
                };
                let reply = remote.call(Command::Query {
                    sql: sql.clone(),
                    values: values.into_iter().map(SqlValue::from).collect(),
                })?;
                steps.set(reply.steps);
                Ok(Rows {
                    backend: RowsBackend::Remote {
                        columns: reply.columns,
                        values: reply.rows.into_iter(),
                        current: None,
                    },
                })
            }
        }
    }
    pub fn query_map<T, P: Params, F: FnMut(&Row<'_>) -> Result<T>>(
        &mut self,
        params: P,
        f: F,
    ) -> Result<MappedRows<'_, F>> {
        Ok(MappedRows {
            rows: self.query(params)?,
            f,
        })
    }
    pub fn query_row<T, P: Params>(
        &mut self,
        params: P,
        f: impl FnOnce(&Row<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut rows = self.query(params)?;
        f(&rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?)
    }
    pub fn execute<P: Params>(&mut self, params: P) -> Result<usize> {
        let values = params.values()?;
        match &mut self.backend {
            StatementBackend::Local(stmt) => stmt.execute(rusqlite::params_from_iter(values)),
            StatementBackend::Remote {
                connection, sql, ..
            } => connection.execute(sql, params_from_iter(values)),
        }
    }
}

enum RowsBackend<'a> {
    Local(rusqlite::Rows<'a>),
    Remote {
        columns: Vec<String>,
        values: std::vec::IntoIter<Vec<SqlValue>>,
        current: Option<Vec<SqlValue>>,
    },
}
pub struct Rows<'a> {
    backend: RowsBackend<'a>,
}
impl Rows<'_> {
    // The returned row borrows this cursor, unlike Iterator's owned items.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Row<'_>>> {
        match &mut self.backend {
            RowsBackend::Local(rows) => Ok(rows.next()?.map(Row::Local)),
            RowsBackend::Remote {
                columns,
                values,
                current,
            } => {
                *current = values.next();
                Ok(current
                    .as_ref()
                    .map(|values| Row::Remote { columns, values }))
            }
        }
    }
}
pub struct MappedRows<'a, F> {
    rows: Rows<'a>,
    f: F,
}
impl<T, F: FnMut(&Row<'_>) -> Result<T>> Iterator for MappedRows<'_, F> {
    type Item = Result<T>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.rows.next() {
            Ok(Some(row)) => Some((self.f)(&row)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}
pub enum Row<'a> {
    Local(&'a rusqlite::Row<'a>),
    Remote {
        columns: &'a [String],
        values: &'a [SqlValue],
    },
}
pub trait RowIndex {
    fn index(&self, columns: &[String]) -> Result<usize>;
    fn local<T: FromSql>(&self, row: &rusqlite::Row<'_>) -> Result<T>;
    fn local_ref<'a>(&self, row: &'a rusqlite::Row<'_>) -> Result<ValueRef<'a>>;
}
impl RowIndex for usize {
    fn index(&self, columns: &[String]) -> Result<usize> {
        if *self < columns.len() {
            Ok(*self)
        } else {
            Err(rusqlite::Error::InvalidColumnIndex(*self))
        }
    }
    fn local<T: FromSql>(&self, row: &rusqlite::Row<'_>) -> Result<T> {
        row.get(*self)
    }
    fn local_ref<'a>(&self, row: &'a rusqlite::Row<'_>) -> Result<ValueRef<'a>> {
        row.get_ref(*self)
    }
}
impl RowIndex for &str {
    fn index(&self, columns: &[String]) -> Result<usize> {
        columns
            .iter()
            .position(|v| v.eq_ignore_ascii_case(self))
            .ok_or_else(|| rusqlite::Error::InvalidColumnName(self.to_string()))
    }
    fn local<T: FromSql>(&self, row: &rusqlite::Row<'_>) -> Result<T> {
        row.get(*self)
    }
    fn local_ref<'a>(&self, row: &'a rusqlite::Row<'_>) -> Result<ValueRef<'a>> {
        row.get_ref(*self)
    }
}
impl Row<'_> {
    pub fn get<I: RowIndex, T: FromSql>(&self, index: I) -> Result<T> {
        match self {
            Self::Local(row) => index.local(row),
            Self::Remote { columns, values } => {
                let i = index.index(columns)?;
                let value = values[i].as_ref();
                T::column_result(value).map_err(|e| match e {
                    FromSqlError::InvalidType => {
                        rusqlite::Error::InvalidColumnType(i, columns[i].clone(), value.data_type())
                    }
                    FromSqlError::OutOfRange(n) => rusqlite::Error::IntegralValueOutOfRange(i, n),
                    e => {
                        rusqlite::Error::FromSqlConversionFailure(i, value.data_type(), Box::new(e))
                    }
                })
            }
        }
    }
    pub fn get_ref<I: RowIndex>(&self, index: I) -> Result<ValueRef<'_>> {
        match self {
            Self::Local(row) => index.local_ref(row),
            Self::Remote { columns, values } => Ok(values[index.index(columns)?].as_ref()),
        }
    }
}

#[cfg(test)]
mod tests;
