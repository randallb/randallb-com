//! Storage-owned SQLite operations and an opaque native store.
//!
//! Physical connections, SQL execution and transaction callbacks stay inside
//! CWPDB. Rust callers use the named domain operations on `NativeStore`.
//! The independent Deno C ABI serializes its legacy SQL requests within this
//! crate and does not grant CWR a physical callback or query capability.

use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use rusqlite::{
    hooks::{AuthAction, AuthContext, Authorization},
    params_from_iter,
    types::{Value as SqliteValue, ValueRef},
    Connection, ErrorCode, OpenFlags, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{initialize, upgrade, CwpdbResult};

mod diagnostics;
pub use diagnostics::{DiagnosticPage, DiagnosticRecord};
mod entity;
pub use entity::{insert_entity, replace_entity};
mod graph;
pub use graph::{GraphBatchRequest, GraphOperation};

mod connection;
use connection::*;
mod ffi;
pub use ffi::FfiStore;
#[cfg(test)]
use ffi::*;

const MAX_BATCH_STATEMENTS: usize = 256;
const DEFAULT_QUERY_ROWS: usize = 1_000;
const MAX_QUERY_ROWS: usize = 10_000;
const READ_ONLY_BUSY_TIMEOUT: Duration = Duration::from_millis(250);
const INITIALIZATION_ATTEMPTS: usize = 8;
const INITIALIZATION_RETRY_DELAY: Duration = Duration::from_millis(10);

/// One parameterized SQL statement in a native batch.
///
/// Parameters must be JSON null, booleans, finite numbers, or strings. Boolean
/// values become SQLite integers and bytes are intentionally unsupported at the
/// JSON boundary.
#[derive(Debug, Deserialize, Serialize)]
pub struct SqlStatement {
    /// One SQLite statement, with positional `?` parameters.
    pub sql: String,
    /// Parameter values bound separately from the statement text.
    #[serde(default)]
    pub params: Vec<Value>,
}

/// A group of writes that either all commit or all roll back.
#[derive(Debug, Deserialize, Serialize)]
pub struct BatchRequest {
    /// Statements executed in order inside one `BEGIN IMMEDIATE` transaction.
    pub statements: Vec<SqlStatement>,
}

/// The rows changed by each committed statement in a batch.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BatchResponse {
    /// Change count for each statement, in request order.
    pub changes: Vec<u64>,
}

/// One read-only parameterized query.
#[derive(Debug, Deserialize, Serialize)]
pub struct QueryRequest {
    /// SQLite query text, with positional `?` parameters.
    pub sql: String,
    /// Parameter values bound separately from the query text.
    #[serde(default)]
    pub params: Vec<Value>,
    /// Maximum returned rows, defaulting to 1,000 and capped at 10,000.
    #[serde(default)]
    pub max_rows: Option<usize>,
}

/// JSON-safe rows returned by a native query.
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct QueryResponse {
    /// Column names in every row array's order.
    pub columns: Vec<String>,
    /// Rows made of null, booleans, numbers, or strings.
    pub rows: Vec<Vec<Value>>,
    /// Whether a row beyond `max_rows` was available.
    pub truncated: bool,
}

/// The result of asking SQLite to checkpoint its write-ahead log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WalCheckpoint {
    /// Whether another connection kept SQLite from completing the checkpoint.
    pub busy: bool,
    /// Frames present in the WAL when SQLite performed the checkpoint.
    pub log_frames: u64,
    /// Frames SQLite copied back into the main database.
    pub checkpointed_frames: u64,
}

/// An opaque connection-owning store with storage-owned domain operations.
pub struct NativeStore {
    connection: Connection,
}

impl NativeStore {
    /// Whether this connection was opened without write access.
    pub fn is_read_only(&self) -> CwpdbResult<bool> {
        Ok(self.connection.is_readonly("main")?)
    }

