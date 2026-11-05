//! Opt-in, content-free CLI startup diagnostics. No I/O on the measured path.
use serde_json::{json, Value};
use std::{
    io::Write,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const MAX_SPANS: usize = 64;
static TRACE: OnceLock<Trace> = OnceLock::new();

struct Trace {
    started: Instant,
    enabled: bool,
    spans: Mutex<Vec<Value>>,
    dropped: AtomicU64,
}

pub(super) fn initialize() {
    let started = Instant::now();
    let _ = TRACE.set(Trace {
        started,
        enabled: std::env::var_os("CWR_STARTUP_TIMING").is_some_and(|v| v == "1"),
        spans: Mutex::new(Vec::new()),
        dropped: AtomicU64::new(0),
    });
}

pub(super) struct Span {
    name: &'static str,
    started: Option<Instant>,
    completed: bool,
}

pub(super) fn span(name: &'static str) -> Span {
    Span {
        name,
        started: TRACE.get().filter(|t| t.enabled).map(|_| Instant::now()),
        completed: false,
    }
}

impl Span {
    pub(super) fn finish(mut self) {
        self.completed = true;
    }
    pub(super) fn cancel(mut self) {
        self.started = None;
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let (Some(trace), Some(started)) = (TRACE.get(), self.started) else {
            return;
        };
        let ended = Instant::now();
        let value = json!({
            "stage": self.name,
            "start_ms": started.duration_since(trace.started).as_millis() as u64,
            "duration_ms": ended.duration_since(started).as_millis() as u64,
            "completed": self.completed,
        });
        if self.name == "workspace_inspection" {
            trace.record_essential(value);
        } else {
            trace.record(value);
        }
    }
}

impl Trace {
    fn record(&self, value: Value) {
        if let Ok(mut spans) = self.spans.try_lock() {
            if spans.len() < MAX_SPANS {
                spans.push(value);
                return;
            }
        }
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }

    // Workspace inspection is the first request to the runtime. Its elapsed
    // time is the useful failure signal when the bridge exits before replying,
    // so retain it even when the reader records an event at the same moment.
    fn record_essential(&self, value: Value) {
        if let Ok(mut spans) = self.spans.lock() {
            if spans.len() < MAX_SPANS {
                spans.push(value);
                return;
            }
        }
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn flush(success: bool) {
    let Some(trace) = TRACE.get().filter(|t| t.enabled) else {
        return;
    };
    let Ok(mut spans) = trace.spans.lock() else {
        return;
    };
    spans.sort_by_key(|v| v["start_ms"].as_u64());
    let record = json!({
        "schema_version": 1,
        "producer": "cli",
        "checkpoint": "cli_startup_timing",
        "process_id": std::process::id(),
        "recorded_at_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
        "command_elapsed_ms": trace.started.elapsed().as_millis() as u64,
        "success": success,
        "spans": *spans,
        "dropped_spans": trace.dropped.load(Ordering::Relaxed),
    });
    // Diagnostics must not change the command result if stderr is unavailable.
    let _ = writeln!(std::io::stderr().lock(), "{record}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_are_bounded_and_contention_drops_without_waiting() {
        let trace = Trace {
            started: Instant::now(),
            enabled: true,
            spans: Mutex::new(Vec::new()),
            dropped: AtomicU64::new(0),
        };
        let held = trace.spans.lock().unwrap();
        trace.record(json!({"stage":"test"}));
        drop(held);
        for _ in 0..MAX_SPANS + 1 {
            trace.record(json!({"stage":"test"}));
        }
        assert_eq!(trace.spans.lock().unwrap().len(), MAX_SPANS);
        assert_eq!(trace.dropped.load(Ordering::Relaxed), 2);
    }
}
