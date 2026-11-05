//! File permissions, SQLite connection setup, and graph initialization.
use super::*;

/// Create private directories only where the database path requires them. An
/// existing parent belongs to the caller and keeps its permissions.
pub(super) fn database_parent(path: &Path) -> CwpdbResult<&Path> {
    let parent = path
        .parent()
        .ok_or_else(|| "cwpDb database path has no parent directory".to_owned())?;
    Ok(if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    })
}

pub(super) fn create_private_parent_directories(parent: &Path) -> CwpdbResult<()> {
    let mut created = Vec::new();
    let mut ancestor = parent;
    while !ancestor.try_exists()? {
        created.push(ancestor.to_path_buf());
        ancestor = database_parent(ancestor)?;
    }
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    for directory in created {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn sqlite_sidecar_paths(path: &Path) -> CwpdbResult<[PathBuf; 2]> {
    let file_name = path
        .file_name()
        .ok_or_else(|| "cwpDb database path has no file name".to_owned())?;
    let sidecar = |suffix: &str| {
        let mut name = file_name.to_os_string();
        name.push(suffix);
        path.with_file_name(name)
    };
    Ok([sidecar("-wal"), sidecar("-shm")])
}

/// Create the database and its SQLite sidecars before SQLite can create them
/// with the caller's umask. The sidecars are empty until WAL mode initializes
/// them, which SQLite supports and then fills in-place.
#[cfg(unix)]
pub(super) fn prepare_private_database_files(path: &Path) -> CwpdbResult<()> {
    prepare_private_file(path)?;
    for sidecar in sqlite_sidecar_paths(path)? {
        prepare_private_file(&sidecar)?;
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn restrict_private_database_files(path: &Path) -> CwpdbResult<()> {
    restrict_private_file(path)?;
    for sidecar in sqlite_sidecar_paths(path)? {
        restrict_private_file(&sidecar)?;
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_private_file(path: &Path) -> CwpdbResult<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => restrict_private_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
            {
                Ok(_) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    restrict_private_file(path)
                }
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn restrict_private_file(path: &Path) -> CwpdbResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "{}: cwpDb database file must not be a symlink",
            path.display()
        )
        .into());
    }
    if !metadata.is_file() {
        return Err(format!("{}: cwpDb database file must be regular", path.display()).into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub(super) fn configure_connection(connection: &Connection, require_wal: bool) -> CwpdbResult<()> {
    crate::profiling::trace(connection);
    connection.set_prepared_statement_cache_capacity(128);
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "recursive_triggers", "ON")?;
    let journal: String = connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    if require_wal && !journal.eq_ignore_ascii_case("wal") {
        return Err("SQLite did not enable WAL mode".into());
    }
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

pub(super) fn configure_read_only_connection(connection: &Connection) -> CwpdbResult<()> {
    crate::profiling::trace(connection);
    connection.set_prepared_statement_cache_capacity(32);
    connection.busy_timeout(READ_ONLY_BUSY_TIMEOUT)?;
    // This is connection-local and ensures a future native API cannot mutate
    // a diagnostics database merely because it holds a read-only handle.
    connection.pragma_update(None, "query_only", "ON")?;
    Ok(())
}

/// Open and initialize one file-backed store. Concurrent first opens can race
/// both WAL activation and schema creation, so retry only SQLite's transient
/// lock errors while each attempt owns a fresh connection.
pub(super) fn open_file_connection(path: &Path) -> CwpdbResult<Connection> {
    for attempt in 0..INITIALIZATION_ATTEMPTS {
        let mut connection = Connection::open(path)?;
        let initialized = configure_connection(&connection, true)
            .and_then(|()| ensure_graph_schema(&mut connection));
        match initialized {
            Ok(()) => return Ok(connection),
            Err(error)
                if is_transient_initialization_error(error.as_ref())
                    && attempt + 1 < INITIALIZATION_ATTEMPTS =>
            {
                drop(connection);
                std::thread::sleep(INITIALIZATION_RETRY_DELAY.saturating_mul(
                    u32::try_from(attempt + 1).expect("initialization attempts fit u32"),
                ));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the initialization loop either returns a connection or an error")
}

fn is_transient_initialization_error(
    error: &(dyn std::error::Error + Send + Sync + 'static),
) -> bool {
    matches!(
        error.downcast_ref::<rusqlite::Error>(),
        Some(rusqlite::Error::SqliteFailure(code, _))
            if matches!(code.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

pub(super) fn ensure_graph_schema(connection: &mut Connection) -> CwpdbResult<()> {
    if crate::schema_is_current(connection)? {
        return Ok(());
    }
    // The existence check belongs inside the same immediate transaction as
    // initialization. Otherwise two valid first-open processes can both see
    // an empty database and one fails after the other creates its tables.
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let tables: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    if tables == 0 {
        initialize(&transaction)?;
        transaction.commit()?;
        return Ok(());
    }
    let graph_tables: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('nodes','edges','ref_bindings')",
        [],
        |row| row.get(0),
    )?;
    if graph_tables != 3 {
        return Err("existing cwpDb database is missing its shared graph schema".into());
    }
    // Preserve existing graph tables while applying small physical-schema
    // upgrades, such as the opt-in disposable-identity flag.
    upgrade(&transaction)?;
    transaction.commit()?;
    Ok(())
}