    /// Open a file-backed store, create the shared graph schema if it is new,
    /// and configure private local SQLite settings.
    pub fn open(path: impl AsRef<Path>) -> CwpdbResult<Self> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return Err("cwpDb database path is empty".into());
        }
        if path == Path::new(":memory:") {
            return Self::open_in_memory();
        }
        let parent = database_parent(path)?;
        if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(format!("{}: cwpDb database must not be a symlink", path.display()).into());
        }
        create_private_parent_directories(parent)?;
        #[cfg(unix)]
        prepare_private_database_files(path)?;
        let connection = open_file_connection(path)?;
        #[cfg(unix)]
        restrict_private_database_files(path)?;
        Ok(Self { connection })
    }

    /// Open an existing store for a short, read-only diagnostic query.
    ///
    /// Unlike [`Self::open`], this never creates a database or parent
    /// directory, changes persistent SQLite settings, or initializes schema.
    /// It is intended for an application detail view that must not make its
    /// own diagnostic store appear when the runtime has not recorded one.
    pub fn open_read_only(path: impl AsRef<Path>) -> CwpdbResult<Self> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return Err("cwpDb database path is empty".into());
        }
        if path == Path::new(":memory:") {
            return Err("cwpDb read-only open requires an existing database file".into());
        }
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(format!("{}: cwpDb database must not be a symlink", path.display()).into());
        }
        if !metadata.is_file() {
            return Err(
                format!("{}: cwpDb database must be a regular file", path.display()).into(),
            );
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        configure_read_only_connection(&connection)?;
        Ok(Self { connection })
    }

    /// Open a fresh in-memory store for tests and isolated callers.
    pub fn open_in_memory() -> CwpdbResult<Self> {
        let mut connection = Connection::open_in_memory()?;
        configure_connection(&connection, false)?;
        ensure_graph_schema(&mut connection)?;
        Ok(Self { connection })
    }

    /// Read one declared domain operation from a consistent, deferred snapshot.
    pub(crate) fn graph_read<T>(
        &mut self,
        work: impl FnOnce(&rusqlite::Connection) -> CwpdbResult<T>,
    ) -> CwpdbResult<T> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)?;
        let value = work(&transaction)?;
        transaction.commit()?;
        Ok(value)
    }

    /// One read snapshot on the existing handle, including read-only stores.
    pub(crate) fn graph_read_transaction<T>(
        &mut self,
        work: impl FnOnce(&rusqlite::Transaction<'_>) -> CwpdbResult<T>,
    ) -> CwpdbResult<T> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)?;
        let value = work(&transaction)?;
        transaction.commit()?;
        Ok(value)
    }

    /// Run one trusted Rust graph operation in an immediate transaction.
    ///
    /// The JSON FFI boundary intentionally does not expose this escape hatch.
    /// Rust callers use it to compose the graph primitives in this crate while
    /// keeping a connection and its transaction lifetime owned by `NativeStore`.
    pub(crate) fn graph_transaction<T>(
        &mut self,
        work: impl FnOnce(&rusqlite::Transaction<'_>) -> CwpdbResult<T>,
    ) -> CwpdbResult<T> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = work(&transaction)?;
        transaction.commit()?;
        Ok(value)
    }

    /// Execute the supplied statements in one immediate transaction.
    pub(crate) fn execute_batch(&mut self, request: &BatchRequest) -> CwpdbResult<BatchResponse> {
        if request.statements.is_empty() {
            return Err("cwpDb batch must contain at least one statement".into());
        }
        if request.statements.len() > MAX_BATCH_STATEMENTS {
            return Err(format!(
                "cwpDb batch has {} statements; the limit is {MAX_BATCH_STATEMENTS}",
                request.statements.len()
            )
            .into());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.authorizer(Some(batch_authorization))?;
        let mut changes = Vec::with_capacity(request.statements.len());
        let result = (|| -> CwpdbResult<()> {
            for statement in &request.statements {
                reject_transaction_control(&statement.sql)?;
                let params = sqlite_params(&statement.params)?;
                changes.push(transaction.execute(&statement.sql, params_from_iter(params))? as u64);
            }
            Ok(())
        })();
        let reset = transaction.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        match (result, reset) {
            (Ok(()), Ok(())) => {}
            (Err(error), Ok(())) => return Err(error),
            (Ok(()), Err(error)) | (Err(_), Err(error)) => return Err(error.into()),
        }
        transaction.commit()?;
        Ok(BatchResponse { changes })
    }

    /// Run a query that cannot alter this connection or its database.
    pub(crate) fn query(&mut self, request: &QueryRequest) -> CwpdbResult<QueryResponse> {
        let max_rows = request.max_rows.unwrap_or(DEFAULT_QUERY_ROWS);
        if max_rows > MAX_QUERY_ROWS {
            return Err(format!(
                "cwpDb query requests {max_rows} rows; the limit is {MAX_QUERY_ROWS}"
            )
            .into());
        }
        self.connection.authorizer(Some(query_authorization))?;
        let result = query_rows(&self.connection, request, max_rows);
        let reset = self
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        match (result, reset) {
            (Ok(response), Ok(())) => Ok(response),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) | (Err(_), Err(error)) => Err(error.into()),
        }
    }

    /// Set a ceiling for the whole database file, rounded up to SQLite pages.
    ///
    /// The caller owns this policy. It is useful for a disposable database
    /// whose writer can discard samples when the local disk budget is reached.
    pub fn set_max_database_bytes(&mut self, bytes: u64) -> CwpdbResult<u64> {
        // Avoid spilling a page repeatedly into WAL within one bounded transaction.
        // A caller can reserve one database worth of WAL between checkpoints.
        self.connection.pragma_update(None, "cache_spill", "OFF")?;
        let page_size: i64 = self
            .connection
            .pragma_query_value(None, "page_size", |row| row.get(0))?;
        let page_size = u64::try_from(page_size)?;
        let pages = bytes
            .max(page_size)
            .saturating_add(page_size.saturating_sub(1))
            / page_size;
        self.connection
            .pragma_update(None, "max_page_count", i64::try_from(pages)?)?;
        let effective_pages: i64 =
            self.connection
                .pragma_query_value(None, "max_page_count", |row| row.get(0))?;
        let effective_pages = u64::try_from(effective_pages)?;
        Ok(effective_pages.saturating_mul(page_size))
    }

    /// Attempt to checkpoint and truncate the write-ahead log.
    ///
    /// A long-lived reader may leave `busy` true and prevent truncation. The
    /// caller can then stop an expendable writer before the WAL consumes more
    /// of its own storage budget.
    pub fn checkpoint_wal(&mut self) -> CwpdbResult<WalCheckpoint> {
        let (busy, log_frames, checkpointed_frames): (i64, i64, i64) =
            self.connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
        Ok(WalCheckpoint {
            busy: busy != 0,
            // SQLite reports -1 for frame counts when WAL is unavailable,
            // such as an in-memory connection. That means no file-backed WAL
            // bytes for a caller to budget.
            log_frames: log_frames.max(0) as u64,
            checkpointed_frames: checkpointed_frames.max(0) as u64,
        })
    }
}

