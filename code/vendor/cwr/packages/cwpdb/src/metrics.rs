//! A compact local graph projection over CWR measurements.
//!
//! Content-free records use cwpdb nodes and edges for raw observations and
//! derived views, separate from diagnostic text and authoritative runtime state.
//! Every metric identity is disposable: retention removes its observation
//! nodes, relationship edges, and any otherwise-unreferenced bindings.

use crate::{
    put_disposable_edge, put_disposable_node, remove_disposables, BatchRequest, NativeStore,
    QueryRequest,
};
use serde_json::{json, Value};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

pub mod projection;
mod schema;
#[cfg(test)]
mod tests;
use schema::*;
const RETAIN_DAYS: u64 = 14;

// Storage is disposable performance data, separate from runtime state.
const MAX_MAIN_DATABASE_BYTES: u64 = 56 * 1024 * 1024;
const MAX_STORAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RECORDS: u64 = 250_000;
const MAX_RECORD_BYTES: usize = 64 * 1024;
const DATABASE_BATCH_RECORDS: usize = 8;
const MAX_PENDING_WRITE_BYTES: u64 = 1024 * 1024;
const MAINTENANCE_BATCH_RECORDS: usize = 8;

/// A validated content-free CWR performance observation. Storage owns its indexes.
#[derive(Clone)]
pub struct MetricObservation {
    recorded_at_unix_ms: u64,
    context: ObservationContext,
    record: Value,
}
#[derive(Clone)]
struct ObservationContext {
    invocation_ref: String,
    model_run_ref: Option<String>,
    conversation_ref: Option<String>,
}
impl MetricObservation {
    /// Validate the versioned CWR metric record before any retention or write.
    pub fn from_value(record: Value) -> crate::CwpdbResult<Self> {
        if record["schema_version"] != 2 {
            return Err("unsupported metric schema".into());
        }
        let recorded_at_unix_ms = record["recorded_at_unix_ms"]
            .as_u64()
            .ok_or("invalid metric receipt time")?;
        let invocation_ref = record["invocation_ref"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("missing metric invocation")?
            .to_owned();
        let optional = |name: &str| -> crate::CwpdbResult<Option<String>> {
            match &record[name] {
                Value::Null => Ok(None),
                Value::String(s) if !s.is_empty() => Ok(Some(s.clone())),
                _ => Err("invalid optional metric identity".into()),
            }
        };
        let context = ObservationContext {
            invocation_ref,
            model_run_ref: optional("model_run_ref")?,
            conversation_ref: optional("conversation_ref")?,
        };
        let observation = Self {
            recorded_at_unix_ms,
            context,
            record,
        };
        metric_record_props(&observation)?;
        Ok(observation)
    }
}
/// A bounded page of retained metric records. No SQL or physical identities escape.
pub struct MetricPage {
    pub records: Vec<Value>,
    pub truncated: bool,
}
impl NativeStore {
    /// Read observations in receipt/insertion order, with explicit truncation.
    pub fn metric_records(
        &mut self,
        invocation: Option<&str>,
        limit: usize,
    ) -> crate::CwpdbResult<MetricPage> {
        if limit == 0 || limit > 10_000 {
            return Err("metric page limit must be 1..=10000".into());
        }
        let result = self.query(&QueryRequest {
            sql: "SELECT json_extract(props, '$.record') FROM nodes WHERE kind='cwr_metric_observation' AND (?1 IS NULL OR json_extract(props, '$.record.invocation_ref')=?1) ORDER BY ref_id".into(),
            params: vec![invocation.map_or(Value::Null, |s| json!(s))], max_rows: Some(limit),
        })?;
        let records = result
            .rows
            .into_iter()
            .map(|row| {
                serde_json::from_str(row[0].as_str().ok_or("invalid metric observation")?)
                    .map_err(Into::into)
            })
            .collect::<crate::CwpdbResult<Vec<Value>>>()?;
        Ok(MetricPage {
            records,
            truncated: result.truncated,
        })
    }
}

/// A single writer-thread owner for the CWR metrics database.
pub struct PerformanceStore {
    path: PathBuf,
    store: Option<NativeStore>,
    last_pruned_day: Option<u64>,
}

#[derive(Debug)]
pub struct PerformanceWriteFailure {
    pub unwritten: u64,
    pub evicted: u64,
    pub error: Box<dyn Error + Send + Sync>,
}

impl PerformanceStore {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            store: None,
            last_pruned_day: None,
        }
    }

    /// Returns the number of observations discarded by the record budget.
    pub fn write(&mut self, records: &[MetricObservation]) -> Result<u64, PerformanceWriteFailure> {
        self.write_with_limit(records, MAX_RECORDS)
    }

    fn write_with_limit(
        &mut self,
        records: &[MetricObservation],
        limit: u64,
    ) -> Result<u64, PerformanceWriteFailure> {
        if records.is_empty() {
            return Ok(0);
        }
        let mut committed = 0;
        let mut evicted = 0;
        let result = self
            .write_inner(records, &mut committed, limit, &mut evicted)
            .map(|()| evicted)
            .map_err(|error| PerformanceWriteFailure {
                unwritten: (records.len() - committed) as u64,
                evicted,
                error,
            });
        if result.is_err() {
            // A later record gets a fresh connection and can recover from a
            // transient lock, full disk, or broken local file.
            self.store = None;
        }
        result
    }

    fn write_inner(
        &mut self,
        records: &[MetricObservation],
        committed: &mut usize,
        limit: u64,
        evicted: &mut u64,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Reject oversized records before retention can delete anything. The
        // JSONL writer follows the same content-free record shape.
        for record in records {
            let _ = metric_record_props(record)?;
        }

        let record_day = records
            .iter()
            .map(|record| record.recorded_at_unix_ms / 86_400_000)
            .max()
            .expect("nonempty records have a latest receipt date");
        let prune_before = record_day
            .saturating_sub(RETAIN_DAYS - 1)
            .saturating_mul(86_400_000);

        if self.last_pruned_day != Some(record_day) {
            self.delete_expired(prune_before)?;
            self.last_pruned_day = Some(record_day);
        }

        // Keep a write small enough that a WAL held by a reader cannot jump
        // far past the total store budget before the next checkpoint attempt.
        for records in records.chunks(DATABASE_BATCH_RECORDS) {
            self.ensure_capacity_before_write()?;
            self.write_graph_records(records)?;
            *committed += records.len();
            // Observations already committed remain valid even when a reader
            // now holds the WAL. The next batch retries this check first.
            let _ = self.storage_is_within_budget()?;
        }
        self.trim_excess(limit, evicted)?;
        let _ = self.storage_is_within_budget()?;
        Ok(())
    }

    fn open(&mut self) -> Result<&mut NativeStore, Box<dyn Error + Send + Sync>> {
        if self.store.is_none() {
            let mut store = NativeStore::open(&self.path)?;
            store.execute_batch(&BatchRequest {
                statements: schema_statements(),
            })?;
            let effective_limit = store.set_max_database_bytes(MAX_MAIN_DATABASE_BYTES)?;
            if effective_limit > MAX_MAIN_DATABASE_BYTES {
                return Err(format!(
                    "CWR metrics database already exceeds its {} MiB page budget (effective limit: {} MiB); delete the disposable metrics database before retrying",
                    MAX_MAIN_DATABASE_BYTES / (1024 * 1024),
                    effective_limit / (1024 * 1024),
                )
                .into());
            }
            self.store = Some(store);
        }
        Ok(self.store.as_mut().expect("store was opened above"))
    }

    fn write_graph_records(
        &mut self,
        records: &[MetricObservation],
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.open()?.graph_transaction(|transaction| {
            for record in records {
                let observation = metric_observation_ref(transaction)?;
                let props = metric_record_props(record)?;
                put_disposable_node(transaction, &observation, OBSERVATION_KIND, &props)?;
                let position = i64::try_from(record.recorded_at_unix_ms)?;
                let relationship_props = json!({
                    "recorded_at_unix_ms": record.recorded_at_unix_ms,
                });
                // These source references are ordinary CWP references, but
                // their bindings are disposable unless another graph caller
                // has already promoted them to durable application identity.
                put_disposable_edge(
                    transaction,
                    &record.context.invocation_ref,
                    INVOCATION_OBSERVATION_EDGE_KIND,
                    &observation,
                    position,
                    &relationship_props,
                )?;
                if let Some(model_run_ref) = &record.context.model_run_ref {
                    put_disposable_edge(
                        transaction,
                        model_run_ref,
                        MODEL_RUN_OBSERVATION_EDGE_KIND,
                        &observation,
                        position,
                        &relationship_props,
                    )?;
                }
                if let Some(conversation_ref) = &record.context.conversation_ref {
                    put_disposable_edge(
                        transaction,
                        conversation_ref,
                        CONVERSATION_OBSERVATION_EDGE_KIND,
                        &observation,
                        position,
                        &relationship_props,
                    )?;
                }
            }
            Ok(())
        })
    }

    fn trim_excess(
        &mut self,
        limit: u64,
        evicted: &mut u64,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let count = self.observation_count()?;
        let mut excess = count.saturating_sub(limit);
        while excess > 0 {
            self.ensure_capacity_before_write()?;
            let references = self.oldest_observations(excess)?;
            let removed = self.remove_observations(references)?;
            if removed == 0 {
                break;
            }
            *evicted += removed;
            let _ = self.storage_is_within_budget()?;
            excess = excess.saturating_sub(removed);
        }
        Ok(())
    }

    fn delete_expired(&mut self, cutoff: u64) -> Result<(), Box<dyn Error + Send + Sync>> {
        loop {
            self.ensure_capacity_before_write()?;
            let references = self.expired_observations(cutoff)?;
            let removed = self.remove_observations(references)?;
            let _ = self.storage_is_within_budget()?;
            if removed < MAINTENANCE_BATCH_RECORDS as u64 {
                return Ok(());
            }
        }
    }

    fn observation_count(&mut self) -> Result<u64, Box<dyn Error + Send + Sync>> {
        let result = self.open()?.query(&QueryRequest {
            sql: "SELECT COUNT(*) FROM nodes WHERE kind = ?".into(),
            params: vec![Value::String(OBSERVATION_KIND.into())],
            max_rows: Some(1),
        })?;
        result
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(Value::as_u64)
            .ok_or_else(|| "CWR metrics count query returned no unsigned count".into())
    }

    fn oldest_observations(
        &mut self,
        excess: u64,
    ) -> Result<Vec<String>, Box<dyn Error + Send + Sync>> {
        self.observation_references(
            r#"
                SELECT bindings.canonical_ref
                FROM nodes observation
                JOIN ref_bindings bindings ON bindings.ref_id = observation.ref_id
                WHERE observation.kind = ?
                ORDER BY json_extract(observation.props, '$.record.recorded_at_unix_ms'), observation.ref_id
                LIMIT ?
            "#,
            vec![
                Value::String(OBSERVATION_KIND.into()),
                json!(excess.min(MAINTENANCE_BATCH_RECORDS as u64)),
            ],
        )
    }

    fn expired_observations(
        &mut self,
        cutoff: u64,
    ) -> Result<Vec<String>, Box<dyn Error + Send + Sync>> {
        self.observation_references(
            r#"
                SELECT bindings.canonical_ref
                FROM nodes observation
                JOIN ref_bindings bindings ON bindings.ref_id = observation.ref_id
                WHERE observation.kind = ?
                  AND json_extract(observation.props, '$.record.recorded_at_unix_ms') < ?
                ORDER BY json_extract(observation.props, '$.record.recorded_at_unix_ms'), observation.ref_id
                LIMIT ?
            "#,
            vec![
                Value::String(OBSERVATION_KIND.into()),
                json!(cutoff),
                json!(MAINTENANCE_BATCH_RECORDS),
            ],
        )
    }

    fn observation_references(
        &mut self,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<Vec<String>, Box<dyn Error + Send + Sync>> {
        let result = self.open()?.query(&QueryRequest {
            sql: sql.into(),
            params,
            max_rows: Some(MAINTENANCE_BATCH_RECORDS),
        })?;
        result
            .rows
            .into_iter()
            .map(|row| match row.as_slice() {
                [Value::String(reference)] => Ok(reference.clone()),
                _ => Err("CWR metrics retention query returned an invalid reference".into()),
            })
            .collect()
    }

    fn remove_observations(
        &mut self,
        references: Vec<String>,
    ) -> Result<u64, Box<dyn Error + Send + Sync>> {
        if references.is_empty() {
            return Ok(0);
        }
        self.ensure_capacity_before_write()?;
        let count = references.len() as u64;
        self.open()?.graph_transaction(|transaction| {
            remove_disposables(transaction, references.iter().map(String::as_str))?;
            Ok(())
        })?;
        Ok(count)
    }

    fn ensure_capacity_before_write(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if self.storage_bytes_after_checkpoint()? <= MAX_STORAGE_BYTES - MAX_PENDING_WRITE_BYTES {
            return Ok(());
        }
        Err(format!(
            "CWR metrics storage reached its {} MiB budget; a reader is preventing WAL reclamation or the database needs retention cleanup",
            MAX_STORAGE_BYTES / (1024 * 1024),
        )
        .into())
    }

    fn storage_is_within_budget(&mut self) -> Result<bool, Box<dyn Error + Send + Sync>> {
        Ok(self.storage_bytes_after_checkpoint()? <= MAX_STORAGE_BYTES)
    }

    fn storage_bytes_after_checkpoint(&mut self) -> Result<u64, Box<dyn Error + Send + Sync>> {
        let _checkpoint = self.open()?.checkpoint_wal()?;
        Ok(storage_bytes(&self.path)?)
    }
}

fn metric_observation_ref(db: &rusqlite::Connection) -> crate::CwpdbResult<String> {
    let nonce: String = db.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))?;
    Ok(crate::derived_ref("cwr-metric-observation", &[&nonce]))
}

fn metric_record_props(record: &MetricObservation) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let props = json!({ "record": record.record });
    let encoded_record = serde_json::to_vec(&props)?;
    if encoded_record.len() > MAX_RECORD_BYTES {
        return Err(format!(
            "CWR metrics record is {} bytes; the limit is {MAX_RECORD_BYTES}",
            encoded_record.len(),
        )
        .into());
    }
    Ok(props)
}

fn storage_bytes(path: &Path) -> std::io::Result<u64> {
    ["", "-wal", "-shm"]
        .into_iter()
        .try_fold(0_u64, |bytes, suffix| {
            let mut companion = path.as_os_str().to_os_string();
            companion.push(suffix);
            match fs::metadata(PathBuf::from(companion)) {
                Ok(metadata) => Ok(bytes.saturating_add(metadata.len())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(bytes),
                Err(error) => Err(error),
            }
        })
}
