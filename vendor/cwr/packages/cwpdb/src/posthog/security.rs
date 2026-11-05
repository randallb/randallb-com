//! Preserve the pre-migration queue's reject-on-unsafe-file policy.
use crate::CwpdbResult;
use rusqlite::Connection;
#[cfg(unix)]
use rusqlite::OpenFlags;
use std::{path::Path, time::Duration};

pub(super) fn open(path: &Path) -> CwpdbResult<Connection> {
    open_with_checkpoint(path, || Ok(()))
}

pub(super) fn open_with_checkpoint(
    path: &Path,
    before_sqlite: impl FnOnce() -> CwpdbResult<()>,
) -> CwpdbResult<Connection> {
    private_directory(path).map_err(|_| rusqlite::Error::InvalidPath(path.to_owned()))?;
    // Resolve ancestor aliases (macOS /var -> /private/var) after rejecting a
    // symlink at the selected private directory. SQLite NOFOLLOW then guards
    // the leaf without rejecting legitimate system directory aliases.
    let database = std::fs::canonicalize(path)?.join("queue.sqlite3");
    if std::fs::symlink_metadata(&database).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(rusqlite::Error::InvalidPath(database).into());
    }
    // Create the file privately before SQLite opens it; reject redirected or shared files.
    #[cfg(unix)]
    let original = {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&database)
        {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(rusqlite::Error::InvalidPath(database).into()),
        }
        let metadata = std::fs::symlink_metadata(&database)
            .map_err(|_| rusqlite::Error::InvalidPath(database.clone()))?;
        // SAFETY: geteuid takes no arguments and has no memory safety preconditions.
        let current_uid = unsafe { libc::geteuid() };
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != current_uid
        {
            return Err(rusqlite::Error::InvalidPath(database).into());
        }
        metadata
    };
    before_sqlite()?;
    #[cfg(unix)]
    let db = Connection::open_with_flags(
        &database,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    #[cfg(not(unix))]
    let db = Connection::open(&database)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current = std::fs::symlink_metadata(&database)?;
        if current.dev() != original.dev()
            || current.ino() != original.ino()
            || current.file_type().is_symlink()
            || current.nlink() != 1
            || current.mode() & 0o077 != 0
            || current.uid() != original.uid()
        {
            return Err("retained outbox file changed while opening".into());
        }
    }
    db.busy_timeout(Duration::from_millis(100))?;
    db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA max_page_count=4096; PRAGMA foreign_keys=ON; PRAGMA recursive_triggers=ON; PRAGMA synchronous=FULL; PRAGMA temp_store=MEMORY;")?;
    Ok(db)
}
fn private_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid takes no arguments and has no memory safety preconditions.
        let current_uid = unsafe { libc::geteuid() };
        if metadata.mode() & 0o077 != 0 || metadata.uid() != current_uid {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
    }
    Ok(())
}