fn reject_transaction_control(sql: &str) -> CwpdbResult<()> {
    if matches!(
        leading_keyword(sql).as_str(),
        "BEGIN" | "COMMIT" | "END" | "ROLLBACK" | "SAVEPOINT" | "RELEASE"
    ) {
        return Err(
            "cwpDb batch owns its transaction; transaction-control SQL is not allowed".into(),
        );
    }
    Ok(())
}

/// The native boundary owns transaction scope. Callers can create and mutate
/// their application tables, but cannot attach another database, load an
/// extension, or escape the current transaction through comments or another
/// lexical trick.
fn batch_authorization(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        | AuthAction::Pragma { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. }
        | AuthAction::Unknown { .. } => Authorization::Deny,
        AuthAction::Function { function_name }
            if function_name.eq_ignore_ascii_case("load_extension") =>
        {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }
}

/// Queries only need reads, selects, recursive CTEs, and ordinary scalar
/// functions. Denying every other SQLite action prevents a query from changing
/// connection settings, starting a transaction, or loading an extension.
fn query_authorization(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Select | AuthAction::Read { .. } | AuthAction::Recursive => {
            Authorization::Allow
        }
        AuthAction::Function { function_name }
            if !function_name.eq_ignore_ascii_case("load_extension") =>
        {
            Authorization::Allow
        }
        _ => Authorization::Deny,
    }
}

