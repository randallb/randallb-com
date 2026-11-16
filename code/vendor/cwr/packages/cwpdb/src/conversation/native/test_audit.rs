//! Test-only physical write counts; no SQL or native handle leaves cwpdb.
use super::*;
impl NativeConversationStore {
    pub(super) fn publication_write_audit(
        &mut self,
        action: PublicationAudit,
    ) -> CwpdbResult<PublicationWriteCounts> {
        if matches!(action, PublicationAudit::Start) {
            self.connection.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS cwpdb_publication_write_audit (relation TEXT);",
            )?;
            for kind in ["timeline_message", "timeline_item", "timeline_activity"] {
                for operation in ["INSERT", "UPDATE", "DELETE"] {
                    let row = if operation == "DELETE" { "OLD" } else { "NEW" };
                    // Both identifiers are selected solely from the finite
                    // storage-owned vocabulary above, never caller strings.
                    self.connection.execute_batch(&format!("CREATE TEMP TRIGGER IF NOT EXISTS cwpdb_audit_{kind}_{operation} AFTER {operation} ON edges WHEN {row}.kind='{kind}' BEGIN INSERT INTO cwpdb_publication_write_audit VALUES ('{kind}'); END;"))?;
                }
            }
        }
        let mut counts = PublicationWriteCounts::default();
        let mut statement = self.connection.prepare(
            "SELECT relation,COUNT(*) FROM cwpdb_publication_write_audit GROUP BY relation",
        )?;
        for row in statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })? {
            let (kind, count) = row?;
            let count: u64 = count.try_into()?;
            match kind.as_str() {
                "timeline_message" => counts.message = count,
                "timeline_item" => counts.anchor = count,
                "timeline_activity" => counts.activity = count,
                _ => return Err("unexpected publication audit kind".into()),
            }
        }
        drop(statement);
        self.connection
            .execute("DELETE FROM cwpdb_publication_write_audit", [])?;
        Ok(counts)
    }
}
