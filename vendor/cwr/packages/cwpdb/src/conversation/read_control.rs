//! Cancellation belongs to the storage read; no SQLite handle leaves cwpdb.
use crate::CwpdbResult;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;
#[derive(Clone, Default)]
pub struct ReadCancellation(Arc<AtomicBool>);
impl ReadCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
#[derive(Clone)]
pub struct ReadControl {
    pub cancellation: ReadCancellation,
    pub deadline: Instant,
    pub vm_steps: Option<Arc<std::sync::atomic::AtomicUsize>>,
}
impl ReadControl {
    pub fn check(&self) -> CwpdbResult<()> {
        if self.cancellation.is_cancelled() {
            return Err("conversation read operation was cancelled".into());
        }
        if Instant::now() >= self.deadline {
            return Err("conversation read operation exceeded its deadline".into());
        }
        Ok(())
    }
}