fn reject_non_query(sql: &str) -> CwpdbResult<()> {
    if !matches!(leading_keyword(sql).as_str(), "SELECT" | "WITH" | "EXPLAIN") {
        return Err("cwpDb query must begin with SELECT, WITH, or EXPLAIN".into());
    }
    Ok(())
}

fn leading_keyword(sql: &str) -> String {
    sql.trim_start()
        .split(|character: char| !character.is_ascii_alphabetic())
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase()
}

fn sqlite_params(params: &[Value]) -> CwpdbResult<Vec<SqliteValue>> {
    params.iter().map(sqlite_value).collect()
}

fn sqlite_value(value: &Value) -> CwpdbResult<SqliteValue> {
    match value {
        Value::Null => Ok(SqliteValue::Null),
        Value::Bool(value) => Ok(SqliteValue::Integer(i64::from(*value))),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(SqliteValue::Integer(value))
            } else if let Some(value) = value.as_u64() {
                Ok(SqliteValue::Integer(i64::try_from(value)?))
            } else if let Some(value) = value.as_f64() {
                Ok(SqliteValue::Real(value))
            } else {
                Err("cwpDb JSON number is not a SQLite value".into())
            }
        }
        Value::String(value) => Ok(SqliteValue::Text(value.clone())),
        Value::Array(_) | Value::Object(_) => {
            Err("cwpDb SQL parameters must be JSON scalars".into())
        }
    }
}

fn query_rows(
    connection: &Connection,
    request: &QueryRequest,
    max_rows: usize,
) -> CwpdbResult<QueryResponse> {
    reject_non_query(&request.sql)?;
    let params = sqlite_params(&request.params)?;
    let mut statement = connection.prepare(&request.sql)?;
    if !statement.readonly() {
        return Err("cwpDb query must be read-only".into());
    }
    let columns = statement
        .column_names()
        .iter()
        .map(ToString::to_string)
        .collect();
    let column_count = statement.column_count();
    let mut query = statement.query(params_from_iter(params))?;
    let mut rows = Vec::new();
    while rows.len() < max_rows {
        let Some(row) = query.next()? else {
            return Ok(QueryResponse {
                columns,
                rows,
                truncated: false,
            });
        };
        let mut values = Vec::with_capacity(column_count);
        for index in 0..column_count {
            values.push(json_value(row.get_ref(index)?)?);
        }
        rows.push(values);
    }
    Ok(QueryResponse {
        columns,
        rows,
        truncated: query.next()?.is_some(),
    })
}

fn json_value(value: ValueRef<'_>) -> CwpdbResult<Value> {
    match value {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(value) if (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&value) => Ok(Value::from(value)),
        ValueRef::Integer(_) => Err("cwpDb query integer exceeds the JavaScript safe integer range; CAST it AS TEXT for a lossless result".into()),
        ValueRef::Real(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| "cwpDb query returned a non-finite floating-point value".into()),
        ValueRef::Text(value) => Ok(Value::String(std::str::from_utf8(value)?.to_owned())),
        ValueRef::Blob(_) => {
            Err("cwpDb query returned binary data, which JSON FFI does not support".into())
        }
    }
}

#[cfg(test)]
mod graph_tests;
#[cfg(test)]
#[path = "native/tests.rs"]
mod tests;
