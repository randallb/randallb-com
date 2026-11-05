//! Fixed read work for downstream cancellation/deadline compatibility checks.
use super::*;
impl NativeReadSnapshot {
    #[cfg(feature = "test-support")]
    pub fn cancellation_probe(&self, exhaustive: bool) -> CwpdbResult<i64> {
        self.control.check()?;
        let maximum = if exhaustive {
            100000000_i64
        } else {
            1000000_i64
        };
        Ok(self.connection.query_row("WITH RECURSIVE count(value) AS (VALUES(0) UNION ALL SELECT value + 1 FROM count WHERE value < ?1) SELECT sum(value) FROM count", [maximum], |row|row.get(0))?)
    }
}
