//! Selected-record domain transitions. Selection itself belongs to Track 2.
use super::*;
use crate::ref_read::{RefRecord, RefRevision};
use rusqlite::TransactionBehavior;
use std::collections::BTreeSet;

/// A selection's original comparison token. It contains no saved HTTP payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub reference: String,
    pub revision: RefRevision,
}
impl From<&RefRecord> for Selection {
    fn from(record: &RefRecord) -> Self {
        Self {
            reference: record.reference.clone(),
            revision: record.revision.clone(),
        }
    }
}
fn validate_selection(records: &[Selection]) -> CwpdbResult<()> {
    if records.len() > 250 {
        return Err("outbox selection limit exceeded".into());
    }
    let unique: BTreeSet<_> = records.iter().map(|r| r.reference.as_str()).collect();
    if unique.len() != records.len() {
        return Err("duplicate outbox selection".into());
    }
    Ok(())
}
impl PosthogOutbox {
    /// Only the exporter calls this after successful HTTP delivery. The entire
    /// selected batch commits together; newer admissions are never swept up.
    /// No network operation executes while this transaction is held.
    pub fn acknowledge(&mut self, records: &[Selection]) -> CwpdbResult<()> {
        validate_selection(records)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut values = Vec::new();
        for selected in records {
            let record = self.declarations.event.get_in_connection(
                &tx,
                EVENT_DEFINITION,
                &selected.reference,
            )?;
            if record.revision != selected.revision {
                return Err("entity revision conflict".into());
            }
            values.push(record.value.properties);
        }
        let records: Vec<_> = records
            .iter()
            .map(|r| (r.reference.as_str(), &r.revision))
            .collect();
        self.declarations
            .event
            .purge_batch_in_transaction(&tx, EVENT_DEFINITION, &records)?;
        accounting::remove(&tx, &self.declarations, &values)?;
        tx.commit()?;
        Ok(())
    }

    /// Preserve event bytes/UUID, destination and admission order while marking
    /// a rejected or malformed event as quarantined. No drop is counted here.
    pub fn quarantine(&mut self, records: &[Selection]) -> CwpdbResult<()> {
        validate_selection(records)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut values = Vec::new();
        let mut bytes = 0usize;
        for selected in records {
            let record = self.declarations.event.get_in_connection(
                &tx,
                EVENT_DEFINITION,
                &selected.reference,
            )?;
            if record.revision != selected.revision {
                return Err("entity revision conflict".into());
            }
            bytes += serde_json::to_vec(&record.value.properties)?.len();
            if bytes > 1_048_576 {
                return Err("outbox selection byte limit exceeded".into());
            }
            values.push(record);
        }
        for mut record in values {
            if record.value.properties["record"]["quarantined"] == true {
                continue;
            }
            record.value.properties["record"]["quarantined"] = json!(true);
            self.declarations.event.replace_in_transaction(
                &tx,
                EVENT_DEFINITION,
                &record.reference,
                &record.revision,
                &record.value.properties,
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
