//! Thread-scoped test measurement across the read connections opened by public APIs.
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
thread_local! { static COUNTER: RefCell<Option<Arc<AtomicUsize>>> = const { RefCell::new(None) }; }
pub(super) fn current() -> Option<Arc<AtomicUsize>> {
    COUNTER.with(|counter| counter.borrow().clone())
}
#[cfg(test)]
pub(crate) fn measure<T>(run: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            COUNTER.with(|counter| *counter.borrow_mut() = None);
        }
    }
    let counter = Arc::new(AtomicUsize::new(0));
    COUNTER.with(|slot| {
        assert!(slot.borrow().is_none());
        *slot.borrow_mut() = Some(counter.clone());
    });
    let _reset = Reset;
    let result = run();
    (result, counter.load(Ordering::Relaxed))
}

#[cfg(feature = "test-read-metrics")]
fn paths(
) -> &'static std::sync::Mutex<std::collections::BTreeMap<std::path::PathBuf, Arc<AtomicUsize>>> {
    static PATHS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<std::path::PathBuf, Arc<AtomicUsize>>>,
    > = std::sync::OnceLock::new();
    PATHS.get_or_init(Default::default)
}

pub(in crate::v2026_09::conversation) fn current_for_path(
    path: &std::path::Path,
) -> Option<Arc<AtomicUsize>> {
    #[cfg(feature = "test-read-metrics")]
    if let Some(counter) = paths().lock().unwrap().get(path).cloned() {
        return Some(counter);
    }
    let _ = path;
    current()
}

/// Opt-in regression instrumentation for worker threads reading one fixture DB.
/// Registrations are isolated by absolute database path and removed on unwind.
#[cfg(feature = "test-read-metrics")]
pub fn measure_path_reads<T>(path: &std::path::Path, run: impl FnOnce() -> T) -> (T, usize) {
    struct Reset(std::path::PathBuf);
    impl Drop for Reset {
        fn drop(&mut self) {
            paths().lock().unwrap().remove(&self.0);
        }
    }
    assert!(path.is_absolute());
    let canonical = path
        .canonicalize()
        .expect("measurement database must exist");
    let path = canonical.as_path();
    let counter = Arc::new(AtomicUsize::new(0));
    {
        let mut registered = paths().lock().unwrap();
        assert!(
            !registered.contains_key(path),
            "database path already measured"
        );
        registered.insert(path.into(), counter.clone());
    }
    let _reset = Reset(path.into());
    let result = run();
    (result, counter.load(Ordering::Relaxed))
}
