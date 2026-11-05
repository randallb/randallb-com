//! Optional storage-owned statement counts for startup diagnostics.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock,
};
static ENABLED: OnceLock<bool> = OnceLock::new();
static STATEMENTS: AtomicU64 = AtomicU64::new(0);

/// Process-wide completed statement count. SQL text and handles stay private.
pub fn completed_statements() -> u64 {
    STATEMENTS.load(Ordering::Relaxed)
}

pub(crate) fn trace(connection: &rusqlite::Connection) {
    use rusqlite::trace::{TraceEvent, TraceEventCodes};
    if !*ENABLED.get_or_init(|| std::env::var_os("CWR_STARTUP_PROFILE_PATH").is_some()) {
        return;
    }
    fn count(event: TraceEvent<'_>) {
        if matches!(event, TraceEvent::Profile(..)) {
            STATEMENTS.fetch_add(1, Ordering::Relaxed);
        }
    }
    connection.trace_v2(TraceEventCodes::SQLITE_TRACE_PROFILE, Some(count));
}
