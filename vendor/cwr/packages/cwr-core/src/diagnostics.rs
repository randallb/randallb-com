//! Private local failure evidence. Never included in metrics or conversation text.
use crate::{adapter::ProcessAdapterFailure, metrics::MetricContext};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod resident;
mod store;
pub use resident::{capture_resident_stderr, ResidentDiagnostics};
mod text;
fn process_run_id() -> &'static str {
    static RUN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RUN.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

#[derive(Clone)]
pub(crate) struct DiagnosticsRecorder {
    writer: Arc<Writer>,
    pub directory: PathBuf,
}

struct Writer {
    sender: Option<mpsc::SyncSender<Value>>,
    finished: Mutex<mpsc::Receiver<()>>,
}

impl Drop for Writer {
    fn drop(&mut self) {
        drop(self.sender.take());
        let finished = self.finished.get_mut().unwrap_or_else(|e| e.into_inner());
        if finished.recv_timeout(Duration::from_secs(1)).is_err() {
            eprintln!("cwr: diagnostics writer did not finish; evidence may be incomplete");
        }
    }
}

impl DiagnosticsRecorder {
    pub fn new(state_path: &Path) -> Self {
        let directory = state_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("diagnostics");
        let destination = directory.clone();
        // At most 16 capped records await disk; storage cannot block execution.
        let (sender, receiver) = mpsc::sync_channel::<Value>(16);
        let (finished_send, finished) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("cwr-diagnostics".into())
            .spawn(move || {
                for record in receiver {
                    match store::write_record(&destination, &record) {
                        Ok(path) if record["source"] == "conversation_browse_overage" => {
                            eprintln!(
                                "cwr: browse overage diagnostic: {} (event {})",
                                path.display(),
                                record["event_id"]
                            )
                        }
                        Ok(path) => eprintln!(
                            "cwr: failure diagnostics: {} (invocation {}, attempt {})",
                            path.display(),
                            record["invocation_ref"],
                            record["attempt"]
                        ),
                        Err(_) => eprintln!("cwr: failure diagnostics could not be saved"),
                    }
                }
                let _ = finished_send.send(());
            });
        if spawn.is_err() {
            eprintln!("cwr: failure diagnostics writer could not start");
        }
        Self {
            directory,
            writer: Arc::new(Writer {
                sender: Some(sender),
                finished: Mutex::new(finished),
            }),
        }
    }

    pub fn record_failure(
        &self,
        context: &MetricContext,
        error: &(dyn std::error::Error + Send + Sync + 'static),
    ) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut record = json!({
            "schema_version": 1, "recorded_at_unix_ms": now,
            "run_id": process_run_id(), "pid": std::process::id(),
            "runtime_ref": context.runtime_ref, "invocation_ref": context.invocation_ref,
            "attempt": context.attempt, "model_run_ref": context.model_run_ref,
            "actor_ref": context.actor_ref, "app_version": env!("CARGO_PKG_VERSION"),
            "source": "adapter_execution", "error": text::scrub(&error.to_string(), 16 * 1024),
        });
        if let Some(failure) = error.downcast_ref::<ProcessAdapterFailure>() {
            record["source"] = json!("adapter_process");
            record["process"] = json!({
                "exit_code": failure.exit_code, "signal": failure.signal,
                "stdout_state": failure.stdout_state,
                "stderr_tail": text::scrub(&failure.stderr_tail, 128 * 1024),
                "stderr_total_bytes": failure.stderr_total_bytes,
                "stderr_truncated": failure.stderr_truncated,
                "stderr_complete": failure.stderr_complete,
                "stderr_capture_error": failure.stderr_capture_error.as_ref().map(|s| text::scrub(s, 4096)),
            });
        }
        if self
            .writer
            .sender
            .as_ref()
            .unwrap()
            .try_send(record)
            .is_err()
        {
            eprintln!("cwr: failure diagnostics queue unavailable; evidence was not saved");
        }
    }

    /// Store only the metadata needed to locate a browse payload overage.
    /// The retained message, action JSON, and search query never enter this record.
    pub(crate) fn record_browse_overage(
        &self,
        overage: &crate::v2026_09::conversation::BrowseMessageOverage,
        invocation_ref: Option<&str>,
        request_id: Option<&str>,
        operation_id: Option<&str>,
    ) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let record = json!({
            "schema_version": 1, "recorded_at_unix_ms": now,
            "run_id": process_run_id(), "pid": std::process::id(),
            "source": "conversation_browse_overage",
            "event_id": uuid::Uuid::new_v4().to_string(),
            "conversation_ref": overage.conversation_ref,
            "message_ref": overage.message_ref,
            "operation": overage.operation,
            "payload_bytes": overage.payload_bytes,
            "limit_bytes": overage.limit_bytes,
            "invocation_ref": invocation_ref,
            "request_id_sha256": request_id.map(|id| hex::encode(Sha256::digest(id.as_bytes()))),
            "operation_id_sha256": operation_id.map(|id| hex::encode(Sha256::digest(id.as_bytes()))),
        });
        if self
            .writer
            .sender
            .as_ref()
            .unwrap()
            .try_send(record)
            .is_err()
        {
            eprintln!("cwr: browse overage diagnostics queue unavailable; evidence was not saved");
        }
    }

    pub(crate) fn evidence_for_invocation(
        &self,
        invocation_ref: &str,
    ) -> crate::v2026_09::conversation::InvocationDiagnosticEvidence {
        use crate::v2026_09::conversation::{
            InvocationDiagnosticEvidence, InvocationDiagnosticRecord,
        };
        let lookup = match store::records_for_invocation(&self.directory, invocation_ref) {
            Ok(lookup) => lookup,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return InvocationDiagnosticEvidence {
                    availability: "not_recorded".into(),
                    records: Vec::new(),
                };
            }
            Err(_) => {
                return InvocationDiagnosticEvidence {
                    availability: "unreadable".into(),
                    records: Vec::new(),
                };
            }
        };
        let records = lookup
            .records
            .into_iter()
            .map(|record| InvocationDiagnosticRecord {
                recorded_at_unix_ms: record.value["recorded_at_unix_ms"].as_u64(),
                attempt: record.value["attempt"].as_u64(),
                source: record.value["source"].as_str().map(str::to_owned),
                error: record.value["error"].as_str().map(str::to_owned),
                exit_code: record.value["process"]["exit_code"].as_i64(),
                signal: record.value["process"]["signal"].as_i64(),
                stdout_state: record.value["process"]["stdout_state"]
                    .as_str()
                    .map(str::to_owned),
                stderr_tail: record.value["process"]["stderr_tail"]
                    .as_str()
                    .map(str::to_owned),
                raw_log_locator: match record.reference {
                    Some(reference) => format!("{}#{reference}", record.path.display()),
                    None => format!("{}:{}", record.path.display(), record.line),
                },
            })
            .collect::<Vec<_>>();
        InvocationDiagnosticEvidence {
            availability: if records.is_empty() {
                if lookup.unreadable {
                    "unreadable"
                } else {
                    "not_recorded"
                }
            } else {
                "available"
            }
            .into(),
            records,
        }
    }
}

#[cfg(test)]
mod tests;
