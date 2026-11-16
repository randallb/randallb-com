//! Declared disposable diagnostic record operations; physical SQL is cwpdb-private.
use super::NativeStore;
use crate::CwpdbResult;
use serde_json::{json, Value};

const FILE_LIMIT: usize = 8 * 1024 * 1024;
const STORE_LIMIT: u64 = 32 * 1024 * 1024;
const RETAIN_MS: u64 = 7 * 86_400_000;

/// One retained diagnostic observation and its canonical disposable reference.
pub struct DiagnosticRecord {
    pub reference: String,
    pub value: Value,
}
/// A bounded diagnostic result. A full page never claims complete coverage.
pub struct DiagnosticPage {
    pub records: Vec<DiagnosticRecord>,
    pub truncated: bool,
}
impl NativeStore {
    /// Read records for one invocation without exposing native handles or SQL.
    pub fn diagnostics_for_invocation(
        &mut self,
        invocation_ref: &str,
        limit: usize,
    ) -> CwpdbResult<DiagnosticPage> {
        if invocation_ref.is_empty() {
            return Err("invalid diagnostic invocation".into());
        }
        self.diagnostic_records(Some(invocation_ref), limit)
    }
    /// Read a bounded page of the private diagnostic store, optionally filtered.
    pub fn diagnostic_records(
        &mut self,
        invocation: Option<&str>,
        limit: usize,
    ) -> CwpdbResult<DiagnosticPage> {
        if limit == 0 || limit > 10_000 {
            return Err("invalid diagnostic query".into());
        }
        let result = self.query(&super::QueryRequest {
            sql: "SELECT r.canonical_ref, json_extract(n.props,'$.record') FROM nodes n JOIN ref_bindings r ON n.ref_id=r.ref_id WHERE n.kind='runtime_diagnostic' AND (?1 IS NULL OR json_extract(n.props,'$.record.invocation_ref')=?1) ORDER BY json_extract(n.props,'$.record.recorded_at_unix_ms'), n.ref_id".into(),
            params: vec![invocation.map_or(Value::Null, |s| json!(s))], max_rows: Some(limit),
        })?;
        let records = result
            .rows
            .into_iter()
            .map(|row| {
                Ok(DiagnosticRecord {
                    reference: row[0]
                        .as_str()
                        .ok_or("invalid diagnostic reference")?
                        .to_owned(),
                    value: serde_json::from_str(
                        row[1].as_str().ok_or("invalid diagnostic record")?,
                    )?,
                })
            })
            .collect::<CwpdbResult<Vec<_>>>()?;
        Ok(DiagnosticPage {
            records,
            truncated: result.truncated,
        })
    }
    /// Append a scrubbed diagnostic and apply its seven-day/32 MiB retention.
    /// The caller owns content scrubbing; cwpdb owns atomic pruning and identity.
    pub fn append_diagnostic(&mut self, reference: &str, record: &Value) -> CwpdbResult<()> {
        if reference.is_empty()
            || !record.is_object()
            || serde_json::to_vec(record)?.len() > FILE_LIMIT
        {
            return Err("invalid or oversized diagnostic record".into());
        }
        self.set_max_database_bytes(STORE_LIMIT)?;
        let now = record["recorded_at_unix_ms"].as_u64().unwrap_or(0);
        self.graph_transaction(|tx| {
            let mut statement = tx.prepare("SELECT r.canonical_ref FROM nodes n JOIN ref_bindings r ON n.ref_id=r.ref_id WHERE n.kind='runtime_diagnostic' AND r.disposable=1 AND json_extract(n.props,'$.record.recorded_at_unix_ms') < ?1")?;
            let expired = statement.query_map([now.saturating_sub(RETAIN_MS) as i64], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            crate::remove_disposables(tx, expired.iter().map(String::as_str))?;
            crate::put_disposable_node(tx, reference, "runtime_diagnostic", &json!({"record":record}))?;
            Ok(())
        }).or_else(|error| {
            if !matches!(error.downcast_ref::<rusqlite::Error>(), Some(rusqlite::Error::SqliteFailure(code, _)) if code.code == rusqlite::ErrorCode::DiskFull) { return Err(error); }
            self.graph_transaction(|tx| {
                let mut statement = tx.prepare("SELECT r.canonical_ref FROM nodes n JOIN ref_bindings r ON n.ref_id=r.ref_id WHERE n.kind='runtime_diagnostic' AND r.disposable=1 ORDER BY json_extract(n.props,'$.record.recorded_at_unix_ms') LIMIT 256")?;
                let oldest = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                drop(statement);
                crate::remove_disposables(tx, oldest.iter().map(String::as_str))?;
                Ok(())
            })?;
            self.graph_transaction(|tx| {
                crate::put_disposable_node(tx, reference, "runtime_diagnostic", &json!({"record":record}))?;
                Ok(())
            })
        })?;
        self.checkpoint_wal()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_diagnostics_reopen_with_explicit_page_limit_and_prune() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("diagnostics.sqlite3");
        let mut db = NativeStore::open(&path).unwrap();
        for i in 1..=3 {
            db.append_diagnostic(&format!("cwp://example.test/diagnostics/{i}"), &json!({"recorded_at_unix_ms":i,"invocation_ref":"invocation:1","message":"scrubbed"})).unwrap();
        }
        drop(db);
        let mut db = NativeStore::open_read_only(&path).unwrap();
        let page = db.diagnostics_for_invocation("invocation:1", 2).unwrap();
        assert_eq!(page.records.len(), 2);
        assert!(page.truncated);
        assert_eq!(
            page.records[0].reference,
            "cwp://example.test/diagnostics/1"
        );
        drop(db);
        let mut db = NativeStore::open(&path).unwrap();
        db.append_diagnostic(
            "cwp://example.test/diagnostics/later",
            &json!({"recorded_at_unix_ms":RETAIN_MS+10,"invocation_ref":"invocation:1"}),
        )
        .unwrap();
        let page = db.diagnostics_for_invocation("invocation:1", 10).unwrap();
        assert_eq!(page.records.len(), 1);
        assert!(!page.truncated);
        assert_eq!(
            page.records[0].reference,
            "cwp://example.test/diagnostics/later"
        );
    }
}
