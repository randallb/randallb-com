//! CWR passes validated metric facts through cwpdb's declared performance API.
use super::MetricRecord;
use cwpdb_store::metrics::{MetricObservation, PerformanceStore, PerformanceWriteFailure};
use std::path::Path;

pub(super) struct MetricStore(PerformanceStore);
pub(super) type WriteFailure = PerformanceWriteFailure;
impl MetricStore {
    pub(super) fn new(path: &Path) -> Self {
        Self(PerformanceStore::new(path))
    }
    pub(super) fn write(&mut self, records: &[MetricRecord]) -> Result<u64, WriteFailure> {
        let observations = records
            .iter()
            .map(|record| MetricObservation::from_value(serde_json::to_value(record)?))
            .collect::<cwpdb_store::CwpdbResult<Vec<_>>>()
            .map_err(|error| WriteFailure {
                unwritten: records.len() as u64,
                evicted: 0,
                error,
            })?;
        self.0.write(&observations)
    }
}
