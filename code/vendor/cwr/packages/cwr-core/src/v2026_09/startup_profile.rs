//! Optional startup phase timings for cold-store profiling.

use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::OnceLock,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

static OUTPUT: OnceLock<Option<PathBuf>> = OnceLock::new();

fn output() -> Option<&'static PathBuf> {
    OUTPUT
        .get_or_init(|| std::env::var_os("CWR_STARTUP_PROFILE_PATH").map(PathBuf::from))
        .as_ref()
}

/// Records one completed phase as a TSV row: Unix microseconds, PID, phase,
/// duration in microseconds, and completed SQL statement count. The environment variable is opt-in and inherited
/// by a spawned resident. An unsuccessful diagnostic write never stops startup.
pub struct StartupPhase {
    name: &'static str,
    started: Option<Instant>,
    sql_before: u64,
}

impl StartupPhase {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            started: output().map(|_| Instant::now()),
            sql_before: crate::cwpdb::profiling::completed_statements(),
        }
    }
}

impl Drop for StartupPhase {
    fn drop(&mut self) {
        let (Some(started), Some(path)) = (self.started, output()) else {
            return;
        };
        let elapsed_us = started.elapsed().as_micros();
        let sql_statements =
            crate::cwpdb::profiling::completed_statements().saturating_sub(self.sql_before);
        let unix_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
        {
            let _ = writeln!(
                file,
                "{unix_us}\t{}\t{}\t{elapsed_us}\t{sql_statements}",
                std::process::id(),
                self.name
            );
        }
    }
}

pub fn time<T>(name: &'static str, action: impl FnOnce() -> T) -> T {
    let _phase = StartupPhase::new(name);
    action()
}
